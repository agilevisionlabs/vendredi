use super::*;
use crate::cases::{describe, preview};
use egui::{RichText, ScrollArea, Ui};
use vendredi_core::{
    case::{self, Case},
    import::{self, Change, Incoming},
};
use workspace_edit::FileEdit;

type Preview = std::result::Result<(Vec<Incoming>, Vec<Change>), String>;

type CheckResults = std::result::Result<Vec<(String, Vec<String>)>, String>;

/// The case checks running in the background, or their last results.
pub(super) struct Checks {
    case: String,
    pending: Option<mpsc::Receiver<CheckResults>>,
    pub(super) results: Option<CheckResults>,
}

/// The Import cases dialog. Fetching runs off the UI thread.
pub(super) struct ImportForm {
    source: String,
    pending: Option<mpsc::Receiver<Preview>>,
    preview: Option<Preview>,
}

impl Desktop {
    pub(super) fn start_preview(&mut self) {
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
            let home = self.root.clone();
            let mut folder = false;
            form::path_input(
                ui,
                "Folder, file or URL",
                &mut form.source,
                "https://example.com/cases/catalog.json",
                || {
                    let chosen = form::pick_file(&home, &[("Cases and catalogs", &["json"])])?;
                    // A chosen file is previewed straight away.
                    start = true;
                    Some(chosen.display().to_string())
                },
            );
            if ui.link("Choose a case folder instead…").clicked() {
                folder = true;
            }
            if folder && let Some(dir) = form::pick_folder(&home) {
                form.source = dir.display().to_string();
                start = true;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !form.source.trim().is_empty() {
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
                            col::amber(),
                            "Changed cases replace the installed copies, including any local edits.",
                        );
                    }
                }
                Some(Err(e)) => {
                    ui.colored_label(col::amber(), e);
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

/// The New case dialog.
pub(super) enum CaseForm {
    New { name: String, error: String },
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
            (Err(e), _) => self.fail(format!("Reloading the workspace: {e:#}")),
            (_, Err(e)) => self.fail(format!("Reading vendredi.toml: {e:#}")),
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
            let picker = button(ui, &format!("{current}  {}", icon::CARET_DOWN), false);
            egui::Popup::menu(&picker).show(|ui| {
                for (i, p) in self.workspace.partners.iter().enumerate() {
                    if ui.selectable_label(i == self.partner, &p.name).clicked() {
                        self.partner = i;
                        self.case.clear();
                    }
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(
                    ui,
                    &with_icon(icon::DOWNLOAD_SIMPLE, "Import cases…"),
                    false,
                )
                .on_hover_text("Import cases from a folder, a JSON file or an https:// URL")
                .clicked()
                {
                    self.open_import();
                }
                let tip = shortcuts::hint(ui.ctx(), "Start an empty case", &shortcuts::NEW);
                if button(ui, &with_icon(icon::PLUS, "New case"), true)
                    .on_hover_text(tip)
                    .clicked()
                {
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
                    card_frame(col::panel(), col::line()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        self.case_detail(ui, &c, active.as_deref() == Some(c.id.as_str()));
                    });
                });
            }
        });
    }

    pub(super) fn open_import(&mut self) {
        self.import = Some(ImportForm {
            source: String::new(),
            pending: None,
            preview: None,
        });
    }

    /// Opens the import dialog on a dropped or chosen case, already previewing.
    pub(super) fn import_from(&mut self, source: &Path) {
        self.open_import();
        if let Some(form) = &mut self.import {
            form.source = source.display().to_string();
        }
        self.start_preview();
    }

    /// The empty list: what cases are for and the ways to get some.
    fn no_cases(&mut self, ui: &mut Ui) {
        card_frame(col::bg(), col::line()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            title_row(ui, |ui| {
                ui.label(RichText::new("No cases installed").font(ty::strong()));
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
            if selected { col::surface() } else { col::bg() },
            if selected { col::blue() } else { col::line() },
        );
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            title_row(ui, |ui| {
                if ui.link(RichText::new(&c.name).font(ty::strong())).clicked() {
                    self.case = c.id.clone();
                }
                if active {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        badge(ui, "Active", col::blue(), col::surface(), false);
                    });
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(muted(&c.id).monospace().size(ty::CAPTION));
                if c.standard != self.workspace.partners[self.partner].standard {
                    ui.label(muted(c.standard.label()).size(ty::CAPTION));
                }
                for tag in &c.tags {
                    ui.label(muted(format!("#{tag}")).size(ty::CAPTION));
                }
            });
        });
        ui.add_space(GAP_XS);
    }

    fn activate_button(&mut self, ui: &mut Ui, id: &str, active: bool, compatible: bool) {
        let partner = self.workspace.partners[self.partner].name.clone();
        if active {
            badge(ui, "Active", col::blue(), col::surface(), false);
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
            Err(e) => self.fail(format!("Copying {}: {e:#}", c.id)),
        }
    }

    fn case_detail(&mut self, ui: &mut Ui, c: &Case, active: bool) {
        let partner = &self.workspace.partners[self.partner];
        let compatible = c.standard == partner.standard;
        let name = partner.name.clone();
        title_row(ui, |ui| {
            ui.label(RichText::new(&c.name).font(ty::heading()));
            badge(
                ui,
                c.standard.label(),
                col::secondary_text(),
                col::surface(),
                true,
            );
            if let Some(source) = &c.source {
                ui.label(muted("Imported").size(ty::SMALL))
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
                    .add_enabled(
                        users.is_empty(),
                        form::removal_button(&with_icon(icon::TRASH, "Delete")),
                    )
                    .on_disabled_hover_text(format!(
                        "Active for {}. Make another case active first.",
                        users.join(", ")
                    ))
                    .clicked()
                {
                    // Deleting is immediate; the toast offers Undo.
                    match self.delete_case(&c.id) {
                        Ok(()) => self.case.clear(),
                        Err(e) => self.fail(format!("{e:#}")),
                    }
                }
                if button(ui, &with_icon(icon::COPY, "Duplicate"), false)
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
                    if small_button(ui, &with_icon(icon::PLUS, "Add rule")).clicked() {
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
        ui.add_space(GAP_M);
        ui.separator();
        ui.add_space(GAP_S);
        section_row(ui, |ui| {
            form::label(ui, "Try it");
            if !active {
                ui.label(muted("Make the case active to send its samples."));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if c.source.is_none()
                    && small_button(ui, &with_icon(icon::PLUS, "Add sample"))
                        .on_hover_text("Choose a message the integration would send")
                        .clicked()
                {
                    let folder = case::folder(&self.root, &c.id);
                    if let Some(file) =
                        form::pick_file(&folder, &[("EDI documents", &["edi", "x12", "txt"])])
                        && let Err(e) = self.add_sample(c, &file)
                    {
                        self.fail(format!("{e:#}"));
                    }
                }
                if !c.expect.is_empty() {
                    let running = self.checks.as_ref().is_some_and(|k| k.case == c.id && k.pending.is_some());
                    if ui
                        .add_enabled(!running, styled(&with_icon(icon::CHECK_CIRCLE, "Run checks"), false, SMALL_BUTTON))
                        .on_hover_text("Send each sample in a scratch workspace and compare the replies with the expected ones")
                        .clicked()
                    {
                        self.run_checks(c);
                    }
                }
            });
        });
        self.check_results(ui, c);
        if c.samples.is_empty() && c.initiate.is_empty() {
            ui.label(muted(
                "Samples are messages the integration would send. Add one to try the case with a click.",
            ));
        } else {
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
            if c.source.is_none() && !c.samples.is_empty() {
                let mut remove = None;
                ui.horizontal_wrapped(|ui| {
                    for s in &c.samples {
                        if small_button(ui, &with_icon(icon::X, &format!("Remove {}", s.name)))
                            .on_hover_text("Remove this sample and its expected replies")
                            .clicked()
                        {
                            remove = Some(s.name.clone());
                        }
                    }
                });
                if let Some(sample) = remove
                    && let Err(e) = self.remove_sample(c, &sample)
                {
                    self.fail(format!("{e:#}"));
                }
            }
        }
    }

    /// Rewrites `case.json` of an editable case, offering Undo.
    fn edit_case(&mut self, c: &Case, what: &str, change: impl FnOnce(&mut Case)) -> Result<()> {
        ensure!(
            c.source.is_none(),
            "This case was imported. Duplicate it to change it."
        );
        let path = case::folder(&self.root, &c.id).join(case::FILE);
        let before = fs::read(&path)?;
        let mut case = Case::parse(&before)?;
        change(&mut case);
        let after = case::to_json(&case)?;
        self.apply_files(&[FileEdit {
            path: &path,
            before: Some(&before),
            after: after.as_bytes(),
        }])?;
        self.undoable(what.to_owned(), toast::Undo::Files(vec![(path, before)]));
        Ok(())
    }

    fn add_sample(&mut self, c: &Case, file: &Path) -> Result<()> {
        let folder = case::folder(&self.root, &c.id);
        // A sample is one message, checked before it joins the case.
        vendredi_core::interchange::document(c.standard, &fs::read(file)?).with_context(|| {
            format!(
                "{} is not a single {} message",
                file.display(),
                c.standard.label()
            )
        })?;
        let inside = form::into_folder(&folder, file)?;
        let stem = inside
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Sample".into());
        let mut name = stem.clone();
        for n in 2.. {
            if !c.samples.iter().any(|s| s.name == name) {
                break;
            }
            name = format!("{stem} {n}");
        }
        let sample = case::Sample {
            name: name.clone(),
            document: inside,
        };
        self.edit_case(c, &format!("Added sample {name}"), |case| {
            case.samples.push(sample)
        })
    }

    fn remove_sample(&mut self, c: &Case, name: &str) -> Result<()> {
        // Expectations on the sample go with it.
        self.edit_case(c, &format!("Removed sample {name}"), |case| {
            case.samples.retain(|s| s.name != name);
            case.expect.retain(|e| e.sample.as_deref() != Some(name));
        })
    }

    fn run_checks(&mut self, c: &Case) {
        let folder = case::folder(&self.root, &c.id);
        let (tx, rx) = mpsc::channel();
        let out = self.output.clone();
        thread::spawn(move || {
            let result = case::Bundle::load(&folder)
                .and_then(|b| vendredi_core::selftest::run(&b))
                .map(|outcomes| {
                    outcomes
                        .into_iter()
                        .map(|o| (o.check, o.failures))
                        .collect()
                })
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            out(json!({"event":"checks_ready"}));
        });
        self.checks = Some(Checks {
            case: c.id.clone(),
            pending: Some(rx),
            results: None,
        });
    }

    /// Collects finished checks from the background thread.
    pub(super) fn poll_checks(&mut self) {
        if let Some(k) = &mut self.checks
            && let Some(result) = k.pending.as_ref().and_then(|rx| rx.try_recv().ok())
        {
            k.pending = None;
            k.results = Some(result);
        }
    }

    /// The outcome of the last checks of this case, one line each.
    fn check_results(&mut self, ui: &mut Ui, c: &Case) {
        let Some(k) = &mut self.checks else {
            return;
        };
        if k.case != c.id {
            return;
        }
        if k.pending.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(muted("Running the case's checks…"));
            });
            return;
        }
        match &k.results {
            Some(Ok(outcomes)) => {
                for (check, failures) in outcomes {
                    let (mark, color) = if failures.is_empty() {
                        (icon::CHECK_CIRCLE, col::green())
                    } else {
                        (icon::WARNING_CIRCLE, col::amber())
                    };
                    let line = ui.label(
                        RichText::new(format!("{mark} {check}"))
                            .size(ty::SMALL)
                            .color(color),
                    );
                    if !failures.is_empty() {
                        line.on_hover_text(failures.join("\n"));
                    }
                }
            }
            Some(Err(e)) => {
                ui.colored_label(col::amber(), e);
            }
            None => {}
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
        // The folder is set aside, not removed, so the deletion can be undone.
        let folder = case::folder(&self.root, id);
        let trash = self.root.join(".vendredi/trash");
        fs::create_dir_all(&trash)?;
        let aside = tempfile::Builder::new()
            .prefix(&format!("{id}-"))
            .tempdir_in(&trash)?
            .keep();
        fs::remove_dir(&aside)?;
        fs::rename(&folder, &aside)?;
        if let Err(e) = self.apply_files(&[]) {
            fs::rename(&aside, &folder)?;
            return Err(e);
        }
        self.undoable(
            format!("Deleted case {id}"),
            toast::Undo::Folder {
                from: aside,
                to: folder,
            },
        );
        Ok(())
    }

    pub(super) fn case_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.case_form.take() else {
            return;
        };
        let mut close = false;
        let title = match &f {
            CaseForm::New { .. } => "New case",
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
                    ui.colored_label(col::amber(), error.as_str());
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
        click(&mut app, &ctx, &format!("Supplier  {}", icon::CARET_DOWN))?;
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
        click(&mut app, &ctx, &with_icon(icon::PLUS, "New case"))?;
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
        let row = [
            "Make active for Supplier".to_owned(),
            with_icon(icon::COPY, "Duplicate"),
            with_icon(icon::TRASH, "Delete"),
        ]
        .map(|l| rect(&l));
        for r in &row {
            assert_eq!(r.height(), BUTTON, "{row:?}");
            assert!((r.center().y - row[0].center().y).abs() < 0.5, "{row:?}");
        }
        let add = rect(&with_icon(icon::PLUS, "Add rule"));
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
        click(&mut app, &ctx, &with_icon(icon::COPY, "Duplicate"))?;
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
        click(&mut app, &ctx, &with_icon(icon::TRASH, "Delete"))?;
        assert!(!copy.exists());
        assert_eq!(app.status, "Deleted case confirm-every-order-copy");
        click(&mut app, &ctx, "Undo")?;
        assert!(copy.join("confirm.js").is_file());
        assert!(
            app.workspace
                .cases
                .iter()
                .any(|c| c.id == "confirm-every-order-copy")
        );
        click(&mut app, &ctx, "Confirm every order (copy)")?;
        click(&mut app, &ctx, &with_icon(icon::TRASH, "Delete"))?;
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
    fn dropped_files_open_send_or_import() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let (mut app, ctx) = open(dir.path())?;
        let drop = |app: &mut Desktop, path: &Path| {
            let _ = ctx.run(
                egui::RawInput {
                    dropped_files: vec![egui::DroppedFile {
                        path: Some(path.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                |ctx| app.show(ctx),
            );
        };
        drop(&mut app, &dir.path().join("order.edi"));
        assert!(app.send_dialog && !app.send_script);
        assert_eq!(app.send_file, "order.edi");
        app.send_dialog = false;
        let case = dir.path().join("cases/reject-orders/case.json");
        drop(&mut app, &case);
        let form = app.import.as_ref().context("import dialog")?;
        assert_eq!(form.source, case.display().to_string());
        assert!(form.pending.is_some() || form.preview.is_some());
        Ok(())
    }

    #[test]
    fn chosen_files_are_copied_into_the_case() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let folder = dir.path().join("cases/orders");
        fs::create_dir_all(&folder)?;
        fs::write(folder.join("inside.edi"), "a")?;
        assert_eq!(
            form::into_folder(&folder, &folder.join("inside.edi"))?,
            Path::new("inside.edi")
        );
        let outside = dir.path().join("reply.edi");
        fs::write(&outside, "b")?;
        assert_eq!(
            form::into_folder(&folder, &outside)?,
            Path::new("reply.edi")
        );
        assert_eq!(fs::read_to_string(folder.join("reply.edi"))?, "b");
        // The same file again is fine; a different one with that name is not.
        assert!(form::into_folder(&folder, &outside).is_ok());
        fs::write(&outside, "c")?;
        assert!(
            form::into_folder(&folder, &outside)
                .unwrap_err()
                .to_string()
                .contains("different reply.edi")
        );
        Ok(())
    }

    #[test]
    #[ignore = "opens a local workspace listener"]
    fn samples_are_added_removed_and_checked() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let (mut app, ctx) = open(dir.path())?;
        click(&mut app, &ctx, "Cases")?;
        click(&mut app, &ctx, &with_icon(icon::CHECK_CIRCLE, "Run checks"))?;
        wait_for(&mut app, "checks", |a| {
            a.checks.as_ref().is_some_and(|k| k.results.is_some())
        })?;
        // The results show on the next frame; the starter case passes.
        crate::gui::tests::frame(&mut app, &ctx, vec![]);
        let Some(Ok(results)) = &app.checks.as_ref().unwrap().results else {
            panic!("checks failed to run");
        };
        assert!(
            !results.is_empty() && results.iter().all(|(_, f)| f.is_empty()),
            "{results:?}"
        );
        let c = app
            .workspace
            .cases
            .iter()
            .find(|c| c.id == "confirm-orders")
            .unwrap()
            .clone();
        // Only single messages of the case's standard can be samples.
        let bad = dir.path().join("bad.edi");
        fs::write(&bad, "not edi")?;
        assert!(app.add_sample(&c, &bad).is_err());
        let order = dir.path().join("big-order.edi");
        fs::write(&order, "UNH+1+ORDERS:D:96A:UN'BGM+220+PO9+9'UNT+3+1'")?;
        app.add_sample(&c, &order)?;
        let c = app
            .workspace
            .cases
            .iter()
            .find(|c| c.id == "confirm-orders")
            .unwrap()
            .clone();
        assert_eq!(
            c.samples
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["Order", "big-order"]
        );
        assert!(
            case::folder(dir.path(), "confirm-orders")
                .join("big-order.edi")
                .is_file()
        );
        app.remove_sample(&c, "Order")?;
        let c = app
            .workspace
            .cases
            .iter()
            .find(|c| c.id == "confirm-orders")
            .unwrap()
            .clone();
        assert_eq!(c.samples.len(), 1);
        // Its expectation went with it, so the case still loads.
        assert!(
            c.expect
                .iter()
                .all(|e| e.sample.as_deref() != Some("Order"))
        );
        click(&mut app, &ctx, "Undo")?;
        let c = app
            .workspace
            .cases
            .iter()
            .find(|c| c.id == "confirm-orders")
            .unwrap();
        assert_eq!(c.samples.len(), 2);
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
        click(
            &mut app,
            &ctx,
            &with_icon(icon::DOWNLOAD_SIMPLE, "Import cases…"),
        )?;
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
