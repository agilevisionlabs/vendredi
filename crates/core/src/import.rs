//! Importing cases from a folder, a file or an HTTPS URL into a workspace.
//!
//! Everything is fetched and validated before anything is written, and
//! nothing imported runs: scripts are only parsed. Each installed case
//! records where it came from so a later import can show what changed.
use crate::case::{self, Bundle, Case};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// The `vendredi_catalog` value this version reads.
pub const CATALOG_FORMAT: u32 = 1;
/// The catalog file looked for in a folder without `case.json`.
pub const CATALOG_FILE: &str = "catalog.json";
/// The provenance record written next to an installed `case.json`.
pub const SOURCE_FILE: &str = ".vendredi-source.json";
const CASE_LIMIT: usize = 500;
const TOTAL_LIMIT: usize = 32 * 1024 * 1024;

/// Where a case, catalog or file comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Location {
    File(PathBuf),
    Url(url::Url),
}
impl Location {
    pub fn parse(source: &str) -> Result<Self> {
        if source.contains("://") {
            let url = url::Url::parse(source).with_context(|| format!("invalid URL {source}"))?;
            ensure!(url.scheme() == "https", "only https URLs can be imported");
            Ok(Self::Url(url))
        } else {
            Ok(Self::File(PathBuf::from(source)))
        }
    }
    /// A path relative to this location's folder.
    pub fn join(&self, relative: &str) -> Result<Self> {
        case::relative(Path::new(relative.trim_end_matches('/')))?;
        Ok(match self {
            Self::File(p) => Self::File(p.parent().unwrap_or(Path::new("")).join(relative)),
            Self::Url(u) => Self::Url(u.join(relative)?),
        })
    }
}
impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(p) => write!(f, "{}", p.display()),
            Self::Url(u) => write!(f, "{u}"),
        }
    }
}

/// Reads the bytes at a location. Callers enforce size and time limits.
pub type Fetch<'a> = dyn FnMut(&Location) -> Result<Vec<u8>> + 'a;

/// Reads local files with the per-file limit that applies to fetched ones.
pub fn read_file(location: &Location) -> Result<Vec<u8>> {
    match location {
        Location::File(p) => {
            use std::io::Read;
            let mut bytes = Vec::new();
            fs::File::open(p)
                .with_context(|| p.display().to_string())?
                .take(case::FILE_LIMIT as u64 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= case::FILE_LIMIT,
                "{} exceeds {} bytes",
                p.display(),
                case::FILE_LIMIT
            );
            Ok(bytes)
        }
        Location::Url(u) => bail!("{u} needs a network fetch"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    vendredi_catalog: u32,
    #[serde(default)]
    #[allow(dead_code)]
    name: String,
    cases: Vec<String>,
}

/// A validated case ready to install, with its provenance.
#[derive(Clone, Debug)]
pub struct Incoming {
    pub bundle: Bundle,
    pub source: Source,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Source {
    /// The case file or folder it was imported from.
    pub source: String,
    /// SHA-256 of the case JSON as fetched.
    pub sha256: String,
    /// Unix milliseconds of the import.
    pub imported: i64,
}

/// Fetches and validates every case a source names. A source is a case
/// folder, a case or catalog JSON file, a folder with `catalog.json`, or an
/// HTTPS URL of a case or catalog JSON file.
pub fn gather(source: &str, fetch: &mut Fetch<'_>, now: i64) -> Result<Vec<Incoming>> {
    let mut location = Location::parse(source)?;
    if let Location::File(p) = &location
        && p.is_dir()
    {
        location = if p.join(case::FILE).is_file() {
            Location::File(p.join(case::FILE))
        } else if p.join(CATALOG_FILE).is_file() {
            Location::File(p.join(CATALOG_FILE))
        } else {
            bail!(
                "{} contains neither {} nor {CATALOG_FILE}",
                p.display(),
                case::FILE
            );
        };
    }
    let mut total = 0;
    let mut counted = |bytes: Vec<u8>| -> Result<Vec<u8>> {
        total += bytes.len();
        ensure!(
            total <= TOTAL_LIMIT,
            "the import exceeds {TOTAL_LIMIT} bytes"
        );
        Ok(bytes)
    };
    let bytes = counted(fetch(&location).with_context(|| location.to_string())?)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).with_context(|| format!("{location}: invalid JSON"))?;
    let entries = if value.get("vendredi_catalog").is_some() {
        let catalog: Catalog = serde_json::from_value(value)
            .with_context(|| format!("{location}: invalid catalog"))?;
        ensure!(
            catalog.vendredi_catalog == CATALOG_FORMAT,
            "{location}: expected \"vendredi_catalog\": {CATALOG_FORMAT}"
        );
        ensure!(
            catalog.cases.len() <= CASE_LIMIT,
            "{location}: a catalog may list at most {CASE_LIMIT} cases"
        );
        let mut entries = Vec::new();
        for entry in &catalog.cases {
            let mut at = location
                .join(entry)
                .with_context(|| format!("{location}: catalog entry {entry}"))?;
            if entry.ends_with('/') || matches!(&at, Location::File(p) if p.is_dir()) {
                at = at.join_folder(case::FILE)?;
            }
            let bytes = counted(fetch(&at).with_context(|| at.to_string())?)?;
            entries.push((at, bytes));
        }
        entries
    } else {
        vec![(location, bytes)]
    };
    let mut incoming = Vec::new();
    let mut ids = BTreeMap::new();
    for (at, bytes) in entries {
        let case = Case::parse(&bytes).with_context(|| at.to_string())?;
        let bundle = Bundle::resolve(case, |p| {
            let file = at.join(&p.to_string_lossy())?;
            counted(fetch(&file).with_context(|| file.to_string())?)
        })
        .with_context(|| at.to_string())?;
        if let Some(previous) = ids.insert(bundle.case.id.clone(), at.to_string()) {
            bail!("{at} and {previous} both define case {}", bundle.case.id);
        }
        let shown = match &at {
            Location::File(p) => {
                let p = p.canonicalize().unwrap_or_else(|_| p.clone());
                if p.file_name() == Some(case::FILE.as_ref()) {
                    p.parent().unwrap_or(&p).display().to_string()
                } else {
                    p.display().to_string()
                }
            }
            other => other.to_string(),
        };
        incoming.push(Incoming {
            bundle,
            source: Source {
                source: shown,
                sha256: hex(&Sha256::digest(&bytes)),
                imported: now,
            },
        });
    }
    ensure!(!incoming.is_empty(), "the catalog lists no cases");
    Ok(incoming)
}
impl Location {
    fn join_folder(&self, file: &str) -> Result<Self> {
        Ok(match self {
            Self::File(p) => Self::File(p.join(file)),
            Self::Url(u) => {
                let mut u = u.clone();
                if !u.path().ends_with('/') {
                    u.set_path(&format!("{}/", u.path()));
                }
                Self::Url(u.join(file)?)
            }
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What an import would do to one case.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    New,
    Unchanged,
    /// Differences from the installed copy, one line each.
    Changed(Vec<String>),
}

pub fn plan(root: &Path, incoming: &[Incoming]) -> Result<Vec<Change>> {
    incoming
        .iter()
        .map(|i| {
            let dir = root.join(case::DIRECTORY).join(&i.bundle.case.id);
            if !dir.join(case::FILE).exists() {
                return Ok(Change::New);
            }
            let installed = Bundle::read(&dir)
                .with_context(|| format!("reading the installed case {}", dir.display()))?;
            let lines = differences(&installed, &i.bundle)?;
            Ok(if lines.is_empty() {
                Change::Unchanged
            } else {
                Change::Changed(lines)
            })
        })
        .collect()
}

fn differences(old: &Bundle, new: &Bundle) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    let a = serde_json::to_value(&old.case)?;
    let b = serde_json::to_value(&new.case)?;
    if let (Some(a), Some(b)) = (a.as_object(), b.as_object()) {
        for key in a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))) {
            if a.get(key) != b.get(key) {
                lines.push(format!("{} {key}", case::FILE));
            }
        }
    }
    for (path, bytes) in &new.files {
        match old.files.get(path) {
            None => lines.push(format!("added {}", path.display())),
            Some(previous) if previous != bytes => {
                lines.push(format!("changed {}", path.display()))
            }
            _ => {}
        }
    }
    for path in old.files.keys().filter(|p| !new.files.contains_key(*p)) {
        lines.push(format!("removed {}", path.display()));
    }
    Ok(lines)
}

/// Installs the cases, replacing changed ones only when `replace` is set.
/// Nothing is written unless every case can be installed.
pub fn apply(root: &Path, incoming: &[Incoming], replace: bool) -> Result<Vec<Change>> {
    let changes = plan(root, incoming)?;
    let blocked: Vec<_> = incoming
        .iter()
        .zip(&changes)
        .filter(|(_, c)| matches!(c, Change::Changed(_)))
        .map(|(i, _)| i.bundle.case.id.as_str())
        .collect();
    ensure!(
        replace || blocked.is_empty(),
        "already installed with different content: {}",
        blocked.join(", ")
    );
    let cases = root.join(case::DIRECTORY);
    fs::create_dir_all(&cases)?;
    for (i, change) in incoming.iter().zip(&changes) {
        if *change == Change::Unchanged {
            continue;
        }
        let id = &i.bundle.case.id;
        let staging = cases.join(format!(".import-{id}"));
        let retired = cases.join(format!(".replaced-{id}"));
        for dir in [&staging, &retired] {
            if dir.exists() {
                fs::remove_dir_all(dir)?;
            }
        }
        i.bundle.write(&staging)?;
        let mut source = serde_json::to_string_pretty(&i.source)?;
        source.push('\n');
        fs::write(staging.join(SOURCE_FILE), source)?;
        let target = cases.join(id);
        if target.exists() {
            fs::rename(&target, &retired)?;
        }
        fs::rename(&staging, &target)?;
        if retired.exists() {
            fs::remove_dir_all(&retired)?;
        }
    }
    Ok(changes)
}

/// The provenance of an installed case, when it was imported.
pub fn source(root: &Path, id: &str) -> Option<Source> {
    let path = root.join(case::DIRECTORY).join(id).join(SOURCE_FILE);
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starter() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cases")
    }

    #[test]
    fn folder_catalog_and_url_resolution() -> Result<()> {
        let catalog = starter();
        let incoming = gather(&catalog.display().to_string(), &mut read_file, 7)?;
        assert!(incoming.len() >= 10);
        assert!(
            incoming
                .iter()
                .all(|i| i.source.imported == 7 && i.source.sha256.len() == 64)
        );
        let base = Location::parse("https://example.com/library/catalog.json")?;
        assert_eq!(
            base.join("orders/case.json")?.to_string(),
            "https://example.com/library/orders/case.json"
        );
        assert!(base.join("../secret.json").is_err());
        assert!(Location::parse("http://example.com/c.json").is_err());
        assert_eq!(
            Location::parse("https://example.com/library/orders")?
                .join_folder("case.json")?
                .to_string(),
            "https://example.com/library/orders/case.json"
        );
        Ok(())
    }

    #[test]
    fn imports_are_atomic_and_report_differences() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = starter().join("confirm-orders.json").display().to_string();
        let first = gather(&source, &mut read_file, 1)?;
        assert_eq!(apply(dir.path(), &first, false)?, [Change::New]);
        assert_eq!(apply(dir.path(), &first, false)?, [Change::Unchanged]);
        let installed = dir.path().join("cases/confirm-orders");
        assert_eq!(
            Path::new(&super::source(dir.path(), "confirm-orders").unwrap().source),
            Path::new(&source).canonicalize()?
        );
        let copy = dir.path().join("edited");
        Bundle::read(&installed)?.write(&copy)?;
        fs::write(
            copy.join("confirm.js"),
            "function confirm(i, api) { api.ack(); }",
        )?;
        fs::remove_file(copy.join("ack.edi"))?;
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(copy.join("case.json"))?)?;
        json["name"] = "Renamed".into();
        json["expect"] = serde_json::json!([]);
        fs::write(copy.join("case.json"), serde_json::to_vec(&json)?)?;
        let edited = gather(&copy.display().to_string(), &mut read_file, 2)?;
        let mut both = edited.clone();
        both.extend(gather(
            &starter().join("reject-orders.json").display().to_string(),
            &mut read_file,
            2,
        )?);
        let Change::Changed(lines) = &plan(dir.path(), &both)?[0] else {
            panic!("expected a change");
        };
        assert_eq!(
            lines,
            &[
                "case.json expect",
                "case.json name",
                "changed confirm.js",
                "removed ack.edi",
                "removed ordrsp.edi"
            ]
        );
        // A refused replacement writes neither the changed nor the new case.
        assert!(apply(dir.path(), &both, false).is_err());
        assert!(!dir.path().join("cases/reject-orders").exists());
        apply(dir.path(), &both, true)?;
        assert!(!installed.join("ack.edi").exists());
        assert_eq!(Bundle::read(&installed)?.case.name, "Renamed");
        assert_eq!(
            super::source(dir.path(), "confirm-orders")
                .unwrap()
                .imported,
            2
        );
        Ok(())
    }

    #[test]
    fn invalid_sources_write_nothing() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let library = dir.path().join("library");
        fs::create_dir(&library)?;
        fs::copy(
            starter().join("reject-orders.json"),
            library.join("reject-orders.json"),
        )?;
        fs::write(
            library.join("broken.json"),
            b"{\"vendredi_case\": 1, \"id\": \"broken\"}",
        )?;
        fs::write(
            library.join(CATALOG_FILE),
            br#"{"vendredi_catalog": 1, "cases": ["reject-orders.json", "broken.json"]}"#,
        )?;
        let error = format!(
            "{:#}",
            gather(&library.display().to_string(), &mut read_file, 0).unwrap_err()
        );
        assert!(error.contains("broken.json"), "{error}");
        fs::write(
            library.join(CATALOG_FILE),
            br#"{"vendredi_catalog": 1, "cases": ["../outside.json"]}"#,
        )?;
        let error = format!(
            "{:#}",
            gather(&library.display().to_string(), &mut read_file, 0).unwrap_err()
        );
        assert!(error.contains("relative path"), "{error}");
        fs::write(
            library.join(CATALOG_FILE),
            br#"{"vendredi_catalog": 1, "cases": ["reject-orders.json", "reject-orders.json"]}"#,
        )?;
        let error = format!(
            "{:#}",
            gather(&library.display().to_string(), &mut read_file, 0).unwrap_err()
        );
        assert!(error.contains("both define"), "{error}");
        Ok(())
    }
}
