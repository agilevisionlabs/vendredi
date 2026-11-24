use super::*;
use egui::{ComboBox, Modal, ScrollArea, Ui};
use partners::field;
use toml_edit::{DocumentMut, Table};
use workspace_edit::{FileEdit, tables};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Binding {
    partner: usize,
    rule: usize,
    action: usize,
}

pub(super) struct ScriptForm {
    binding: Option<Binding>,
    partner: usize,
    rule: Option<usize>,
    new_rule: String,
    message: String,
    file: String,
    function: String,
    source: String,
    original: Option<String>,
    error: String,
    confirm_detach: bool,
}

impl ScriptForm {
    fn new(w: &Workspace, partner: usize) -> Self {
        Self {
            binding: None,
            partner,
            rule: (!w.partners[partner].rules.is_empty()).then_some(0),
            new_rule: String::new(),
            message: "ORDERS".into(),
            file: String::new(),
            function: "onMessage".into(),
            source:
                "function onMessage(interchange, api) {\n    api.log(\"Received message\");\n}\n"
                    .into(),
            original: None,
            error: String::new(),
            confirm_detach: false,
        }
    }

    fn load(&mut self, root: &Path) -> Result<()> {
        let text = fs::read_to_string(root.join(&self.file))
            .with_context(|| format!("Opening {}", self.file))?;
        self.source.clone_from(&text);
        self.original = Some(text);
        self.error.clear();
        Ok(())
    }

    fn select_rule(&mut self, ui: &mut Ui, w: &Workspace) {
        ui.label("Partner");
        let before = self.partner;
        ComboBox::from_id_salt("script partner")
            .selected_text(&w.partners[self.partner].name)
            .show_ui(ui, |ui| {
                for (i, p) in w.partners.iter().enumerate() {
                    ui.selectable_value(&mut self.partner, i, &p.name);
                }
            });
        let rules = &w.partners[self.partner].rules;
        if before != self.partner {
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
        self.select_rule(ui, w);
        ui.separator();
        if self.original.is_some() {
            ui.label("Script file");
            ui.monospace(&self.file);
        } else {
            let existing: BTreeSet<_> = w
                .partners
                .iter()
                .flat_map(|p| &p.rules)
                .flat_map(|r| &r.actions)
                .filter_map(|a| match a {
                    Action::Script { script, function } => Some((script, function)),
                    _ => None,
                })
                .collect();
            if !existing.is_empty() {
                ComboBox::from_id_salt("reuse script")
                    .selected_text("Reuse a configured script…")
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
            field(ui, "Script file", &mut self.file);
            ui.label(muted("Use a workspace path such as scripts/orders.js."));
            if w.root.join(&self.file).is_file()
                && ui.button("Load existing script").clicked()
                && let Err(e) = self.check_new_path().and_then(|()| self.load(&w.root))
            {
                self.error = format!("{e:#}");
            }
        }
        field(ui, "Entry function", &mut self.function);
        ui.label("JavaScript");
        ui.add(
            egui::TextEdit::multiline(&mut self.source)
                .code_editor()
                .desired_rows(10)
                .desired_width(f32::INFINITY),
        );
    }

    fn check_new_path(&self) -> Result<()> {
        let path = Path::new(&self.file);
        ensure!(
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("js"))
                && path.components().all(|p| matches!(
                    p,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )),
            "Enter a workspace-relative JavaScript file ending in .js."
        );
        Ok(())
    }

    fn update(&self, app: &mut Desktop, detach: bool) -> Result<()> {
        let mut doc: DocumentMut = app.config_source.parse()?;
        let partners = tables(doc.as_table_mut(), "partners")?;
        let same_rule = self
            .binding
            .is_some_and(|b| b.partner == self.partner && Some(b.rule) == self.rule);
        if let Some(b) = self.binding
            && (detach || !same_rule)
        {
            let rules = tables(
                partners.get_mut(b.partner).context("missing partner")?,
                "rules",
            )?;
            let rule = rules.get_mut(b.rule).context("missing rule")?;
            let actions = tables(rule, "actions")?;
            actions.remove(b.action);
            if actions.is_empty() {
                rule["actions"] = toml_edit::value(toml_edit::Array::new());
            }
        }
        if detach {
            return app.save_configuration(doc.to_string(), None);
        }

        if self.original.is_none() {
            self.check_new_path()?;
        }
        let path = app.root.join(&self.file);
        if self.original.is_none() {
            ensure!(
                !path.exists(),
                "This file exists. Load the existing script to edit it."
            );
        }
        ensure!(
            !self.source.trim().is_empty(),
            "Enter the JavaScript source."
        );
        vendredi_core::script::validate(&self.source, &self.function)?;
        let rules = tables(
            partners.get_mut(self.partner).context("missing partner")?,
            "rules",
        )?;
        let rule = if let Some(i) = self.rule {
            rules.get_mut(i).context("missing rule")?
        } else {
            ensure!(
                !self.new_rule.trim().is_empty() && !self.message.trim().is_empty(),
                "Enter the new rule name and message type."
            );
            let mut rule = Table::new();
            rule["name"] = toml_edit::value(self.new_rule.trim());
            rule["message"] = toml_edit::value(self.message.trim().to_uppercase());
            rules.push(rule);
            rules.get_mut(rules.len() - 1).unwrap()
        };
        if let Some(b) = self.binding
            && same_rule
        {
            let action = rule
                .get_mut("actions")
                .and_then(|a| a.get_mut(b.action))
                .context("missing script action")?;
            if action["function"].as_str() != Some(self.function.as_str()) {
                action["function"] = toml_edit::value(&self.function);
            }
        } else {
            let mut action = Table::new();
            action["script"] = toml_edit::value(&self.file);
            action["function"] = toml_edit::value(&self.function);
            tables(rule, "actions")?.push(action);
        }
        app.save_configuration(
            doc.to_string(),
            Some(FileEdit {
                path: &path,
                before: self.original.as_deref(),
                after: &self.source,
            }),
        )
    }
}

impl Desktop {
    fn edit_binding(&mut self, binding: Binding) {
        let Binding {
            partner,
            rule,
            action,
        } = binding;
        let Action::Script { script, function } =
            &self.workspace.partners[partner].rules[rule].actions[action]
        else {
            return;
        };
        let mut f = ScriptForm::new(&self.workspace, partner);
        f.binding = Some(binding);
        f.rule = Some(rule);
        f.file = script.display().to_string();
        f.function.clone_from(function);
        match f.load(&self.root) {
            Ok(()) => self.script_form = Some(f),
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    pub(super) fn edit_script_link(
        &mut self,
        path: &Path,
        partner_name: &str,
        rule_name: Option<&str>,
    ) {
        self.navigate(View::Scripts);
        let mut found = None;
        for (partner, p) in self
            .workspace
            .partners
            .iter()
            .enumerate()
            .filter(|(_, p)| p.name == partner_name)
        {
            for (rule, r) in p
                .rules
                .iter()
                .enumerate()
                .filter(|(_, r)| rule_name.is_none_or(|n| r.name == n))
            {
                for (action, _) in r
                    .actions
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| matches!(a,Action::Script{script,..} if script==path))
                {
                    if found.is_some() {
                        self.status = "Choose the script entry function to edit.".into();
                        return;
                    }
                    found = Some(Binding {
                        partner,
                        rule,
                        action,
                    });
                }
            }
        }
        if let Some(b) = found {
            self.edit_binding(b);
        } else {
            self.status = format!(
                "{} has no current binding for this execution",
                path.display()
            );
        }
    }

    pub(super) fn scripts(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.heading("Scripts");
            if button(ui, "Add script", true).clicked() {
                self.script_form = Some(ScriptForm::new(&self.workspace, self.partner));
            }
        });
        ui.add_space(8.);
        let mut picked = None;
        ScrollArea::vertical().show(ui, |ui| {
            let mut any=false;
            for (partner,p) in self.workspace.partners.iter().enumerate() {
                let mut heading=false;
                for (rule,r) in p.rules.iter().enumerate() {
                    for (action,a) in r.actions.iter().enumerate() {
                        if let Action::Script{script,function}=a {
                            if !heading {ui.heading(&p.name); heading=true; any=true;}
                            ui.push_id((partner,rule,action),|ui| {ui.horizontal_wrapped(|ui|{
                                if ui.link(script.display().to_string()).clicked() {picked=Some(Binding{partner,rule,action});}
                                ui.monospace(format!("{function}()"));
                                ui.label(muted(format!("{} · {}",r.name,r.message)));
                            });});
                        }
                    }
                }
                if heading {ui.separator();}
            }
            if !any {ui.label("No scripts configured. Add a script to run JavaScript when a partner rule matches.");}
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
        Modal::new("script editor".into()).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 48.).min(680.));
            ui.heading(if f.binding.is_some() {
                "Edit script"
            } else {
                "Add script"
            });
            ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 220.).max(100.))
                .show(ui, |ui| f.fields(ui, &self.workspace));
            ui.separator();
            if !f.error.is_empty() {
                ui.colored_label(AMBER, &f.error);
            }
            if f.confirm_detach {
                ui.label(
                    "Remove this action from its rule? The JavaScript file stays in the workspace.",
                );
            }
            ui.horizontal(|ui| {
                let detach = f.confirm_detach && ui.button("Confirm detach").clicked();
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
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if !f.confirm_detach
                    && f.binding.is_some()
                    && ui.button("Detach from rule").clicked()
                {
                    f.confirm_detach = true;
                }
            });
        });
        if !close {
            self.script_form = Some(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests::click;
    use std::time::{Duration, Instant};

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
        let script = dir.path().join("scripts/orders.js");
        assert!(!script.exists());
        let f = app.script_form.as_mut().unwrap();
        f.source = "function onMessage(i, api) { api.log('order arrived'); }".into();
        click(&mut app, &ctx, "Save script")?;
        ensure!(
            app.script_form.is_none(),
            "save failed: {:?}",
            app.script_form.as_ref().map(|f| &f.error)
        );
        let w = Workspace::load(dir.path())?;
        assert!(
            matches!(w.partners[0].rules[0].actions.as_slice(),[Action::Document{..},Action::Script{function,..}] if function=="onMessage")
        );

        fs::write(dir.path().join("inbox/order.edi"), crate::ORDER)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.drain();
            if app.detail["events"].as_array().is_some_and(|events| {
                events
                    .iter()
                    .any(|e| e["event"] == "script_log" && e.to_string().contains("order arrived"))
            }) {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "script did not log for folder intake: {}",
                app.detail
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        click(&mut app, &ctx, "scripts/orders.js")?;
        let f = app.script_form.as_mut().unwrap();
        f.rule = None;
        f.new_rule = "Invoice log".into();
        f.message = "INVOIC".into();
        f.function = "onInvoice".into();
        f.source = "function onInvoice(i, api) { api.log('invoice arrived'); }".into();
        click(&mut app, &ctx, "Save script")?;
        let w = Workspace::load(dir.path())?;
        assert_eq!(w.partners[0].rules[0].actions.len(), 1);
        assert!(
            matches!(w.partners[0].rules[1].actions.as_slice(),[Action::Script{function,..}] if function=="onInvoice")
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
        assert!(
            Workspace::load(dir.path())?.partners[0].rules[1]
                .actions
                .is_empty()
        );
        assert_eq!(fs::read_to_string(script)?, edited);
        Ok(())
    }
}
