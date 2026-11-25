use super::*;
use egui::{ComboBox, DragValue, ScrollArea, Ui};
use toml_edit::DocumentMut;
use vendredi_core::config::{Fault, Rule};
use workspace_edit::tables;

fn rule_row(ui: &mut Ui, height: f32, mut cell: impl FnMut(usize, &mut Ui)) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let mut left = rect.left();
    for (index, column) in [
        48.,
        (width - 102.) * 0.58,
        (width - 102.) * 0.21,
        (width - 102.) * 0.21,
        54.,
    ]
    .into_iter()
    .enumerate()
    {
        let bounds = egui::Rect::from_min_max(
            egui::pos2(left, rect.top()),
            egui::pos2(left + column - 5., rect.bottom()),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(bounds)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        cell(index, &mut child);
        left += column;
    }
}

pub(super) struct RuleForm {
    partner: usize,
    index: Option<usize>,
    order: usize,
    rule: Rule,
    error: String,
    removing: bool,
}

impl RuleForm {
    fn new(workspace: &Workspace, partner: usize, index: Option<usize>) -> Self {
        let rules = &workspace.partners[partner].rules;
        let rule = index.map(|i| rules[i].clone()).unwrap_or_else(|| Rule {
            name: String::new(),
            message: String::new(),
            version: None,
            release: None,
            actions: vec![],
        });
        Self {
            partner,
            index,
            order: index.unwrap_or(rules.len()) + 1,
            rule,
            error: String::new(),
            removing: false,
        }
    }

    fn save(&self, app: &mut Desktop) -> Result<()> {
        let mut doc: DocumentMut = app.config_source.parse()?;
        let partner = tables(doc.as_table_mut(), "partners")?
            .get_mut(self.partner)
            .context("missing partner")?;
        let position = partner.position();
        let rules = tables(partner, "rules")?;
        if self.removing {
            rules.remove(self.index.context("select a rule")?);
        } else {
            let rule = &self.rule;
            ensure!(!rule.name.trim().is_empty(), "Enter a rule name.");
            ensure!(
                !rule.message.trim().is_empty(),
                "Enter the EDIFACT message type."
            );
            for action in &rule.actions {
                if let Action::Script {
                    script: path,
                    function,
                } = action
                {
                    let src = vendredi_core::script::read_source(app.root.join(path))
                        .with_context(|| format!("Opening {}", path.display()))?;
                    vendredi_core::script::validate(&src, function)?;
                }
            }
            let new = toml_edit::ser::to_document(rule)?.into_table();
            let data = if let Some(i) = self.index {
                let old =
                    toml_edit::ser::to_document(&app.workspace.partners[self.partner].rules[i])?;
                let mut table = rules.remove(i);
                for key in ["name", "message", "version", "release", "actions"] {
                    if old.get(key).map(ToString::to_string)
                        != new.get(key).map(ToString::to_string)
                    {
                        if let Some(value) = new.get(key) {
                            table.insert(key, value.clone());
                        } else {
                            table.remove(key);
                        }
                    }
                }
                table
            } else {
                new
            };
            rules.insert(self.order - 1, data);
            if self.order - 1 != self.index.unwrap_or(rules.len() - 1) {
                // Keep reordered headers and their actions together under this partner.
                for table in rules.iter_mut() {
                    table.set_position(position);
                    if let Some(actions) = table
                        .get_mut("actions")
                        .and_then(|v| v.as_array_of_tables_mut())
                    {
                        for action in actions.iter_mut() {
                            action.set_position(position);
                        }
                    }
                }
            }
        }
        app.save_configuration(doc.to_string(), None)
    }

    fn fields(&mut self, ui: &mut Ui, workspace: &Workspace) {
        let p = &workspace.partners[self.partner];
        form::label(ui, "Partner");
        if self.index.is_some() {
            ui.label(&p.name);
        } else {
            let previous = self.partner;
            ComboBox::from_id_salt("rule partner")
                .selected_text(&p.name)
                .show_ui(ui, |ui| {
                    for (i, p) in workspace.partners.iter().enumerate() {
                        ui.selectable_value(&mut self.partner, i, &p.name);
                    }
                });
            if previous != self.partner {
                self.order = workspace.partners[self.partner].rules.len() + 1;
            }
        }
        form::field(ui, "Rule name", &mut self.rule.name);
        ui.columns(3, |cols| {
            form::input(
                &mut cols[0],
                "Message type",
                &mut self.rule.message,
                "ORDERS",
            );
            for (col, title, field) in [
                (1, "Version", &mut self.rule.version),
                (2, "Release", &mut self.rule.release),
            ] {
                let mut s = field.clone().unwrap_or_default();
                if form::input(&mut cols[col], title, &mut s, "Any").changed() {
                    *field = (!s.is_empty()).then_some(s);
                }
            }
        });
        ui.horizontal(|ui| {
            form::label(ui, "Match order");
            let count =
                workspace.partners[self.partner].rules.len() + usize::from(self.index.is_none());
            ui.add(DragValue::new(&mut self.order).range(1..=count));
            ui.label(muted("First matching rule runs."));
        });
        ui.separator();
        let mut add = None;
        ui.horizontal(|ui| {
            form::label(ui, "Actions");
            ui.menu_button("+ Add action", |ui| {
                for (label, v) in [
                    (
                        "Send document",
                        Action::Document {
                            document: PathBuf::new(),
                        },
                    ),
                    (
                        "Run script",
                        Action::Script {
                            script: PathBuf::new(),
                            function: String::new(),
                        },
                    ),
                    ("Delay", Action::Delay { delay_ms: 1000 }),
                    (
                        "Simulate fault",
                        Action::Fault {
                            fault: Fault::Reject,
                        },
                    ),
                ] {
                    if ui.button(label).clicked() {
                        add = Some(v);
                        ui.close();
                    }
                }
            });
        });
        if let Some(action) = add {
            self.rule.actions.push(action);
        }
        let count = self.rule.actions.len();
        let mut movement = None;
        let mut remove = None;
        for (i, action) in self.rule.actions.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                form::card(ui, |ui| {
                    match action {
                        Action::Document { document } => path_input(ui, "Document file", document),
                        Action::Script { script, function } => {
                            path_input(ui, "JavaScript file", script);
                            form::field(ui, "Entry function", function);
                        }
                        Action::Delay { delay_ms } => {
                            form::label(ui, "Delay");
                            ui.add(DragValue::new(delay_ms).suffix(" ms"));
                        }
                        Action::Fault { fault } => {
                            form::label(ui, "Simulated fault");
                            let faults = [
                                (Fault::Reject, "Reject"),
                                (Fault::SuppressAck, "Suppress acknowledgement"),
                                (Fault::DuplicateReference, "Duplicate reference"),
                                (Fault::WrongCount, "Wrong count"),
                                (Fault::Http500, "HTTP 500"),
                                (Fault::SlowResponse, "Slow response"),
                                (Fault::NoResponse, "No response"),
                                (Fault::Duplicate, "Duplicate delivery"),
                                (Fault::Malformed, "Malformed content"),
                            ];
                            let label = faults.iter().find(|(f, _)| f == fault).unwrap().1;
                            ComboBox::from_id_salt("fault")
                                .selected_text(label)
                                .show_ui(ui, |ui| {
                                    for (f, s) in faults {
                                        ui.selectable_value(fault, f, s);
                                    }
                                });
                        }
                    }
                    ui.horizontal(|ui| {
                        if count > 1 {
                            if ui
                                .add_enabled(i > 0, egui::Button::new("Earlier"))
                                .clicked()
                            {
                                movement = Some((i, i - 1));
                            }
                            if ui
                                .add_enabled(i + 1 < count, egui::Button::new("Later"))
                                .clicked()
                            {
                                movement = Some((i, i + 1));
                            }
                        }
                        if ui.link("Remove action").clicked() {
                            remove = Some(i);
                        }
                    });
                })
            });
        }
        if let Some((from, to)) = movement {
            self.rule.actions.swap(from, to);
        }
        if let Some(i) = remove {
            self.rule.actions.remove(i);
        }
    }
}
fn path_input(ui: &mut Ui, label: &str, path: &mut PathBuf) {
    let mut s = path.display().to_string();
    if form::input(ui, label, &mut s, "Workspace path").changed() {
        *path = s.into();
    }
}

impl Desktop {
    pub(super) fn edit_rule_link(&mut self, partner: &str, name: &str) {
        self.navigate(View::Rules);
        if let Some(p) = self
            .workspace
            .partners
            .iter()
            .position(|p| p.name == partner)
        {
            let mut matches = self.workspace.partners[p]
                .rules
                .iter()
                .enumerate()
                .filter(|(_, r)| r.name == name);
            if let Some((i, _)) = matches.next() {
                if matches.next().is_none() {
                    self.rule_form = Some(RuleForm::new(&self.workspace, p, Some(i)));
                    return;
                }
                self.status = format!("Several rules are named {name}. Choose the matching rule.");
                return;
            }
        }
        self.status = format!("The recorded rule {name} is no longer configured for {partner}.");
    }

    pub(super) fn rules(&mut self, ui: &mut Ui) {
        ui.set_max_width(ui.available_width().min(820.));
        ui.horizontal(|ui| {
            ui.heading("Rules");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(ui, "Add rule", true).clicked() {
                    self.rule_form = Some(RuleForm::new(&self.workspace, self.partner, None));
                }
            });
        });
        ui.add_space(12.);
        ScrollArea::vertical().show(ui, |ui| {
            for (p, partner) in self.workspace.partners.iter().enumerate() {
                ui.push_id(p, |ui| {
                    form::card(ui, |ui| {
                        ui.label(egui::RichText::new(&partner.name).font(bold(15.)));
                        ui.add_space(4.);
                        if partner.rules.is_empty() {
                            ui.label(muted("No rules configured."));
                            return;
                        }
                        rule_row(ui, 24., |col, ui| {
                            form::label(
                                ui,
                                ["Order", "Rule", "Message", "Directory", "Actions"][col],
                            );
                        });
                        ui.separator();
                        for (i, rule) in partner.rules.iter().enumerate() {
                            rule_row(ui, 30., |col, ui| match col {
                                0 => {
                                    ui.label(muted((i + 1).to_string()));
                                }
                                1 => {
                                    if ui.link(&rule.name).on_hover_text(&rule.name).clicked() {
                                        self.rule_form =
                                            Some(RuleForm::new(&self.workspace, p, Some(i)));
                                    }
                                }
                                2 => {
                                    ui.monospace(&rule.message).on_hover_text(&rule.message);
                                }
                                3 => {
                                    let directory = match (&rule.version, &rule.release) {
                                        (None, None) => "Any".into(),
                                        (v, r) => format!(
                                            "{}.{}",
                                            v.as_deref().unwrap_or("*"),
                                            r.as_deref().unwrap_or("*")
                                        ),
                                    };
                                    ui.label(muted(&directory)).on_hover_text(directory);
                                }
                                _ => {
                                    ui.label(rule.actions.len().to_string());
                                }
                            });
                        }
                    });
                });
                ui.add_space(10.);
            }
        });
    }
    pub(super) fn rule_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.rule_form.take() else {
            return;
        };
        let mut close = false;
        let title = if f.removing {
            "Remove rule?"
        } else if f.index.is_some() {
            "Edit rule"
        } else {
            "Add rule"
        };
        let escaped = form::modal(ctx, "rule editor", title, 510., |ui| {
            if f.removing {
                ui.label(format!(
                    "Remove {} from {}?",
                    self.workspace.partners[f.partner].rules[f.index.unwrap()].name,
                    self.workspace.partners[f.partner].name
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
                let apply = if f.removing {
                    ui.add(form::removal_button("Confirm removal"))
                } else {
                    button(ui, "Save rule", true)
                };
                if apply.clicked() {
                    match f.save(self) {
                        Ok(()) => {
                            self.status = if f.removing {
                                "Rule removed"
                            } else {
                                "Rule saved"
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
                if !f.removing
                    && f.index.is_some()
                    && ui
                        .with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.link("Remove rule")
                        })
                        .inner
                        .clicked()
                {
                    f.removing = true;
                }
            });
        });
        if !close && !escaped {
            self.rule_form = Some(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests::click;
    #[test]
    #[ignore = "opens a local workspace listener"]
    fn edit_and_reorder_partner_rules() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let file = dir.path().join("vendredi.toml");
        let mut text = fs::read_to_string(&file)?.replace(":8180", ":0");
        text.push_str(
            r#"
[[partners]]
name="Freight"
identity=["FREIGHT","ZZ"]
charset="UNOC"
acknowledge="never"
receive=[{directory="freight",suffix=".edi"}]
send={directory="freight-out"}
[[partners.rules]]
name="Freight updates"
message="DESADV"
actions=[{delay_ms=25}]
"#,
        );
        fs::write(file, text)?;
        let ctx = egui::Context::default();
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        click(&mut app, &ctx, "Rules")?;
        click(&mut app, &ctx, "Add rule")?;
        click(&mut app, &ctx, "Save rule")?;
        assert!(app.rule_form.as_ref().unwrap().error.contains("name"));
        let f = app.rule_form.as_mut().unwrap();
        f.rule.name = "Audit orders".into();
        f.rule.message = "ORDERS".into();
        f.order = 1;
        f.rule.actions = vec![
            Action::Delay { delay_ms: 2 },
            Action::Document {
                document: "confirmation.edi".into(),
            },
        ];
        click(&mut app, &ctx, "Save rule")?;
        let w = Workspace::load(dir.path())?;
        assert_eq!(w.partners[0].rules[0].name, "Audit orders");
        assert_eq!(w.partners[0].rules[1].name, "Confirm orders");
        assert_eq!(w.partners[1].rules[0].name, "Freight updates");
        app.edit_rule_link("Supplier", "Audit orders");
        app.rule_form.as_mut().unwrap().rule.release = Some("96A".into());
        click(&mut app, &ctx, "Save rule")?;
        assert_eq!(
            app.workspace.partners[0].rules[0].release.as_deref(),
            Some("96A")
        );
        click(&mut app, &ctx, "Audit orders")?;
        click(&mut app, &ctx, "Remove rule")?;
        click(&mut app, &ctx, "Confirm removal")?;
        assert_eq!(Workspace::load(dir.path())?.partners[0].rules.len(), 1);
        Ok(())
    }
}
