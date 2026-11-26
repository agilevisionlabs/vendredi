use super::*;
use eframe::egui::*;
impl Desktop {
    pub(super) fn show(&mut self, ctx: &egui::Context) {
        self.dispatch_search(ctx);
        self.drain();
        self.shortcuts(ctx);
        self.file_drops(ctx);
        self.toasts(ctx);
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
                    // Messages appear as toasts, and the sidebar counts what
                    // needs attention, so the footer only places the workspace.
                    if ui.available_width() > 240. {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add(
                                    Label::new(muted(self.root.display().to_string()).monospace())
                                        .truncate()
                                        .sense(Sense::click()),
                                )
                                .on_hover_text("Show the workspace in Finder")
                                .on_hover_cursor(CursorIcon::PointingHand)
                                .clicked()
                            {
                                self.perform(shortcuts::Do::Reveal);
                            }
                        });
                    }
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
                    self.workspace_title(ui);
                    ui.add_space(GAP_L);
                    section(ui, "  TRAFFIC");
                    ui.add_space(GAP_S);
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
                    ui.add_space(GAP_XL);
                    section(ui, "  PARTNERS");
                    ui.add_space(GAP_S);
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
                            self.navigate(View::Partner(p));
                            if self.view == View::Partner(p) {
                                self.partner = p;
                                self.compact_detail = false;
                            }
                        }
                    }
                    ui.add_space(GAP_XS);
                    if add_partner_button(ui) {
                        self.open_partner(None);
                    }
                    ui.add_space(GAP_XL);
                    section(ui, "  WORKSPACE");
                    ui.add_space(GAP_S);
                    for (name, view) in [
                        ("Cases", View::Cases),
                        ("Scripts", View::Scripts),
                        ("Documents", View::Documents),
                        ("Settings", View::Settings),
                    ] {
                        if nav(
                            ui,
                            name,
                            "",
                            self.view == view
                                || (view == View::Settings && self.view == View::Config),
                            false,
                        ) {
                            self.navigate(view);
                        }
                    }
                });
            });
        CentralPanel::default()
            .frame(Frame::new().fill(BG))
            .show(ctx, |ui| {
                if self.view == View::Settings {
                    Frame::new()
                        .inner_margin(PAGE_MARGIN)
                        .show(ui, |ui| self.settings(ui));
                    return;
                }
                if self.view == View::Cases {
                    Frame::new()
                        .inner_margin(PAGE_MARGIN)
                        .show(ui, |ui| self.cases(ui));
                    return;
                }
                if self.view == View::Scripts {
                    Frame::new()
                        .inner_margin(PAGE_MARGIN)
                        .show(ui, |ui| self.scripts(ui));
                    return;
                }
                if !matches!(self.view, View::All | View::Attention | View::Partner(_)) {
                    Frame::new()
                        .inner_margin(PAGE_MARGIN)
                        .show(ui, |ui| self.editor(ui));
                    return;
                }
                Frame::new()
                    .inner_margin(Margin {
                        left: PAGE_MARGIN,
                        right: PAGE_MARGIN,
                        top: 12,
                        bottom: 8,
                    })
                    .show(ui, |ui| self.traffic_header(ui));
                ui.separator();
                let r = ui.available_rect_before_wrap();
                if r.width() < 880. && r.height() < 485. {
                    if self.compact_detail && self.selected.is_some() {
                        if ui.button("‹  Back to traffic").clicked() {
                            self.compact_detail = false;
                        }
                        let height = ui.available_height().max(if self.expand_timeline {
                            440.
                        } else {
                            310.
                        });
                        ScrollArea::vertical()
                            .id_salt("compact-inspection")
                            .show(ui, |ui| {
                                let size = vec2(ui.available_width(), height);
                                let (_, rect) = ui.allocate_space(size);
                                self.inspector(&mut area(ui, rect, "compact-inspector"), true);
                            });
                    } else {
                        self.table(&mut area(ui, r, "compact-table"), true);
                        self.reconcile_selection();
                    }
                    return;
                }
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
                self.table(&mut area(ui, list, "list"), false);
                self.reconcile_selection();
                self.inspector(&mut area(ui, inspect, "inspector"), false);
            });
        self.dialogs(ctx);
    }
    fn table(&mut self, ui: &mut Ui, compact: bool) {
        let r = ui.max_rect();
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let bar = Rect::from_min_size(r.min, vec2(r.width(), 57.));
        let mut toolbar = area(ui, bar.shrink2(vec2(12., 10.)), "filters");
        toolbar.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            ui.spacing_mut().interact_size.y = 35.;
            let width = (ui.available_width() - 218.).max(80.);
            // The search field is one control, as tall as the filter buttons,
            // with the magnifier painted inside its left margin.
            let field = ui.scope(|ui| {
                let v = ui.visuals_mut();
                v.extreme_bg_color = PANEL;
                v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, EDGE);
                v.widgets.inactive.corner_radius = 6.into();
                v.widgets.hovered.corner_radius = 6.into();
                v.widgets.active.corner_radius = 6.into();
                ui.add(
                    TextEdit::singleline(&mut self.search)
                        .id(Id::new(shortcuts::SEARCH_ID))
                        .hint_text(RichText::new("Search traffic and message text").color(MUTED))
                        .margin(Margin {
                            left: 30,
                            right: 8,
                            top: 0,
                            bottom: 0,
                        })
                        .desired_width(width)
                        .min_size(vec2(width, BUTTON))
                        .vertical_align(Align::Center),
                )
            });
            let c = field.inner.rect.left_center() + vec2(15., -1.);
            ui.painter()
                .circle_stroke(c, 4., Stroke::new(1.4_f32, MUTED));
            ui.painter().line_segment(
                [c + vec2(3., 3.), c + vec2(6., 6.)],
                Stroke::new(1.4_f32, MUTED),
            );
            let tip = shortcuts::hint(ui.ctx(), "Search traffic", &shortcuts::SEARCH);
            let field = field.inner.on_hover_text(tip);
            if std::mem::take(&mut self.focus_search) {
                field.request_focus();
            }
            if field.changed() {
                self.edit_search(ui.ctx());
            }
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
                let w = ui.available_width().max(600.);
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
                    let paint = |painter: Painter, pos, align: Align2, text: &str, face, color| {
                        let mut job = egui::text::LayoutJob::default();
                        let hits = self.hits(text);
                        search::append(&mut job, text, 0, &hits, TextFormat::simple(face, color));
                        let galley = painter.layout_job(job);
                        let mut origin = align.anchor_size(pos, galley.size()).min;
                        if let Some(hit) = hits.first() {
                            let cursor_x = |byte| {
                                galley
                                    .pos_from_cursor(egui::text::CCursor::new(
                                        text[..byte].chars().count(),
                                    ))
                                    .left()
                            };
                            let overflow =
                                origin.x + cursor_x(hit.end) - painter.clip_rect().right();
                            origin.x -= overflow.max(0.).min(cursor_x(hit.start));
                        }
                        painter.galley(origin, galley, color);
                    };
                    let cell = |i: usize, s: &str, face: FontId, color: Color32| {
                        let rect = Rect::from_min_max(
                            row.min + vec2(cols[i] * w + 12., 0.),
                            pos2(row.left() + cols[i + 1] * w - 3., row.bottom()),
                        );
                        paint(
                            ui.painter().with_clip_rect(ui.clip_rect().intersect(rect)),
                            rect.left_center(),
                            Align2::LEFT_CENTER,
                            s,
                            face,
                            color,
                        );
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
                    paint(
                        ui.painter().clone(),
                        chip.center(),
                        Align2::CENTER_CENTER,
                        t.direction_label(),
                        font(11.),
                        if t.outgoing { SECONDARY_TEXT } else { BG },
                    );
                    cell(2, &t.partner, font(13.), TEXT);
                    cell(3, &t.kind, mono_bold(12.), TEXT);
                    cell(4, &t.reference, mono(12.), MUTED);
                    cell(
                        5,
                        &t.status_label(),
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
                                && let Err(e) = self.sample(self.partner)
                            {
                                self.fail(e.to_string());
                            }
                        } else {
                            ui.label(muted("No matching transfers."));
                            if (!self.search.is_empty()
                                || self.direction != "Any"
                                || self.filter_status != "Any")
                                && ui.link("Clear filters").clicked()
                            {
                                self.clear_search();
                                self.direction = "Any".into();
                                self.filter_status = "Any".into();
                            }
                        }
                    });
                }
            });
        if let Some(id) = pick {
            self.compact_detail = compact;
            self.select(id);
        }
    }
    fn dialogs(&mut self, ctx: &egui::Context) {
        self.send_window(ctx);
        self.cleanup_dialog(ctx);
        self.partner_dialog(ctx);
        self.script_dialog(ctx);
        self.rule_dialog(ctx);
        self.import_dialog(ctx);
        self.case_dialog(ctx);
        self.settings_dialog(ctx);
    }

    /// The workspace name and folder, above the navigation.
    fn workspace_title(&self, ui: &mut Ui) {
        let path = self.root.display().to_string();
        let short = std::env::var_os("HOME")
            .and_then(|h| self.root.strip_prefix(h).ok())
            .map(|p| format!("~/{}", p.display()))
            .unwrap_or_else(|| path.clone());
        ui.horizontal(|ui| {
            ui.add_space(GAP_S);
            ui.vertical(|ui| {
                ui.label(RichText::new("Vendredi").font(bold(15.)));
                ui.add(Label::new(muted(short).monospace().size(11.)).truncate())
                    .on_hover_text(path);
            });
        });
    }

    /// The heading of the traffic views, with whether Vendredi is receiving
    /// and the actions that belong to traffic.
    fn traffic_header(&mut self, ui: &mut Ui) {
        let title = match self.view {
            View::Partner(i) => self.workspace.partners[i].name.clone(),
            View::Attention => "Needs attention".into(),
            _ => "All traffic".into(),
        };
        header_row(ui, |ui| {
            ui.heading(title);
            ui.add_space(GAP_M);
            let folders = self
                .workspace
                .partners
                .iter()
                .flat_map(|p| &p.receive)
                .filter(|r| matches!(r, Receive::Folder { .. }))
                .count();
            let (dot, label, detail) = if self.listening {
                (
                    GREEN,
                    "Working",
                    format!(
                        "Receiving: HTTP on :{} · {folders} folder{} watched",
                        self.address.port(),
                        if folders == 1 { "" } else { "s" }
                    ),
                )
            } else {
                (
                    MUTED,
                    "Paused",
                    "Not receiving: HTTP and folders are off until you resume".to_owned(),
                )
            };
            Frame::new()
                .fill(SURFACE)
                .corner_radius(10)
                .inner_margin(Margin::symmetric(8, 2))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 4.;
                    ui.label(RichText::new("●").size(9.).color(dot));
                    ui.label(RichText::new(label).size(11.).color(SECONDARY_TEXT));
                })
                .response
                .on_hover_text(detail);
            if let View::Partner(i) = self.view {
                ui.add_space(GAP_M);
                if button(ui, "Edit partner", false).clicked() {
                    self.open_partner(Some(i));
                }
                let can_remove = self.workspace.partners.len() > 1;
                if ui
                    .add_enabled(can_remove, form::removal_button("Remove partner"))
                    .on_disabled_hover_text("The workspace needs at least one partner.")
                    .clicked()
                    && let Err(e) = self.remove_partner(i)
                {
                    self.fail(format!("{e:#}"));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tip = shortcuts::hint(ui.ctx(), "Send a document or script", &shortcuts::SEND);
                if button(ui, "→  Send document…", true)
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.open_send();
                }
                let toggle = if self.listening {
                    "⏸  Pause"
                } else {
                    "▷  Resume"
                };
                if button(ui, toggle, false).clicked() {
                    self.command(Command::Listen(!self.listening));
                }
            });
        });
    }

    /// Files dragged onto the window: an EDI document or script opens the
    /// send dialog, a case JSON file or folder opens the import preview.
    fn file_drops(&mut self, ctx: &egui::Context) {
        let (hovering, dropped) = ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw.dropped_files.iter().find_map(|f| f.path.clone()),
            )
        });
        if self.dialog_open() {
            return;
        }
        if hovering {
            let screen = ctx.content_rect();
            let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("drop")));
            painter.rect_filled(
                screen,
                0,
                Color32::from_rgba_unmultiplied(247, 249, 252, 230),
            );
            let target = screen.shrink(GAP_XL);
            painter.rect_stroke(target, 12, Stroke::new(2.0_f32, BLUE), StrokeKind::Inside);
            painter.text(
                target.center() - vec2(0., 12.),
                Align2::CENTER_CENTER,
                "Drop to send or import",
                bold(20.),
                TEXT,
            );
            painter.text(
                target.center() + vec2(0., 16.),
                Align2::CENTER_CENTER,
                "An EDI document or script is sent to a partner. A case file or folder is imported.",
                font(13.),
                MUTED,
            );
        }
        let Some(path) = dropped else {
            return;
        };
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if path.is_dir() || extension == "json" {
            self.import_from(&path);
        } else {
            self.open_send();
            self.send_script = extension == "js";
            self.function.clear();
            self.send_file = match path.strip_prefix(&self.root) {
                Ok(inside) => inside.display().to_string(),
                Err(_) => path.display().to_string(),
            };
        }
    }
}
