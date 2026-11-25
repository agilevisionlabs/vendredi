use anyhow::{Context, Result, anyhow, bail, ensure};
use flate2::read::GzDecoder;
use std::{fs::File, io::Read, path::Path, sync::LazyLock};
use wasmtime::{
    Engine, InstancePre, Linker, Module, Store, StoreLimits as Limits, StoreLimitsBuilder,
};
#[path = "script_config.rs"]
mod script_config;
mod wasi;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct Input<'a, L = Value> {
    /// The standard-neutral view from `contract::interchange`, or null for a
    /// partner-initiated send.
    pub interchange: &'a Value,
    /// The 0.1 interchange layout, served through deprecated properties.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy: Option<&'a L>,
    pub state: &'a BTreeMap<String, Value>,
    pub reference: i64,
    pub now: i64,
}

#[derive(Deserialize, Default)]
pub struct Effects {
    /// Message objects from `api.build`, or raw EDIFACT text from 0.1 scripts.
    pub sends: Vec<Value>,
    pub logs: Vec<String>,
    pub state: BTreeMap<String, Value>,
    pub reference: i64,
    pub acknowledge: bool,
    /// The deprecated script features this invocation used.
    #[serde(default)]
    pub deprecated: Vec<String>,
}

const SOURCE_LIMIT: usize = 64 * 1024;
const REQUEST_LIMIT: usize = 1024 * 1024;
const RESPONSE_LIMIT: usize = 256 * 1024;
const STATE_LIMIT: usize = 64 * 1024;
const FUEL: u64 = 50_000_000;

pub fn read_source(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    let mut source = String::new();
    File::open(path)?
        .take(SOURCE_LIMIT as u64 + 1)
        .read_to_string(&mut source)?;
    ensure!(
        source.len() <= SOURCE_LIMIT,
        "script source exceeds {SOURCE_LIMIT} bytes"
    );
    Ok(source)
}

struct Sandbox {
    engine: Engine,
    instance: InstancePre<Limits>,
}

impl Sandbox {
    fn load() -> Result<Self> {
        let engine = script_config::engine()?;
        let mut artifact = Vec::new();
        let embedded = include_bytes!(concat!(env!("OUT_DIR"), "/boa.cwasm.gz"));
        GzDecoder::new(&embedded[..]).read_to_end(&mut artifact)?;
        // SAFETY: Only the trusted artifact compiled from our bundled source
        // by build.rs with the same locked Wasmtime and config reaches here.
        let module = unsafe { Module::deserialize(&engine, artifact)? };
        let mut linker = Linker::new(&engine);
        wasi::link(&mut linker)?;
        Ok(Self {
            engine,
            instance: linker.instantiate_pre(&module)?,
        })
    }

    fn call(&self, request: &[u8]) -> Result<Vec<u8>> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(script_config::MEMORY_LIMIT)
            .table_elements(16_384)
            .memories(1)
            .tables(1)
            .instances(1)
            .trap_on_grow_failure(true)
            .build();
        let mut store = Store::new(&self.engine, limits);
        store.limiter(|limits| limits);
        store.set_fuel(FUEL)?;
        let instance = self.instance.instantiate(&mut store)?;
        let mem = instance
            .get_memory(&mut store, "memory")
            .context("missing guest memory")?;
        let alloc = instance.get_typed_func::<u32, u32>(&mut store, "allocate")?;
        let invoke = instance.get_typed_func::<(u32, u32), u64>(&mut store, "invoke_v1")?;
        let len = u32::try_from(request.len())?;
        let at = alloc.call(&mut store, len)?;
        mem.write(&mut store, at as usize, request)?;
        let packed = invoke.call(&mut store, (at, len))?;
        let start = (packed >> 32) as usize;
        let size = packed as u32 as usize;
        ensure!(
            size <= RESPONSE_LIMIT,
            "script result exceeds {RESPONSE_LIMIT} bytes"
        );
        let end = start
            .checked_add(size)
            .context("invalid script response range")?;
        let bytes = mem
            .data(&store)
            .get(start..end)
            .context("invalid script response range")?;
        match bytes.split_first() {
            Some((0, body)) => Ok(body.to_vec()),
            Some((1, error)) if error.len() <= 1024 => bail!("{}", std::str::from_utf8(error)?),
            _ => bail!("invalid script response"),
        }
    }
}

#[derive(Serialize)]
struct Request<'a, T> {
    source: &'a str,
    function: &'a str,
    input: Option<&'a Input<'a, T>>,
}

// A fixed-size writer bounds even escaped JSON while it is being serialized.
fn encode(value: &impl Serialize, limit: usize) -> Result<Vec<u8>> {
    struct Writer {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for Writer {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            if data.len() > self.limit - self.bytes.len() {
                return Err(std::io::Error::other(format!(
                    "script data exceeds {} bytes",
                    self.limit
                )));
            }
            self.bytes.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, value)?;
    Ok(writer.bytes)
}

fn call<T: Serialize>(
    source: &str,
    function: &str,
    input: Option<&Input<'_, T>>,
) -> Result<Vec<u8>> {
    ensure!(
        source.len() <= SOURCE_LIMIT,
        "script source exceeds {SOURCE_LIMIT} bytes"
    );
    let request = encode(
        &Request {
            source,
            function,
            input,
        },
        REQUEST_LIMIT,
    )?;
    static SANDBOX: LazyLock<Result<Sandbox>> = LazyLock::new(Sandbox::load);
    SANDBOX
        .as_ref()
        .map_err(|e| anyhow!("loading script sandbox: {e:#}"))?
        .call(&request)
}

pub fn validate(source: &str, function: &str) -> Result<()> {
    serde_json::from_slice::<()>(&call::<Value>(source, function, None)?)?;
    Ok(())
}

pub fn run<L: Serialize>(source: &str, function: &str, input: &Input<'_, L>) -> Result<Effects> {
    let effects: Effects = serde_json::from_slice(&call(source, function, Some(input))?)?;
    ensure!(
        effects.sends.len() <= 64 && effects.logs.len() <= 128 && effects.deprecated.len() <= 16,
        "too many script sends or logs"
    );
    encode(&effects.state, STATE_LIMIT)?;
    // The existing JS API increments references using exact integer arithmetic.
    ensure!(
        effects.reference >= input.reference
            && effects.reference <= (1_i64 << 53) - 1
            && effects.reference.abs_diff(input.reference) <= 100_000,
        "invalid script reference progression"
    );
    Ok(effects)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_recovers_after_each_limit() {
        let input = Input::<Value> {
            interchange: &Value::Null,
            legacy: None,
            state: &BTreeMap::new(),
            reference: 0,
            now: 1,
        };
        for bad in [
            "function act(){while(true){}}",
            "function act(){act()}",
            "function act(){for(let i=0;i<99999;i++)for(let j=0;j<99999;j++){} }",
        ] {
            assert!(run(bad, "act", &input).is_err());
            let ok = run(
                "function act(i, api){api.log(api.nextReference())}",
                "act",
                &input,
            )
            .unwrap();
            assert_eq!(ok.logs, ["1"]);
        }
    }
}
