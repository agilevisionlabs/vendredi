use super::*;
use eframe::egui::*;
use serde_json::Value;

fn array<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v[k].as_array().map(Vec::as_slice).unwrap_or_default()
}
fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn transport(v: &Value) -> String {
    for (key, prefix) in [
        ("path", "HTTP POST "),
        ("url", "HTTP POST "),
        ("file", "File "),
        ("directory", "Folder "),
    ] {
        if let Some(s) = v[key].as_str() {
            return format!("{prefix}{s}");
        }
    }
    String::new()
}
enum Link {
    Transfer(i64, String),
    Rule(String),
    Case(String),
    Script(PathBuf, Option<String>),
}
struct Step {
    at: i64,
    title: String,
    note: String,
    link: Option<Link>,
}
impl Desktop {
    fn steps(&self, row: &TrafficRow) -> Vec<Step> {
        let events = array(&self.detail, "events");
        let mut steps = Vec::new();
        for v in events {
            let event = text(v, "event");
            let mut link = None;
            let (title, note) = match event {
                "received" => ("Received".into(), transport(&v["metadata"])),
                "validated" => (
                    "Parsed and validated".into(),
                    format!(
                        "1 interchange · {} message{} · {} findings",
                        v["messages"],
                        if v["messages"] == 1 { "" } else { "s" },
                        v["findings"]
                    ),
                ),
                "finding" => (
                    "Validation finding".into(),
                    text(&v["finding"], "text").into(),
                ),
                "no_match" => ("No rule matched".into(), text(v, "type").into()),
                "execution" if text(v, "name") == "acknowledge" => continue,
                "execution" if text(v, "name").starts_with("command:") => (
                    text(v, "name").trim_start_matches("command: ").to_owned(),
                    "Partner command".into(),
                ),
                "execution" if text(v, "name").starts_with("initiate:") => {
                    link = Some(Link::Case(text(v, "case").into()));
                    (
                        text(v, "name").trim_start_matches("initiate: ").to_owned(),
                        "Partner-initiated send".into(),
                    )
                }
                "execution" => {
                    // The link opens the rule in the partner's active case.
                    link = Some(Link::Rule(text(v, "name").into()));
                    let case = text(v, "case");
                    (
                        "Rule matched".into(),
                        if matches!(case, "" | "default") {
                            String::new()
                        } else {
                            format!("in case {case}")
                        },
                    )
                }
                "outbound" | "resent" => {
                    let Some(id) = v["transfer"].as_i64() else {
                        continue;
                    };
                    let Some(t) = self.transfers.get(&id) else {
                        continue;
                    };
                    link = Some(Link::Transfer(
                        id,
                        if t.reference.is_empty() {
                            format!("#{id}")
                        } else {
                            t.reference.clone()
                        },
                    ));
                    let label = if t.status == "Delivered" {
                        "sent"
                    } else {
                        "outbound"
                    };
                    (format!("{} {label}", t.kind), t.status.to_lowercase())
                }
                "scheduled" => (
                    "Delay scheduled".into(),
                    format!(
                        "{} ms",
                        v["due"].as_i64().unwrap_or_default()
                            - v["time"].as_i64().unwrap_or_default()
                    ),
                ),
                "action" if v["error"].is_string() => {
                    ("Action failed".into(), text(v, "error").into())
                }
                "action" if text(v, "action").starts_with("Script") => {
                    if let Some(p) = v["script"].as_str() {
                        let rule = events
                            .iter()
                            .find(|e| e["event"] == "execution" && e["execution"] == v["execution"])
                            .and_then(|e| e["name"].as_str())
                            .map(str::to_owned);
                        link = Some(Link::Script(p.into(), rule));
                    }
                    let logs = events
                        .iter()
                        .filter(|e| e["event"] == "script_log" && e["execution"] == v["execution"])
                        .count();
                    (
                        "Script ran".into(),
                        format!("{logs} log line{}", if logs == 1 { "" } else { "s" }),
                    )
                }
                "acknowledged" => ("Acknowledgement received".into(), text(v, "status").into()),
                "failed" => ("Delivery failed".into(), text(v, "detail").into()),
                "interrupted" => ("Delivery interrupted".into(), String::new()),
                _ => continue,
            };
            steps.push(Step {
                at: v["time"].as_i64().unwrap_or(row.time),
                title,
                note,
                link,
            });
        }
        steps
    }
    pub(super) fn inspector(&mut self, ui: &mut Ui, compact: bool) {
        let Some(row) = self
            .selected
            .and_then(|id| self.transfers.get(&id))
            .cloned()
        else {
            Frame::new().inner_margin(16).show(ui, |ui| {
                ui.label(muted("Select a transfer to inspect it."));
            });
            return;
        };
        if self.detail["transfer"].as_i64() != Some(row.id) {
            ui.label(muted("Loading transfer…"));
            return;
        }
        let metadata: Value =
            serde_json::from_str(text(&self.detail, "metadata")).unwrap_or_default();
        let r = ui.max_rect();
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let header = Frame::new()
            .inner_margin(Margin::symmetric(16, 16))
            .show(ui, |ui| {
                ui.set_width(r.width() - 32.);
                ui.spacing_mut().item_spacing = vec2(9., 5.);
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(if row.kind.is_empty() {
                            "Transfer"
                        } else {
                            &row.kind
                        })
                        .font(mono_bold(18.)),
                    );
                    if !row.version.is_empty() {
                        badge(ui, &row.version, SECONDARY_TEXT, SURFACE, true);
                    }
                    badge(
                        ui,
                        &row.status,
                        if row.attention { AMBER } else { GREEN },
                        if row.attention { AMBER_BG } else { GREEN_BG },
                        false,
                    );
                });
                let groups = array(&self.detail, "groups");
                if !groups.is_empty() {
                    let summary: Vec<_> = groups
                        .iter()
                        .map(|g| format!("GS {} · {}", text(g, "reference"), text(g, "sets")))
                        .collect();
                    ui.label(muted(summary.join("   ")).monospace().size(11.));
                }
                let source = if row.outgoing {
                    "Sent by partner"
                } else {
                    "Received from your system"
                };
                ui.label(muted(format!(
                    "{source} · {} · {} · {} · {} bytes",
                    row.partner,
                    time(row.time),
                    transport(&metadata),
                    array(&self.detail, "bytes").len()
                )));
            })
            .response
            .rect;
        line(ui, header.left_bottom(), header.right_bottom());
        let steps = self.steps(&row);
        let expanded = !compact || self.expand_timeline;
        let height = if expanded {
            (42. + 26. * steps.len() as f32).min((r.height() * 0.36).max(62.))
        } else {
            34.
        };
        let timeline = Rect::from_min_size(header.left_bottom(), vec2(r.width(), height));
        let mut timeline_ui = area(ui, timeline.shrink2(vec2(16., 9.)), "timeline");
        if compact {
            let label = if self.expand_timeline {
                "Hide timeline"
            } else {
                "Show timeline"
            };
            if timeline_ui.link(label).clicked() {
                self.expand_timeline = !self.expand_timeline;
            }
        } else {
            section(&mut timeline_ui, "WHAT HAPPENED");
        }
        if expanded {
            timeline_ui.add_space(GAP_S);
            ScrollArea::vertical()
                .id_salt("steps")
                .show(&mut timeline_ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.;
                    for (i, step) in steps.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.set_min_height(26.);
                            ui.spacing_mut().item_spacing.x = 8.;
                            ui.add_sized([60., 22.], Label::new(muted(time(step.at)).monospace()));
                            let (marker, _) = ui.allocate_exact_size(vec2(9., 26.), Sense::hover());
                            let c = marker.center();
                            if i > 0 {
                                line(ui, marker.center_top(), c);
                            }
                            if i + 1 < steps.len() {
                                line(ui, c, marker.center_bottom());
                            }
                            let last = i + 1 == steps.len();
                            ui.painter().circle(
                                c,
                                3.7,
                                if last { BLUE } else { BG },
                                Stroke::new(1.0_f32, if last { BLUE } else { EDGE }),
                            );
                            ui.label(RichText::new(&step.title).font(bold(13.)));
                            if !step.note.is_empty() {
                                ui.label(RichText::new(&step.note).size(12.).color(SECONDARY_TEXT));
                            }
                            if let Some(link) = &step.link {
                                let label = match link {
                                    Link::Transfer(_, s) | Link::Rule(s) | Link::Case(s) => {
                                        s.clone()
                                    }
                                    Link::Script(p, _) => p.display().to_string(),
                                };
                                if ui
                                    .link(
                                        RichText::new(label)
                                            .monospace()
                                            .size(12.)
                                            .color(BLUE)
                                            .underline(),
                                    )
                                    .clicked()
                                {
                                    match link {
                                        Link::Transfer(id, _) => {
                                            if self
                                                .transfers
                                                .get(id)
                                                .is_some_and(|r| !self.visible(r))
                                            {
                                                self.view = View::All;
                                                self.clear_search();
                                                self.direction = "Any".into();
                                                self.filter_status = "Any".into();
                                            }
                                            self.select(*id);
                                        }
                                        Link::Rule(name) => self.edit_rule_link(&row.partner, name),
                                        Link::Case(id) => self.show_case(id),
                                        Link::Script(p, rule) => {
                                            self.edit_script_link(p, &row.partner, rule.as_deref());
                                        }
                                    }
                                }
                            }
                        });
                    }
                });
        }
        let tabs = Rect::from_min_size(timeline.left_bottom(), vec2(r.width(), 40.));
        ui.painter().rect_filled(tabs, 0, PANEL);
        line(ui, tabs.left_top(), tabs.right_top());
        line(ui, tabs.left_bottom(), tabs.right_bottom());
        let mut bar = area(ui, tabs.shrink2(vec2(10., 0.)), "detail-tabs");
        bar.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.;
            let findings = array(&self.detail, "findings").len();
            let logs = array(&self.detail, "events")
                .iter()
                .filter(|e| e["event"] == "script_log")
                .count();
            for (tab, label) in [
                (DetailTab::Message, "Message".into()),
                (DetailTab::Findings, format!("Findings {findings}")),
                (DetailTab::Log, format!("Script log {logs}")),
                (DetailTab::Transport, "Transport".into()),
            ] {
                let active = self.detail_tab == tab;
                let width = label.len() as f32 * 6.5 + 16.;
                let response = ui.add_sized(
                    [width, 36.],
                    Button::new(RichText::new(label).size(12.).color(if active {
                        TEXT
                    } else {
                        MUTED
                    }))
                    .frame(false),
                );
                if active {
                    let rect = response.rect;
                    line_accent(ui, rect.left(), rect.right(), tabs.bottom() - 1.);
                }
                if response.clicked() {
                    self.detail_tab = tab;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 12.;
                if ui.link("Copy raw").clicked() {
                    let bytes = array(&self.detail, "bytes")
                        .iter()
                        .filter_map(|v| v.as_u64().map(|n| n as u8))
                        .collect::<Vec<_>>();
                    ui.ctx().copy_text(vendredi_core::edi::latin1(&bytes));
                    self.status = "Raw message copied".into();
                }
                if ui
                    .add_enabled(
                        self.listening
                            && self
                                .workspace
                                .partners
                                .iter()
                                .any(|p| p.name == row.partner),
                        LinkWidget::new(if row.outgoing { "Resend" } else { "Replay" }),
                    )
                    .on_disabled_hover_text(if self.listening {
                        "This partner has been removed."
                    } else {
                        "Start the listener to send traffic."
                    })
                    .clicked()
                {
                    self.command(if row.outgoing {
                        Command::Resend(row.id)
                    } else {
                        Command::Replay(row.id)
                    });
                }
            });
        });
        let content = Rect::from_min_max(tabs.left_bottom(), r.max);
        match self.detail_tab {
            DetailTab::Message => self.message(ui, content),
            DetailTab::Findings | DetailTab::Log | DetailTab::Transport => {
                let mut content_ui = area(ui, content.shrink(12.), "tab-content");
                ScrollArea::both().show(&mut content_ui, |ui| match self.detail_tab {
                    DetailTab::Findings => {
                        let findings = array(&self.detail, "findings");
                        if findings.is_empty() {
                            ui.label(muted("No findings."));
                        }
                        for f in findings {
                            ui.colored_label(AMBER, text(f, "code"));
                            ui.label(format!("Byte {} · {}", f["offset"], text(f, "text")));
                        }
                    }
                    DetailTab::Log => {
                        let mut count = 0;
                        for e in array(&self.detail, "events")
                            .iter()
                            .filter(|e| e["event"] == "script_log")
                        {
                            ui.monospace(format!(
                                "{}  {}",
                                time(e["time"].as_i64().unwrap_or_default()),
                                text(e, "text")
                            ));
                            count += 1;
                        }
                        if count == 0 {
                            ui.label(muted("No script log entries."));
                        }
                    }
                    _ => {
                        if let Some(data) = metadata.as_object() {
                            for (name, value) in data {
                                section(ui, name);
                                ui.monospace(value.as_str().map(str::to_owned).unwrap_or_else(
                                    || serde_json::to_string_pretty(value).unwrap_or_default(),
                                ));
                            }
                        }
                    }
                });
            }
        }
    }
    fn message(&mut self, ui: &mut Ui, code: Rect) {
        let segments = array(&self.detail, "segments");
        let message = text(&self.detail, "raw");
        let hits = self.hits(message);
        let reveal = self.reveal_match && !hits.is_empty();
        if !segments.is_empty() {
            self.reveal_match = false;
        }
        let mut offset = 0;
        ui.painter().rect_filled(code, 0, CODE);
        let mut body = area(ui, code, "message-lines");
        body.spacing_mut().item_spacing = Vec2::ZERO;
        ScrollArea::both()
            .auto_shrink([false, false])
            .show(&mut body, |ui| {
                ui.add_space(GAP_M);
                let widest = segments
                    .iter()
                    .filter_map(|s| s["raw"].as_str())
                    .map(|s| s.chars().count())
                    .max()
                    .unwrap_or(0);
                let width = ui.available_width().max(55. + widest as f32 * 7.3);
                for (i, seg) in segments.iter().enumerate() {
                    let (row, res) = ui.allocate_exact_size(vec2(width, 22.), Sense::click());
                    let active = self.segment == i;
                    if active || res.hovered() {
                        ui.painter()
                            .rect_filled(row, 0, if active { SELECTED } else { SURFACE });
                    }
                    if active {
                        ui.painter().rect_filled(
                            Rect::from_min_size(row.min, vec2(2., 22.)),
                            0,
                            BLUE,
                        );
                    }
                    ui.painter().text(
                        row.left_center() + vec2(30., 0.),
                        Align2::RIGHT_CENTER,
                        (i + 1).to_string(),
                        mono(12.),
                        MUTED,
                    );
                    let raw = text(seg, "raw");
                    let tag = text(seg, "tag");
                    let tag = if raw.starts_with(tag) { tag } else { "" };
                    offset += message[offset..].len()
                        - message[offset..].trim_start_matches(['\r', '\n']).len();
                    let mut job = egui::text::LayoutJob::default();
                    let tag_color = if matches!(tag, "UNB" | "UNH" | "UNT" | "UNZ") {
                        MUTED
                    } else {
                        BLUE
                    };
                    search::append(
                        &mut job,
                        tag,
                        offset,
                        &hits,
                        TextFormat::simple(mono(12.), tag_color),
                    );
                    search::append(
                        &mut job,
                        &raw[tag.len()..],
                        offset + tag.len(),
                        &hits,
                        TextFormat::simple(mono(12.), TEXT),
                    );
                    let galley = ui.painter().layout_job(job);
                    let pos = Align2::LEFT_CENTER
                        .anchor_size(row.left_center() + vec2(43., 0.), galley.size())
                        .min;
                    if reveal
                        && let Some(hit) = hits.first()
                        && (offset..offset + raw.len()).contains(&hit.start)
                    {
                        let index = raw[..hit.start - offset].chars().count();
                        let target = galley
                            .pos_from_cursor(egui::text::CCursor::new(index))
                            .translate(pos.to_vec2());
                        ui.scroll_to_rect(target.expand(8.), None);
                    }
                    ui.painter().galley(pos, galley, TEXT);
                    offset += raw.len();
                    if res.clicked() {
                        self.segment = i;
                    }
                }
            });
    }
}
use egui::Link as LinkWidget;
fn line_accent(ui: &Ui, left: f32, right: f32, y: f32) {
    ui.painter()
        .line_segment([pos2(left, y), pos2(right, y)], Stroke::new(2.0_f32, BLUE));
}
