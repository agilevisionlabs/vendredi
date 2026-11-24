use super::*;
use egui::{Align, ComboBox, Layout, RichText, ScrollArea, Ui};
use form::field;
use toml_edit::DocumentMut;
use vendredi_core::config::{Partner, Policy};
use workspace_edit::tables;

pub(super) struct PartnerForm {
    index: Option<usize>,
    partner: Partner,
    headers: Vec<(String, String)>,
    error: String,
    alternate_reply: Option<SendChannel>,
    focus_name: bool,
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
            alternate_reply: None,
            focus_name: index.is_none(),
        }
    }

    fn configuration(&mut self, app: &Desktop) -> Result<String> {
        let mut doc: DocumentMut = app.config_source.parse()?;
        let list = tables(doc.as_table_mut(), "partners")?;
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
        ui.columns(2, |cols| {
            let name = form::input(
                &mut cols[0],
                "Partner name",
                &mut p.name,
                "e.g. Acme Retail",
            );
            if self.focus_name {
                name.request_focus();
                self.focus_name = false;
            }
            form::input(
                &mut cols[1],
                "EDIFACT identifier",
                &mut p.identity[0],
                "e.g. ACME",
            );
        });
        if self
            .index
            .is_some_and(|i| workspace.partners[i].name != p.name)
        {
            ui.label(muted(
                "Existing traffic and script state are stored under the previous name.",
            ));
        }
        let policy = match p.acknowledge {
            Policy::Always => "Always acknowledge",
            Policy::Requested => "Acknowledge on request",
            Policy::Never => "Never acknowledge",
        };
        let mut header = egui::text::LayoutJob::default();
        header.append(
            "EDIFACT settings",
            0.,
            egui::TextFormat::simple(font(13.), TEXT),
        );
        header.append(
            &format!("{} · {policy}", p.charset),
            12.,
            egui::TextFormat::simple(font(12.), MUTED),
        );
        let options = egui::CollapsingHeader::new(header).id_salt("partner EDIFACT settings");
        options.show(ui, |ui| {
            form::label(ui, "Send acknowledgements");
            ui.horizontal_wrapped(|ui| {
                for (v, s) in [
                    (Policy::Always, "Always"),
                    (Policy::Requested, "When requested"),
                    (Policy::Never, "Never"),
                ] {
                    ui.radio_value(&mut p.acknowledge, v, s);
                }
            });
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
        ui.add_space(2.);
        let mut add = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Intake channels").font(bold(15.)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button(RichText::new("+ Add channel").color(BLUE), |ui| {
                    if ui.button("Folder").clicked() {
                        add = Some(Receive::Folder {
                            directory: PathBuf::new(),
                            suffix: ".edi".into(),
                        });
                        ui.close();
                    }
                    if ui.button("HTTP path").clicked() {
                        add = Some(Receive::Http { path: "/".into() });
                        ui.close();
                    }
                });
            });
        });
        if let Some(channel) = add {
            p.receive.push(channel);
        }
        let multiple = p.receive.len() > 1;
        let mut remove = None;
        for (i, channel) in p.receive.iter_mut().enumerate() {
            ui.push_id(("intake", i), |ui| {
                form::card(ui, |ui| {
                    match channel {
                        Receive::Http { path } => field(ui, "HTTP path", path),
                        Receive::Folder { directory, suffix } => {
                            ui.horizontal_top(|ui| {
                                let suffix_width = 104.;
                                let folder_width = ui.available_width()
                                    - suffix_width
                                    - ui.spacing().item_spacing.x;
                                ui.vertical(|ui| {
                                    ui.set_width(folder_width);
                                    path_field(ui, "Intake folder", directory);
                                });
                                ui.vertical(|ui| {
                                    ui.set_width(suffix_width);
                                    field(ui, "Filename suffix", suffix);
                                });
                            });
                            resolved_path(ui, directory, &workspace.root);
                        }
                    }
                    if multiple && ui.link("Remove channel").clicked() {
                        remove = Some(i);
                    }
                })
            });
        }
        if let Some(i) = remove {
            p.receive.remove(i);
        }
        ui.add_space(2.);
        let folder = matches!(p.send, SendChannel::Folder { .. });
        let mut use_folder = folder;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Reply channel").font(bold(15.)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ComboBox::from_id_salt("reply transport")
                    .selected_text(if folder { "Folder" } else { "HTTP" })
                    .width(104.)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut use_folder, true, "Folder");
                        ui.selectable_value(&mut use_folder, false, "HTTP");
                    });
            });
        });
        if use_folder != folder {
            let next = self.alternate_reply.take().unwrap_or_else(|| {
                if use_folder {
                    SendChannel::Folder {
                        directory: PathBuf::new(),
                    }
                } else {
                    SendChannel::Http {
                        url: String::new(),
                        headers: Default::default(),
                    }
                }
            });
            self.alternate_reply = Some(std::mem::replace(&mut p.send, next));
        }
        form::card(ui, |ui| match &mut p.send {
            SendChannel::Folder { directory } => {
                path_field(ui, "Reply folder", directory);
                resolved_path(ui, directory, &workspace.root);
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
        });
    }
}

fn path_field(ui: &mut Ui, label: &str, path: &mut PathBuf) {
    let mut text = path.display().to_string();
    if form::input(ui, label, &mut text, "Folder path").changed() {
        *path = text.into();
    }
}
fn resolved_path(ui: &mut Ui, path: &Path, root: &Path) {
    if path.is_relative() && !path.as_os_str().is_empty() {
        let full = root.join(path).display().to_string();
        ui.add(egui::Label::new(muted(&full)).truncate())
            .on_hover_text(full);
    }
}

pub(super) struct PartnerRemoval {
    index: usize,
    error: String,
}
impl PartnerRemoval {
    pub(super) fn new(index: usize) -> Self {
        Self {
            index,
            error: String::new(),
        }
    }
}

impl Desktop {
    fn remove_partner(&mut self, index: usize) -> Result<()> {
        let mut doc: DocumentMut = self.config_source.parse()?;
        let list = tables(doc.as_table_mut(), "partners")?;
        ensure!(list.len() > 1, "The workspace needs at least one partner.");
        list.remove(index);
        self.save_configuration(doc.to_string(), None)?;
        self.navigate(View::All);
        self.status = "Partner removed".into();
        Ok(())
    }

    pub(super) fn partner_removal_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut removal) = self.partner_removal.take() else {
            return;
        };
        let mut close = false;
        let escaped = form::modal(ctx, "remove partner", "Remove partner?", 400., |ui| {
            let name = &self.workspace.partners[removal.index].name;
            ui.label(format!("Remove {name} and its rules from this workspace?"));
            if !removal.error.is_empty() {
                ui.colored_label(AMBER, &removal.error);
            }
            form::footer(ui, |ui| {
                if ui.add(form::removal_button("Confirm removal")).clicked() {
                    match self.remove_partner(removal.index) {
                        Ok(()) => close = true,
                        Err(e) => removal.error = format!("{e:#}"),
                    }
                }
                close |= button(ui, "Cancel", false).clicked();
            });
        });
        if !close && !escaped {
            self.partner_removal = Some(removal);
        }
    }

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
        let title = if form.index.is_some() {
            "Edit partner"
        } else {
            "Add partner"
        };
        let escaped = form::modal(ctx, "partner editor", title, 560., |ui| {
            ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 220.).max(100.))
                .show(ui, |ui| form.fields(ui, &self.workspace));
            if !form.error.is_empty() {
                ui.colored_label(AMBER, &form.error);
            }
            form::footer(ui, |ui| {
                if button(ui, "Save partner", true).clicked() {
                    match form
                        .configuration(self)
                        .and_then(|doc| self.save_configuration(doc, None))
                    {
                        Ok(()) => {
                            let i = form.index.unwrap_or(self.workspace.partners.len() - 1);
                            self.partner = i;
                            self.navigate(View::Partner(i));
                            self.status = "Partner saved".into();
                            close = true;
                        }
                        Err(e) => form.error = format!("{e:#}"),
                    }
                }
                if button(ui, "Cancel", false).clicked() {
                    close = true;
                }
            });
        });
        if !close && !escaped {
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
        click(&mut app, &ctx, "Folder")?;
        click(&mut app, &ctx, "HTTP")?;
        assert!(matches!(
            app.partner_form.as_ref().unwrap().partner.send,
            SendChannel::Http { .. }
        ));
        click(&mut app, &ctx, "HTTP")?;
        click(&mut app, &ctx, "Folder")?;
        assert!(
            matches!(&app.partner_form.as_ref().unwrap().partner.send,SendChannel::Folder {directory} if directory == Path::new("replies-2"))
        );
        click(&mut app, &ctx, "+ Add channel")?;
        click(&mut app, &ctx, "HTTP path")?;
        assert_eq!(app.partner_form.as_ref().unwrap().partner.receive.len(), 2);
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
        click(&mut app, &ctx, "Remove partner")?;
        assert!(app.partner_form.is_none());
        assert!(app.partner_removal.is_some());
        click(&mut app, &ctx, "Cancel")?;
        assert_eq!(Workspace::load(dir.path())?.partners.len(), 2);
        click(&mut app, &ctx, "Remove partner")?;
        click(&mut app, &ctx, "Confirm removal")?;
        let w = Workspace::load(dir.path())?;
        assert_eq!(w.partners.len(), 1);
        assert_eq!(w.partners[0].name, "North Freight");
        click(&mut app, &ctx, "North Freight")?;
        click(&mut app, &ctx, "Remove partner")?;
        assert!(app.partner_removal.is_none());
        Ok(())
    }
}
