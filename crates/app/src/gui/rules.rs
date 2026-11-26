use super::*;
use egui::{ComboBox, DragValue, ScrollArea, Ui};
use vendredi_core::{
    case::{self, Case},
    config::{Fault, Rule},
};
use workspace_edit::FileEdit;

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

/// Edits one rule of a case in its `case.json`.
pub(super) struct RuleForm {
    case: String,
    /// `case.json` as it was when the form opened.
    original: Vec<u8>,
    index: Option<usize>,
    order: usize,
    count: usize,
    rule: Rule,
    pub(super) error: String,
    removing: bool,
}

impl RuleForm {
    pub(super) fn new(root: &Path, case: &Case, index: Option<usize>) -> Self {
        let rules = &case.rules;
        let mut rule = index.map(|i| rules[i].clone()).unwrap_or_else(|| Rule {
            name: String::new(),
            message: String::new(),
            version: None,
            release: None,
            actions: vec![],
        });
        // Editors show and save one version, such as D96A, for every standard.
        rule.version = rule.combined_version();
        rule.release = None;
        let path = case::folder(root, &case.id).join(case::FILE);
        Self {
            case: case.id.clone(),
            original: fs::read(path).unwrap_or_default(),
            index,
            order: index.unwrap_or(rules.len()) + 1,
            count: rules.len() + usize::from(index.is_none()),
            rule,
            error: String::new(),
            removing: false,
        }
    }

    fn save(&self, app: &mut Desktop) -> Result<()> {
        let folder = case::folder(&app.root, &self.case);
        let mut case = case::Case::parse(&self.original)?;
        ensure!(
            app.workspace
                .cases
                .iter()
                .any(|c| c.id == self.case && c.source.is_none()),
            "This case was imported. Duplicate it to change its rules."
        );
        if self.removing {
            case.rules.remove(self.index.context("select a rule")?);
        } else {
            let rule = &self.rule;
            ensure!(!rule.name.trim().is_empty(), "Enter a rule name.");
            ensure!(
                !rule.message.trim().is_empty(),
                "Enter the message type or transaction set."
            );
            for action in &rule.actions {
                match action {
                    Action::Script {
                        script: path,
                        function,
                    } => {
                        case::relative(path)?;
                        let src = vendredi_core::script::read_source(folder.join(path))
                            .with_context(|| format!("Opening {}", path.display()))?;
                        vendredi_core::script::validate(&src, function)?;
                    }
                    Action::Document { document } => {
                        case::relative(document)?;
                        ensure!(
                            folder.join(document).is_file(),
                            "{} is not in the case folder cases/{}",
                            document.display(),
                            self.case
                        );
                    }
                    _ => {}
                }
            }
            if let Some(i) = self.index {
                case.rules.remove(i);
            }
            case.rules.insert(self.order - 1, rule.clone());
        }
        let json = case::to_json(&case)?;
        app.apply_files(&[FileEdit {
            path: &folder.join(case::FILE),
            before: Some(&self.original),
            after: json.as_bytes(),
        }])
    }

    fn fields(&mut self, ui: &mut Ui) {
        form::label(ui, "Case");
        ui.monospace(&self.case);
        form::field(ui, "Rule name", &mut self.rule.name);
        ui.columns(2, |cols| {
            form::input(
                &mut cols[0],
                "Message type",
                &mut self.rule.message,
                "ORDERS or 850",
            );
            let mut s = self.rule.version.clone().unwrap_or_default();
            if form::input(&mut cols[1], "Version", &mut s, "Any, D96A or 005010").changed() {
                let s = s.trim();
                self.rule.version = (!s.is_empty()).then(|| s.to_owned());
            }
        });
        ui.horizontal(|ui| {
            form::label(ui, "Match order");
            ui.add(DragValue::new(&mut self.order).range(1..=self.count));
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
    if form::input(ui, label, &mut s, "Path inside the case folder").changed() {
        *path = s.into();
    }
}

/// The rules of a case as a table. Returns the rule whose name was clicked.
pub(super) fn rule_table(ui: &mut Ui, rules: &[Rule]) -> Option<usize> {
    let mut clicked = None;
    rule_row(ui, 24., |col, ui| {
        form::label(ui, ["Order", "Rule", "Message", "Version", "Actions"][col]);
    });
    ui.separator();
    for (i, rule) in rules.iter().enumerate() {
        ui.push_id(i, |ui| {
            rule_row(ui, 30., |col, ui| match col {
                0 => {
                    ui.label(muted((i + 1).to_string()));
                }
                1 => {
                    if ui.link(&rule.name).on_hover_text(&rule.name).clicked() {
                        clicked = Some(i);
                    }
                }
                2 => {
                    ui.monospace(&rule.message).on_hover_text(&rule.message);
                }
                3 => {
                    let version = rule.combined_version().unwrap_or_else(|| "Any".into());
                    ui.label(muted(&version)).on_hover_text(version);
                }
                _ => {
                    ui.label(rule.actions.len().to_string());
                }
            });
        });
    }
    clicked
}

impl Desktop {
    /// Opens the rule a recorded execution ran, in the partner's active case.
    pub(super) fn edit_rule_link(&mut self, partner: &str, name: &str) {
        let Some(id) = self
            .workspace
            .partners
            .iter()
            .find(|p| p.name == partner)
            .and_then(|p| p.case_id())
            .map(str::to_owned)
        else {
            self.status = format!("{partner} has no active case.");
            return;
        };
        self.show_case(&id);
        let Some(c) = self.workspace.cases.iter().find(|c| c.id == id).cloned() else {
            return;
        };
        let mut matches = c.rules.iter().enumerate().filter(|(_, r)| r.name == name);
        match (matches.next(), matches.next()) {
            (Some((i, _)), None) => {
                self.rule_form = Some(RuleForm::new(&self.root, &c, Some(i)));
            }
            (Some(_), Some(_)) => {
                self.status = format!("Several rules are named {name}. Choose the matching rule.");
            }
            _ => {
                self.status = format!("The recorded rule {name} is no longer in case {id}.");
            }
        }
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
                ui.label(format!("Remove {} from case {}?", f.rule.name, f.case));
            } else {
                ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 220.).max(100.))
                    .show(ui, |ui| f.fields(ui));
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

    fn rules(dir: &Path) -> Result<Vec<Rule>> {
        Ok(case::Bundle::read(&case::folder(dir, "confirm-orders"))?
            .case
            .rules)
    }

    #[test]
    #[ignore = "opens a local workspace listener"]
    fn edit_and_reorder_case_rules() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let file = dir.path().join("vendredi.toml");
        fs::write(&file, fs::read_to_string(&file)?.replace(":8180", ":0"))?;
        let ctx = egui::Context::default();
        ctx.data_mut(|d| {
            d.insert_temp(egui::Id::new("test screen size"), egui::vec2(1440., 1400.))
        });
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        click(&mut app, &ctx, "Cases")?;
        assert_eq!(app.case, "confirm-orders");
        click(&mut app, &ctx, "+ Add rule")?;
        click(&mut app, &ctx, "Save rule")?;
        assert!(app.rule_form.as_ref().unwrap().error.contains("name"));
        let f = app.rule_form.as_mut().unwrap();
        f.rule.name = "Audit orders".into();
        f.rule.message = "ORDERS".into();
        f.order = 1;
        f.rule.actions = vec![
            Action::Delay { delay_ms: 2 },
            Action::Document {
                document: "../order.edi".into(),
            },
        ];
        click(&mut app, &ctx, "Save rule")?;
        let error = app.rule_form.as_ref().unwrap().error.clone();
        assert!(error.contains("inside the case"), "{error}");
        app.rule_form.as_mut().unwrap().rule.actions[1] = Action::Document {
            document: "ordrsp.edi".into(),
        };
        click(&mut app, &ctx, "Save rule")?;
        assert!(app.rule_form.is_none());
        let names: Vec<_> = rules(dir.path())?.into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["Audit orders", "Orders"]);
        // The engine runs the edited case right away.
        assert_eq!(app.workspace.partners[0].scenario()[0].name, "Audit orders");
        // A recorded execution opens its rule in the active case.
        app.edit_rule_link("Supplier", "Audit orders");
        app.rule_form.as_mut().unwrap().rule.version = Some("D96A".into());
        app.rule_form.as_mut().unwrap().order = 2;
        click(&mut app, &ctx, "Save rule")?;
        let saved = rules(dir.path())?;
        assert_eq!(saved[1].name, "Audit orders");
        assert_eq!(saved[1].version.as_deref(), Some("D96A"));
        click(&mut app, &ctx, "Audit orders")?;
        click(&mut app, &ctx, "Remove rule")?;
        click(&mut app, &ctx, "Confirm removal")?;
        assert_eq!(rules(dir.path())?.len(), 1);
        // Imported cases are replaced by the next import, so they are not edited.
        fs::write(
            case::folder(dir.path(), "confirm-orders").join(vendredi_core::import::SOURCE_FILE),
            r#"{"source":"https://example.com/catalog.json"}"#,
        )?;
        app.apply_files(&[])?;
        let c = app
            .workspace
            .cases
            .iter()
            .find(|c| c.id == "confirm-orders")
            .unwrap()
            .clone();
        let mut f = RuleForm::new(&app.root, &c, Some(0));
        f.rule.name = "Changed".into();
        assert!(
            f.save(&mut app)
                .unwrap_err()
                .to_string()
                .contains("Duplicate it")
        );
        Ok(())
    }
}
