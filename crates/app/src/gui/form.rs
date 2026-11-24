use super::theme::*;
use eframe::egui::*;

pub(super) fn modal(
    ctx: &Context,
    id: &'static str,
    title: &str,
    width: f32,
    contents: impl FnOnce(&mut Ui),
) {
    Modal::new(id.into())
        .backdrop_color(Color32::from_rgba_unmultiplied(17, 32, 52, 82))
        .frame(
            Frame::popup(&ctx.style())
                .fill(BG)
                .stroke(Stroke::new(1_f32, LINE))
                .corner_radius(12)
                .inner_margin(Margin::same(22)),
        )
        .show(ctx, |ui| {
            ui.set_width(width.min(ctx.content_rect().width() - 76.));
            ui.spacing_mut().item_spacing = vec2(10., 8.);
            ui.spacing_mut().interact_size.y = 30.;
            ui.visuals_mut().extreme_bg_color = BG;
            ui.visuals_mut().widgets.inactive.bg_stroke.color = EDGE;
            ui.visuals_mut().widgets.hovered.bg_stroke.color = BLUE;
            ui.visuals_mut().widgets.active.bg_stroke.color = BLUE;
            ui.label(RichText::new(title).font(bold(22.)));
            ui.add_space(4.);
            contents(ui);
        });
}

pub(super) fn label(ui: &mut Ui, text: &str) -> Response {
    ui.label(RichText::new(text).font(bold(12.)).color(SECONDARY_TEXT))
}

pub(super) fn input(ui: &mut Ui, caption: &str, value: &mut String, hint: &str) -> Response {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 5.;
        let name = label(ui, caption);
        ui.add(
            TextEdit::singleline(value)
                .hint_text(RichText::new(hint).color(MUTED))
                .background_color(BG)
                .margin(Margin::symmetric(10, 7))
                .desired_width(f32::INFINITY)
                .min_size(vec2(0., 33.)),
        )
        .labelled_by(name.id)
    })
    .inner
}

pub(super) fn field(ui: &mut Ui, caption: &str, value: &mut String) {
    input(ui, caption, value, "");
}

pub(super) fn card(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1_f32, LINE))
        .corner_radius(8)
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            contents(ui);
        });
}

pub(super) fn footer(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    ui.add_space(2.);
    ui.separator();
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        contents(ui);
    });
}

pub(super) fn removal_button(label: &str) -> Button<'_> {
    Button::new(RichText::new(label).color(Color32::WHITE)).fill(Color32::from_rgb(173, 40, 42))
}
