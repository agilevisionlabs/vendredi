use super::*;
use crate::cases::{describe, preview};
use egui::{RichText, ScrollArea, Ui};
use vendredi_core::{
    case::{self, Case},
    import::{self, Change, Incoming},
};
use workspace_edit::FileEdit;

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

/// The New case and Delete case dialogs.
pub(super) enum CaseForm {
    New { name: String, error: String },
    Delete { id: String, error: String },
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
                self.status = format!("{partner} now uses case {to}");
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

    /// The partners whose active case this is.
    fn users(&self, id: &str) -> Vec<String> {
        self.workspace
            .partners
            .iter()
            .filter(|p| p.case_id() == Some(id))
            .map(|p| p.name.clone())
            .collect()
    }

    pub(super) fn cases(&mut self, ui: &mut Ui) {
        // The page uses the whole pane: the header, the list and the detail
        // card share its left and right edges.
        header_row(ui, |ui| {
            ui.heading("Cases");
            ui.add_space(GAP_XL);
            form::label(ui, "Partner");
            // A button of the row's height opens the partner menu, so the
            // picker matches the other buttons in size and centre line.
            let current = self.workspace.partners[self.partner].name.clone();
            let picker = button(ui, &format!("{current}  ⏷"), false);
            egui::Popup::menu(&picker).show(|ui| {
                for (i, p) in self.workspace.partners.iter().enumerate() {
                    if ui.selectable_label(i == self.partner, &p.name).clicked() {
                        self.partner = i;
                        self.case.clear();
                    }
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(ui, "Import cases…", false)
                    .on_hover_text("Import cases from a folder, a JSON file or an https:// URL")
                    .clicked()
                {
                    self.open_import();
                }
                let tip = shortcuts::hint(ui.ctx(), "Start an empty case", &shortcuts::NEW);
                if button(ui, "New case", true).on_hover_text(tip).clicked() {
                    self.case_form = Some(CaseForm::New {
                        name: String::new(),
                        error: String::new(),
                    });
                }
            });
        });
        ui.add_space(GAP_S);
        ui.label(muted(
            "A case is a partner scenario: the rules that answer its messages, with samples to try them. \
             The active case answers the partner's traffic.",
        ));
        ui.add_space(GAP_M);
        let partner = &self.workspace.partners[self.partner];
        let active = partner.case_id().map(str::to_owned);
        if active.is_none() {
            notice(
                ui,
                &format!(
                    "{} has no active case, so it only acknowledges what it receives. Make a case active to answer its messages.",
                    partner.name
                ),
            );
            ui.add_space(GAP_M);
        }
        if self.case.is_empty() || !self.workspace.cases.iter().any(|c| c.id == self.case) {
            self.case = active
                .clone()
                .or_else(|| self.workspace.cases.first().map(|c| c.id.clone()))
                .unwrap_or_default();
        }
        let width = ui.available_width();
        let list = (width * 0.36).clamp(260., 400.);
        ui.horizontal_top(|ui| {
            let top_down = egui::Layout::top_down(egui::Align::Min);
            ui.allocate_ui_with_layout(egui::vec2(list, ui.available_height()), top_down, |ui| {
                ScrollArea::vertical().id_salt("case list").show(ui, |ui| {
                    ui.set_width(list);
                    let cases = self.workspace.cases.clone();
                    for c in &cases {
                        self.case_row(ui, c, active.as_deref() == Some(c.id.as_str()));
                    }
                    if cases.is_empty() {
                        self.no_cases(ui);
                    }
                });
            });
            ui.add_space(GAP_M);
            let selected = self
                .workspace
                .cases
                .iter()
                .find(|c| c.id == self.case)
                .cloned();
            if let Some(c) = selected {
                ui.vertical(|ui| {
                    card_frame(PANEL, LINE).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        self.case_detail(ui, &c, active.as_deref() == Some(c.id.as_str()));
                    });
                });
            }
        });
    }

    fn open_import(&mut self) {
        self.import = Some(ImportForm {
            source: String::new(),
            pending: None,
            preview: None,
        });
    }

    /// The empty list: what cases are for and the ways to get some.
    fn no_cases(&mut self, ui: &mut Ui) {
        card_frame(BG, LINE).show(ui, |ui| {
            ui.set_width(ui.available_width());
            title_row(ui, |ui| {
                ui.label(RichText::new("No cases installed").font(bold(13.)));
            });
            ui.label(muted(
                "Install the starter set of common scenarios, import cases your team keeps in a folder or repository, or start a new one.",
            ));
            ui.add_space(GAP_S);
            ui.horizontal(|ui| {
                if button(ui, "Install starter cases", true).clicked() {
                    self.install_starter_cases();
                }
                if button(ui, "Import…", false).clicked() {
                    self.open_import();
                }
            });
        });
    }

    fn install_starter_cases(&mut self) {
        let result = crate::install_starter_cases(&self.root).and_then(|(_, n)| {
            self.apply_files(&[])?;
            Ok(n)
        });
        self.status = match result {
            Ok(n) => format!(
                "Installed {n} starter case{}",
                if n == 1 { "" } else { "s" }
            ),
            Err(e) => format!("Installing starter cases: {e:#}"),
        };
    }

    fn case_row(&mut self, ui: &mut Ui, c: &Case, active: bool) {
        let selected = self.case == c.id;
        let frame = card_frame(
            if selected { SURFACE } else { BG },
            if selected { BLUE } else { LINE },
        );
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            title_row(ui, |ui| {
                if ui.link(RichText::new(&c.name).font(bold(13.))).clicked() {
                    self.case = c.id.clone();
                }
                if active {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        badge(ui, "Active", BLUE, SURFACE, false);
                    });
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(muted(&c.id).monospace().size(11.));
                if c.standard != self.workspace.partners[self.partner].standard {
                    ui.label(muted(c.standard.label()).size(11.));
                }
                for tag in &c.tags {
                    ui.label(muted(format!("#{tag}")).size(11.));
                }
            });
        });
        ui.add_space(GAP_XS);
    }

    fn activate_button(&mut self, ui: &mut Ui, id: &str, active: bool, compatible: bool) {
        let partner = self.workspace.partners[self.partner].name.clone();
        if active {
            badge(ui, "Active", BLUE, SURFACE, false);
            ui.label(muted(format!("Answering {partner}'s traffic")));
        } else if ui
            .add_enabled(
                compatible,
                styled(&format!("Make active for {partner}"), false, BUTTON),
            )
            .on_disabled_hover_text("This case is written for another EDI standard.")
            .clicked()
        {
            self.command(Command::UseCase(partner, id.into()));
        }
    }

    fn duplicate_case(&mut self, c: &Case) {
        let result = (|| -> Result<String> {
            let mut bundle = case::Bundle::read(&case::folder(&self.root, &c.id))?;
            let name = format!("{} (copy)", c.name);
            let id = case::unused_id(&self.root, &name);
            bundle.case.id.clone_from(&id);
            bundle.case.name = name;
            bundle.write(&case::folder(&self.root, &id))?;
            self.apply_files(&[])?;
            Ok(id)
        })();
        match result {
            Ok(id) => {
                self.status = format!("Copied {} to {id}", c.id);
                self.case = id;
            }
            Err(e) => self.status = format!("Copying {}: {e:#}", c.id),
        }
    }

    fn case_detail(&mut self, ui: &mut Ui, c: &Case, active: bool) {
        let partner = &self.workspace.partners[self.partner];
        let compatible = c.standard == partner.standard;
        let name = partner.name.clone();
        title_row(ui, |ui| {
            ui.label(RichText::new(&c.name).font(bold(17.)));
            badge(ui, c.standard.label(), SECONDARY_TEXT, SURFACE, true);
            if let Some(source) = &c.source {
                ui.label(muted("Imported").size(12.))
                    .on_hover_text(format!("Imported from {source}"));
            }
        });
        ui.label(muted(&c.id).monospace());
        if !c.description.is_empty() {
            ui.add_space(GAP_XS);
            ui.label(&c.description);
        }
        ui.add_space(GAP_M);
        header_row(ui, |ui| {
            self.activate_button(ui, &c.id, active, compatible);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let users = self.users(&c.id);
                if ui
                    .add_enabled(users.is_empty(), form::removal_button("Delete"))
                    .on_disabled_hover_text(format!(
                        "Active for {}. Make another case active first.",
                        users.join(", ")
                    ))
                    .clicked()
                {
                    self.case_form = Some(CaseForm::Delete {
                        id: c.id.clone(),
                        error: String::new(),
                    });
                }
                if button(ui, "Duplicate", false)
                    .on_hover_text("Copy this case, with its files, to change it")
                    .clicked()
                {
                    self.duplicate_case(c);
                }
            });
        });
        ui.add_space(GAP_M);
        ui.separator();
        ui.add_space(GAP_S);
        section_row(ui, |ui| {
            form::label(ui, "Rules");
            ui.label(muted("The first rule that matches a message runs."));
            if c.source.is_none() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if small_button(ui, "+ Add rule").clicked() {
                        self.rule_form = Some(rules::RuleForm::new(&self.root, c, None));
                    }
                });
            }
        });
        if let Some(source) = &c.source {
            ui.label(muted(format!(
                "Imported from {source}. Importing it again replaces it, so duplicate it to make changes."
            )));
            ui.add_space(GAP_XS);
        }
        if c.rules.is_empty() {
            ui.label(muted("No rules yet: messages are only acknowledged."));
        } else if let Some(i) = rules::rule_table(ui, &c.rules) {
            self.rule_form = Some(rules::RuleForm::new(&self.root, c, Some(i)));
        }
        if let Some(policy) = c.acknowledge {
            ui.add_space(GAP_XS);
            ui.label(muted(format!("Acknowledgement: {policy:?}").to_lowercase()));
        }
        if !c.samples.is_empty() || !c.initiate.is_empty() {
            ui.add_space(GAP_M);
            ui.separator();
            ui.add_space(GAP_S);
            section_row(ui, |ui| {
                form::label(ui, "Try it");
                if !active {
                    ui.label(muted("Make the case active to send its samples."));
                }
            });
            ui.add_enabled_ui(active, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for s in &c.samples {
                        if small_button(ui, &format!("Send sample {}", s.name))
                            .on_hover_text("Receive this message as if the integration sent it")
                            .clicked()
                        {
                            self.command(Command::Sample(name.clone(), s.name.clone()));
                            self.status = format!("Sample {} sent to {name}", s.name);
                        }
                    }
                    for i in &c.initiate {
                        if small_button(ui, &format!("Partner sends {}", i.name))
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

    fn create_case(&mut self, name: &str) -> Result<String> {
        let name = name.trim();
        ensure!(!name.is_empty(), "Enter a case name.");
        let id = case::unused_id(&self.root, name);
        let standard = self.workspace.partners[self.partner].standard;
        let json = case::to_json(&case::empty(&id, name, standard))?;
        let path = case::folder(&self.root, &id).join(case::FILE);
        self.apply_files(&[FileEdit {
            path: &path,
            before: None,
            after: json.as_bytes(),
        }])?;
        Ok(id)
    }

    fn delete_case(&mut self, id: &str) -> Result<()> {
        let users = self.users(id);
        ensure!(
            users.is_empty(),
            "{id} is active for {}. Make another case active first.",
            users.join(", ")
        );
        fs::remove_dir_all(case::folder(&self.root, id))?;
        self.apply_files(&[])
    }

    pub(super) fn case_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.case_form.take() else {
            return;
        };
        let mut close = false;
        let title = match &f {
            CaseForm::New { .. } => "New case",
            CaseForm::Delete { .. } => "Delete case?",
        };
        let escaped = form::modal(ctx, "case editor", title, 480., |ui| match &mut f {
            CaseForm::New { name, error } => {
                let standard = self.workspace.partners[self.partner].standard;
                ui.label(muted(format!(
                    "An empty {} case. Add rules to it once it exists.",
                    standard.label()
                )));
                form::input(ui, "Name", name, "Northline Supply orders");
                if !name.trim().is_empty() {
                    ui.label(muted(format!(
                        "Folder: cases/{}",
                        case::unused_id(&self.root, name)
                    )));
                }
                if !error.is_empty() {
                    ui.colored_label(AMBER, error.as_str());
                }
                form::footer(ui, |ui| {
                    if button(ui, "Create case", true).clicked() {
                        match self.create_case(name) {
                            Ok(id) => {
                                self.status = format!("Created case {id}");
                                self.case = id;
                                close = true;
                            }
                            Err(e) => *error = format!("{e:#}"),
                        }
                    }
                    if button(ui, "Cancel", false).clicked() {
                        close = true;
                    }
                });
            }
            CaseForm::Delete { id, error } => {
                ui.label(format!(
                    "Delete cases/{id} with its rules, documents and scripts? This cannot be undone."
                ));
                if !error.is_empty() {
                    ui.colored_label(AMBER, error.as_str());
                }
                form::footer(ui, |ui| {
                    if ui.add(form::removal_button("Delete case")).clicked() {
                        match self.delete_case(id) {
                            Ok(()) => {
                                self.status = format!("Deleted case {id}");
                                self.case.clear();
                                close = true;
                            }
                            Err(e) => *error = format!("{e:#}"),
                        }
                    }
                    if button(ui, "Cancel", false).clicked() {
                        close = true;
                    }
                });
            }
        });
        if !close && !escaped {
            self.case_form = Some(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests::{click, wait_for};

    fn open(dir: &Path) -> Result<(Desktop, egui::Context)> {
        crate::sample(dir)?;
        let file = dir.join("vendredi.toml");
        fs::write(&file, fs::read_to_string(&file)?.replace(":8180", ":0"))?;
        let ctx = egui::Context::default();
        ctx.data_mut(|d| {
            d.insert_temp(egui::Id::new("test screen size"), egui::vec2(1440., 1400.))
        });
        Ok((Desktop::open(dir.into(), &ctx)?, ctx))
    }

    #[test]
    #[ignore = "opens a local workspace listener"]
    fn switch_cases_and_send_samples() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("vendredi.toml");
        let (mut app, ctx) = open(dir.path())?;
        click(&mut app, &ctx, "Cases")?;
        assert_eq!(app.case, "confirm-orders");
        // The partner picker opens a menu of partners.
        click(&mut app, &ctx, "Supplier  ⏷")?;
        click(&mut app, &ctx, "Supplier")?;
        assert_eq!(app.partner, 0);
        click(&mut app, &ctx, "Reject every order")?;
        assert_eq!(app.case, "reject-orders");
        click(&mut app, &ctx, "Make active for Supplier")?;
        wait_for(&mut app, "case switch", |a| {
            a.workspace.partners[0].case_id() == Some("reject-orders")
        })?;
        assert!(app.config_source.contains("case = \"reject-orders\""));
        click(&mut app, &ctx, "Send sample Order")?;
        wait_for(&mut app, "sample reply", |a| {
            a.transfers
                .values()
                .any(|t| t.outgoing && t.status == "Delivered")
        })?;
        click(&mut app, &ctx, "Send an invoice")?;
        click(&mut app, &ctx, "Make active for Supplier")?;
        wait_for(&mut app, "second switch", |a| {
            a.workspace.partners[0].case_id() == Some("send-invoice")
        })?;
        click(&mut app, &ctx, "Partner sends Invoice")?;
        wait_for(&mut app, "invoice", |a| {
            a.transfers.values().any(|t| t.kind == "INVOIC")
        })?;
        assert!(fs::read_to_string(&file)?.contains("case = \"send-invoice\""));
        Ok(())
    }

    #[test]
    #[ignore = "opens a local workspace listener"]
    fn create_duplicate_and_delete_cases() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let (mut app, ctx) = open(dir.path())?;
        click(&mut app, &ctx, "Cases")?;
        click(&mut app, &ctx, "New case")?;
        click(&mut app, &ctx, "Create case")?;
        assert!(
            matches!(&app.case_form, Some(CaseForm::New { error, .. }) if error.contains("name"))
        );
        if let Some(CaseForm::New { name, .. }) = &mut app.case_form {
            *name = "Northline Supply".into();
        }
        click(&mut app, &ctx, "Create case")?;
        assert!(app.case_form.is_none());
        assert_eq!(app.case, "northline-supply");
        // Buttons in a row share one height and centre line.
        let output = crate::gui::tests::render(&ctx, vec![], |ctx| app.show(ctx));
        let rect = |label: &str| {
            let text = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == label => {
                        Some(t.visual_bounding_rect())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing {label}"));
            // The button frame is the smallest filled rectangle around the text.
            output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Rect(r) if r.rect.contains_rect(text) && r.rect.height() < 60. => {
                        Some(r.rect)
                    }
                    _ => None,
                })
                .min_by(|a, b| a.area().total_cmp(&b.area()))
                .unwrap_or(text)
        };
        let row = ["Make active for Supplier", "Duplicate", "Delete"].map(rect);
        for r in &row {
            assert_eq!(r.height(), BUTTON, "{row:?}");
            assert!((r.center().y - row[0].center().y).abs() < 0.5, "{row:?}");
        }
        let add = rect("+ Add rule");
        assert_eq!(add.height(), SMALL_BUTTON);
        let rules = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "Rules" => {
                    Some(t.visual_bounding_rect())
                }
                _ => None,
            })
            .unwrap();
        assert!(
            (rules.center().y - add.center().y).abs() < 1.,
            "{rules:?} {add:?}"
        );
        // The section buttons end at the same right edge as the row above.
        assert!(
            (add.right() - row[2].right()).abs() < 0.5,
            "{add:?} {:?}",
            row[2]
        );
        let created = app
            .workspace
            .cases
            .iter()
            .find(|c| c.id == "northline-supply")
            .unwrap();
        assert!(created.rules.is_empty() && created.source.is_none());
        // The active case cannot be deleted; a copy of it can.
        click(&mut app, &ctx, "Confirm every order")?;
        click(&mut app, &ctx, "Duplicate")?;
        assert_eq!(app.case, "confirm-every-order-copy");
        let copy = case::folder(dir.path(), "confirm-every-order-copy");
        assert!(copy.join("confirm.js").is_file());
        assert_eq!(
            app.workspace
                .cases
                .iter()
                .find(|c| c.id == "confirm-every-order-copy")
                .unwrap()
                .name,
            "Confirm every order (copy)"
        );
        click(&mut app, &ctx, "Delete")?;
        click(&mut app, &ctx, "Delete case")?;
        assert!(app.case_form.is_none());
        assert!(!copy.exists());
        assert!(
            app.workspace
                .cases
                .iter()
                .all(|c| c.id != "confirm-every-order-copy")
        );
        click(&mut app, &ctx, "Confirm every order")?;
        assert!(
            app.delete_case("confirm-orders")
                .unwrap_err()
                .to_string()
                .contains("active for Supplier")
        );
        Ok(())
    }

    #[test]
    #[ignore = "opens a local workspace listener"]
    fn old_partner_rules_are_ignored_quietly() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let file = dir.path().join("vendredi.toml");
        let config = fs::read_to_string(&file)?.replace(":8180", ":0").replace(
            "case = \"confirm-orders\"\n",
            "\n[[partners.rules]]\nname = \"Old\"\nmessage = \"ORDERS\"\nactions = []\n",
        );
        fs::write(&file, config)?;
        let ctx = egui::Context::default();
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        assert_eq!(app.workspace.warnings.len(), 1);
        // The desktop opens normally and does not show the warning.
        click(&mut app, &ctx, "Cases")?;
        let warning = app.workspace.warnings[0].clone();
        assert!(click(&mut app, &ctx, &warning).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "opens a local workspace listener"]
    fn the_last_page_is_restored() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let (mut app, ctx) = open(dir.path())?;
        click(&mut app, &ctx, "Cases")?;
        assert_eq!(app.place(), "cases");
        click(&mut app, &ctx, "Supplier")?;
        let place = app.place();
        assert_eq!(place, "partner Supplier");
        click(&mut app, &ctx, "All traffic")?;
        app.restore(Some(&place));
        assert!(app.view == View::Partner(0));
        // A page that no longer exists leaves the view alone.
        app.restore(Some("partner Removed"));
        assert!(app.view == View::Partner(0));
        Ok(())
    }

    #[test]
    #[ignore = "opens a local workspace listener"]
    fn install_starter_cases_from_the_empty_list() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let file = dir.path().join("vendredi.toml");
        let config = fs::read_to_string(&file)?
            .replace(":8180", ":0")
            .replace("case = \"confirm-orders\"\n", "");
        fs::write(&file, config)?;
        fs::remove_dir_all(dir.path().join(case::DIRECTORY))?;
        let ctx = egui::Context::default();
        ctx.data_mut(|d| {
            d.insert_temp(egui::Id::new("test screen size"), egui::vec2(1440., 1200.))
        });
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        assert!(app.workspace.cases.is_empty());
        assert_eq!(app.workspace.partners[0].case_id(), None);
        click(&mut app, &ctx, "Cases")?;
        click(&mut app, &ctx, "Install starter cases")?;
        assert_eq!(app.status, "Installed 13 starter cases");
        assert_eq!(app.workspace.cases.len(), 13);
        click(&mut app, &ctx, "Reject every order")?;
        assert_eq!(app.case, "reject-orders");
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
