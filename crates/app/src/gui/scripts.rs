use super::*;
use egui::{ComboBox, ScrollArea, Ui};
use form::field;
use vendredi_core::case::{self, Case};
use workspace_edit::FileEdit;

/// A script action: which case, rule and action run it.
#[derive(Clone, PartialEq, Eq)]
struct Binding {
    case: String,
    rule: usize,
    action: usize,
}

/// Edits a script of a case and the rule that runs it. Paths are inside the
/// case folder.
pub(super) struct ScriptForm {
    binding: Option<Binding>,
    case: String,
    rule: Option<usize>,
    new_rule: String,
    message: String,
    pub(super) file: String,
    pub(super) function: String,
    pub(super) source: String,
    original: Option<String>,
    pub(super) error: String,
    confirm_detach: bool,
}

/// Cases whose files the editors may change: imported cases are replaced by
/// the next import.
fn editable(w: &Workspace) -> impl Iterator<Item = &Case> {
    w.cases.iter().filter(|c| c.source.is_none())
}

fn find<'a>(w: &'a Workspace, id: &str) -> Option<&'a Case> {
    w.cases.iter().find(|c| c.id == id)
}

impl ScriptForm {
    fn new(case: &Case) -> Self {
        Self {
            binding: None,
            case: case.id.clone(),
            rule: (!case.rules.is_empty()).then_some(0),
            new_rule: String::new(),
            message: "ORDERS".into(),
            file: String::new(),
            function: "onMessage".into(),
            source:
                "function onMessage(interchange, api) {\n    const message = interchange.groups[0].messages[0];\n    api.log(\"Received \" + message.type);\n    // api.send(api.build(\"ORDRSP\", message.version, [[\"BGM\", \"231\", api.nextReference(), \"9\"]]));\n}\n"
                    .into(),
            original: None,
            error: String::new(),
            confirm_detach: false,
        }
    }

    fn path(&self, root: &Path) -> PathBuf {
        case::folder(root, &self.case).join(&self.file)
    }

    fn load(&mut self, root: &Path) -> Result<()> {
        let text = vendredi_core::script::read_source(self.path(root))
            .with_context(|| format!("Opening {}", self.file))?;
        self.source.clone_from(&text);
        self.original = Some(text);
        self.error.clear();
        Ok(())
    }

    fn select_rule(&mut self, ui: &mut Ui, w: &Workspace) {
        ui.label("Case");
        let before = self.case.clone();
        let name = find(w, &self.case).map_or(self.case.as_str(), |c| &c.name);
        ComboBox::from_id_salt("script case")
            .selected_text(name)
            .show_ui(ui, |ui| {
                for c in editable(w) {
                    ui.selectable_value(&mut self.case, c.id.clone(), &c.name);
                }
            });
        let rules = find(w, &self.case).map_or(&[][..], |c| &c.rules);
        if before != self.case {
            self.rule = (!rules.is_empty()).then_some(0);
        }
        ui.label("Run when rule matches");
        ComboBox::from_id_salt("script rule")
            .selected_text(self.rule.map_or("New rule", |i| &rules[i].name))
            .show_ui(ui, |ui| {
                for (i, r) in rules.iter().enumerate() {
                    ui.selectable_value(&mut self.rule, Some(i), &r.name);
                }
                ui.selectable_value(&mut self.rule, None, "New rule");
            });
        if self.rule.is_none() {
            field(ui, "Rule name", &mut self.new_rule);
            field(ui, "Message type", &mut self.message);
        }
    }

    fn fields(&mut self, ui: &mut Ui, w: &Workspace) {
        if self.binding.is_none() {
            self.select_rule(ui, w);
            ui.separator();
        }
        if self.original.is_some() {
            ui.columns(2, |cols| {
                form::label(&mut cols[0], "Script file");
                cols[0].monospace(format!("cases/{}/{}", self.case, self.file));
                field(&mut cols[1], "Entry function", &mut self.function);
            });
        } else {
            let existing: BTreeSet<_> = find(w, &self.case)
                .into_iter()
                .flat_map(|c| &c.rules)
                .flat_map(|r| &r.actions)
                .filter_map(|a| match a {
                    Action::Script { script, function } => Some((script, function)),
                    _ => None,
                })
                .collect();
            if !existing.is_empty() {
                ComboBox::from_id_salt("reuse script")
                    .selected_text("Reuse a script of this case…")
                    .show_ui(ui, |ui| {
                        for (path, function) in existing {
                            if ui
                                .selectable_label(
                                    false,
                                    format!("{} · {function}()", path.display()),
                                )
                                .clicked()
                            {
                                self.file = path.display().to_string();
                                self.function.clone_from(function);
                                if let Err(e) = self.load(&w.root) {
                                    self.error = format!("{e:#}");
                                }
                            }
                        }
                    });
            }
            let folder = case::folder(&w.root, &self.case);
            let mut chosen = None;
            form::path_input(ui, "Script file", &mut self.file, "orders.js", || {
                let picked = form::pick_file(&folder, &[("JavaScript", &["js"])])?;
                match form::into_folder(&folder, &picked) {
                    Ok(inside) => {
                        chosen = Some(());
                        Some(inside.display().to_string())
                    }
                    Err(e) => {
                        self.error = format!("{e:#}");
                        None
                    }
                }
            });
            // A chosen script opens for editing.
            if chosen.is_some()
                && let Err(e) = self.load(&w.root)
            {
                self.error = format!("{e:#}");
            }
            ui.label(muted(format!(
                "A path inside the case folder cases/{}. Choosing a file elsewhere copies it into the case.",
                self.case
            )));
            if self.path(&w.root).is_file()
                && ui.button("Load existing script").clicked()
                && let Err(e) = self.check_new_path().and_then(|()| self.load(&w.root))
            {
                self.error = format!("{e:#}");
            }
        }
        if self.original.is_none() {
            field(ui, "Entry function", &mut self.function);
        }
        form::label(ui, "JavaScript");
        ui.add(
            egui::TextEdit::multiline(&mut self.source)
                .code_editor()
                .margin(egui::Margin::symmetric(10, 8))
                .desired_rows(10)
                .desired_width(f32::INFINITY),
        );
        if self.binding.is_some() {
            let case = find(w, &self.case);
            let name = case.map_or(self.case.as_str(), |c| &c.name);
            let rule = self
                .rule
                .and_then(|i| case.and_then(|c| c.rules.get(i)))
                .map_or("New rule", |r| r.name.as_str());
            egui::CollapsingHeader::new(format!("Run in {name} · {rule}"))
                .id_salt("script binding")
                .show(ui, |ui| self.select_rule(ui, w));
        }
    }

    fn check_new_path(&self) -> Result<()> {
        let path = Path::new(&self.file);
        ensure!(
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("js"))
                && case::relative(path).is_ok(),
            "Enter a JavaScript file inside the case folder, ending in .js."
        );
        Ok(())
    }

    /// Reads a case's `case.json` for editing.
    fn read_case(root: &Path, w: &Workspace, id: &str) -> Result<(PathBuf, Vec<u8>, Case)> {
        ensure!(
            find(w, id).is_some_and(|c| c.source.is_none()),
            "Case {id} was imported. Duplicate it to change its scripts."
        );
        let path = case::folder(root, id).join(case::FILE);
        let bytes = fs::read(&path).with_context(|| path.display().to_string())?;
        let case = Case::parse(&bytes)?;
        Ok((path, bytes, case))
    }

    fn update(&self, app: &mut Desktop, detach: bool) -> Result<()> {
        let same_rule = self
            .binding
            .as_ref()
            .is_some_and(|b| b.case == self.case && Some(b.rule) == self.rule);
        let mut files = Vec::new();
        // Detaching or moving takes the action out of its old rule first.
        if let Some(b) = &self.binding
            && (detach || !same_rule)
        {
            let (path, before, mut case) = Self::read_case(&app.root, &app.workspace, &b.case)?;
            case.rules
                .get_mut(b.rule)
                .context("missing rule")?
                .actions
                .remove(b.action);
            files.push((path, Some(before), case::to_json(&case)?.into_bytes()));
        }
        if !detach {
            if self.original.is_none() {
                self.check_new_path()?;
            }
            let script = self.path(&app.root);
            if self.original.is_none() {
                ensure!(
                    !script.exists(),
                    "This file exists. Load the existing script to edit it."
                );
            }
            ensure!(
                !self.source.trim().is_empty(),
                "Enter the JavaScript source."
            );
            vendredi_core::script::validate(&self.source, &self.function)?;
            // The case may be the one just edited above.
            let (path, before, mut case) = match files.pop() {
                Some((path, before, after))
                    if self.binding.as_ref().is_some_and(|b| b.case == self.case) =>
                {
                    (path, before.unwrap_or_default(), Case::parse(&after)?)
                }
                other => {
                    files.extend(other);
                    Self::read_case(&app.root, &app.workspace, &self.case)?
                }
            };
            let rule = if let Some(i) = self.rule {
                case.rules.get_mut(i).context("missing rule")?
            } else {
                ensure!(
                    !self.new_rule.trim().is_empty() && !self.message.trim().is_empty(),
                    "Enter the new rule name and message type."
                );
                case.rules.push(vendredi_core::config::Rule {
                    name: self.new_rule.trim().into(),
                    message: self.message.trim().to_uppercase(),
                    version: None,
                    release: None,
                    actions: vec![],
                });
                case.rules.last_mut().unwrap()
            };
            match &self.binding {
                Some(b) if same_rule => {
                    let Some(Action::Script { function, .. }) = rule.actions.get_mut(b.action)
                    else {
                        anyhow::bail!("missing script action");
                    };
                    function.clone_from(&self.function);
                }
                _ => rule.actions.push(Action::Script {
                    script: self.file.clone().into(),
                    function: self.function.clone(),
                }),
            }
            files.push((path, Some(before), case::to_json(&case)?.into_bytes()));
            files.push((
                script,
                self.original.clone().map(String::into_bytes),
                self.source.clone().into_bytes(),
            ));
        }
        let edits: Vec<_> = files
            .iter()
            .map(|(path, before, after)| FileEdit {
                path,
                before: before.as_deref(),
                after,
            })
            .collect();
        app.apply_files(&edits)
    }
}

impl Desktop {
    fn edit_binding(&mut self, binding: Binding) {
        let Some(c) = find(&self.workspace, &binding.case).cloned() else {
            return;
        };
        let Some(Action::Script { script, function }) = c
            .rules
            .get(binding.rule)
            .and_then(|r| r.actions.get(binding.action))
        else {
            return;
        };
        let mut f = ScriptForm::new(&c);
        f.rule = Some(binding.rule);
        f.file = script.display().to_string();
        f.function.clone_from(function);
        f.binding = Some(binding);
        match f.load(&self.root) {
            Ok(()) => self.script_form = Some(f),
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// Opens the script a recorded execution ran. The recorded path is
    /// inside the partner's active case.
    pub(super) fn edit_script_link(
        &mut self,
        path: &Path,
        partner_name: &str,
        rule_name: Option<&str>,
    ) {
        self.navigate(View::Scripts);
        let Some(c) = self
            .workspace
            .partners
            .iter()
            .find(|p| p.name == partner_name)
            .and_then(|p| p.case_id())
            .and_then(|id| find(&self.workspace, id))
        else {
            self.status = format!("{partner_name} has no active case");
            return;
        };
        let folder = Path::new(case::DIRECTORY).join(&c.id);
        let path = path.strip_prefix(&folder).unwrap_or(path);
        let mut found = Vec::new();
        for (rule, r) in c
            .rules
            .iter()
            .enumerate()
            .filter(|(_, r)| rule_name.is_none_or(|n| r.name == n))
        {
            for (action, a) in r.actions.iter().enumerate() {
                if matches!(a, Action::Script { script, .. } if script == path) {
                    found.push(Binding {
                        case: c.id.clone(),
                        rule,
                        action,
                    });
                }
            }
        }
        match found.len() {
            1 => self.edit_binding(found.pop().unwrap()),
            0 => {
                self.status = format!(
                    "{} has no current binding for this execution",
                    path.display()
                );
            }
            _ => self.status = "Choose the script entry function to edit.".into(),
        }
    }

    pub(super) fn scripts(&mut self, ui: &mut Ui) {
        header_row(ui, |ui| {
            ui.heading("Scripts");
            if button(ui, "Add script", true).clicked() {
                let active = self.workspace.partners[self.partner].case_id();
                let target = active
                    .and_then(|id| editable(&self.workspace).find(|c| c.id == id))
                    .or_else(|| editable(&self.workspace).next())
                    .cloned();
                match target {
                    Some(c) => self.script_form = Some(ScriptForm::new(&c)),
                    None => {
                        self.status =
                            "Create or duplicate a case first: scripts belong to cases.".into();
                    }
                }
            }
        });
        ui.add_space(GAP_XS);
        ui.label(muted(
            "Scripts belong to cases and run when one of the case's rules matches.",
        ));
        ui.add_space(GAP_S);
        let mut picked = None;
        ScrollArea::vertical().show(ui, |ui| {
            let mut any = false;
            for c in &self.workspace.cases {
                let mut first = true;
                for (rule, r) in c.rules.iter().enumerate() {
                    for (action, a) in r.actions.iter().enumerate() {
                        let Action::Script { script, function } = a else {
                            continue;
                        };
                        if first {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&c.name).font(bold(15.)));
                                ui.label(muted(&c.id).monospace());
                            });
                            first = false;
                            any = true;
                        }
                        ui.push_id((&c.id, rule, action), |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if ui.link(script.display().to_string()).clicked() {
                                    picked = Some(Binding {
                                        case: c.id.clone(),
                                        rule,
                                        action,
                                    });
                                }
                                ui.monospace(format!("{function}()"));
                                ui.label(muted(format!("{} · {}", r.name, r.message)));
                            });
                        });
                    }
                }
                if !first {
                    ui.separator();
                }
            }
            if !any {
                ui.label(muted(
                    "No scripts yet. Add a script to run when a rule of a case matches.",
                ));
            }
        });
        if let Some(b) = picked {
            self.edit_binding(b);
        }
    }

    pub(super) fn script_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.script_form.take() else {
            return;
        };
        let mut close = false;
        let title = if f.confirm_detach {
            "Detach script?"
        } else if f.binding.is_some() {
            "Edit script"
        } else {
            "Add script"
        };
        let escaped = form::modal(ctx, "script editor", title, 640., |ui| {
            if f.confirm_detach {
                ui.label(format!(
                    "Remove {} from this rule? The JavaScript file stays in the case folder.",
                    f.file
                ));
            } else {
                ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 220.).max(100.))
                    .show(ui, |ui| f.fields(ui, &self.workspace));
            }
            if !f.error.is_empty() {
                ui.colored_label(AMBER, &f.error);
            }
            form::footer(ui, |ui| {
                let detach =
                    f.confirm_detach && ui.add(form::removal_button("Confirm detach")).clicked();
                let save = !f.confirm_detach && button(ui, "Save script", true).clicked();
                if save || detach {
                    match f.update(self, detach) {
                        Ok(()) => {
                            self.status = if detach {
                                "Script detached"
                            } else {
                                "Script saved"
                            }
                            .into();
                            close = true;
                        }
                        Err(e) => f.error = format!("{e:#}"),
                    }
                }
                if button(ui, "Cancel", false).clicked() {
                    close = true;
                }
                if !f.confirm_detach
                    && f.binding.is_some()
                    && ui
                        .with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.link("Detach from rule")
                        })
                        .inner
                        .clicked()
                {
                    f.confirm_detach = true;
                    f.error.clear();
                }
            });
        });
        if !close && !escaped {
            self.script_form = Some(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests::{click, wait_for};

    #[test]
    #[ignore = "uses a local listener and folder watcher"]
    fn manage_script_and_run_from_folder() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let config = dir.path().join("vendredi.toml");
        let baseline = fs::read_to_string(&config)?.replace(":8180", ":0");
        fs::write(&config, &baseline)?;
        let ctx = egui::Context::default();
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        click(&mut app, &ctx, "Scripts")?;
        click(&mut app, &ctx, "Add script")?;
        let f = app.script_form.as_mut().context("missing script editor")?;
        f.file = "scripts/orders.js".into();
        f.source = "function onMessage( {".into();
        click(&mut app, &ctx, "Save script")?;
        assert!(!app.script_form.as_ref().unwrap().error.is_empty());
        assert_eq!(fs::read_to_string(&config)?, baseline);
        let script = case::folder(dir.path(), "confirm-orders").join("scripts/orders.js");
        let rules = || -> Result<Vec<vendredi_core::config::Rule>> {
            Ok(
                case::Bundle::read(&case::folder(dir.path(), "confirm-orders"))?
                    .case
                    .rules,
            )
        };
        assert!(!script.exists());
        let f = app.script_form.as_mut().unwrap();
        f.source = "function onMessage(i, api) { api.log('order arrived'); }".into();
        click(&mut app, &ctx, "Save script")?;
        ensure!(
            app.script_form.is_none(),
            "save failed: {:?}",
            app.script_form.as_ref().map(|f| &f.error)
        );
        assert!(
            matches!(rules()?[0].actions.as_slice(),[Action::Script{..},Action::Script{function,..}] if function=="onMessage")
        );

        fs::rename(
            dir.path().join("order.edi"),
            dir.path().join("inbox/order.edi"),
        )?;
        wait_for(&mut app, "folder script log", |app| {
            app.detail["events"].as_array().is_some_and(|events| {
                events
                    .iter()
                    .any(|e| e["event"] == "script_log" && e["text"] == "order arrived")
            })
        })?;

        click(&mut app, &ctx, "scripts/orders.js")?;
        let f = app.script_form.as_mut().unwrap();
        f.rule = None;
        f.new_rule = "Invoice log".into();
        f.message = "INVOIC".into();
        f.function = "onInvoice".into();
        f.source = "function onInvoice(i, api) { api.log('invoice arrived'); }".into();
        click(&mut app, &ctx, "Save script")?;
        let saved = rules()?;
        assert_eq!(saved[0].actions.len(), 1);
        assert!(
            matches!(saved[1].actions.as_slice(),[Action::Script{function,..}] if function=="onInvoice")
        );
        click(&mut app, &ctx, "scripts/orders.js")?;
        let edited = "function onInvoice() { /* external change */ }";
        fs::write(&script, edited)?;
        click(&mut app, &ctx, "Save script")?;
        assert!(
            app.script_form
                .as_ref()
                .unwrap()
                .error
                .contains("changed on disk")
        );
        assert_eq!(fs::read_to_string(&script)?, edited);
        click(&mut app, &ctx, "Detach from rule")?;
        click(&mut app, &ctx, "Confirm detach")?;
        assert!(rules()?[1].actions.is_empty());
        assert_eq!(fs::read_to_string(script)?, edited);
        Ok(())
    }
}
