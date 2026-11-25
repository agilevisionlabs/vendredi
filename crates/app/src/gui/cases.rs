use super::*;
use crate::cases::{describe, preview};
use egui::{RichText, ScrollArea, Ui};
use vendredi_core::{
    case::{self, Case},
    import::{self, Change, Incoming},
};

type Preview = std::result::Result<(Vec<Incoming>, Vec<Change>), String>;

/// The Import cases dialog. Fetching runs off the UI thread.
pub(super) struct ImportForm {
    source: String,
    pending: Option<mpsc::Receiver<Preview>>,
    preview: Option<Preview>,
}

impl Desktop {
    fn start_preview(&mut self) {
        let Some(form) = &mut self.import else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let root = self.root.clone();
        let source = form.source.trim().to_owned();
        let out = self.output.clone();
        form.preview = None;
        form.pending = Some(rx);
        thread::spawn(move || {
            let _ = tx.send(preview(&root, &source).map_err(|e| format!("{e:#}")));
            out(json!({"event":"import_ready"}));
        });
    }

    fn install(&mut self, incoming: &[Incoming]) -> Result<usize> {
        let changes = import::apply(&self.root, incoming, true)?;
        // Restart the engine so it loads the new cases.
        self.apply_files(&[])?;
        Ok(changes.iter().filter(|c| **c != Change::Unchanged).count())
    }

    pub(super) fn poll_import(&mut self) {
        if let Some(form) = &mut self.import
            && let Some(result) = form.pending.as_ref().and_then(|rx| rx.try_recv().ok())
        {
            form.pending = None;
            form.preview = Some(result);
        }
    }

    pub(super) fn import_dialog(&mut self, ctx: &egui::Context) {
        self.poll_import();
        let Some(form) = &mut self.import else {
            return;
        };
        let mut close = false;
        let mut start = false;
        let mut install = None;
        let escaped = form::modal(ctx, "import cases", "Import cases", 560., |ui| {
            ui.label(muted(
                "Enter a case folder, a case or catalog JSON file, or an https:// URL. Nothing is installed until you confirm.",
            ));
            let field = form::input(
                ui,
                "Folder, file or URL",
                &mut form.source,
                "https://example.com/cases/catalog.json",
            );
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                start = true;
            }
            match &form.preview {
                _ if form.pending.is_some() => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Fetching and checking cases…");
                    });
                }
                Some(Ok((incoming, changes))) => {
                    ScrollArea::vertical().max_height(260.).show(ui, |ui| {
                        for line in describe(incoming, changes) {
                            ui.monospace(line);
                        }
                    });
                    if changes.iter().any(|c| matches!(c, Change::Changed(_))) {
                        ui.colored_label(
                            AMBER,
                            "Changed cases replace the installed copies, including any local edits.",
                        );
                    }
                }
                Some(Err(e)) => {
                    ui.colored_label(AMBER, e);
                }
                None => {}
            }
            form::footer(ui, |ui| {
                let ready = match &form.preview {
                    Some(Ok((incoming, changes))) if form.pending.is_none() => {
                        let count = changes.iter().filter(|c| **c != Change::Unchanged).count();
                        Some((incoming, count))
                    }
                    _ => None,
                };
                match ready {
                    Some((incoming, count)) if count > 0 => {
                        let label =
                            format!("Import {count} case{}", if count == 1 { "" } else { "s" });
                        if button(ui, &label, true).clicked() {
                            install = Some(incoming.clone());
                        }
                    }
                    _ => {
                        let can = !form.source.trim().is_empty() && form.pending.is_none();
                        if ui.add_enabled(can, egui::Button::new("Preview")).clicked() {
                            start = true;
                        }
                    }
                }
                if button(ui, "Cancel", false).clicked() {
                    close = true;
                }
            });
        });
        if start {
            self.start_preview();
        }
        if let Some(incoming) = install {
            match self.install(&incoming) {
                Ok(n) => {
                    self.status = format!("Imported {n} case{}", if n == 1 { "" } else { "s" });
                    if let [one] = incoming.as_slice() {
                        self.case = one.bundle.case.id.clone();
                    }
                    close = true;
                }
                Err(e) => {
                    if let Some(form) = &mut self.import {
                        form.preview = Some(Err(format!("{e:#}")));
                    }
                }
            }
        }
        if close || escaped {
            self.import = None;
        }
    }
}

/// Shared by the case list and the detail card, so both columns have the
/// same border, corners and inner padding.
fn case_frame(fill: egui::Color32, stroke: egui::Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1_f32, stroke))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(16, 12))
}

/// The first line of a card: a fixed height, so titles of different sizes
/// in the two columns share a centre line.
fn title_row(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    let size = egui::vec2(ui.available_width(), 24.);
    ui.allocate_ui_with_layout(
        size,
        egui::Layout::left_to_right(egui::Align::Center),
        contents,
    );
}

impl Desktop {
    /// Rereads the configuration after the engine switched a case and wrote
    /// `vendredi.toml`, so later editor saves start from the new file.
    pub(super) fn case_switched(&mut self, v: &Value) {
        let partner = v["partner"].as_str().unwrap_or_default();
        let to = v["to"].as_str().unwrap_or_default();
        match (
            Workspace::load(&self.root),
            fs::read_to_string(self.root.join("vendredi.toml")),
        ) {
            (Ok(workspace), Ok(source)) => {
                if self.edit_file == Path::new("vendredi.toml") && self.edit == self.saved {
                    self.edit.clone_from(&source);
                    self.saved.clone_from(&source);
                }
                self.config_source = source;
                self.workspace = workspace;
                self.partner = self.partner.min(self.workspace.partners.len() - 1);
                self.status = if to == case::DEFAULT {
                    format!("{partner} now uses its own rules")
                } else {
                    format!("{partner} now uses case {to}")
                };
            }
            (Err(e), _) => self.status = format!("Reloading the workspace: {e:#}"),
            (_, Err(e)) => self.status = format!("Reading vendredi.toml: {e:#}"),
        }
    }

    pub(super) fn show_case(&mut self, id: &str) {
        self.navigate(View::Cases);
        if self.view == View::Cases {
            self.case = id.into();
        }
    }

    pub(super) fn cases(&mut self, ui: &mut Ui) {
        ui.set_max_width(ui.available_width().min(980.));
        header_row(ui, |ui| {
            ui.heading("Cases");
            ui.add_space(20.);
            form::label(ui, "Partner");
            // A button of the row's height opens the partner menu, so the
            // picker matches Import cases… in size and centre line.
            let current = self.workspace.partners[self.partner].name.clone();
            let picker = button(ui, &format!("{current}  ⏷"), false);
            egui::Popup::menu(&picker).show(|ui| {
                for (i, p) in self.workspace.partners.iter().enumerate() {
                    if ui.selectable_label(i == self.partner, &p.name).clicked() {
                        self.partner = i;
                    }
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(ui, "Import cases…", true).clicked() {
                    self.import = Some(ImportForm {
                        source: String::new(),
                        pending: None,
                        preview: None,
                    });
                }
            });
        });
        ui.add_space(6.);
        ui.label(muted(
            "A case is a reusable partner scenario. The active case answers the partner's traffic.",
        ));
        ui.add_space(10.);
        let partner = &self.workspace.partners[self.partner];
        let active = partner.case_id().to_owned();
        if self.case.is_empty() {
            self.case = active.clone();
        }
        let width = ui.available_width();
        let list = (width * 0.42).clamp(240., 380.);
        ui.horizontal_top(|ui| {
            let top_down = egui::Layout::top_down(egui::Align::Min);
            ui.allocate_ui_with_layout(egui::vec2(list, ui.available_height()), top_down, |ui| {
                ScrollArea::vertical().id_salt("case list").show(ui, |ui| {
                    ui.set_width(list);
                    self.case_row(
                        ui,
                        case::DEFAULT,
                        "Partner's own rules",
                        &[],
                        active == case::DEFAULT,
                    );
                    let cases = self.workspace.cases.clone();
                    for c in &cases {
                        self.case_row(ui, &c.id, &c.name, &c.tags, active == c.id);
                    }
                    if cases.is_empty() {
                        ui.add_space(6.);
                        ui.label(muted("No cases are installed under cases/."));
                    }
                });
            });
            ui.add_space(12.);
            ui.vertical(|ui| {
                let selected = self
                    .workspace
                    .cases
                    .iter()
                    .find(|c| c.id == self.case)
                    .cloned();
                case_frame(PANEL, LINE).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match &selected {
                        Some(c) => self.case_detail(ui, c, active == c.id),
                        None => self.default_case_detail(ui, active == case::DEFAULT),
                    }
                });
            });
        });
    }

    fn case_row(&mut self, ui: &mut Ui, id: &str, name: &str, tags: &[String], active: bool) {
        let selected = self.case == id;
        let frame = case_frame(
            if selected { SURFACE } else { BG },
            if selected { BLUE } else { LINE },
        );
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            title_row(ui, |ui| {
                if ui.link(RichText::new(name).font(bold(13.))).clicked() {
                    self.case = id.into();
                }
                if active {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        badge(ui, "Active", BLUE, SURFACE, false);
                    });
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(muted(id).monospace().size(11.));
                for tag in tags {
                    ui.label(muted(format!("#{tag}")).size(11.));
                }
            });
        });
        ui.add_space(4.);
    }

    fn activate_button(&mut self, ui: &mut Ui, id: &str, active: bool, compatible: bool) {
        let partner = self.workspace.partners[self.partner].name.clone();
        if active {
            badge(ui, "Active", BLUE, SURFACE, false);
            ui.label(muted(format!("for {partner}")));
        } else if ui
            .add_enabled(
                compatible,
                egui::Button::new("Make active").min_size(egui::vec2(0., 36.)),
            )
            .on_disabled_hover_text("This case is written for another EDI standard.")
            .clicked()
        {
            self.command(Command::UseCase(partner, id.into()));
        }
    }

    fn default_case_detail(&mut self, ui: &mut Ui, active: bool) {
        let p = &self.workspace.partners[self.partner];
        title_row(ui, |ui| {
            ui.label(RichText::new("Partner's own rules").font(bold(17.)));
        });
        ui.label(muted(format!(
            "The {} rule{} configured on {} under Rules.",
            p.rules.len(),
            if p.rules.len() == 1 { "" } else { "s" },
            p.name
        )));
        ui.add_space(8.);
        header_row(ui, |ui| {
            self.activate_button(ui, case::DEFAULT, active, true)
        });
    }

    fn case_detail(&mut self, ui: &mut Ui, c: &Case, active: bool) {
        let partner = &self.workspace.partners[self.partner];
        let compatible = c.standard == partner.standard;
        let name = partner.name.clone();
        title_row(ui, |ui| {
            ui.label(RichText::new(&c.name).font(bold(17.)));
            badge(ui, c.standard.label(), SECONDARY_TEXT, SURFACE, true);
        });
        ui.label(muted(&c.id).monospace());
        if !c.description.is_empty() {
            ui.add_space(4.);
            ui.label(&c.description);
        }
        ui.add_space(8.);
        form::label(ui, "Rules");
        for r in &c.rules {
            let version = r.combined_version().unwrap_or_else(|| "any version".into());
            ui.label(format!(
                "{} — {} {} · {} action{}",
                r.name,
                r.message,
                version,
                r.actions.len(),
                if r.actions.len() == 1 { "" } else { "s" }
            ));
        }
        if let Some(policy) = c.acknowledge {
            ui.label(muted(format!("Acknowledgement: {policy:?}").to_lowercase()));
        }
        ui.add_space(8.);
        header_row(ui, |ui| self.activate_button(ui, &c.id, active, compatible));
        if !c.samples.is_empty() || !c.initiate.is_empty() {
            ui.add_space(8.);
            form::label(
                ui,
                if active {
                    "Try it"
                } else {
                    "Make the case active to try it"
                },
            );
            ui.add_enabled_ui(active, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for s in &c.samples {
                        if ui
                            .button(format!("Send sample {}", s.name))
                            .on_hover_text("Receive this message as if the integration sent it")
                            .clicked()
                        {
                            self.command(Command::Sample(name.clone(), s.name.clone()));
                            self.status = format!("Sample {} sent to {name}", s.name);
                        }
                    }
                    for i in &c.initiate {
                        if ui
                            .button(format!("Partner sends {}", i.name))
                            .on_hover_text("Run this partner-initiated send")
                            .clicked()
                        {
                            self.command(Command::Initiate(name.clone(), i.name.clone()));
                            self.status = format!("{name} is sending {}", i.name);
                        }
                    }
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests::{click, wait_for};

    #[test]
    #[ignore = "opens a local workspace listener"]
    fn switch_cases_and_send_samples() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let file = dir.path().join("vendredi.toml");
        fs::write(&file, fs::read_to_string(&file)?.replace(":8180", ":0"))?;
        let ctx = egui::Context::default();
        ctx.data_mut(|d| {
            d.insert_temp(egui::Id::new("test screen size"), egui::vec2(1440., 1200.))
        });
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        click(&mut app, &ctx, "Cases")?;
        // The partner picker opens a menu of partners.
        click(&mut app, &ctx, "Supplier  ⏷")?;
        click(&mut app, &ctx, "Supplier")?;
        assert_eq!(app.partner, 0);
        click(&mut app, &ctx, "Reject every order")?;
        assert_eq!(app.case, "reject-orders");
        click(&mut app, &ctx, "Make active")?;
        wait_for(&mut app, "case switch", |a| {
            a.workspace.partners[0].case_id() == "reject-orders"
        })?;
        assert!(app.config_source.contains("case = \"reject-orders\""));
        click(&mut app, &ctx, "Send sample Order")?;
        wait_for(&mut app, "sample reply", |a| {
            a.transfers
                .values()
                .any(|t| t.outgoing && t.status == "Delivered")
        })?;
        // A saved rule edit starts from the configuration the engine wrote.
        click(&mut app, &ctx, "Send an invoice")?;
        click(&mut app, &ctx, "Make active")?;
        wait_for(&mut app, "second switch", |a| {
            a.workspace.partners[0].case_id() == "send-invoice"
        })?;
        click(&mut app, &ctx, "Partner sends Invoice")?;
        wait_for(&mut app, "invoice", |a| {
            a.transfers.values().any(|t| t.kind == "INVOIC")
        })?;
        click(&mut app, &ctx, "Partner's own rules")?;
        click(&mut app, &ctx, "Make active")?;
        wait_for(&mut app, "default case", |a| {
            a.workspace.partners[0].case_id() == case::DEFAULT
        })?;
        assert!(!fs::read_to_string(&file)?.contains("\ncase ="));
        Ok(())
    }

    #[test]
    #[ignore = "opens a local workspace listener"]
    fn import_cases_after_previewing_changes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let file = dir.path().join("vendredi.toml");
        fs::write(&file, fs::read_to_string(&file)?.replace(":8180", ":0"))?;
        let edited = dir.path().join("edited.json");
        let mut json: Value = serde_json::from_str(include_str!(
            "../../../../examples/cases/reject-orders.json"
        ))?;
        json["name"] = "Reject all orders".into();
        fs::write(&edited, serde_json::to_vec(&json)?)?;
        let ctx = egui::Context::default();
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        click(&mut app, &ctx, "Cases")?;
        click(&mut app, &ctx, "Import cases…")?;
        app.import.as_mut().unwrap().source = edited.display().to_string();
        click(&mut app, &ctx, "Preview")?;
        wait_for(&mut app, "import preview", |a| {
            a.import
                .as_ref()
                .is_some_and(|f| f.pending.is_none() && f.preview.is_some())
        })?;
        let Some(Ok((_, changes))) = &app.import.as_ref().unwrap().preview else {
            panic!("expected a preview");
        };
        assert_eq!(changes, &[Change::Changed(vec!["case.json name".into()])]);
        click(&mut app, &ctx, "Import 1 case")?;
        assert!(app.import.is_none(), "{}", app.status);
        assert_eq!(app.status, "Imported 1 case");
        assert_eq!(app.case, "reject-orders");
        let installed = app
            .workspace
            .cases
            .iter()
            .find(|c| c.id == "reject-orders")
            .unwrap();
        assert_eq!(installed.name, "Reject all orders");
        Ok(())
    }
}
