use super::*;
use eframe::egui::*;
impl Desktop {
    pub(super) fn show(&mut self, ctx: &egui::Context) {
        self.drain();
        TopBottomPanel::top("header")
            .exact_height(52.)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("Vendredi").font(bold(15.)));
                    let path = self.root.display().to_string();
                    let short = std::env::var_os("HOME")
                        .and_then(|h| self.root.strip_prefix(h).ok())
                        .map(|p| format!("~/{}", p.display()))
                        .unwrap_or_else(|| path.clone());
                    if ui.available_width() > 800. {
                        badge(ui, &short, SECONDARY_TEXT, SURFACE, true);
                    }
                    ui.colored_label(if self.listening { GREEN } else { MUTED }, "●");
                    ui.label(
                        RichText::new(if self.listening { "Running" } else { "Stopped" })
                            .font(bold(13.)),
                    );
                    let n = self
                        .workspace
                        .partners
                        .iter()
                        .flat_map(|p| &p.receive)
                        .filter(|r| matches!(r, Receive::Folder { .. }))
                        .count();
                    if ui.available_width() > 440. && self.listening {
                        ui.label(muted(format!(
                            "HTTP on :{} · {n} folder{} watched",
                            self.workspace.listen.port(),
                            if n == 1 { "" } else { "s" }
                        )));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if button(ui, "→  Send document…", true).clicked() {
                            self.send_dialog = true;
                        }
                        if button(
                            ui,
                            if self.listening {
                                "▪  Stop"
                            } else {
                                "▷  Start"
                            },
                            false,
                        )
                        .clicked()
                        {
                            self.command(Command::Listen(!self.listening));
                        }
                    });
                });
            });
        let count = self.transfers.values().filter(|t| self.visible(t)).count();
        let attention = self.transfers.values().filter(|t| t.attention).count();
        TopBottomPanel::bottom("footer")
            .exact_height(30.)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(16, 5)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(muted(format!(
                        "{count} transfer{} shown",
                        if count == 1 { "" } else { "s" }
                    )));
                    ui.add_space(8.);
                    ui.label(
                        RichText::new(format!("{attention} need attention"))
                            .size(12.)
                            .color(if attention > 0 { AMBER } else { MUTED }),
                    );
                    if !self.status.is_empty() {
                        ui.label(muted(&self.status)).on_hover_text(&self.status);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(muted(".vendredi/vendredi.db").monospace());
                    });
                });
            });
        let rail = if ctx.content_rect().width() < 1000. {
            158.
        } else {
            200.
        };
        SidePanel::left("rail")
            .exact_width(rail)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(8, 14)),
            )
            .show(ctx, |ui| {
                ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.;
                    section(ui, "  TRAFFIC");
                    ui.add_space(6.);
                    if nav(ui, "All traffic", "", self.view == View::All, false) {
                        self.navigate(View::All);
                    }
                    if nav(
                        ui,
                        "Needs attention",
                        &attention.to_string(),
                        self.view == View::Attention,
                        true,
                    ) {
                        self.navigate(View::Attention);
                    }
                    if nav(ui, "Clear history…", "", false, false) {
                        self.open_cleanup();
                    }
                    ui.add_space(18.);
                    section(ui, "  PARTNERS");
                    ui.add_space(6.);
                    for p in 0..self.workspace.partners.len() {
                        let partner = &self.workspace.partners[p];
                        let kind = match partner.receive.first() {
                            Some(Receive::Http { .. }) => "HTTP",
                            _ => "Folder",
                        };
                        if nav(
                            ui,
                            &partner.name,
                            kind,
                            self.view == View::Partner(p),
                            false,
                        ) {
                            self.partner = p;
                            self.navigate(View::Partner(p));
                        }
                    }
                    ui.add_space(4.);
                    if add_partner_button(ui) {
                        self.open_partner(None);
                    }
                    ui.add_space(18.);
                    section(ui, "  WORKSPACE");
                    ui.add_space(6.);
                    for (name, view) in [
                        ("Rules", View::Rules),
                        ("Scripts", View::Scripts),
                        ("Documents", View::Documents),
                        ("Settings", View::Settings),
                    ] {
                        if nav(ui, name, "", self.view == view, false) {
                            self.navigate(view);
                        }
                    }
                });
            });
        CentralPanel::default()
            .frame(Frame::new().fill(BG))
            .show(ctx, |ui| {
                if self.view == View::Scripts {
                    Frame::new()
                        .inner_margin(16)
                        .show(ui, |ui| self.scripts(ui));
                    return;
                }
                if !matches!(self.view, View::All | View::Attention | View::Partner(_)) {
                    Frame::new().inner_margin(16).show(ui, |ui| self.editor(ui));
                    return;
                }
                if let View::Partner(i) = self.view {
                    Frame::new().inner_margin(12).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.heading(&self.workspace.partners[i].name);
                            if ui.button("Edit partner").clicked() {
                                self.open_partner(Some(i));
                            }
                            let can_remove = self.workspace.partners.len() > 1;
                            if ui
                                .add_enabled(can_remove, form::removal_button("Remove partner"))
                                .on_disabled_hover_text("The workspace needs at least one partner.")
                                .clicked()
                            {
                                self.partner_removal = Some(partners::PartnerRemoval::new(i));
                            }
                        });
                    });
                    ui.separator();
                }
                let r = ui.available_rect_before_wrap();
                let (list, inspect) = if r.width() >= 880. {
                    let mid = r.center().x;
                    (
                        Rect::from_min_max(r.min, pos2(mid, r.bottom())),
                        Rect::from_min_max(pos2(mid + 1., r.top()), r.max),
                    )
                } else {
                    let mid = r.top() + r.height() * 0.38;
                    (
                        Rect::from_min_max(r.min, pos2(r.right(), mid)),
                        Rect::from_min_max(pos2(r.left(), mid + 1.), r.max),
                    )
                };
                ui.painter().rect_filled(r, 0, LINE);
                for rect in [list, inspect] {
                    ui.painter().rect_filled(rect, 0, BG);
                }
                self.table(&mut area(ui, list, "list"));
                self.reconcile_selection();
                self.inspector(&mut area(ui, inspect, "inspector"));
            });
        self.dialogs(ctx);
    }
    fn table(&mut self, ui: &mut Ui) {
        let r = ui.max_rect();
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let bar = Rect::from_min_size(r.min, vec2(r.width(), 57.));
        let mut toolbar = area(ui, bar.shrink2(vec2(12., 10.)), "filters");
        toolbar.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            ui.spacing_mut().interact_size.y = 35.;
            let width = (ui.available_width() - 218.).max(80.);
            Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1.0_f32, EDGE))
                .corner_radius(6)
                .inner_margin(Margin::symmetric(8, 0))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.set_width(width - 16.);
                        ui.set_height(34.);
                        let (icon, _) = ui.allocate_exact_size(vec2(14., 14.), Sense::hover());
                        let c = icon.center() - vec2(1., 1.);
                        ui.painter()
                            .circle_stroke(c, 4., Stroke::new(1.4_f32, MUTED));
                        ui.painter().line_segment(
                            [c + vec2(3., 3.), c + vec2(6., 6.)],
                            Stroke::new(1.4_f32, MUTED),
                        );
                        if ui
                            .add(
                                TextEdit::singleline(&mut self.search)
                                    .frame(false)
                                    .hint_text(
                                        RichText::new("Search by reference, type or content")
                                            .color(MUTED),
                                    )
                                    .desired_width(ui.available_width()),
                            )
                            .changed()
                        {
                            self.matches.clear();
                            if !self.search.is_empty() {
                                self.command(Command::Traffic(self.search.clone()));
                            }
                        }
                    });
                });
            filter(
                ui,
                "Direction",
                108.,
                &mut self.direction,
                ["Any", "In", "Out"].map(str::to_owned),
            );
            let statuses = std::iter::once("Any".to_owned()).chain(
                self.transfers
                    .values()
                    .map(|r| r.status.clone())
                    .collect::<BTreeSet<_>>(),
            );
            filter(ui, "Status", 92., &mut self.filter_status, statuses);
        });
        line(ui, bar.left_bottom(), bar.right_bottom());
        let mut body = area(
            ui,
            Rect::from_min_max(bar.left_bottom(), r.max),
            "traffic-table",
        );
        let mut pick = None;
        ScrollArea::both()
            .id_salt("traffic-scroll")
            .auto_shrink([false, false])
            .show(&mut body, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let w = ui.available_width().max(500.);
                let cols = [0., 0.132, 0.233, 0.41, 0.547, 0.706, 1.];
                let (head, _) = ui.allocate_exact_size(vec2(w, 36.), Sense::hover());
                ui.painter().rect_filled(head, 0, PANEL);
                for (i, name) in ["TIME", "DIR", "PARTNER", "MESSAGE", "REFERENCE", "STATUS"]
                    .iter()
                    .enumerate()
                {
                    ui.painter().text(
                        head.left_center() + vec2(cols[i] * w + 12., 0.),
                        Align2::LEFT_CENTER,
                        name,
                        font(11.),
                        MUTED,
                    );
                }
                let rows: Vec<_> = self
                    .transfers
                    .values()
                    .rev()
                    .filter(|t| self.visible(t))
                    .collect();
                for t in &rows {
                    let (row, response) = ui.allocate_exact_size(vec2(w, 37.), Sense::click());
                    let selected = self.selected == Some(t.id);
                    if selected || response.hovered() {
                        ui.painter()
                            .rect_filled(row, 0, if selected { SELECTED } else { PANEL });
                    }
                    if selected {
                        ui.painter().rect_filled(
                            Rect::from_min_size(row.min, vec2(2., row.height())),
                            0,
                            BLUE,
                        );
                    }
                    line(ui, row.left_bottom(), row.right_bottom());
                    let cell = |i: usize, s: &str, face: FontId, color: Color32| {
                        let rect = Rect::from_min_max(
                            row.min + vec2(cols[i] * w + 12., 0.),
                            pos2(row.left() + cols[i + 1] * w - 3., row.bottom()),
                        );
                        ui.painter()
                            .with_clip_rect(ui.clip_rect().intersect(rect))
                            .text(rect.left_center(), Align2::LEFT_CENTER, s, face, color)
                    };
                    cell(0, &time(t.time), mono(12.), MUTED);
                    let chip = Rect::from_center_size(
                        row.left_center() + vec2(cols[1] * w + 30., 0.),
                        vec2(37., 18.),
                    );
                    ui.painter().rect(
                        chip,
                        4,
                        if t.outgoing { BG } else { TEXT },
                        Stroke::new(1.0_f32, if t.outgoing { EDGE } else { TEXT }),
                        StrokeKind::Inside,
                    );
                    ui.painter().text(
                        chip.center(),
                        Align2::CENTER_CENTER,
                        if t.outgoing { "OUT" } else { "IN" },
                        font(11.),
                        if t.outgoing { SECONDARY_TEXT } else { BG },
                    );
                    cell(2, &t.partner, font(13.), TEXT);
                    cell(3, &t.kind, mono_bold(12.), TEXT);
                    cell(4, &t.reference, mono(12.), MUTED);
                    let status = if t.attention {
                        format!("⚠ {}", t.status)
                    } else {
                        t.status.clone()
                    };
                    cell(
                        5,
                        &status,
                        font(13.),
                        if t.attention { AMBER } else { TEXT },
                    );
                    response.widget_info(|| {
                        WidgetInfo::selected(
                            WidgetType::SelectableLabel,
                            true,
                            selected,
                            format!("{} {}", t.kind, t.reference),
                        )
                    });
                    if response
                        .on_hover_text(format!("{} · {} · {}", t.partner, t.reference, t.status))
                        .clicked()
                    {
                        pick = Some(t.id);
                    }
                }
                if rows.is_empty() {
                    Frame::new().inner_margin(12).show(ui, |ui| {
                        if self.transfers.is_empty() {
                            ui.label(muted("Waiting for the first interchange."));
                            if self.listening
                                && self.root.join("order.edi").is_file()
                                && ui.link("Send sample order").clicked()
                                && let Err(e) = self.sample()
                            {
                                self.status = e.to_string();
                            }
                        } else {
                            ui.label(muted("No matching transfers."));
                        }
                    });
                }
            });
        if let Some(id) = pick {
            self.select(id);
        }
    }
    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.send_dialog {
            let mut open = true;
            Window::new("Send document…")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    ComboBox::from_label("Partner")
                        .selected_text(&self.workspace.partners[self.partner].name)
                        .show_ui(ui, |ui| {
                            for (i, p) in self.workspace.partners.iter().enumerate() {
                                ui.selectable_value(&mut self.partner, i, &p.name);
                            }
                        });
                    ui.label("Document or script");
                    ui.text_edit_singleline(&mut self.send_file);
                    ui.label("Function (scripts)");
                    ui.text_edit_singleline(&mut self.function);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(self.listening, Button::new("Send"))
                            .clicked()
                        {
                            self.command(Command::Send(
                                self.workspace.partners[self.partner].name.clone(),
                                self.send_file.trim().into(),
                                (!self.function.trim().is_empty())
                                    .then(|| self.function.trim().into()),
                            ));
                            self.send_dialog = false;
                        }
                        if self.root.join("order.edi").is_file()
                            && ui
                                .add_enabled(self.listening, Button::new("Send sample order"))
                                .clicked()
                        {
                            if let Err(e) = self.sample() {
                                self.status = e.to_string();
                            }
                            self.send_dialog = false;
                        }
                    });
                });
            self.send_dialog &= open;
        }
        self.cleanup_dialog(ctx);
        self.partner_dialog(ctx);
        self.partner_removal_dialog(ctx);
        self.script_dialog(ctx);
    }
}
