use crate::{
    case::{self, Case},
    edi::{self, Syntax},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub listen: SocketAddr,
    pub identity: Vec<String>,
    pub retention_days: u32,
    pub partners: Vec<Partner>,
    #[serde(skip)]
    pub root: PathBuf,
    /// Every case installed under `cases/`, loaded and validated.
    #[serde(skip)]
    pub cases: Vec<Case>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Partner {
    pub name: String,
    #[serde(default, skip_serializing_if = "Standard::is_default")]
    pub standard: Standard,
    pub identity: Vec<String>,
    pub charset: String,
    pub acknowledge: Policy,
    pub receive: Vec<Receive>,
    pub send: SendChannel,
    #[serde(default)]
    pub separators: Option<String>,
    /// The partner's own rules, which form its default case.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Installed cases attached to this partner.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cases: Vec<String>,
    /// The active case. Absent or `default` selects the partner's own rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<String>,
    /// The active case's rules and settings, resolved when the workspace loads.
    #[serde(skip)]
    pub active: Option<Active>,
}
/// A case as it runs for one partner. Rule file paths are relative to the
/// workspace root.
#[derive(Clone, Debug)]
pub struct Active {
    pub id: String,
    pub rules: Vec<Rule>,
    pub acknowledge: Option<Policy>,
    pub initiate: Vec<case::Initiate>,
}
impl Partner {
    /// The rules of the active case, or the partner's own rules.
    pub fn scenario(&self) -> &[Rule] {
        self.active.as_ref().map_or(&self.rules, |a| &a.rules)
    }
    pub fn policy(&self) -> Policy {
        self.active
            .as_ref()
            .and_then(|a| a.acknowledge)
            .unwrap_or(self.acknowledge)
    }
    /// The id of the active case, `default` for the partner's own rules.
    pub fn case_id(&self) -> &str {
        self.active.as_ref().map_or(case::DEFAULT, |a| &a.id)
    }
    /// Selects a case from the workspace library, or the default case.
    pub fn activate(&mut self, cases: &[Case], id: &str) -> Result<()> {
        if id == case::DEFAULT {
            self.active = None;
            self.case = None;
            return Ok(());
        }
        let c = cases
            .iter()
            .find(|c| c.id == id)
            .with_context(|| format!("case {id} is not installed in this workspace"))?;
        ensure!(
            c.standard == self.standard,
            "case {id} is written for {} but partner {} uses {}",
            c.standard.label(),
            self.name,
            self.standard.label()
        );
        let folder = Path::new(case::DIRECTORY).join(id);
        let mut rules = c.rules.clone();
        let mut initiate = c.initiate.clone();
        for action in rules
            .iter_mut()
            .flat_map(|r| &mut r.actions)
            .chain(initiate.iter_mut().flat_map(|i| &mut i.actions))
        {
            match action {
                Action::Document { document: path } | Action::Script { script: path, .. } => {
                    *path = folder.join(&*path);
                }
                _ => {}
            }
        }
        self.active = Some(Active {
            id: id.into(),
            rules,
            acknowledge: c.acknowledge,
            initiate,
        });
        self.case = Some(id.into());
        if !self.cases.iter().any(|c| c == id) {
            self.cases.push(id.into());
        }
        Ok(())
    }
    pub fn syntax(&self) -> Result<Syntax> {
        let Some(s) = &self.separators else {
            return Ok(Syntax::default());
        };
        let syntax = Syntax(
            s.as_bytes()
                .try_into()
                .context("expected six service characters")?,
        );
        syntax.validate()?;
        Ok(syntax)
    }
}
/// The EDI standard a partner exchanges and a case is written for.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Standard {
    #[default]
    Edifact,
}
impl Standard {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Edifact => "EDIFACT",
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Always,
    Requested,
    Never,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Receive {
    Http { path: String },
    Folder { directory: PathBuf, suffix: String },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum SendChannel {
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    Folder {
        directory: PathBuf,
    },
}

impl SendChannel {
    pub fn metadata(&self) -> serde_json::Value {
        match self {
            Self::Http { url, headers } => serde_json::json!({"url":url,"headers":headers}),
            Self::Folder { directory } => serde_json::json!({"directory":directory}),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    /// EDIFACT message type or X12 transaction set, such as `ORDERS` or `850`.
    pub message: String,
    /// EDIFACT directory version and release (`D96A`) or X12 `GS08` (`005010`).
    /// A bare EDIFACT directory version such as `D` matches every release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Read from 0.1 workspaces, which split `D96A` into `version = "D"` and
    /// `release = "96A"`. Editors write the combined `version` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    pub actions: Vec<Action>,
}
impl Rule {
    pub fn matches(&self, m: &edi::Message) -> bool {
        self.message == m.kind()
            && match (&self.version, &self.release) {
                (None, None) => true,
                (version, Some(release)) => {
                    version.as_ref().is_none_or(|v| v == m.directory()) && release == m.release()
                }
                (Some(version), None) => *version == m.version() || version == m.directory(),
            }
    }
    /// The version as editors show and write it, merging a legacy `release`.
    pub fn combined_version(&self) -> Option<String> {
        match (&self.version, &self.release) {
            (v, Some(r)) => Some(format!("{}{r}", v.as_deref().unwrap_or_default())),
            (v, None) => v.clone(),
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Action {
    Document { document: PathBuf },
    Script { script: PathBuf, function: String },
    Delay { delay_ms: u32 },
    Fault { fault: Fault },
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    Reject,
    SuppressAck,
    DuplicateReference,
    WrongCount,
    Http500,
    SlowResponse,
    NoResponse,
    Duplicate,
    Malformed,
}

impl Workspace {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("vendredi.toml");
        let mut w: Self =
            toml::from_str(&fs::read_to_string(&path).with_context(|| path.display().to_string())?)
                .with_context(|| path.display().to_string())?;
        w.root = dir.canonicalize()?;
        w.validate().with_context(|| path.display().to_string())?;
        w.cases = case::library(&w.root)?;
        let cases = std::mem::take(&mut w.cases);
        for p in &mut w.partners {
            for id in &p.cases {
                ensure!(
                    cases.iter().any(|c| c.id == *id),
                    "{}: partner {} attaches case {id}, which is not installed under {}/",
                    path.display(),
                    p.name,
                    case::DIRECTORY
                );
            }
            if let Some(id) = p.case.clone() {
                p.activate(&cases, &id)
                    .with_context(|| path.display().to_string())?;
                if id != case::DEFAULT {
                    case::Bundle::load(&w.root.join(case::DIRECTORY).join(&id))?;
                }
            }
        }
        w.cases = cases;
        Ok(w)
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.identity.is_empty() && !self.identity[0].is_empty(),
            "workspace identity required"
        );
        ensure!(!self.partners.is_empty(), "at least one partner required");
        let mut names = HashSet::new();
        let mut paths = HashSet::new();
        let mut folders = HashSet::new();
        let mut outputs = HashSet::new();
        for p in &self.partners {
            ensure!(
                !p.name.is_empty() && names.insert(&p.name),
                "empty or duplicate partner name"
            );
            ensure!(
                !p.identity.is_empty() && !p.identity[0].is_empty(),
                "partner identity required"
            );
            ensure!(
                matches!(p.charset.as_str(), "UNOA" | "UNOB" | "UNOC"),
                "unsupported UNB charset"
            );
            p.syntax()?;
            ensure!(!p.receive.is_empty(), "partner requires a receive channel");
            for r in &p.receive {
                match r {
                    Receive::Http { path } => ensure!(
                        path.starts_with('/') && paths.insert(path),
                        "invalid or ambiguous receive path {path}"
                    ),
                    Receive::Folder { directory, suffix } => {
                        ensure!(
                            !suffix.is_empty() && !suffix.contains(['/', '\\']),
                            "invalid filename suffix"
                        );
                        let path = channel_directory(&self.root, directory)?;
                        ensure!(folders.insert(path), "ambiguous receive folder");
                    }
                }
            }
            match &p.send {
                SendChannel::Http { url, .. } => ensure!(
                    url.starts_with("http://") && url.len() > 7,
                    "expected HTTP send URL"
                ),
                SendChannel::Folder { directory } => {
                    outputs.insert(channel_directory(&self.root, directory)?);
                }
            }
            for rule in &p.rules {
                for action in &rule.actions {
                    let check = (|| -> Result<()> {
                        match action {
                            Action::Document { document } => {
                                edi::document(
                                    &fs::read(self.root.join(document))
                                        .with_context(|| document.display().to_string())?,
                                )?;
                            }
                            Action::Script { script, .. } => {
                                crate::script::read_source(self.root.join(script))
                                    .with_context(|| script.display().to_string())?;
                            }
                            _ => {}
                        }
                        Ok(())
                    })();
                    check.with_context(|| format!("partner {}, rule {}", p.name, rule.name))?;
                }
            }
        }
        ensure!(
            outputs.is_disjoint(&folders),
            "receive and send folders overlap"
        );
        Ok(())
    }
}
/// Records a partner's active case in `vendredi.toml`, keeping the rest of the
/// file as written. The default case removes the setting.
pub fn persist_case(root: &Path, partner: &str, id: &str) -> Result<()> {
    use toml_edit::{Array, DocumentMut, value};
    let path = root.join("vendredi.toml");
    let mut doc: DocumentMut = fs::read_to_string(&path)?.parse()?;
    let partners = doc
        .get_mut("partners")
        .and_then(|p| p.as_array_of_tables_mut())
        .context("expected [[partners]] tables")?;
    let table = partners
        .iter_mut()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(partner))
        .with_context(|| format!("partner {partner} is not in vendredi.toml"))?;
    if id == case::DEFAULT {
        table.remove("case");
    } else {
        let attached = table
            .entry("cases")
            .or_insert_with(|| value(Array::new()))
            .as_array_mut()
            .context("expected cases to be a list")?;
        if !attached.iter().any(|v| v.as_str() == Some(id)) {
            attached.push(id);
        }
        table.insert("case", value(id));
    }
    let mut file = tempfile_in(root)?;
    std::io::Write::write_all(&mut file.0, doc.to_string().as_bytes())?;
    file.0.sync_all()?;
    fs::rename(&file.1, &path)?;
    Ok(())
}
fn tempfile_in(root: &Path) -> Result<(fs::File, PathBuf)> {
    for n in 0..100u32 {
        let path = root.join(format!(".vendredi.toml.{}.{n}", std::process::id()));
        match fs::File::options().write(true).create_new(true).open(&path) {
            Ok(f) => return Ok((f, path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("could not create a temporary configuration file")
}
fn channel_directory(root: &Path, path: &Path) -> Result<PathBuf> {
    let dir = root.join(path);
    fs::create_dir_all(&dir)?;
    Ok(dir.canonicalize()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_workspace_loads_and_identifies_bad_rule() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(
            dir.path().join("vendredi.toml"),
            include_str!("../../../examples/orders/vendredi.toml"),
        )?;
        let err = Workspace::load(dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains("Confirm orders"));
        fs::write(
            dir.path().join("confirmation.edi"),
            include_bytes!("../../../examples/orders/confirmation.edi"),
        )?;
        let w = Workspace::load(dir.path())?;
        let p = edi::parse(include_bytes!("../../../examples/orders/order.edi"));
        assert!(w.partners[0].rules[0].matches(&p.interchanges[0].messages[0]));
        assert_eq!(w.partners[0].standard, Standard::Edifact);
        Ok(())
    }
    #[test]
    fn rule_versions_cover_combined_and_legacy_forms() {
        let parsed = edi::parse(include_bytes!("../../../examples/orders/order.edi"));
        let order = &parsed.interchanges[0].messages[0];
        let rule = |version: Option<&str>, release: Option<&str>| Rule {
            name: "r".into(),
            message: "ORDERS".into(),
            version: version.map(Into::into),
            release: release.map(Into::into),
            actions: vec![],
        };
        for (version, release, matches) in [
            (None, None, true),
            (Some("D96A"), None, true),
            (Some("D"), None, true),
            (Some("D01B"), None, false),
            (Some("D"), Some("96A"), true),
            (None, Some("96A"), true),
            (Some("D"), Some("01B"), false),
        ] {
            assert_eq!(
                rule(version, release).matches(order),
                matches,
                "{version:?} {release:?}"
            );
        }
        assert_eq!(
            rule(Some("D"), Some("96A")).combined_version().as_deref(),
            Some("D96A")
        );
    }
}
