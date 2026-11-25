//! Cases: partner scenarios that move between workspaces.
//!
//! A case holds rules, the documents and scripts they use, sample inbound
//! messages and optional expected replies. It never holds identities or
//! channels, so one case attaches to any partner of the same standard.
use crate::{
    config::{Action, Policy, Rule, Standard},
    edi,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

/// The `vendredi_case` value this version reads and writes.
pub const FORMAT: u32 = 1;
/// The file that describes a case inside its folder.
pub const FILE: &str = "case.json";
/// The workspace folder that holds one subfolder per case.
pub const DIRECTORY: &str = "cases";
/// The pseudo case that stands for a partner's own rules.
pub const DEFAULT: &str = "default";

/// The largest file a case may contain.
pub const FILE_LIMIT: usize = 1024 * 1024;
const FILE_COUNT_LIMIT: usize = 256;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub name: String,
    /// A single `UNH`…`UNT` message. The engine adds the envelope.
    pub document: PathBuf,
}

/// A named send the partner starts on its own, such as an invoice.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Initiate {
    pub name: String,
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// The sample whose replies are checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// Or the partner-initiated send whose messages are checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiate: Option<String>,
    /// The messages Vendredi should send in reply, in order.
    pub replies: Vec<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub vendredi_case: u32,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub standard: Standard,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Overrides the partner's acknowledgement policy while the case is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledge: Option<Policy>,
    pub rules: Vec<Rule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<Sample>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initiate: Vec<Initiate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expect: Vec<Expect>,
    /// Embedded file contents for the single-file form, keyed by relative path.
    /// Import writes them next to `case.json` and removes this map.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
}

/// A case together with the contents of every file it references.
#[derive(Clone, Debug)]
pub struct Bundle {
    pub case: Case,
    pub files: BTreeMap<PathBuf, Vec<u8>>,
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id != DEFAULT
        && id.bytes().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-_.".contains(&c))
}

/// Case paths must stay inside the case folder.
pub fn relative(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && path.components().all(|c| matches!(c, Component::Normal(_))),
        "{} must be a relative path inside the case",
        path.display()
    );
    Ok(())
}

impl Case {
    pub fn parse(json: &[u8]) -> Result<Self> {
        let value: serde_json::Value = serde_json::from_slice(json).context("invalid JSON")?;
        let format = value.get("vendredi_case").and_then(|v| v.as_u64());
        ensure!(
            format == Some(u64::from(FORMAT)),
            "expected \"vendredi_case\": {FORMAT}, found {}",
            value
                .get("vendredi_case")
                .map_or("nothing".into(), |v| v.to_string())
        );
        serde_json::from_value(value).context("invalid case")
    }

    /// Every file the case refers to, with whether it holds a script.
    pub fn references(&self) -> BTreeMap<PathBuf, bool> {
        let mut files = BTreeMap::new();
        let actions = self
            .rules
            .iter()
            .flat_map(|r| &r.actions)
            .chain(self.initiate.iter().flat_map(|i| &i.actions));
        {
            for action in actions {
                match action {
                    Action::Document { document } => {
                        files.entry(document.clone()).or_insert(false);
                    }
                    Action::Script { script, .. } => {
                        files.insert(script.clone(), true);
                    }
                    _ => {}
                }
            }
        }
        for s in &self.samples {
            files.entry(s.document.clone()).or_insert(false);
        }
        for e in &self.expect {
            for r in &e.replies {
                files.entry(r.clone()).or_insert(false);
            }
        }
        files
    }

    pub fn sample(&self, name: &str) -> Option<&Sample> {
        self.samples.iter().find(|s| s.name == name)
    }
    pub fn initiated(&self, name: &str) -> Option<&Initiate> {
        self.initiate.iter().find(|s| s.name == name)
    }
}

impl Bundle {
    /// Reads `case.json` from a case folder, or a single-file case, and
    /// checks everything including script syntax.
    pub fn load(path: &Path) -> Result<Self> {
        let bundle = Self::read(path)?;
        bundle.check_scripts()?;
        Ok(bundle)
    }
    /// Like `load`, without running script syntax checks in the sandbox.
    pub fn read(path: &Path) -> Result<Self> {
        let (file, dir) = if path.is_dir() {
            (path.join(FILE), Some(path))
        } else {
            (path.to_path_buf(), None)
        };
        let text = read_limited(&file).with_context(|| file.display().to_string())?;
        let case = Case::parse(&text).with_context(|| file.display().to_string())?;
        Self::collect(case, |p| {
            let dir = dir.with_context(|| {
                format!("{} is not embedded in the single-file case", p.display())
            })?;
            read_limited(&dir.join(p))
        })
        .with_context(|| path.display().to_string())
    }

    /// Collects the referenced files, from the embedded map first, and checks
    /// the whole case.
    pub fn resolve(case: Case, read: impl FnMut(&Path) -> Result<Vec<u8>>) -> Result<Self> {
        let bundle = Self::collect(case, read)?;
        bundle.check_scripts()?;
        Ok(bundle)
    }
    fn collect(mut case: Case, mut read: impl FnMut(&Path) -> Result<Vec<u8>>) -> Result<Self> {
        let references = case.references();
        ensure!(
            references.len() <= FILE_COUNT_LIMIT,
            "a case may reference at most {FILE_COUNT_LIMIT} files"
        );
        for path in references.keys() {
            relative(path)?;
        }
        let embedded = std::mem::take(&mut case.files);
        for name in embedded.keys() {
            relative(Path::new(name))?;
            ensure!(
                references.contains_key(Path::new(name)),
                "embedded file {name} is not referenced by the case"
            );
        }
        let mut files = BTreeMap::new();
        for (path, script) in references {
            let bytes = match embedded.get(&path.to_string_lossy().into_owned()) {
                // Scripts are UTF-8 source. EDI documents are Latin-1 bytes.
                Some(text) if script => text.clone().into_bytes(),
                Some(text) => text
                    .chars()
                    .map(|c| u8::try_from(u32::from(c)))
                    .collect::<Result<Vec<_>, _>>()
                    .with_context(|| {
                        format!("{} contains a character outside Latin-1", path.display())
                    })?,
                None => read(&path).with_context(|| format!("reading {}", path.display()))?,
            };
            ensure!(bytes.len() <= FILE_LIMIT, "{} is too large", path.display());
            files.insert(path, bytes);
        }
        let bundle = Self { case, files };
        bundle.validate()?;
        Ok(bundle)
    }

    pub fn validate(&self) -> Result<()> {
        let c = &self.case;
        ensure!(
            valid_id(&c.id),
            "invalid case id {:?}: use lowercase letters, digits, '-', '_' or '.'",
            c.id
        );
        ensure!(!c.name.trim().is_empty(), "case {} needs a name", c.id);
        let mut names = BTreeSet::new();
        for s in &c.samples {
            ensure!(
                !s.name.trim().is_empty() && names.insert(&s.name),
                "empty or duplicate sample name {:?}",
                s.name
            );
        }
        let mut sends = BTreeSet::new();
        for i in &c.initiate {
            ensure!(
                !i.name.trim().is_empty() && sends.insert(&i.name),
                "empty or duplicate initiate name {:?}",
                i.name
            );
            ensure!(
                !i.actions.is_empty(),
                "initiate {} needs at least one action",
                i.name
            );
        }
        for e in &c.expect {
            match (&e.sample, &e.initiate) {
                (Some(s), None) => {
                    ensure!(names.contains(s), "expect refers to unknown sample {s:?}")
                }
                (None, Some(i)) => {
                    ensure!(sends.contains(i), "expect refers to unknown initiate {i:?}")
                }
                _ => bail!("each expect entry names either a sample or an initiate"),
            }
        }
        let Standard::Edifact = c.standard;
        for rule in &c.rules {
            ensure!(!rule.name.trim().is_empty(), "every rule needs a name");
            ensure!(
                !rule.message.trim().is_empty(),
                "rule {} needs a message type",
                rule.name
            );
        }
        for (path, script) in c.references() {
            if !script {
                edi::document(&self.files[&path])
                    .with_context(|| format!("{} is not a single message", path.display()))?;
            }
        }
        Ok(())
    }

    /// Checks the syntax of every script in the guest sandbox.
    pub fn check_scripts(&self) -> Result<()> {
        let c = &self.case;
        let actions = c
            .rules
            .iter()
            .map(|r| (format!("rule {}", r.name), &r.actions))
            .chain(
                c.initiate
                    .iter()
                    .map(|i| (format!("initiate {}", i.name), &i.actions)),
            );
        for (owner, actions) in actions {
            for action in actions {
                if let Action::Script { script, function } = action {
                    let source = std::str::from_utf8(&self.files[script])
                        .with_context(|| format!("{} is not UTF-8", script.display()))?;
                    crate::script::validate(source, function)
                        .with_context(|| format!("{owner}, script {}", script.display()))?;
                }
            }
        }
        Ok(())
    }

    /// Writes the case folder form: `case.json` and every referenced file.
    pub fn write(&self, dir: &Path) -> Result<()> {
        fs::create_dir_all(dir)?;
        for (path, bytes) in &self.files {
            let target = dir.join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(target, bytes)?;
        }
        let mut json = serde_json::to_string_pretty(&self.case)?;
        json.push('\n');
        fs::write(dir.join(FILE), json)?;
        Ok(())
    }
}

fn read_limited(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(FILE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= FILE_LIMIT, "file exceeds {FILE_LIMIT} bytes");
    Ok(bytes)
}

/// Loads every case folder under `cases/` in a workspace.
pub fn library(root: &Path) -> Result<Vec<Case>> {
    let dir = root.join(DIRECTORY);
    let mut cases = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(cases);
    };
    let mut folders: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join(FILE).is_file())
        .map(|e| e.path())
        .collect();
    folders.sort();
    for folder in folders {
        // Script syntax is checked when a case is imported or activated.
        let bundle = Bundle::read(&folder)?;
        let name = folder.file_name().unwrap().to_string_lossy();
        if bundle.case.id != name {
            bail!(
                "{}: case id {} does not match its folder {name}",
                folder.join(FILE).display(),
                bundle.case.id
            );
        }
        cases.push(bundle.case);
    }
    Ok(cases)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn single(extra: serde_json::Value) -> Vec<u8> {
        let mut v = json!({
            "vendredi_case": 1,
            "id": "orders",
            "name": "Orders",
            "standard": "edifact",
            "rules": [{"name":"Reply","message":"ORDERS","version":"D96A","actions":[{"script":"reply.js","function":"reply"},{"document":"docs/ordrsp.edi"}]}],
            "samples": [{"name":"Order","document":"order.edi"}],
            "expect": [{"sample":"Order","replies":["docs/ordrsp.edi"]}],
            "files": {
                "reply.js": "function reply(i, api) { api.log('é'); }",
                "docs/ordrsp.edi": "UNH+1+ORDRSP:D:96A:UN'FTX+Café'UNT+3+1'",
                "order.edi": "UNH+1+ORDERS:D:96A:UN'BGM+220+PO1+9'UNT+3+1'"
            }
        });
        for (k, val) in extra.as_object().unwrap() {
            if val.is_null() {
                v.as_object_mut().unwrap().remove(k);
            } else {
                v[k] = val.clone();
            }
        }
        serde_json::to_vec(&v).unwrap()
    }

    fn load(json: &[u8]) -> Result<Bundle> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("case.json");
        fs::write(&path, json)?;
        Bundle::load(&path)
    }

    #[test]
    fn single_file_and_folder_forms_load_the_same_case() -> Result<()> {
        let bundle = load(&single(json!({})))?;
        assert_eq!(
            bundle.files[Path::new("docs/ordrsp.edi")],
            b"UNH+1+ORDRSP:D:96A:UN'FTX+Caf\xe9'UNT+3+1'"
        );
        assert_eq!(
            bundle.files[Path::new("reply.js")],
            "function reply(i, api) { api.log('é'); }".as_bytes()
        );
        let dir = tempfile::tempdir()?;
        bundle.write(&dir.path().join("orders"))?;
        let folder = Bundle::load(&dir.path().join("orders"))?;
        assert!(folder.case.files.is_empty());
        assert_eq!(folder.files, bundle.files);
        fs::create_dir(dir.path().join(DIRECTORY))?;
        fs::rename(
            dir.path().join("orders"),
            dir.path().join(DIRECTORY).join("orders"),
        )?;
        assert_eq!(library(dir.path())?[0].id, "orders");
        Ok(())
    }

    #[test]
    fn invalid_cases_name_the_problem() {
        for (extra, message) in [
            (json!({"vendredi_case": 2}), "vendredi_case"),
            (json!({"id": "Bad Id"}), "invalid case id"),
            (json!({"id": "default"}), "invalid case id"),
            (json!({"standard": "tradacoms"}), "invalid case"),
            (json!({"unknown": true}), "unknown"),
            (
                json!({"samples": [{"name":"Order","document":"../order.edi"}]}),
                "relative path",
            ),
            (
                json!({"expect": [{"sample":"Missing","replies":[]}]}),
                "unknown sample",
            ),
            (
                json!({"expect": [{"replies":[]}]}),
                "either a sample or an initiate",
            ),
            (
                json!({"initiate": [{"name":"Invoice","actions":[]}]}),
                "at least one action",
            ),
            (
                json!({"files": {"reply.js": "function reply( {", "docs/ordrsp.edi": "UNH+1+ORDRSP:D:96A:UN'UNT+2+1'", "order.edi": "UNH+1+ORDERS:D:96A:UN'UNT+2+1'"}}),
                "reply.js",
            ),
            (
                json!({"files": {"reply.js": "function reply(){}", "docs/ordrsp.edi": "not edi", "order.edi": "UNH+1+ORDERS:D:96A:UN'UNT+2+1'"}}),
                "docs/ordrsp.edi",
            ),
            (
                json!({"files": {"reply.js": "function reply(){}", "docs/ordrsp.edi": "UNH+1+ORDRSP:D:96A:UN'UNT+2+1'"}}),
                "order.edi",
            ),
            (
                json!({"files": {"extra.edi": "x", "reply.js": "function reply(){}", "docs/ordrsp.edi": "UNH+1+ORDRSP:D:96A:UN'UNT+2+1'", "order.edi": "UNH+1+ORDERS:D:96A:UN'UNT+2+1'"}}),
                "extra.edi",
            ),
        ] {
            let error = format!("{:#}", load(&single(extra.clone())).unwrap_err());
            assert!(error.contains(message), "{extra}: {error}");
        }
    }
}
