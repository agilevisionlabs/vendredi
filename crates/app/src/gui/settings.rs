use super::*;
use egui::{ScrollArea, Ui};
use toml_edit::DocumentMut;

pub(super) struct SettingsForm {
    listen: String,
    identity: Vec<String>,
    days: u32,
    error: String,
    confirm: bool,
}
impl Desktop {
    pub(super) fn settings(&mut self, ui: &mut Ui) {
        ui.heading("Workspace settings");
        ui.add_space(GAP_S);
        ScrollArea::vertical().show(ui, |ui| {
            ui.set_max_width(580.);
            form::card(ui, |ui| {
                egui::Grid::new("workspace summary")
                    .num_columns(2)
                    .spacing([20., 10.])
                    .show(ui, |ui| {
                        for (label, value) in [
                            ("Workspace", self.workspace.root.display().to_string()),
                            ("HTTP listener", self.workspace.listen.to_string()),
                            ("EDIFACT identity", self.workspace.identity.join(" · ")),
                            (
                                "Traffic retention",
                                format!("{} days", self.workspace.retention_days),
                            ),
                        ] {
                            form::label(ui, label);
                            ui.label(value);
                            ui.end_row();
                        }
                    });
            });
            ui.horizontal_wrapped(|ui| {
                if button(ui, "Edit settings", true).clicked() {
                    self.settings_form = Some(SettingsForm {
                        listen: self.workspace.listen.to_string(),
                        identity: self.workspace.identity.clone(),
                        days: self.workspace.retention_days,
                        error: String::new(),
                        confirm: false,
                    });
                }
            });
            ui.add_space(GAP_L);
            // The raw file stays available, out of the way of everyday work.
            // Disclosure headers share the small control height.
            ui.spacing_mut().interact_size.y = SMALL_BUTTON;
            egui::CollapsingHeader::new("Advanced")
                .id_salt("settings advanced")
                .show(ui, |ui| {
                    ui.label(prose(
                        "vendredi.toml holds the listener, identity and partners. The settings above and the partner editor change it for you.",
                    ));
                    ui.add_space(GAP_S);
                    if button(ui, "Edit configuration file", false).clicked() {
                        self.navigate(View::Config);
                    }
                });
            ui.add_space(GAP_L);
            form::label(ui, "Help");
            ui.horizontal(|ui| {
                for (glyph, label, action) in [
                    (icon::BOOK_OPEN, "Guide", shortcuts::Do::Guide),
                    (icon::LIGHTNING, "What's new", shortcuts::Do::WhatsNew),
                    (icon::WARNING_CIRCLE, "Report a problem", shortcuts::Do::Report),
                ] {
                    if small_button(ui, &with_icon(glyph, label)).clicked() {
                        self.perform(action);
                    }
                }
            });
            ui.label(muted(format!("Vendredi {}", env!("CARGO_PKG_VERSION"))));
        });
    }

    pub(super) fn settings_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.settings_form.take() else {
            return;
        };
        let mut close = false;
        let escaped = form::modal(
            ctx,
            "workspace settings",
            "Edit workspace settings",
            500.,
            |ui| {
                if f.confirm {
                    ui.label(format!("Saving will permanently clear exchanges older than {} days across all partners.", f.days));
                } else {
                    ScrollArea::vertical()
                        .max_height((ctx.content_rect().height() - 220.).max(100.))
                        .show(ui, |ui| {
                            form::input(ui, "HTTP listener", &mut f.listen, "127.0.0.1:8180");
                            for (i, s) in f.identity.iter_mut().enumerate() {
                                let label = match i {
                                    0 => "EDIFACT identifier".into(),
                                    1 => "Identifier qualifier".into(),
                                    _ => format!("Identity component {}", i + 1),
                                };
                                form::field(ui, &label, s);
                            }
                            ui.horizontal(|ui| {
                                if ui.small_button("Add identity component").clicked() {
                                    f.identity.push(String::new());
                                }
                                if f.identity.len() > 1
                                    && ui.small_button("Remove last component").clicked()
                                {
                                    f.identity.pop();
                                }
                            });
                            form::label(ui, "Traffic retention");
                            ui.add(egui::DragValue::new(&mut f.days).suffix(" days"));
                        });
                }
                if !f.error.is_empty() {
                    ui.colored_label(col::amber(), &f.error);
                }
                form::footer(ui, |ui| {
                    if button(
                        ui,
                        if f.confirm {
                            "Apply shorter retention"
                        } else {
                            "Save settings"
                        },
                        true,
                    )
                    .clicked()
                    {
                        let save = (|| -> Result<bool> {
                            let addr: std::net::SocketAddr = f.listen.trim().parse().context(
                                "Enter an IP address and port, for example 127.0.0.1:8180.",
                            )?;
                            ensure!(
                                !f.identity[0].trim().is_empty(),
                                "Enter the workspace EDIFACT identifier."
                            );
                            if f.days < self.workspace.retention_days && !f.confirm {
                                f.confirm = true;
                                return Ok(false);
                            }
                            let mut doc: DocumentMut = self.config_source.parse()?;
                            if addr != self.workspace.listen {
                                doc["listen"] = toml_edit::value(addr.to_string());
                            }
                            if f.identity != self.workspace.identity {
                                doc["identity"] = toml_edit::value(
                                    f.identity
                                        .iter()
                                        .map(String::as_str)
                                        .collect::<toml_edit::Array>(),
                                );
                            }
                            if f.days != self.workspace.retention_days {
                                doc["retention_days"] = toml_edit::value(i64::from(f.days));
                            }
                            self.save_configuration(doc.to_string(), None)?;
                            self.status = "Workspace settings saved".into();
                            Ok(true)
                        })();
                        match save {
                            Ok(done) => close = done,
                            Err(e) => {
                                f.error = format!("{e:#}");
                                f.confirm = false;
                            }
                        }
                    }
                    if button(ui, "Cancel", false).clicked() {
                        close = true;
                    }
                });
            },
        );
        if !close && !escaped {
            self.settings_form = Some(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests::click;
    #[test]
    #[ignore = "uses a local workspace listener"]
    fn settings_and_document_round_trip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let path = dir.path().join("vendredi.toml");
        fs::write(&path, fs::read_to_string(&path)?.replace(":8180", ":0"))?;
        // The Documents view lists the documents the active cases send.
        let case = dir.path().join("cases/confirm-orders/case.json");
        let mut json: serde_json::Value = serde_json::from_slice(&fs::read(&case)?)?;
        json["rules"][0]["actions"] = serde_json::json!([{"document": "confirmation.edi"}]);
        fs::write(&case, serde_json::to_vec(&json)?)?;
        let doc = dir.path().join("cases/confirm-orders/confirmation.edi");
        fs::copy(dir.path().join("confirmation.edi"), &doc)?;
        let mut bytes = fs::read(&doc)?;
        // Put the accented character in the message body, leaving the EDIFACT header intact.
        let body = bytes.windows(5).position(|s| s == b"REPLY").unwrap();
        bytes[body] = 0xe9;
        fs::write(&doc, &bytes)?;
        let ctx = egui::Context::default();
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        click(&mut app, &ctx, "Settings")?;
        click(&mut app, &ctx, "Edit settings")?;
        app.settings_form.as_mut().unwrap().listen = "bad".into();
        click(&mut app, &ctx, "Save settings")?;
        assert!(app.settings_form.as_ref().unwrap().error.contains("port"));
        let f = app.settings_form.as_mut().unwrap();
        f.listen = "127.0.0.1:0".into();
        f.days = 2;
        click(&mut app, &ctx, "Save settings")?;
        assert!(app.settings_form.as_ref().unwrap().confirm);
        assert_eq!(Workspace::load(dir.path())?.retention_days, 30);
        click(&mut app, &ctx, "Apply shorter retention")?;
        assert_eq!(Workspace::load(dir.path())?.retention_days, 2);
        click(&mut app, &ctx, "Documents")?;
        app.open_file("cases/confirm-orders/confirmation.edi".into());
        assert!(app.edit.contains('é'));
        app.edit = app.edit.replace("éEPLY", "café");
        click(&mut app, &ctx, "Settings")?;
        assert!(app.view == View::Documents);
        click(&mut app, &ctx, "Save and reload")?;
        assert!(fs::read(&doc)?.windows(4).any(|s| s == [99, 97, 102, 233]));
        app.edit = "invalid".into();
        let saved = fs::read(&doc)?;
        click(&mut app, &ctx, "Save and reload")?;
        assert!(app.file_error.is_some());
        assert_eq!(fs::read(&doc)?, saved);
        click(&mut app, &ctx, "Discard edits")?;
        assert!(app.file_error.is_none() && app.edit.contains("café"));
        fs::rename(&doc, dir.path().join("moved.edi"))?;
        click(&mut app, &ctx, "Reload file")?;
        assert!(app.file_unavailable && app.edit.is_empty());
        fs::write(&doc, &saved)?;
        click(&mut app, &ctx, "Reload file")?;
        assert!(!app.file_unavailable);
        assert!(app.edit.contains("café"));
        Ok(())
    }
}
