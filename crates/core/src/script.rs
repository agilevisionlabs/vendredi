use anyhow::{Result, anyhow, ensure};
use boa_engine::{Context, Source};
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

/// Evaluate each invocation in a fresh realm. Only completed invocations return effects.
pub fn run(source: &str, function: &str, input: &Input<'_>) -> Result<Effects> {
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
    let mut cx = Context::default();
    cx.runtime_limits_mut().set_loop_iteration_limit(100_000);
    cx.runtime_limits_mut().set_recursion_limit(64);
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
        data = serde_json::to_string(&serde_json::to_string(input)?)?
    );
    let result = cx
        .eval(Source::from_bytes(&program))
        .map_err(|e| anyhow!("{e}"))?;
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
        for bad in ["function act(){while(true){}}", "function act(){act()}"] {
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
