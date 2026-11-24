use crate::edi::{self, Syntax};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
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
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Partner {
    pub name: String,
    pub identity: Vec<String>,
    pub charset: String,
    pub acknowledge: Policy,
    pub receive: Vec<Receive>,
    pub send: SendChannel,
    #[serde(default)]
    pub separators: Option<String>,
    #[serde(default)]
    pub rules: Vec<Rule>,
}
impl Partner {
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
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Always,
    Requested,
    Never,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Receive {
    Http { path: String },
    Folder { directory: PathBuf, suffix: String },
}
#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    pub message: String,
    pub version: Option<String>,
    pub release: Option<String>,
    pub actions: Vec<Action>,
}
impl Rule {
    pub fn matches(&self, m: &edi::Message) -> bool {
        self.message == m.kind()
            && self
                .version
                .as_ref()
                .is_none_or(|s| s == m.header().at(1, 1))
            && self
                .release
                .as_ref()
                .is_none_or(|s| s == m.header().at(1, 2))
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Action {
    Document { document: PathBuf },
    Script { script: PathBuf, function: String },
    Delay { delay_ms: u32 },
    Fault { fault: Fault },
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
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
                                fs::read_to_string(self.root.join(script))
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
        Ok(())
    }
}
