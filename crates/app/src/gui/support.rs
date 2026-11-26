//! Help: what changed in this version, and reporting a problem with the
//! details already filled in, shown in full before anything is sent.
use super::*;

const ISSUES: &str = "https://github.com/agilevisionlabs/vendredi/issues/new";
const RELEASES: &str = "https://github.com/agilevisionlabs/vendredi/releases/tag/v";
/// The storage key for the version that last ran.
pub(super) const LAST_VERSION: &str = "last version";

pub(super) fn release_notes_url() -> String {
    format!("{RELEASES}{}", env!("CARGO_PKG_VERSION"))
}

/// A note for the first launch of a new version, if it is one.
pub(super) fn update_note(last: Option<&str>) -> Option<String> {
    let current = env!("CARGO_PKG_VERSION");
    match last {
        Some(previous) if previous != current => Some(format!(
            "Vendredi is updated to {current}. Help → What's New lists the changes."
        )),
        _ => None,
    }
}

/// Percent-encodes text for a URL query.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

impl Desktop {
    /// The report to send, which the dialog lets the reader change first.
    fn report_text(&self) -> String {
        let last = if self.last_error.is_empty() {
            "(none)"
        } else {
            &self.last_error
        };
        format!(
            "What happened:\n\n\nWhat I expected:\n\n\n---\nVendredi {}\n{} {}\nLast error: {last}\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
    }

    pub(super) fn open_report(&mut self) {
        self.report = Some(self.report_text());
    }

    pub(super) fn report_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut report) = self.report.take() else {
            return;
        };
        let mut close = false;
        let escaped = form::modal(ctx, "report a problem", "Report a problem", 560., |ui| {
            ui.label(muted(
                "This is everything the report contains. Change or remove anything before sending it.",
            ));
            ui.add_space(GAP_XS);
            ui.add(
                egui::TextEdit::multiline(&mut report)
                    .desired_rows(12)
                    .desired_width(f32::INFINITY),
            );
            form::footer(ui, |ui| {
                if button(ui, "Open a GitHub issue", true)
                    .on_hover_text("Opens the issue form in your browser, filled in with this text")
                    .clicked()
                {
                    let url = format!(
                        "{ISSUES}?title={}&body={}",
                        encode("Problem report"),
                        encode(&report)
                    );
                    shortcuts::open_externally(&url, false);
                    close = true;
                }
                if button(ui, "Copy report", false).clicked() {
                    ui.ctx().copy_text(report.clone());
                    self.status = "Report copied".into();
                }
                if button(ui, "Cancel", false).clicked() {
                    close = true;
                }
            });
        });
        if !close && !escaped {
            self.report = Some(report);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests::click;

    #[test]
    #[ignore = "opens a local workspace listener"]
    fn a_problem_report_is_shown_in_full_first() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let file = dir.path().join("vendredi.toml");
        fs::write(&file, fs::read_to_string(&file)?.replace(":8180", ":0"))?;
        let ctx = egui::Context::default();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("test screen size"), egui::vec2(1440., 900.)));
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        app.fail("Something broke");
        click(&mut app, &ctx, "Settings")?;
        click(
            &mut app,
            &ctx,
            &with_icon(icon::WARNING_CIRCLE, "Report a problem"),
        )?;
        let report = app.report.clone().expect("report dialog");
        assert!(report.contains(env!("CARGO_PKG_VERSION")));
        assert!(report.contains("Last error: Something broke"));
        // The workspace path is personal, so it is not in the report.
        assert!(!report.contains(&dir.path().display().to_string()));
        click(&mut app, &ctx, "Copy report")?;
        assert_eq!(app.status, "Report copied");
        click(&mut app, &ctx, "Cancel")?;
        assert!(app.report.is_none());
        Ok(())
    }

    #[test]
    fn reports_and_update_notes() {
        assert_eq!(encode("a b&c=é"), "a%20b%26c%3D%C3%A9");
        assert!(update_note(None).is_none());
        assert!(update_note(Some(env!("CARGO_PKG_VERSION"))).is_none());
        assert!(
            update_note(Some("0.0.1"))
                .unwrap()
                .contains(env!("CARGO_PKG_VERSION"))
        );
        assert!(release_notes_url().ends_with(env!("CARGO_PKG_VERSION")));
    }
}
