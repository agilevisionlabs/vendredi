use super::*;
use egui::{ComboBox, RichText, ScrollArea, Ui};
use vendredi_core::case::{self, Case};

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
        ui.horizontal(|ui| {
            ui.heading("Cases");
            ui.add_space(12.);
            form::label(ui, "Partner");
            ComboBox::from_id_salt("case partner")
                .selected_text(&self.workspace.partners[self.partner].name)
                .show_ui(ui, |ui| {
                    for (i, p) in self.workspace.partners.iter().enumerate() {
                        ui.selectable_value(&mut self.partner, i, &p.name);
                    }
                });
        });
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
                form::card(ui, |ui| match &selected {
                    Some(c) => self.case_detail(ui, c, active == c.id),
                    None => self.default_case_detail(ui, active == case::DEFAULT),
                });
            });
        });
    }

    fn case_row(&mut self, ui: &mut Ui, id: &str, name: &str, tags: &[String], active: bool) {
        let selected = self.case == id;
        let frame = egui::Frame::new()
            .fill(if selected { SURFACE } else { BG })
            .stroke(egui::Stroke::new(1_f32, if selected { BLUE } else { LINE }))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(10, 8));
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
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
            ui.label(muted(format!("Active for {partner}")));
        } else if ui
            .add_enabled(compatible, egui::Button::new("Make active"))
            .on_disabled_hover_text("This case is written for another EDI standard.")
            .clicked()
        {
            self.command(Command::UseCase(partner, id.into()));
        }
    }

    fn default_case_detail(&mut self, ui: &mut Ui, active: bool) {
        let p = &self.workspace.partners[self.partner];
        ui.label(RichText::new("Partner's own rules").font(bold(17.)));
        ui.label(muted(format!(
            "The {} rule{} configured on {} under Rules.",
            p.rules.len(),
            if p.rules.len() == 1 { "" } else { "s" },
            p.name
        )));
        ui.add_space(8.);
        ui.horizontal(|ui| self.activate_button(ui, case::DEFAULT, active, true));
    }

    fn case_detail(&mut self, ui: &mut Ui, c: &Case, active: bool) {
        let partner = &self.workspace.partners[self.partner];
        let compatible = c.standard == partner.standard;
        let name = partner.name.clone();
        ui.horizontal_wrapped(|ui| {
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
        ui.horizontal(|ui| self.activate_button(ui, &c.id, active, compatible));
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
}
