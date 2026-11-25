//! Build-owned WASI reactor. All allocations live until its single invocation ends.
use anyhow::{Result, anyhow, ensure};
use boa_engine::{Context, Script, Source, module::IdleModuleLoader};
use serde::Deserialize;
use std::rc::Rc;

#[derive(Deserialize)]
struct Request {
    source: String,
    function: String,
    input: Option<serde_json::Value>,
}

fn compile(cx: &mut Context, source: &str, function: &str, input_json: &str) -> Result<Script> {
    ensure!(
        !function.is_empty()
            && function
                .bytes()
                .enumerate()
                .all(|(i, c)| c.is_ascii_alphabetic()
                    || c == b'_'
                    || (i > 0 && c.is_ascii_digit())),
        "invalid JavaScript function name"
    );
    let program = format!(
        r#"((input) => {{
        const effects = {{ sends: [], logs: [], state: input.state, reference: input.reference, acknowledge: false, deprecated: [] }};
        // Helpers stay in this closure so workspace scripts may reuse their names.
        const [api, vendrediInterchange] = (() => {{
        const deprecated = name => {{ if (!effects.deprecated.includes(name)) effects.deprecated.push(name); }};
        const interchange = input.interchange;
        if (interchange !== null && input.legacy) {{
            for (const key of ["header", "messages", "findings", "syntax"]) {{
                Object.defineProperty(interchange, key, {{ get() {{ deprecated("interchange." + key); return input.legacy[key]; }} }});
            }}
        }}
        const acknowledge = () => {{ effects.acknowledge = true; }};
        const api = Object.freeze({{
            send: message => {{
                if (typeof message === "string") {{
                    deprecated("api.send(text)");
                    effects.sends.push(message);
                }} else if (message !== null && typeof message === "object") {{
                    effects.sends.push(JSON.parse(JSON.stringify(message)));
                }} else {{
                    throw new TypeError("api.send expects a message from api.build");
                }}
            }},
            build: (type, version, segments, options) => {{
                if (segments === undefined && typeof version === "string" && String(type).includes(":")) {{
                    deprecated("api.build(type, text)");
                    return "UNH+1+" + type + "'" + version + "UNT+0+1'";
                }}
                if (!Array.isArray(segments)) throw new TypeError("api.build expects an array of segments");
                const message = {{ type: String(type), version: String(version), segments }};
                for (const key of ["agency", "association", "implementation"]) {{
                    if (options && options[key] !== undefined) message[key] = String(options[key]);
                }}
                return message;
            }},
            ack: acknowledge,
            contrl: acknowledge,
            nextReference: () => String(++effects.reference),
            now: () => input.now,
            log: value => effects.logs.push(String(value)),
            get: key => effects.state[key],
            set: (key, value) => {{ effects.state[key] = value; }}
        }});
        return [api, interchange];
        }})();
        {source}
        {function}(vendrediInterchange, api);
        return JSON.stringify(effects);
    }})(JSON.parse({data}))"#,
        data = serde_json::to_string(input_json)?
    );
    Script::parse(Source::from_bytes(&program), None, cx).map_err(|e| anyhow!("{e}"))
}

fn invoke(bytes: &[u8]) -> Result<String> {
    let request: Request = serde_json::from_slice(bytes)?;
    let mut cx = Context::builder()
        .module_loader(Rc::new(IdleModuleLoader))
        .build()
        .map_err(|e| anyhow!("{e}"))?;
    cx.runtime_limits_mut().set_loop_iteration_limit(100_000);
    cx.runtime_limits_mut().set_recursion_limit(64);
    let script = compile(
        &mut cx,
        &request.source,
        &request.function,
        &serde_json::to_string(&request.input)?,
    )?;
    if request.input.is_none() {
        return Ok("null".into());
    }
    script
        .evaluate(&mut cx)
        .map_err(|e| anyhow!("{e}"))?
        .as_string()
        .ok_or_else(|| anyhow!("missing script result"))?
        .to_std_string()
        .map_err(|_| anyhow!("invalid script result"))
}

#[unsafe(no_mangle)]
pub extern "C" fn allocate(len: u32) -> *mut u8 {
    Box::leak(vec![0; len as usize].into_boxed_slice()).as_mut_ptr()
}

/// Packs the response pointer in the high word and its byte length in the low word.
///
/// # Safety
/// The host must pass the exact allocation returned by `allocate` in this instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn invoke_v1(ptr: *const u8, len: u32) -> u64 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let mut response = Vec::new();
    match invoke(input) {
        Ok(value) => {
            response.push(0);
            response.extend_from_slice(value.as_bytes());
        }
        Err(error) => {
            response.push(1);
            let error = error.to_string();
            response.extend_from_slice(&error.as_bytes()[..error.floor_char_boundary(1024)]);
        }
    }
    let bytes = response.leak();
    ((bytes.as_ptr() as u64) << 32) | bytes.len() as u64
}
