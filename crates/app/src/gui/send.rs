use super::*;
use egui::{ComboBox, ScrollArea};
use vendredi_core::script;

impl Desktop {
    fn send_choices(&self) -> BTreeSet<(String, String)> {
        self.workspace.partners[self.send_partner]
            .scenario()
            .iter()
            .flat_map(|r| &r.actions)
            .filter_map(|a| match a {
                Action::Document { document } if !self.send_script => {
                    Some((document.display().to_string(), String::new()))
                }
                Action::Script { script, function } if self.send_script => {
                    Some((script.display().to_string(), function.clone()))
                }
                _ => None,
            })
            .collect()
    }
    fn initial_send_file(&mut self) {
        (self.send_file, self.function) =
            self.send_choices().into_iter().next().unwrap_or_default();
        self.send_error.clear();
    }
    pub(super) fn open_send(&mut self) {
        self.send_partner = self.partner;
        self.send_dialog = true;
        self.send_script = false;
        self.initial_send_file();
    }
    fn send_selected(&mut self) -> Result<()> {
        let path = PathBuf::from(self.send_file.trim());
        let absolute = self.root.join(&path);
        let function = self.send_script.then(|| self.function.trim().to_owned());
        if let Some(f) = &function {
            let source = script::read_source(&absolute)
                .with_context(|| format!("Opening {}", absolute.display()))?;
            script::validate(&source, f)?;
        } else {
            let bytes =
                fs::read(&absolute).with_context(|| format!("Opening {}", absolute.display()))?;
            vendredi_core::interchange::document(
                self.workspace.partners[self.send_partner].standard,
                &bytes,
            )?;
        }
        self.running
            .tx
            .send(Command::Send(
                self.workspace.partners[self.send_partner].name.clone(),
                path,
                function,
            ))
            .map_err(|_| anyhow!("Engine stopped"))?;
        self.status = "Send requested".into();
        Ok(())
    }
    /// The URL a partner's HTTP intake listens on, if it has one.
    pub(super) fn receive_url(&self, partner: usize) -> Option<String> {
        let path = self.workspace.partners[partner]
            .receive
            .iter()
            .find_map(|r| match r {
                Receive::Http { path } => Some(path),
                Receive::Folder { .. } => None,
            })?;
        let mut addr = self.address;
        if addr.ip().is_unspecified() {
            addr.set_ip(if addr.is_ipv4() {
                std::net::Ipv4Addr::LOCALHOST.into()
            } else {
                std::net::Ipv6Addr::LOCALHOST.into()
            });
        }
        Some(format!("http://{addr}{path}"))
    }

    pub(super) fn sample(&self, partner: usize) -> Result<()> {
        let bytes = fs::read(self.root.join("order.edi"))?;
        let channels = &self.workspace.partners[partner].receive;
        if let Some(path) = channels.iter().find_map(|r| {
            if let Receive::Http { path } = r {
                Some(path)
            } else {
                None
            }
        }) {
            let mut addr = self.address;
            if addr.ip().is_unspecified() {
                addr.set_ip(if addr.is_ipv4() {
                    std::net::Ipv4Addr::LOCALHOST.into()
                } else {
                    std::net::Ipv6Addr::LOCALHOST.into()
                });
            }
            let d = Delivery {
                id: 0,
                channel: SendChannel::Http {
                    url: format!("http://{addr}{path}"),
                    headers: Default::default(),
                },
                bytes,
            };
            let root = self.root.clone();
            let out = self.output.clone();
            thread::spawn(move || {
                if let Err(e) = vendredi_transports::deliver(&root, &d) {
                    out(json!({"event":"error","error":format!("{e:#}")}));
                }
            });
        } else if let Some(Receive::Folder { directory, suffix }) = channels.first() {
            let dir = self.root.join(directory);
            let staging_suffix = if suffix.ends_with('x') {
                ".part-y"
            } else {
                ".part-x"
            };
            let mut f = tempfile::Builder::new()
                .prefix("sample-")
                .suffix(staging_suffix)
                .tempfile_in(&dir)?;
            f.write_all(&bytes)?;
            f.as_file().sync_all()?;
            let dest = dir.join(format!(
                "{}{}",
                f.path().file_stem().unwrap().to_string_lossy(),
                suffix
            ));
            f.persist_noclobber(dest)?;
        } else {
            anyhow::bail!("Add an intake channel to this partner.");
        }
        Ok(())
    }
    pub(super) fn send_window(&mut self, ctx: &egui::Context) {
        if !self.send_dialog {
            return;
        }
        let escaped = form::modal(
            ctx,
            "send document",
            "Send document or script",
            480.,
            |ui| {
                ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 220.).max(100.))
                    .show(ui, |ui| {
                        form::label(ui, "Partner");
                        let previous = (self.send_partner, self.send_script);
                        ComboBox::from_id_salt("send partner")
                            .selected_text(&self.workspace.partners[self.send_partner].name)
                            .show_ui(ui, |ui| {
                                for (i, p) in self.workspace.partners.iter().enumerate() {
                                    ui.selectable_value(&mut self.send_partner, i, &p.name);
                                }
                            });
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut self.send_script, false, "Document");
                            ui.selectable_value(&mut self.send_script, true, "JavaScript");
                        });
                        if previous != (self.send_partner, self.send_script) {
                            self.initial_send_file();
                        }
                        let choices = self.send_choices();
                        if !choices.is_empty() {
                            ComboBox::from_id_salt("send file choices")
                                .selected_text("Choose configured file…")
                                .show_ui(ui, |ui| {
                                    for (file, function) in choices {
                                        let name = if function.is_empty() {
                                            file.clone()
                                        } else {
                                            format!("{file} · {function}()")
                                        };
                                        if ui
                                            .selectable_label(
                                                file == self.send_file && function == self.function,
                                                name,
                                            )
                                            .clicked()
                                        {
                                            self.send_file = file;
                                            self.function = function;
                                            self.send_error.clear();
                                        }
                                    }
                                });
                        }
                        let root = self.root.clone();
                        let script = self.send_script;
                        form::path_input(
                            ui,
                            "File",
                            &mut self.send_file,
                            "A workspace path or any file",
                            || {
                                let filters: &[(&str, &[&str])] = if script {
                                    &[("JavaScript", &["js"])]
                                } else {
                                    &[("EDI documents", &["edi", "x12", "txt"])]
                                };
                                let chosen = form::pick_file(&root, filters)?;
                                // Workspace files keep a short relative path.
                                Some(match chosen.strip_prefix(&root) {
                                    Ok(inside) => inside.display().to_string(),
                                    Err(_) => chosen.display().to_string(),
                                })
                            },
                        );
                        if self.send_script {
                            form::field(ui, "Entry function", &mut self.function);
                        }
                    });
                if !self.listening {
                    ui.label(muted("Start the listener to send traffic."));
                }
                if !self.send_error.is_empty() {
                    ui.colored_label(col::amber(), &self.send_error);
                }
                form::footer(ui, |ui| {
                    let valid = self.listening
                        && !self.send_file.trim().is_empty()
                        && (!self.send_script || !self.function.trim().is_empty());
                    if ui
                        .add_enabled_ui(valid, |ui| button(ui, "Send", true))
                        .inner
                        .clicked()
                    {
                        match self.send_selected() {
                            Ok(()) => self.send_dialog = false,
                            Err(e) => self.send_error = format!("{e:#}"),
                        }
                    }
                    if button(ui, "Cancel", false).clicked() {
                        self.send_dialog = false;
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        if self.root.join("order.edi").is_file()
                            && ui
                                .add_enabled(self.listening, egui::Link::new("Send sample order"))
                                .on_hover_text("Submit order.edi to the selected partner’s intake")
                                .clicked()
                        {
                            match self.sample(self.send_partner) {
                                Ok(()) => self.send_dialog = false,
                                Err(e) => self.send_error = format!("{e:#}"),
                            }
                        }
                    });
                });
            },
        );
        if escaped {
            self.send_dialog = false;
        }
    }
}
