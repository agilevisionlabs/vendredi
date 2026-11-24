use super::*;
use egui::{ComboBox, Modal, ScrollArea, Ui};
use toml_edit::DocumentMut;
use vendredi_core::config::{Partner, Policy};
use workspace_edit::tables;

pub(super) struct PartnerForm {
    index: Option<usize>,
    partner: Partner,
    headers: Vec<(String, String)>,
    error: String,
    confirm_remove: bool,
}
impl PartnerForm {
    fn new(workspace: &Workspace, index: Option<usize>) -> Self {
        let n = workspace.partners.len() + 1;
        let partner = index
            .map(|i| workspace.partners[i].clone())
            .unwrap_or_else(|| Partner {
                name: String::new(),
                identity: vec![String::new(), "ZZ".into()],
                charset: "UNOC".into(),
                acknowledge: Policy::Always,
                separators: None,
                rules: vec![],
                receive: vec![Receive::Folder {
                    directory: format!("inbox-{n}").into(),
                    suffix: ".edi".into(),
                }],
                send: SendChannel::Folder {
                    directory: format!("replies-{n}").into(),
                },
            });
        let headers = match &partner.send {
            SendChannel::Http { headers, .. } => headers.clone().into_iter().collect(),
            _ => vec![],
        };
        Self {
            index,
            partner,
            headers,
            error: String::new(),
            confirm_remove: false,
        }
    }

    fn configuration(&mut self, app: &Desktop, remove: bool) -> Result<String> {
        let mut doc: DocumentMut = app.config_source.parse()?;
        let list = tables(doc.as_table_mut(), "partners")?;
        if remove {
            ensure!(list.len() > 1, "The workspace needs at least one partner.");
            list.remove(self.index.context("select a partner")?);
            return Ok(doc.to_string());
        }
        let p = &mut self.partner;
        ensure!(!p.name.trim().is_empty(), "Enter a partner name.");
        ensure!(
            !p.identity[0].trim().is_empty(),
            "Enter the partner’s EDIFACT identifier."
        );
        ensure!(!p.receive.is_empty(), "Add at least one intake channel.");
        for channel in &p.receive {
            if let Receive::Folder { directory, .. } = channel {
                ensure!(
                    !directory.as_os_str().is_empty(),
                    "Enter the intake folder."
                );
            }
        }
        match &mut p.send {
            SendChannel::Folder { directory } => {
                ensure!(!directory.as_os_str().is_empty(), "Enter the reply folder.")
            }
            SendChannel::Http { headers, .. } => {
                headers.clear();
                for (name, value) in &self.headers {
                    ensure!(
                        !name.trim().is_empty()
                            && headers.insert(name.clone(), value.clone()).is_none(),
                        "Each HTTP header needs a unique name."
                    );
                }
            }
        }
        let mut data = toml_edit::ser::to_document(p)?.into_table();
        data.remove("rules");
        if let Some(i) = self.index {
            let old = toml_edit::ser::to_document(&app.workspace.partners[i])?;
            let table = list.get_mut(i).context("missing partner")?;
            for (key, item) in data {
                if old
                    .get(&key)
                    .is_none_or(|v| v.to_string() != item.to_string())
                {
                    table.insert(&key, item);
                }
            }
            if p.separators.is_none() {
                table.remove("separators");
            }
        } else {
            list.push(data);
        }
        Ok(doc.to_string())
    }
    fn fields(&mut self, ui: &mut Ui, workspace: &Workspace) {
        let p = &mut self.partner;
        field(ui, "Partner name", &mut p.name);
        if self
            .index
            .is_some_and(|i| workspace.partners[i].name != p.name)
        {
            ui.label(muted(
                "Existing traffic and script state are stored under the previous name.",
            ));
        }
        field(ui, "EDIFACT identifier", &mut p.identity[0]);
        ui.horizontal_wrapped(|ui| {
            ui.label("Acknowledge");
            for (v, s) in [
                (Policy::Always, "Always"),
                (Policy::Requested, "When requested"),
                (Policy::Never, "Never"),
            ] {
                ui.selectable_value(&mut p.acknowledge, v, s);
            }
        });
        ui.collapsing("EDIFACT options", |ui| {
            for (i, component) in p.identity.iter_mut().enumerate().skip(1) {
                let label = match i {
                    1 => "Identifier qualifier".into(),
                    _ => format!("Identity component {}", i + 1),
                };
                field(ui, &label, component);
            }
            ui.horizontal(|ui| {
                if ui.small_button("Add identity component").clicked() {
                    p.identity.push(String::new());
                }
                if p.identity.len() > 1 && ui.small_button("Remove last component").clicked() {
                    p.identity.pop();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Character set");
                ComboBox::from_id_salt("charset")
                    .selected_text(&p.charset)
                    .show_ui(ui, |ui| {
                        for s in ["UNOA", "UNOB", "UNOC"] {
                            ui.selectable_value(&mut p.charset, s.into(), s);
                        }
                    });
            });
            let mut custom = p.separators.is_some();
            if ui
                .checkbox(&mut custom, "Custom EDIFACT separators")
                .changed()
            {
                p.separators = custom.then(|| ":+.? '".into());
            }
            if let Some(s) = &mut p.separators {
                field(ui, "Six service characters", s);
            }
        });
        ui.separator();
        ui.heading("Intake channels");
        let mut remove = None;
        for (i, channel) in p.receive.iter_mut().enumerate() {
            ui.push_id(("intake", i), |ui| {
                match channel {
                    Receive::Http { path } => field(ui, "HTTP path", path),
                    Receive::Folder { directory, suffix } => {
                        path_field(ui, "Intake folder", directory, &workspace.root);
                        field(ui, "Filename suffix", suffix);
                    }
                }
                if ui.small_button("Remove channel").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            p.receive.remove(i);
        }
        ui.horizontal(|ui| {
            if ui.button("Add folder").clicked() {
                p.receive.push(Receive::Folder {
                    directory: PathBuf::new(),
                    suffix: ".edi".into(),
                });
            }
            if ui.button("Add HTTP path").clicked() {
                p.receive.push(Receive::Http { path: "/".into() });
            }
        });
        ui.separator();
        ui.heading("Reply channel");
        ui.horizontal(|ui| {
            let folder = matches!(p.send, SendChannel::Folder { .. });
            if ui.selectable_label(folder, "Folder").clicked() && !folder {
                p.send = SendChannel::Folder {
                    directory: PathBuf::new(),
                };
            }
            if ui.selectable_label(!folder, "HTTP").clicked() && folder {
                p.send = SendChannel::Http {
                    url: String::new(),
                    headers: Default::default(),
                };
            }
        });
        match &mut p.send {
            SendChannel::Folder { directory } => {
                path_field(ui, "Reply folder", directory, &workspace.root)
            }
            SendChannel::Http { url, .. } => {
                field(ui, "Reply URL", url);
                let mut remove = None;
                for (i, (name, value)) in self.headers.iter_mut().enumerate() {
                    ui.push_id(("header", i), |ui| {
                        field(ui, "Header name", name);
                        field(ui, "Header value", value);
                        if ui.small_button("Remove header").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    self.headers.remove(i);
                }
                if ui.small_button("Add header").clicked() {
                    self.headers.push(Default::default());
                }
            }
        }
    }
}

pub(super) fn field(ui: &mut Ui, label: &str, text: &mut String) {
    let caption = ui.label(label);
    ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY))
        .labelled_by(caption.id);
}
fn path_field(ui: &mut Ui, label: &str, path: &mut PathBuf, root: &Path) {
    let mut text = path.display().to_string();
    field(ui, label, &mut text);
    *path = text.into();
    if path.is_relative() && !path.as_os_str().is_empty() {
        ui.label(muted(root.join(&*path).display().to_string()));
    }
}

impl Desktop {
    pub(super) fn open_partner(&mut self, index: Option<usize>) {
        if self.edit != self.saved {
            self.status = "Save or reload your current edits before opening a partner.".into();
        } else {
            self.partner_form = Some(PartnerForm::new(&self.workspace, index));
        }
    }

    pub(super) fn partner_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut form) = self.partner_form.take() else {
            return;
        };
        let mut close = false;
        Modal::new("partner editor".into()).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 48.).min(490.));
            ui.heading(if form.index.is_some() {
                "Edit partner"
            } else {
                "Add partner"
            });
            ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 220.).max(100.))
                .show(ui, |ui| form.fields(ui, &self.workspace));
            ui.separator();
            if !form.error.is_empty() {
                ui.colored_label(AMBER, &form.error);
            }
            if form.confirm_remove {
                ui.label("Remove this partner and its rules from the workspace?");
            }
            ui.horizontal(|ui| {
                let remove = form.confirm_remove && ui.button("Confirm removal").clicked();
                let save = !form.confirm_remove && button(ui, "Save partner", true).clicked();
                if save || remove {
                    match form
                        .configuration(self, remove)
                        .and_then(|doc| self.save_configuration(doc, None))
                    {
                        Ok(()) => {
                            if remove {
                                self.navigate(View::All);
                                self.status = "Partner removed".into();
                            } else {
                                let i = form.index.unwrap_or(self.workspace.partners.len() - 1);
                                self.partner = i;
                                self.navigate(View::Partner(i));
                                self.status = "Partner saved".into();
                            }
                            close = true;
                        }
                        Err(e) => form.error = format!("{e:#}"),
                    }
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if !form.confirm_remove
                    && form.index.is_some()
                    && self.workspace.partners.len() > 1
                    && ui.button("Remove partner").clicked()
                {
                    form.confirm_remove = true;
                }
            });
        });
        if !close {
            self.partner_form = Some(form);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests::click;

    #[test]
    #[ignore = "uses a local listener"]
    fn manage_partner_from_sidebar() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let config = dir.path().join("vendredi.toml");
        let original = fs::read_to_string(&config)?.replace(":8180", ":0");
        fs::write(&config, format!("# keep this comment\n{original}"))?;
        let ctx = egui::Context::default();
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        click(&mut app, &ctx, "+ Add partner")?;
        let form = app.partner_form.as_mut().context("missing partner form")?;
        form.partner.name = "North Freight".into();
        form.partner.identity[0] = "NORTH".into();
        click(&mut app, &ctx, "Save partner")?;
        ensure!(
            app.partner_form.is_none(),
            "partner save failed: {:?}",
            app.partner_form.as_ref().map(|f| &f.error)
        );
        let w = Workspace::load(dir.path())?;
        assert_eq!(w.partners[1].name, "North Freight");
        assert!(dir.path().join("inbox-2").is_dir());
        click(&mut app, &ctx, "Supplier")?;
        click(&mut app, &ctx, "Edit partner")?;
        let form = app.partner_form.as_mut().unwrap();
        form.partner.name = "Supplier East".into();
        form.partner.receive.push(Receive::Folder {
            directory: "inbox-2".into(),
            suffix: ".edi".into(),
        });
        click(&mut app, &ctx, "Save partner")?;
        assert!(
            app.partner_form
                .as_ref()
                .unwrap()
                .error
                .contains("ambiguous receive folder")
        );
        let form = app.partner_form.as_mut().unwrap();
        form.partner.receive.pop();
        click(&mut app, &ctx, "Save partner")?;
        let w = Workspace::load(dir.path())?;
        assert_eq!(w.partners[0].name, "Supplier East");
        assert_eq!(w.partners[0].rules[0].name, "Confirm orders");
        assert!(fs::read_to_string(&config)?.starts_with("# keep this comment"));
        Ok(())
    }
}
