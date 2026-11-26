//! The getting-started strip on the traffic pages: four steps through a
//! first exchange that tick themselves off as they happen.
use super::*;
use egui::{RichText, Ui};

/// Written when the steps are done or hidden, so they stay away.
const DONE_FILE: &str = ".vendredi/getting-started-done";
/// The starter case the third step switches to.
const REJECT: &str = "reject-orders";

#[derive(Default)]
pub(super) struct Checklist {
    pub(super) hidden: bool,
    opened_reply: bool,
    switched: bool,
    /// Inbound transfers when the case was switched.
    inbound_at_switch: usize,
}

impl Checklist {
    pub(super) fn load(root: &Path) -> Self {
        Self {
            hidden: root.join(DONE_FILE).exists(),
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Sample,
    Reply,
    Switch,
    Again,
}

impl Desktop {
    fn inbound(&self) -> usize {
        self.transfers.values().filter(|t| !t.outgoing).count()
    }

    /// Records progress from what the user just did.
    pub(super) fn checklist_saw_selection(&mut self, id: i64) {
        if self.transfers.get(&id).is_some_and(|t| t.outgoing) {
            self.checklist.opened_reply = true;
        }
    }

    pub(super) fn checklist_saw_switch(&mut self) {
        if !self.checklist.switched {
            self.checklist.switched = true;
            self.checklist.inbound_at_switch = self.inbound();
        }
    }

    fn checklist_steps(&self) -> [(Step, &'static str, bool); 4] {
        let inbound = self.inbound();
        [
            (Step::Sample, "Send a sample order", inbound > 0),
            (Step::Reply, "Open the reply", self.checklist.opened_reply),
            (
                Step::Switch,
                "Make the partner reject orders",
                self.checklist.switched,
            ),
            (
                Step::Again,
                "Send the order again and compare",
                self.checklist.switched && inbound > self.checklist.inbound_at_switch,
            ),
        ]
    }

    fn hide_checklist(&mut self) {
        self.checklist.hidden = true;
        let path = self.root.join(DONE_FILE);
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(path, "");
    }

    fn take_step(&mut self, step: Step) {
        match step {
            Step::Sample | Step::Again => {
                if let Err(e) = self.sample(self.partner) {
                    self.fail(e.to_string());
                }
            }
            Step::Reply => {
                let reply = self
                    .transfers
                    .values()
                    .rev()
                    .find(|t| t.outgoing && t.kind != "CONTRL")
                    .or_else(|| self.transfers.values().rev().find(|t| t.outgoing))
                    .map(|t| t.id);
                if let Some(id) = reply {
                    self.select(id);
                }
            }
            Step::Switch => {
                let partner = self.workspace.partners[self.partner].name.clone();
                if self.workspace.cases.iter().any(|c| c.id == REJECT) {
                    self.command(Command::UseCase(partner, REJECT.into()));
                } else {
                    self.navigate(View::Cases);
                }
            }
        }
    }

    /// The strip under the traffic header, until the steps are done or hidden.
    pub(super) fn getting_started(&mut self, ui: &mut Ui) {
        if self.checklist.hidden {
            return;
        }
        let steps = self.checklist_steps();
        if steps.iter().all(|(.., done)| *done) {
            self.hide_checklist();
            self.status = "You have run a full exchange and a failure. Nicely done.".into();
            return;
        }
        let next = steps.iter().find(|(.., done)| !done).map(|(s, ..)| *s);
        let mut act = None;
        let mut hide = false;
        egui::Frame::new()
            .fill(col::selected())
            .inner_margin(egui::Margin::symmetric(PAGE_MARGIN, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                section_row(ui, |ui| {
                    ui.label(
                        RichText::new("Getting started")
                            .font(ty::label())
                            .color(col::blue()),
                    );
                    ui.add_space(GAP_S);
                    // A narrow window shows only the next step.
                    let narrow = ui.available_width() < 760.;
                    if narrow
                        && let Some((i, (step, label, _))) =
                            steps.iter().enumerate().find(|(_, (.., done))| !done)
                    {
                        ui.label(
                            RichText::new(format!("{} of {} · {label}", i + 1, steps.len()))
                                .size(ty::SMALL)
                                .color(col::secondary_text()),
                        );
                        if small_button(ui, "Go").clicked() {
                            act = Some(*step);
                        }
                    }
                    for (i, (step, label, done)) in steps.iter().enumerate().filter(|_| !narrow) {
                        let mark = if *done {
                            icon::CHECK_CIRCLE
                        } else {
                            icon::CIRCLE
                        };
                        let color = if *done {
                            col::green()
                        } else {
                            col::secondary_text()
                        };
                        ui.label(
                            RichText::new(format!("{mark} {label}"))
                                .size(ty::SMALL)
                                .color(color),
                        );
                        if Some(*step) == next {
                            let action = match step {
                                Step::Sample => "Send",
                                Step::Reply => "Open",
                                Step::Switch => "Switch",
                                Step::Again => "Send again",
                            };
                            if small_button(ui, action).clicked() {
                                act = Some(*step);
                            }
                        }
                        if i + 1 < steps.len() {
                            ui.label(muted("·"));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        hide = ui
                            .link("Hide")
                            .on_hover_text("Hide these steps for this workspace")
                            .clicked();
                    });
                });
            });
        if let Some(step) = act {
            self.take_step(step);
        }
        if hide {
            self.hide_checklist();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests::{click, wait_for};
    use anyhow::Result;

    #[test]
    #[ignore = "opens a local workspace listener"]
    fn the_first_exchange_steps_tick_themselves_off() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let file = dir.path().join("vendredi.toml");
        fs::write(&file, fs::read_to_string(&file)?.replace(":8180", ":0"))?;
        let ctx = egui::Context::default();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("test screen size"), egui::vec2(1440., 900.)));
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        assert!(!app.checklist.hidden);
        click(&mut app, &ctx, "Send")?;
        wait_for(&mut app, "sample replies", |a| {
            a.transfers.values().filter(|t| t.outgoing).count() >= 2
        })?;
        click(&mut app, &ctx, "Open")?;
        assert!(app.checklist.opened_reply);
        click(&mut app, &ctx, "Switch")?;
        wait_for(&mut app, "case switch", |a| a.checklist.switched)?;
        assert_eq!(app.workspace.partners[0].case_id(), Some("reject-orders"));
        click(&mut app, &ctx, "Send again")?;
        wait_for(&mut app, "second order", |a| a.inbound() >= 2)?;
        // The finished steps go away on the next frame, with a toast.
        crate::gui::tests::frame(&mut app, &ctx, vec![]);
        assert!(app.checklist.hidden);
        assert!(app.status.contains("full exchange"));
        assert!(dir.path().join(DONE_FILE).exists());
        // Reopening the workspace keeps the steps away.
        drop(app);
        let app = Desktop::open(dir.path().into(), &ctx)?;
        assert!(app.checklist.hidden);
        Ok(())
    }
}
