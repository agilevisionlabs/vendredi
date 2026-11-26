//! `vendredi case test`: run a case's samples and partner-initiated sends in
//! a temporary workspace and compare what it sends with the expected replies.
use crate::{
    case::{self, Bundle},
    config::{Standard, Workspace},
    edi,
    engine::{Clock, Delivery, Engine},
    interchange,
};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};

/// Fixed identities, so expected acknowledgements are the same everywhere.
pub const WORKSPACE_IDENTITY: [&str; 2] = ["BUYER", "ZZ"];
pub const PARTNER_IDENTITY: [&str; 2] = ["PARTNER", "ZZ"];
/// 2026-01-01T00:00:00Z, the clock every check starts from.
pub const START: i64 = 1_767_225_600_000;
const STEPS: usize = 1000;

struct Manual(AtomicI64);
impl Clock for Manual {
    fn millis(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// The result of one `expect` entry.
#[derive(Debug)]
pub struct Outcome {
    pub case: String,
    /// `sample Order` or `initiate Invoice`.
    pub check: String,
    /// Empty when the replies matched.
    pub failures: Vec<String>,
}

pub fn run(bundle: &Bundle) -> Result<Vec<Outcome>> {
    let c = &bundle.case;
    c.expect
        .iter()
        .map(|e| {
            let check = match (&e.sample, &e.initiate) {
                (Some(s), _) => format!("sample {s}"),
                (_, Some(i)) => format!("initiate {i}"),
                _ => unreachable!("validated case"),
            };
            let expected = e
                .replies
                .iter()
                .map(|p| {
                    interchange::document(bundle.case.standard, &bundle.files[p])?
                        .text()
                        .with_context(|| p.display().to_string())
                })
                .collect::<Result<Vec<_>>>()?;
            let failures = match replies(bundle, e) {
                Ok(actual) => compare(&expected, &actual, &e.replies),
                Err(error) => vec![format!("{error:#}")],
            };
            Ok(Outcome {
                case: c.id.clone(),
                check,
                failures,
            })
        })
        .collect()
}

fn replies(bundle: &Bundle, e: &case::Expect) -> Result<Vec<String>> {
    let dir = tempfile::tempdir()?;
    let id = &bundle.case.id;
    bundle.write(&dir.path().join(case::DIRECTORY).join(id))?;
    let quote = |v: &[&str]| format!("[{:?}, {:?}]", v[0], v[1]);
    // X12 checks use the first rule's GS08 version, or 005010.
    let x12 = match bundle.case.standard {
        Standard::Edifact => String::new(),
        Standard::X12 => {
            let version = bundle
                .case
                .rules
                .iter()
                .filter_map(|r| r.version.as_deref())
                .find(|v| v.len() >= 6 && v[..6].bytes().all(|b| b.is_ascii_digit()))
                .unwrap_or("005010");
            format!("x12 = {{ version = {version:?} }}\n")
        }
    };
    fs::write(
        dir.path().join("vendredi.toml"),
        format!(
            "listen = \"127.0.0.1:0\"\nidentity = {}\nretention_days = 1\n\n[[partners]]\nname = \"Partner\"\nstandard = {:?}\nidentity = {}\ncharset = \"UNOC\"\nacknowledge = \"always\"\nreceive = [{{ path = \"/partner\" }}]\nsend = {{ directory = \"out\" }}\n{x12}case = {id:?}\n",
            quote(&WORKSPACE_IDENTITY),
            serde_json::to_value(bundle.case.standard)?
                .as_str()
                .unwrap_or_default(),
            quote(&PARTNER_IDENTITY),
        ),
    )?;
    let clock = Arc::new(Manual(AtomicI64::new(START)));
    let mut engine = Engine::open(Workspace::load(dir.path())?, clock.clone())?;
    match (&e.sample, &e.initiate) {
        (Some(s), _) => {
            engine.sample("Partner", s)?;
        }
        (_, Some(i)) => engine.initiate_case("Partner", i)?,
        _ => unreachable!("validated case"),
    }
    let mut sent: Vec<Delivery> = engine.take_deliveries();
    for _ in 0..STEPS {
        let Some(wait) = engine.next_wait() else {
            break;
        };
        clock.0.fetch_add(wait.as_millis() as i64, Ordering::SeqCst);
        engine.tick()?;
        sent.extend(engine.take_deliveries());
    }
    ensure!(
        engine.next_wait().is_none(),
        "the case kept scheduling actions"
    );
    let errors: Vec<_> = engine
        .take_events()
        .into_iter()
        .filter_map(|v| v["error"].as_str().map(str::to_owned))
        .collect();
    ensure!(errors.is_empty(), "an action failed: {}", errors.join("; "));
    let mut texts = Vec::new();
    for d in sent {
        let parsed = interchange::parse(&d.bytes);
        let mut messages = Vec::new();
        for i in &parsed.interchanges {
            messages.extend(i.message_texts()?);
        }
        if messages.is_empty() {
            texts.push(format!("unparseable transfer: {}", edi::latin1(&d.bytes)));
        }
        texts.extend(messages);
    }
    Ok(texts)
}

/// Describes each mismatch with the first differing byte.
fn compare(expected: &[String], actual: &[String], files: &[std::path::PathBuf]) -> Vec<String> {
    let mut failures = Vec::new();
    for (n, pair) in expected.iter().zip(actual).enumerate() {
        let (want, got) = pair;
        if want == got {
            continue;
        }
        let at = want
            .bytes()
            .zip(got.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(want.len().min(got.len()));
        failures.push(format!(
            "reply {} ({}) differs at byte {at}\n    expected: {want}\n    actual:   {got}\n              {}^",
            n + 1,
            files[n].display(),
            " ".repeat(got[..got.floor_char_boundary(at)].chars().count()),
        ));
    }
    if expected.len() != actual.len() {
        failures.push(format!(
            "expected {} repl{}, Vendredi sent {}",
            expected.len(),
            if expected.len() == 1 { "y" } else { "ies" },
            actual.len()
        ));
        for extra in actual.iter().skip(expected.len()) {
            failures.push(format!("    unexpected: {extra}"));
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import;

    #[test]
    fn starter_cases_pass_and_changes_fail_with_a_diff() -> Result<()> {
        let catalog = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/cases");
        let incoming = import::gather(catalog, &mut import::read_file, 0)?;
        for i in &incoming {
            for outcome in run(&i.bundle)? {
                assert!(
                    outcome.failures.is_empty(),
                    "{}: {:#?}",
                    outcome.case,
                    outcome
                );
            }
        }
        let mut bundle = incoming
            .into_iter()
            .find(|i| i.bundle.case.id == "confirm-orders")
            .unwrap()
            .bundle;
        bundle.files.insert(
            "ordrsp.edi".into(),
            b"UNH+1+ORDRSP:D:96A:UN'BGM+231+PO1002+29'UNT+3+1'".to_vec(),
        );
        let outcome = &run(&bundle)?[0];
        assert_eq!(outcome.failures.len(), 1);
        assert!(
            outcome.failures[0].contains("reply 2 (ordrsp.edi) differs at byte 35"),
            "{}",
            outcome.failures[0]
        );
        bundle.case.expect[0].replies.pop();
        let outcome = &run(&bundle)?[0];
        assert!(outcome.failures[0].contains("expected 1 reply, Vendredi sent 2"));
        Ok(())
    }
}
