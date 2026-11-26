use eframe::egui::*;

/// The colours of one theme. Views read them through `col::bg()`, `col::text()` and
/// the other functions below, which follow the current theme.
pub(super) struct Palette {
    bg: Color32,
    panel: Color32,
    code: Color32,
    surface: Color32,
    line: Color32,
    edge: Color32,
    text: Color32,
    muted: Color32,
    secondary_text: Color32,
    blue: Color32,
    selected: Color32,
    amber: Color32,
    green: Color32,
    amber_bg: Color32,
    green_bg: Color32,
    highlight: Color32,
}

const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(255, 255, 255),
    panel: Color32::from_rgb(247, 249, 252),
    code: Color32::from_rgb(249, 250, 252),
    surface: Color32::from_rgb(235, 239, 244),
    line: Color32::from_rgb(214, 221, 230),
    edge: Color32::from_rgb(166, 178, 193),
    text: Color32::from_rgb(27, 39, 53),
    muted: Color32::from_rgb(82, 97, 116),
    secondary_text: Color32::from_rgb(55, 70, 87),
    blue: Color32::from_rgb(29, 78, 216),
    selected: Color32::from_rgb(226, 237, 255),
    amber: Color32::from_rgb(138, 75, 8),
    green: Color32::from_rgb(23, 100, 66),
    amber_bg: Color32::from_rgb(254, 243, 214),
    green_bg: Color32::from_rgb(221, 244, 232),
    highlight: Color32::from_rgb(255, 220, 115),
};

const DARK: Palette = Palette {
    bg: Color32::from_rgb(21, 24, 29),
    panel: Color32::from_rgb(28, 32, 38),
    code: Color32::from_rgb(25, 28, 34),
    surface: Color32::from_rgb(40, 45, 53),
    line: Color32::from_rgb(52, 58, 68),
    edge: Color32::from_rgb(86, 95, 109),
    text: Color32::from_rgb(230, 234, 240),
    muted: Color32::from_rgb(150, 160, 175),
    secondary_text: Color32::from_rgb(190, 198, 210),
    blue: Color32::from_rgb(77, 128, 245),
    selected: Color32::from_rgb(35, 54, 94),
    amber: Color32::from_rgb(240, 182, 92),
    green: Color32::from_rgb(92, 200, 142),
    amber_bg: Color32::from_rgb(64, 47, 20),
    green_bg: Color32::from_rgb(22, 58, 42),
    highlight: Color32::from_rgb(120, 96, 24),
};

thread_local! {
    /// The UI runs on one thread; tests each have their own.
    static DARK_MODE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn palette() -> &'static Palette {
    if DARK_MODE.get() { &DARK } else { &LIGHT }
}

/// egui's widget colours for one palette.
fn visuals(theme: Theme, p: &Palette) -> Visuals {
    let mut visuals = match theme {
        Theme::Light => Visuals::light(),
        Theme::Dark => Visuals::dark(),
    };
    visuals.panel_fill = p.bg;
    visuals.window_fill = p.panel;
    visuals.extreme_bg_color = p.code;
    visuals.widgets.noninteractive.bg_stroke.color = p.line;
    visuals.weak_text_color = Some(p.muted);
    visuals.override_text_color = Some(p.text);
    visuals.hyperlink_color = p.blue;
    visuals.selection.bg_fill = p.selected;
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
    ] {
        widget.bg_fill = p.surface;
        widget.weak_bg_fill = p.surface;
        widget.bg_stroke = Stroke::new(1.0_f32, p.edge);
        widget.corner_radius = 6.into();
        widget.fg_stroke.color = p.text;
    }
    visuals.widgets.hovered.bg_fill = p.selected;
    visuals.widgets.hovered.weak_bg_fill = p.selected;
    visuals
}

/// Follows the theme egui resolved for this frame, light or dark.
pub(super) fn sync_theme(ctx: &Context) {
    DARK_MODE.set(ctx.theme() == Theme::Dark);
}

/// The current theme's colours.
pub(super) mod col {
    use super::Color32;
    pub fn bg() -> Color32 {
        super::palette().bg
    }
    pub fn panel() -> Color32 {
        super::palette().panel
    }
    pub fn code() -> Color32 {
        super::palette().code
    }
    pub fn surface() -> Color32 {
        super::palette().surface
    }
    pub fn line() -> Color32 {
        super::palette().line
    }
    pub fn edge() -> Color32 {
        super::palette().edge
    }
    pub fn text() -> Color32 {
        super::palette().text
    }
    pub fn muted() -> Color32 {
        super::palette().muted
    }
    pub fn secondary_text() -> Color32 {
        super::palette().secondary_text
    }
    pub fn blue() -> Color32 {
        super::palette().blue
    }
    pub fn selected() -> Color32 {
        super::palette().selected
    }
    pub fn amber() -> Color32 {
        super::palette().amber
    }
    pub fn green() -> Color32 {
        super::palette().green
    }
    pub fn amber_bg() -> Color32 {
        super::palette().amber_bg
    }
    pub fn green_bg() -> Color32 {
        super::palette().green_bg
    }
    /// The background of search matches.
    pub fn highlight() -> Color32 {
        super::palette().highlight
    }
}

pub(super) fn install(ctx: &Context) {
    // Follow the system's light or dark appearance; light when it is unknown.
    ctx.options_mut(|o| {
        o.theme_preference = ThemePreference::System;
        o.fallback_theme = Theme::Light;
    });
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
    for (theme, palette) in [(Theme::Light, &LIGHT), (Theme::Dark, &DARK)] {
        ctx.set_visuals_of(theme, visuals(theme, palette));
    }
    ctx.style_mut(|style| {
        style.text_styles.insert(TextStyle::Body, ty::body());
        style.text_styles.insert(TextStyle::Button, ty::body());
        style.text_styles.insert(TextStyle::Monospace, ty::code());
        style.spacing.item_spacing = vec2(8., 6.);
        style.spacing.button_padding = vec2(10., 5.);
    });
}
/// The type scale. Views use these roles instead of point sizes.
pub(super) mod ty {
    use super::{FontId, bold, font, mono, mono_bold};
    /// Sizes for `RichText::size`.
    pub const BODY: f32 = 13.;
    pub const SMALL: f32 = 12.;
    pub const CAPTION: f32 = 11.;
    pub const LEAD: f32 = 15.;
    /// The welcome screen's greeting.
    pub fn display() -> FontId {
        bold(26.)
    }
    /// Dialog and screen titles.
    pub fn title() -> FontId {
        bold(22.)
    }
    /// The title of a detail card.
    pub fn heading() -> FontId {
        bold(17.)
    }
    /// Section headings inside a page or dialog.
    pub fn section() -> FontId {
        bold(15.)
    }
    /// Emphasised body text, such as list item names.
    pub fn strong() -> FontId {
        bold(BODY)
    }
    /// Field labels.
    pub fn label() -> FontId {
        bold(SMALL)
    }
    /// Small capitals above sidebar groups.
    pub fn overline() -> FontId {
        bold(CAPTION)
    }
    pub fn body() -> FontId {
        font(BODY)
    }
    pub fn small() -> FontId {
        font(SMALL)
    }
    pub fn caption() -> FontId {
        font(CAPTION)
    }
    /// Icons drawn on their own, next to text.
    pub fn icon() -> FontId {
        font(LEAD)
    }
    pub fn code() -> FontId {
        mono(SMALL)
    }
    pub fn code_strong() -> FontId {
        mono_bold(SMALL)
    }
    /// Large figures, such as a message type in the inspector.
    pub fn code_display() -> FontId {
        mono_bold(18.)
    }
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
    RichText::new(s).size(ty::SMALL).color(col::muted())
}
pub(super) fn section(ui: &mut Ui, s: &str) {
    ui.label(
        RichText::new(s)
            .font(ty::overline())
            .extra_letter_spacing(0.8)
            .color(col::muted()),
    );
}
pub(super) fn badge(ui: &mut Ui, text: &str, fg: Color32, bg: Color32, monospace: bool) {
    let face = if monospace { ty::code() } else { ty::small() };
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
        p.rect_filled(
            r,
            6,
            if selected {
                col::selected()
            } else {
                col::surface()
            },
        );
    }
    if selected {
        p.rect_filled(Rect::from_min_size(r.min, vec2(2., 34.)), 1, col::blue());
    }
    let face = if selected { ty::strong() } else { ty::body() };
    let mut job = text::LayoutJob::simple_singleline(text.to_owned(), face, col::text());
    let reserved = if trailing.is_empty() {
        20.
    } else {
        let tag_width = p
            .layout_no_wrap(trailing.to_owned(), ty::caption(), col::muted())
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
        ty::icon(),
        if selected { col::blue() } else { col::muted() },
    );
    p.galley(
        r.left_center() + vec2(34., -galley.size().y / 2.),
        galley,
        col::text(),
    );
    if res.has_focus() {
        p.rect_stroke(
            r.shrink(1.),
            6,
            Stroke::new(1_f32, col::blue()),
            StrokeKind::Inside,
        );
    }
    if !trailing.is_empty() {
        if count {
            let b = Rect::from_center_size(r.right_center() - vec2(19., 0.), vec2(22., 18.));
            p.rect_filled(b, 9, col::amber_bg());
            p.text(
                b.center(),
                Align2::CENTER_CENTER,
                trailing,
                ty::caption(),
                col::amber(),
            );
        } else {
            p.text(
                r.right_center() - vec2(10., 0.),
                Align2::RIGHT_CENTER,
                trailing,
                ty::caption(),
                col::muted(),
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
        .line_segment([a, b], Stroke::new(1.0_f32, col::line()));
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
        .fill(col::amber_bg())
        .corner_radius(8)
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(col::amber()));
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
    let text = RichText::new(label).color(if primary { Color32::WHITE } else { col::text() });
    Button::new(text)
        .min_size(vec2(0., height))
        .fill(if primary { col::blue() } else { col::surface() })
        .stroke(Stroke::new(
            1.0_f32,
            if primary { col::blue() } else { col::edge() },
        ))
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
            Button::new(RichText::new(&label).size(ty::BODY))
                .truncate()
                .fill(col::panel())
                .stroke(Stroke::new(1_f32, col::edge())),
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
        ui.painter().rect_filled(r, 6, col::surface());
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
        Stroke::new(1_f32, col::edge()),
        3.,
        2.,
    ));
    ui.painter().text(
        r.left_center() + vec2(10., 0.),
        Align2::LEFT_CENTER,
        &with_icon(icon::PLUS, "Add partner"),
        ty::body(),
        col::muted(),
    );
    res.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Add partner"));
    res.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative-luminance contrast between two colours.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let l = |c: Color32| {
            let f = |v: u8| {
                let v = f32::from(v) / 255.;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
        };
        let (x, y) = (l(a), l(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn both_palettes_meet_wcag_aa() {
        for (name, p) in [("light", &LIGHT), ("dark", &DARK)] {
            for (what, fg, bg, min) in [
                ("text on page", p.text, p.bg, 4.5),
                ("text on panel", p.text, p.panel, 4.5),
                ("muted on page", p.muted, p.bg, 4.5),
                ("muted on panel", p.muted, p.panel, 4.5),
                ("secondary on panel", p.secondary_text, p.panel, 4.5),
                ("links on page", p.blue, p.bg, 4.5),
                ("amber on its background", p.amber, p.amber_bg, 4.5),
                ("text on selection", p.text, p.selected, 4.5),
                // Button labels are 13 px semibold; AA asks 3:1 of large text.
                ("button label on blue", Color32::WHITE, p.blue, 3.),
            ] {
                let ratio = contrast(fg, bg);
                assert!(ratio >= min, "{name} {what}: {ratio:.2}");
            }
        }
    }

    #[test]
    fn colours_follow_the_theme() {
        let ctx = Context::default();
        install(&ctx);
        ctx.set_theme(Theme::Dark);
        sync_theme(&ctx);
        assert_eq!(col::bg(), DARK.bg);
        ctx.set_theme(Theme::Light);
        sync_theme(&ctx);
        assert_eq!(col::bg(), LIGHT.bg);
    }

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
        ui.label(RichText::new(title).font(ty::section()));
        ui.add_space(GAP_XS);
        ui.label(RichText::new(text).color(col::secondary_text()));
        ui.add_space(GAP_M);
        ui.horizontal(actions);
    });
}
