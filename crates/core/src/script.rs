use anyhow::{Result, anyhow, ensure};
use boa_engine::{Context, Script, Source};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct Input<'a> {
    pub interchange: &'a Value,
    pub state: &'a BTreeMap<String, Value>,
    pub reference: i64,
    pub now: i64,
}

#[derive(Deserialize, Default)]
pub struct Effects {
    pub sends: Vec<String>,
    pub logs: Vec<String>,
    pub state: BTreeMap<String, Value>,
    pub reference: i64,
    pub acknowledge: bool,
}

pub fn validate(source: &str, function: &str) -> Result<()> {
    compile(&mut Context::default(), source, function, "null")?;
    Ok(())
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
        const effects = {{ sends: [], logs: [], state: input.state, reference: input.reference, acknowledge: false }};
        const api = Object.freeze({{
            send: message => effects.sends.push(String(message)),
            build: (type, body) => "UNH+1+" + type + "'" + body + "UNT+0+1'",
            contrl: () => {{ effects.acknowledge = true; }},
            nextReference: () => String(++effects.reference),
            now: () => input.now,
            log: value => effects.logs.push(String(value)),
            get: key => effects.state[key],
            set: (key, value) => {{ effects.state[key] = value; }}
        }});
        {source}
        {function}(input.interchange, api);
        return JSON.stringify(effects);
    }})(JSON.parse({data}))"#,
        data = serde_json::to_string(input_json)?
    );
    Script::parse(Source::from_bytes(&program), None, cx).map_err(|e| anyhow!("{e}"))
}

/// Evaluate each invocation in a fresh realm. Only completed invocations return effects.
pub fn run(source: &str, function: &str, input: &Input<'_>) -> Result<Effects> {
    let mut cx = Context::default();
    cx.runtime_limits_mut().set_loop_iteration_limit(100_000);
    cx.runtime_limits_mut().set_recursion_limit(64);
    let script = compile(&mut cx, source, function, &serde_json::to_string(input)?)?;
    let result = {
        // Boa yields after each quantum. An invocation has no asynchronous host I/O,
        // so each pending poll consumes another slice of its instruction budget.
        let mut evaluation = std::pin::pin!(script.evaluate_async_with_budget(&mut cx, 1024));
        let mut task = std::task::Context::from_waker(std::task::Waker::noop());
        (0..1024)
            .find_map(
                |_| match std::future::Future::poll(evaluation.as_mut(), &mut task) {
                    std::task::Poll::Ready(result) => Some(result),
                    std::task::Poll::Pending => None,
                },
            )
            .ok_or_else(|| anyhow!("script exceeded its instruction budget"))?
            .map_err(|e| anyhow!("{e}"))?
    };
    let json = result
        .as_string()
        .ok_or_else(|| anyhow!("missing script result"))?
        .to_std_string_lossy();
    Ok(serde_json::from_str(&json)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_recovers_after_each_limit() {
        let input = Input {
            interchange: &Value::Null,
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
