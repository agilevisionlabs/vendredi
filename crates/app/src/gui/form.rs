use super::theme::*;
use eframe::egui::*;
use std::path::{Path, PathBuf};

pub(super) fn modal(
    ctx: &Context,
    id: &'static str,
    title: &str,
    width: f32,
    contents: impl FnOnce(&mut Ui),
) -> bool {
    let response = Modal::new(id.into())
        .backdrop_color(Color32::from_rgba_unmultiplied(17, 32, 52, 82))
        .frame(
            Frame::popup(&ctx.style())
                .fill(col::bg())
                .stroke(Stroke::new(1_f32, col::line()))
                .corner_radius(12)
                .inner_margin(Margin::same(22)),
        )
        .show(ctx, |ui| {
            ui.set_width(width.min(ctx.content_rect().width() - 76.));
            ui.spacing_mut().item_spacing = vec2(10., 8.);
            ui.spacing_mut().interact_size.y = 30.;
            ui.visuals_mut().extreme_bg_color = col::bg();
            ui.visuals_mut().widgets.inactive.bg_stroke.color = col::edge();
            ui.visuals_mut().widgets.hovered.bg_stroke.color = col::blue();
            ui.visuals_mut().widgets.active.bg_stroke.color = col::blue();
            ui.label(RichText::new(title).font(ty::title()));
            ui.add_space(GAP_XS);
            contents(ui);
        });
    response.is_top_modal
        && !response.any_popup_open
        && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
}

pub(super) fn label(ui: &mut Ui, text: &str) -> Response {
    ui.label(
        RichText::new(text)
            .font(ty::label())
            .color(col::secondary_text()),
    )
}

pub(super) fn input(ui: &mut Ui, caption: &str, value: &mut String, hint: &str) -> Response {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 5.;
        let name = label(ui, caption);
        ui.add(
            TextEdit::singleline(value)
                .hint_text(RichText::new(hint).color(col::muted()))
                .background_color(col::bg())
                .margin(Margin::symmetric(10, 7))
                .desired_width(f32::INFINITY)
                .min_size(vec2(0., BUTTON)),
        )
        .labelled_by(name.id)
    })
    .inner
}

pub(super) fn field(ui: &mut Ui, caption: &str, value: &mut String) {
    input(ui, caption, value, "");
}

pub(super) fn card(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    card_frame(col::panel(), col::line()).show(ui, |ui| {
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

/// A destructive action, the same height as the buttons beside it.
pub(super) fn removal_button(label: &str) -> Button<'_> {
    let red = Color32::from_rgb(173, 40, 42);
    Button::new(RichText::new(label).color(Color32::WHITE))
        .min_size(vec2(0., BUTTON))
        .fill(red)
        .stroke(Stroke::new(1.0_f32, red))
}

/// A text field for a path with a **Choose…** button on the same line.
/// `choose` opens the native dialog and returns the text to put in the field.
pub(super) fn path_input(
    ui: &mut Ui,
    caption: &str,
    value: &mut String,
    hint: &str,
    choose: impl FnOnce() -> Option<String>,
) -> bool {
    let mut changed = false;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 5.;
        let name = label(ui, caption);
        ui.horizontal(|ui| {
            let button_width = 92.;
            let field = ui
                .add(
                    TextEdit::singleline(value)
                        .hint_text(RichText::new(hint).color(col::muted()))
                        .background_color(col::bg())
                        .margin(Margin::symmetric(10, 7))
                        .desired_width(ui.available_width() - button_width - GAP_S)
                        .min_size(vec2(0., BUTTON)),
                )
                .labelled_by(name.id);
            changed |= field.changed();
            if ui
                .add_sized([button_width, BUTTON], styled("Choose…", false, BUTTON))
                .clicked()
                && let Some(picked) = choose()
            {
                *value = picked;
                changed = true;
            }
        });
    });
    changed
}

/// A file chosen in the native dialog, starting in `dir`.
pub(super) fn pick_file(dir: &Path, filters: &[(&str, &[&str])]) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_directory(dir);
    for (name, extensions) in filters {
        dialog = dialog.add_filter(*name, extensions);
    }
    dialog.pick_file()
}

pub(super) fn pick_folder(dir: &Path) -> Option<PathBuf> {
    rfd::FileDialog::new().set_directory(dir).pick_folder()
}

/// Files a case uses live in its folder. A file chosen elsewhere is copied
/// in, keeping its name, and the path inside the case is returned.
pub(super) fn into_folder(folder: &Path, chosen: &Path) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    if let Ok(inside) = chosen.strip_prefix(folder) {
        return Ok(inside.to_path_buf());
    }
    let name = chosen.file_name().context("choose a file")?;
    let target = folder.join(name);
    let bytes = std::fs::read(chosen)?;
    match std::fs::read(&target) {
        Ok(existing) if existing == bytes => {}
        Ok(_) => anyhow::bail!(
            "The case already has a different {}. Rename one of them first.",
            name.to_string_lossy()
        ),
        Err(_) => std::fs::write(&target, bytes)?,
    }
    Ok(PathBuf::from(name))
}
