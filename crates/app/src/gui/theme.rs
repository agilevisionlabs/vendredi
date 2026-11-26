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
    // Icons come from Phosphor, after the text faces.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
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
pub(super) fn nav(
    ui: &mut Ui,
    glyph: &str,
    text: &str,
    trailing: &str,
    selected: bool,
    count: bool,
) -> bool {
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
    job.wrap.max_width = (r.width() - reserved - 24.).max(1.);
    job.wrap.max_rows = 1;
    let galley = p.layout_job(job);
    let truncated = galley.elided;
    // The icon is its own shape, so the item's text stays its plain name.
    p.text(
        r.left_center() + vec2(10., 0.),
        Align2::LEFT_CENTER,
        glyph,
        font(15.),
        if selected { BLUE } else { MUTED },
    );
    p.galley(
        r.left_center() + vec2(34., -galley.size().y / 2.),
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
/// The icon set. Labels put an icon before their text with `with_icon`.
pub(super) use egui_phosphor::regular as icon;
pub(super) use vendredi_core::engine::local_time as time;

pub(super) fn with_icon(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

/// The spacing scale. Views use these instead of literal gaps.
pub(super) const GAP_XS: f32 = 4.;
pub(super) const GAP_S: f32 = 8.;
pub(super) const GAP_M: f32 = 12.;
pub(super) const GAP_L: f32 = 16.;
pub(super) const GAP_XL: f32 = 24.;
/// The inset of every page and card.
pub(super) const PAGE_MARGIN: i8 = 16;

/// The frame of every card: the same border, corners and padding, so cards
/// side by side line up.
pub(super) fn card_frame(fill: Color32, stroke: Color32) -> Frame {
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1_f32, stroke))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(16, 12))
}

/// The first line of a card: a fixed height, so titles of different sizes
/// in neighbouring cards share a centre line.
pub(super) fn title_row(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    let size = vec2(ui.available_width(), 24.);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), contents);
}

/// The heading line of a card section, as tall as its small buttons.
pub(super) fn section_row(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    let size = vec2(ui.available_width(), SMALL_BUTTON);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), contents);
}

/// A highlighted line that needs the reader's attention.
pub(super) fn notice(ui: &mut Ui, text: &str) {
    Frame::new()
        .fill(AMBER_BG)
        .corner_radius(8)
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(AMBER));
        });
}

/// A view's title row. Its fixed height matches `button`, so headings,
/// labels and buttons share one centre line instead of stepping down by
/// widget height.
pub(super) fn header_row(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    let size = vec2(ui.available_width(), 36.);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), contents);
}

/// Every action button has one of two heights: 36 for page and dialog
/// actions, 28 for actions inside a card section.
pub(super) const BUTTON: f32 = 36.;
pub(super) const SMALL_BUTTON: f32 = 28.;

pub(super) fn styled(label: &str, primary: bool, height: f32) -> Button<'_> {
    let text = RichText::new(label).color(if primary { Color32::WHITE } else { TEXT });
    Button::new(text)
        .min_size(vec2(0., height))
        .fill(if primary { BLUE } else { SURFACE })
        .stroke(Stroke::new(1.0_f32, if primary { BLUE } else { EDGE }))
}

pub(super) fn button(ui: &mut Ui, label: &str, primary: bool) -> Response {
    ui.add(styled(label, primary, BUTTON))
}

pub(super) fn small_button(ui: &mut Ui, label: &str) -> Response {
    ui.add(styled(label, false, SMALL_BUTTON))
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
        &with_icon(icon::PLUS, "Add partner"),
        font(13.),
        MUTED,
    );
    res.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Add partner"));
    res.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_icons_have_glyphs() {
        let ctx = Context::default();
        install(&ctx);
        let _ = ctx.run(Default::default(), |_| {});
        for c in [
            icon::PAUSE,
            icon::PLAY,
            icon::PAPER_PLANE_RIGHT,
            icon::CARET_DOWN,
        ]
        .map(|s| s.chars().next().unwrap())
        {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&FontId::proportional(13.), c)),
                "{c}"
            );
        }
    }
}

/// A page or list with nothing in it yet: what it is for and the next step.
pub(super) fn empty_state(ui: &mut Ui, title: &str, text: &str, actions: impl FnOnce(&mut Ui)) {
    ui.add_space(GAP_XL);
    ui.vertical(|ui| {
        ui.set_max_width(460.);
        ui.label(RichText::new(title).font(bold(15.)));
        ui.add_space(GAP_XS);
        ui.label(RichText::new(text).color(SECONDARY_TEXT));
        ui.add_space(GAP_M);
        ui.horizontal(actions);
    });
}
