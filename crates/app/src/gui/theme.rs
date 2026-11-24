use eframe::egui::*;
pub(super) const BG: Color32 = Color32::from_rgb(255, 255, 255);
pub(super) const PANEL: Color32 = Color32::from_rgb(247, 249, 252);
pub(super) const CODE: Color32 = Color32::from_rgb(249, 250, 252);
pub(super) const SURFACE: Color32 = Color32::from_rgb(235, 239, 244);
pub(super) const LINE: Color32 = Color32::from_rgb(214, 221, 230);
pub(super) const EDGE: Color32 = Color32::from_rgb(166, 178, 193);
pub(super) const TEXT: Color32 = Color32::from_rgb(27, 39, 53);
pub(super) const MUTED: Color32 = Color32::from_rgb(82, 97, 116);
pub(super) const SECONDARY_TEXT: Color32 = Color32::from_rgb(55, 70, 87);
pub(super) const BLUE: Color32 = Color32::from_rgb(29, 78, 216);
pub(super) const SELECTED: Color32 = Color32::from_rgb(226, 237, 255);
pub(super) const AMBER: Color32 = Color32::from_rgb(138, 75, 8);
pub(super) const GREEN: Color32 = Color32::from_rgb(23, 100, 66);
pub(super) const AMBER_BG: Color32 = Color32::from_rgb(254, 243, 214);
pub(super) const GREEN_BG: Color32 = Color32::from_rgb(221, 244, 232);

pub(super) fn install(ctx: &Context) {
    ctx.set_theme(Theme::Light);
    let mut fonts = FontDefinitions::default();
    for (name, family, data) in [
        (
            "Inter",
            FontFamily::Proportional,
            include_bytes!("../../fonts/Inter.ttf").as_slice(),
        ),
        (
            "IBM Plex Mono",
            FontFamily::Monospace,
            include_bytes!("../../fonts/IBMPlexMono-Regular.ttf").as_slice(),
        ),
        (
            "Inter SemiBold",
            FontFamily::Name("Inter SemiBold".into()),
            include_bytes!("../../fonts/Inter-SemiBold.ttf").as_slice(),
        ),
        (
            "Plex SemiBold",
            FontFamily::Name("Plex SemiBold".into()),
            include_bytes!("../../fonts/IBMPlexMono-SemiBold.ttf").as_slice(),
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), FontData::from_static(data).into());
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, name.into());
    }
    ctx.set_fonts(fonts);
    let mut visuals = Visuals::light();
    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = CODE;
    visuals.widgets.noninteractive.bg_stroke.color = LINE;
    visuals.weak_text_color = Some(MUTED);
    visuals.override_text_color = Some(TEXT);
    visuals.hyperlink_color = BLUE;
    visuals.selection.bg_fill = SELECTED;
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
    ] {
        widget.bg_fill = SURFACE;
        widget.weak_bg_fill = SURFACE;
        widget.bg_stroke = Stroke::new(1.0_f32, EDGE);
        widget.corner_radius = 6.into();
        widget.fg_stroke.color = TEXT;
    }
    visuals.widgets.hovered.bg_fill = SELECTED;
    visuals.widgets.hovered.weak_bg_fill = SELECTED;
    ctx.set_visuals_of(Theme::Light, visuals);
    ctx.style_mut(|style| {
        style.text_styles.insert(TextStyle::Body, font(13.));
        style.text_styles.insert(TextStyle::Button, font(13.));
        style.text_styles.insert(TextStyle::Monospace, mono(12.));
        style.spacing.item_spacing = vec2(8., 6.);
        style.spacing.button_padding = vec2(10., 5.);
    });
}
pub(super) fn font(size: f32) -> FontId {
    FontId::proportional(size)
}
pub(super) fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}
pub(super) fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Inter SemiBold".into()))
}
pub(super) fn mono_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Plex SemiBold".into()))
}
pub(super) fn muted(s: impl Into<String>) -> RichText {
    RichText::new(s).size(12.).color(MUTED)
}
pub(super) fn section(ui: &mut Ui, s: &str) {
    ui.label(
        RichText::new(s)
            .font(bold(11.))
            .extra_letter_spacing(0.8)
            .color(MUTED),
    );
}
pub(super) fn badge(ui: &mut Ui, text: &str, fg: Color32, bg: Color32, monospace: bool) {
    let face = if monospace { mono(12.) } else { font(12.) };
    let galley = ui.painter().layout_no_wrap(text.into(), face, fg);
    let width = (galley.size().x + 14.).min(310.);
    let (r, response) = ui.allocate_exact_size(vec2(width, 22.), Sense::hover());
    ui.painter().rect_filled(r, 5, bg);
    ui.painter()
        .with_clip_rect(ui.clip_rect().intersect(r.shrink2(vec2(6., 0.))))
        .galley(
            r.left_center() + vec2(7., -galley.size().y / 2.),
            galley,
            fg,
        );
    response.on_hover_text(text);
}
pub(super) fn nav(ui: &mut Ui, text: &str, trailing: &str, selected: bool, count: bool) -> bool {
    let (r, res) = ui.allocate_exact_size(vec2(ui.available_width(), 34.), Sense::click());
    let p = ui.painter();
    if selected || res.hovered() {
        p.rect_filled(r, 6, if selected { SELECTED } else { SURFACE });
    }
    if selected {
        p.rect_filled(Rect::from_min_size(r.min, vec2(2., 34.)), 1, BLUE);
    }
    let face = if selected { bold(13.) } else { font(13.) };
    let mut job = text::LayoutJob::simple_singleline(text.to_owned(), face, TEXT);
    let reserved = if trailing.is_empty() {
        20.
    } else {
        let tag_width = p
            .layout_no_wrap(trailing.to_owned(), font(11.), MUTED)
            .size()
            .x;
        30. + tag_width
    };
    job.wrap.max_width = (r.width() - reserved).max(1.);
    job.wrap.max_rows = 1;
    let galley = p.layout_job(job);
    let truncated = galley.elided;
    p.galley(
        r.left_center() + vec2(10., -galley.size().y / 2.),
        galley,
        TEXT,
    );
    if res.has_focus() {
        p.rect_stroke(
            r.shrink(1.),
            6,
            Stroke::new(1_f32, BLUE),
            StrokeKind::Inside,
        );
    }
    if !trailing.is_empty() {
        if count {
            let b = Rect::from_center_size(r.right_center() - vec2(19., 0.), vec2(22., 18.));
            p.rect_filled(b, 9, AMBER_BG);
            p.text(
                b.center(),
                Align2::CENTER_CENTER,
                trailing,
                font(11.),
                AMBER,
            );
        } else {
            p.text(
                r.right_center() - vec2(10., 0.),
                Align2::RIGHT_CENTER,
                trailing,
                font(11.),
                MUTED,
            );
        }
    }
    res.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, text));
    let res = if truncated {
        res.on_hover_text(text)
    } else {
        res
    };
    res.on_hover_cursor(CursorIcon::PointingHand).clicked()
}
pub(super) fn area(ui: &mut Ui, r: Rect, id: &str) -> Ui {
    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt(id)
            .max_rect(r)
            .layout(Layout::top_down(Align::Min)),
    );
    child.set_clip_rect(ui.clip_rect().intersect(r));
    child
}
pub(super) fn line(ui: &Ui, a: Pos2, b: Pos2) {
    ui.painter()
        .line_segment([a, b], Stroke::new(1.0_f32, LINE));
}
pub(super) fn time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|v| {
            v.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}
pub(super) fn button(ui: &mut Ui, label: &str, primary: bool) -> Response {
    let text = RichText::new(label).color(if primary { Color32::WHITE } else { TEXT });
    ui.add(
        Button::new(text)
            .min_size(vec2(0., 36.))
            .fill(if primary { BLUE } else { SURFACE })
            .stroke(Stroke::new(1.0_f32, if primary { BLUE } else { EDGE })),
    )
}

pub(super) fn filter(
    ui: &mut Ui,
    name: &str,
    width: f32,
    value: &mut String,
    values: impl IntoIterator<Item = String>,
) {
    let label = format!("{name} {value}");
    let response = ui
        .add_sized(
            [width, 36.],
            Button::new(RichText::new(&label).size(13.))
                .truncate()
                .fill(PANEL)
                .stroke(Stroke::new(1_f32, EDGE)),
        )
        .on_hover_text(label);
    Popup::menu(&response).show(|ui| {
        for choice in values {
            if ui
                .selectable_value(value, choice.clone(), &choice)
                .clicked()
            {
                ui.close();
            }
        }
    });
}
pub(super) fn add_partner_button(ui: &mut Ui) -> bool {
    let (r, res) = ui.allocate_exact_size(vec2(ui.available_width(), 34.), Sense::click());
    if res.hovered() {
        ui.painter().rect_filled(r, 6, SURFACE);
    }
    let r = r.shrink(0.5);
    let points = [
        r.left_top() + vec2(4., 0.),
        r.right_top() - vec2(4., 0.),
        r.right_top() + vec2(0., 4.),
        r.right_bottom() - vec2(0., 4.),
        r.right_bottom() - vec2(4., 0.),
        r.left_bottom() + vec2(4., 0.),
        r.left_bottom() - vec2(0., 4.),
        r.left_top() + vec2(0., 4.),
        r.left_top() + vec2(4., 0.),
    ];
    ui.painter().extend(Shape::dashed_line(
        &points,
        Stroke::new(1_f32, EDGE),
        3.,
        2.,
    ));
    ui.painter().text(
        r.left_center() + vec2(10., 0.),
        Align2::LEFT_CENTER,
        "+ Add partner",
        font(13.),
        MUTED,
    );
    res.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Add partner"));
    res.clicked()
}
