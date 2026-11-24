use super::*;
use toml_edit::{ArrayOfTables, Item, Table};

/// Accept both inline arrays and repeated TOML table headers.
pub(super) fn tables<'a>(parent: &'a mut Table, key: &str) -> Result<&'a mut ArrayOfTables> {
    let item = parent.entry(key).or_insert(Item::None);
    if !item.is_array_of_tables() {
        *item = if item.is_none() || item.as_array().is_some_and(|a| a.is_empty()) {
            Item::ArrayOfTables(ArrayOfTables::new())
        } else {
            Item::ArrayOfTables(
                std::mem::take(item)
                    .into_array_of_tables()
                    .map_err(|_| anyhow!("expected a list of {key}"))?,
            )
        };
    }
    Ok(item.as_array_of_tables_mut().unwrap())
}

pub(super) struct FileEdit<'a> {
    pub path: &'a Path,
    pub before: Option<&'a [u8]>,
    pub after: &'a [u8],
}
impl FileEdit<'_> {
    fn verify(&self) -> Result<()> {
        match fs::read(self.path) {
            Ok(text) => ensure!(
                Some(text.as_slice()) == self.before,
                "{} changed on disk. Reopen the workspace before saving.",
                self.path.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && self.before.is_none() => (),
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }
    fn write(&self) -> Result<()> {
        if self.before.is_some() {
            return replace(self.path, self.after);
        }
        let parent = self.path.parent().context("missing parent directory")?;
        fs::create_dir_all(parent)?;
        let mut f = tempfile::NamedTempFile::new_in(parent)?;
        f.write_all(self.after)?;
        f.as_file().sync_all()?;
        f.persist_noclobber(self.path)?;
        Ok(())
    }
    fn rollback(&self) -> Result<()> {
        if let Some(old) = self.before {
            replace(self.path, old)
        } else {
            Ok(fs::remove_file(self.path)?)
        }
    }
}

impl Desktop {
    pub(super) fn apply_files(&mut self, edits: &[FileEdit<'_>]) -> Result<()> {
        for edit in edits {
            edit.verify()?;
        }
        let mut written = Vec::new();
        let apply = (|| -> Result<()> {
            // A running rule reads its JavaScript file on each invocation.
            self.running.stop()?;
            for edit in edits {
                if edit.before != Some(edit.after) {
                    edit.write()?;
                    written.push(edit);
                }
            }
            let source = fs::read_to_string(self.root.join("vendredi.toml"))?;
            let workspace = Workspace::load(&self.root)?;
            self.running = runtime::start(
                workspace.clone(),
                self.output.clone(),
                false,
                self.listening,
            )?;
            self.config_source = source;
            self.workspace = workspace;
            self.partner = self.partner.min(self.workspace.partners.len() - 1);
            self.command(Command::Traffic(String::new()));
            if !self.search.is_empty() {
                self.command(Command::Traffic(self.search.clone()));
            }
            Ok(())
        })();
        if let Err(error) = apply {
            for edit in written.into_iter().rev() {
                edit.rollback().with_context(|| {
                    format!("restoring {} after {error:#}", edit.path.display())
                })?;
            }
            self.running = runtime::start(
                self.workspace.clone(),
                self.output.clone(),
                false,
                self.listening,
            )
            .with_context(|| format!("restarting the previous workspace after {error:#}"))?;
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn save_configuration(
        &mut self,
        config: String,
        script: Option<FileEdit<'_>>,
    ) -> Result<()> {
        let before = self.config_source.clone();
        let path = self.root.join("vendredi.toml");
        let mut edits = Vec::from_iter(script);
        edits.push(FileEdit {
            path: &path,
            before: Some(before.as_bytes()),
            after: config.as_bytes(),
        });
        self.apply_files(&edits)?;
        self.config_source = config;
        if self.edit_file == Path::new("vendredi.toml") {
            self.edit.clone_from(&self.config_source);
            self.saved.clone_from(&self.config_source);
        }
        Ok(())
    }
}
