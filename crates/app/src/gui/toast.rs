//! Short confirmations and errors in the bottom-right corner, with Undo for
//! actions that remove something.
use super::*;
use egui::{Align2, Area, Order, RichText};

/// How to take back a removal.
pub(super) enum Undo {
    /// Moves a folder back from where it was set aside.
    Folder { from: PathBuf, to: PathBuf },
    /// Writes files back as they were.
    Files(Vec<(PathBuf, Vec<u8>)>),
}

pub(super) struct Toast {
    /// Unique for the session, so each toast has its own layer.
    id: u64,
    text: String,
    error: bool,
    /// `egui` time when it appeared.
    since: f64,
    undo: Option<Undo>,
}

/// Seconds a toast stays: errors and undoable actions stay longer.
fn lifetime(t: &Toast) -> f64 {
    if t.error || t.undo.is_some() { 8. } else { 4. }
}

const WIDTH: f32 = 360.;
/// The height of the window footer.
const FOOTER: f32 = 30.;

impl Desktop {
    /// Reports a failure: shown as an error toast and kept in `status`.
    pub(super) fn fail(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.last_error.clone_from(&self.status);
        self.status_error = true;
    }

    /// Shows a confirmation that can be undone.
    pub(super) fn undoable(&mut self, text: impl Into<String>, undo: Undo) {
        self.status = text.into();
        self.pending_undo = Some(undo);
    }

    fn take_back(&mut self, undo: Undo) {
        let result = (|| -> Result<()> {
            match undo {
                Undo::Folder { from, to } => fs::rename(from, to)?,
                Undo::Files(files) => {
                    for (path, bytes) in files {
                        fs::write(path, bytes)?;
                    }
                }
            }
            self.apply_files(&[])?;
            self.config_source = fs::read_to_string(self.root.join("vendredi.toml"))?;
            Ok(())
        })();
        match result {
            Ok(()) => self.status = "Undone".into(),
            Err(e) => self.fail(format!("Undo failed: {e:#}")),
        }
    }

    pub(super) fn toasts(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if !self.status.is_empty() && self.status != self.toasted {
            self.toasted.clone_from(&self.status);
            self.toast_count += 1;
            self.toast_list.push(Toast {
                id: self.toast_count,
                text: self.status.clone(),
                error: std::mem::take(&mut self.status_error),
                since: now,
                undo: self.pending_undo.take(),
            });
            // Older confirmations give way; at most three are visible.
            if self.toast_list.len() > 3 {
                self.toast_list.remove(0);
            }
        }
        // A toast above a dialog would take its clicks, so toasts wait,
        // with their clocks paused, until the dialog closes.
        if self.dialog_open() {
            let dt = ctx.input(|i| i.stable_dt) as f64;
            for t in &mut self.toast_list {
                t.since += dt;
            }
            return;
        }
        self.toast_list.retain(|t| now - t.since < lifetime(t));
        let mut undo = None;
        let mut close = None;
        let count = self.toast_list.len();
        for (i, t) in self.toast_list.iter().enumerate().rev() {
            let age = now - t.since;
            // Fade in over 150 ms and out over the last 400 ms.
            let opacity = (age / 0.15).min((lifetime(t) - age) / 0.4).clamp(0., 1.) as f32;
            // The newest toast is nearest the corner.
            let offset = (count - 1 - i) as f32 * 64.;
            Area::new(egui::Id::new(("toast", t.id)))
                .order(Order::Foreground)
                .default_size(egui::vec2(WIDTH, 44.))
                // Bottom right, above the footer, clear of page actions.
                .anchor(
                    Align2::RIGHT_BOTTOM,
                    egui::vec2(-GAP_L, -(FOOTER + GAP_M + offset)),
                )
                // Plain toasts let clicks through to the page beneath; only
                // an Undo needs to be clicked.
                .interactable(t.undo.is_some())
                .show(ctx, |ui| {
                    ui.set_opacity(opacity);
                    let (fill, stroke, color) = if t.error {
                        (col::amber_bg(), col::amber(), col::amber())
                    } else {
                        (col::text(), col::text(), col::bg())
                    };
                    egui::Frame::new()
                        .fill(fill)
                        .stroke(egui::Stroke::new(1_f32, stroke))
                        .corner_radius(10)
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.set_max_width(WIDTH);
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(&t.text).color(color))
                                        .wrap()
                                        .selectable(false),
                                );
                                if t.undo.is_some()
                                    && ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("Undo").color(color).strong(),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                {
                                    undo = Some(i);
                                }
                                // Plain toasts go away on their own.
                                if t.undo.is_some()
                                    && ui
                                        .add(
                                            egui::Button::new(icon_job(icon::X, ty::body(), color))
                                                .frame(false),
                                        )
                                        .on_hover_text("Dismiss")
                                        .clicked()
                                {
                                    close = Some(i);
                                }
                            });
                        });
                });
        }
        if let Some(i) = undo {
            let t = self.toast_list.remove(i);
            if let Some(u) = t.undo {
                self.take_back(u);
            }
        } else if let Some(i) = close {
            self.toast_list.remove(i);
        }
        if !self.toast_list.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}
