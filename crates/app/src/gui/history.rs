use super::*;
use vendredi_core::engine::HistoryPeriod;

pub(super) struct Cleanup {
    period: HistoryPeriod,
    partner: Option<String>,
}

impl Desktop {
    pub(super) fn open_cleanup(&mut self) {
        self.cleanup = Some(Cleanup {
            period: HistoryPeriod::Minute,
            partner: match self.view {
                View::Partner(i) => Some(self.workspace.partners[i].name.clone()),
                _ => None,
            },
        });
    }

    pub(super) fn cleanup_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut cleanup) = self.cleanup.take() else {
            return;
        };
        let mut close = false;
        form::modal(ctx, "clear history", "Clear traffic history?", 440., |ui| {
            form::label(ui, "Partner");
            let current: BTreeSet<_> = self
                .workspace
                .partners
                .iter()
                .map(|p| p.name.as_str())
                .collect();
            let names: BTreeSet<_> = current
                .iter()
                .copied()
                .chain(self.transfers.values().map(|t| t.partner.as_str()))
                .collect();
            egui::ComboBox::from_id_salt("history partner")
                .width(ui.available_width())
                .selected_text(cleanup.partner.as_deref().unwrap_or("All partners"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut cleanup.partner, None, "All partners");
                    for name in names {
                        let label = if current.contains(name) {
                            name.to_owned()
                        } else {
                            format!("{name} (removed)")
                        };
                        ui.selectable_value(&mut cleanup.partner, Some(name.to_owned()), label);
                    }
                });
            form::label(ui, "Time period");
            ui.horizontal_wrapped(|ui| {
                for (period, label) in [
                    (HistoryPeriod::Minute, "Last minute"),
                    (HistoryPeriod::Hour, "Last hour"),
                    (HistoryPeriod::Day, "Last day"),
                    (HistoryPeriod::Week, "Last week"),
                ] {
                    ui.radio_value(&mut cleanup.period, period, label);
                }
            });
            ui.label("Permanently delete whole exchanges whose last transfer falls in this period. Exchanges with pending deliveries or scheduled actions are skipped.");
            form::footer(ui, |ui| {
                if ui.add(form::removal_button("Clear history")).clicked() {
                    self.command(Command::ClearHistory(
                        cleanup.period,
                        cleanup.partner.take(),
                    ));
                    close = true;
                }
                close |= button(ui, "Cancel", false).clicked();
            });
        });
        if !close {
            self.cleanup = Some(cleanup);
        }
    }
}
