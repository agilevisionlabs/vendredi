//! Keyboard shortcuts. They act only when no dialog is open, so typing in
//! a dialog never triggers a page action.
use super::*;
use egui::{Key, KeyboardShortcut, Modifiers};

pub(super) const SEARCH: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
pub(super) const NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
pub(super) const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
pub(super) const SEND: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
/// ⌘1 to ⌘5 open these pages.
const PAGES: [(Key, View); 5] = [
    (Key::Num1, View::All),
    (Key::Num2, View::Attention),
    (Key::Num3, View::Cases),
    (Key::Num4, View::Scripts),
    (Key::Num5, View::Settings),
];
pub(super) const SEARCH_ID: &str = "traffic search";

/// Something the keyboard or the menu bar asks for.
#[derive(Clone, PartialEq)]
pub(super) enum Do {
    Page(View),
    Search,
    New,
    NewPartner,
    Send,
    Save,
    Import,
    Reveal,
    Guide,
    OpenWorkspace,
    NewWorkspace,
    CloseWorkspace,
}

/// The handbook the Help menu and empty states link to.
pub(super) const GUIDE: &str = "https://vendredi.app/guide/";

/// Opens a URL or reveals a file with the system's handler.
pub(super) fn open_externally(target: &str, reveal: bool) {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        if reveal {
            c.arg("-R");
        }
        c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("explorer");
        if reveal {
            c.arg("/select,");
        }
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let _ = reveal;
        std::process::Command::new("xdg-open")
    };
    let _ = command.arg(target).spawn();
}

/// A label with its shortcut, such as `Search (⌘F)`, for hover text.
pub(super) fn hint(ctx: &egui::Context, label: &str, shortcut: &KeyboardShortcut) -> String {
    format!("{label} ({})", ctx.format_shortcut(shortcut))
}

impl Desktop {
    pub(super) fn dialog_open(&self) -> bool {
        self.send_dialog
            || self.partner_form.is_some()
            || self.cleanup.is_some()
            || self.script_form.is_some()
            || self.rule_form.is_some()
            || self.settings_form.is_some()
            || self.import.is_some()
            || self.case_form.is_some()
    }

    /// Carries out a keyboard or menu command. Commands wait while a dialog
    /// is open, except the ones that only open something outside the app.
    pub(super) fn perform(&mut self, action: Do) {
        if self.dialog_open() && !matches!(action, Do::Reveal | Do::Guide) {
            return;
        }
        match action {
            Do::Page(view) => self.navigate(view),
            Do::Search => {
                if !matches!(self.view, View::All | View::Attention | View::Partner(_)) {
                    self.navigate(View::All);
                }
                self.focus_search = true;
            }
            Do::New if self.view == View::Cases => {
                self.case_form = Some(cases::CaseForm::New {
                    name: String::new(),
                    error: String::new(),
                });
            }
            Do::New | Do::NewPartner => self.open_partner(None),
            Do::Send => self.open_send(),
            Do::Save => {
                if matches!(self.view, View::Documents | View::Config)
                    && self.edit != self.saved
                    && let Err(e) = self.save()
                {
                    self.file_error = Some(format!("{e:#}"));
                }
            }
            Do::Import => self.open_import(),
            Do::Reveal => {
                open_externally(&self.root.join("vendredi.toml").display().to_string(), true)
            }
            Do::Guide => open_externally(GUIDE, false),
            // The session switches workspaces; it handles these first.
            Do::OpenWorkspace | Do::NewWorkspace | Do::CloseWorkspace => {}
        }
    }

    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.dialog_open() {
            return;
        }
        let pressed = |s: &KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(s));
        for (key, view) in PAGES {
            if pressed(&KeyboardShortcut::new(Modifiers::COMMAND, key)) {
                self.perform(Do::Page(view));
            }
        }
        for (shortcut, action) in [
            (SEARCH, Do::Search),
            (NEW, Do::New),
            (SEND, Do::Send),
            (SAVE, Do::Save),
        ] {
            if pressed(&shortcut) {
                self.perform(action);
            }
        }
        // Arrow keys move through the list on the page, unless a text field
        // has the keyboard.
        if ctx.wants_keyboard_input() {
            return;
        }
        let step = ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                1
            } else if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                -1
            } else {
                0
            }
        });
        if step == 0 {
            return;
        }
        match self.view {
            View::All | View::Attention | View::Partner(_) => {
                let rows: Vec<i64> = self
                    .transfers
                    .values()
                    .rev()
                    .filter(|t| self.visible(t))
                    .map(|t| t.id)
                    .collect();
                if let Some(id) = neighbour(&rows, self.selected.as_ref(), step) {
                    self.select(id);
                }
            }
            View::Cases => {
                let ids: Vec<String> = self.workspace.cases.iter().map(|c| c.id.clone()).collect();
                if let Some(id) = neighbour(&ids, Some(&self.case), step) {
                    self.case = id;
                }
            }
            _ => {}
        }
    }
}

/// The item `step` places from `current` in `items`, staying at the ends.
fn neighbour<T: Clone + PartialEq>(items: &[T], current: Option<&T>, step: i32) -> Option<T> {
    let at = current.and_then(|c| items.iter().position(|i| i == c));
    let next = match at {
        Some(i) => (i as i32 + step).clamp(0, items.len() as i32 - 1) as usize,
        None => 0,
    };
    items.get(next).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests::frame;

    fn key(key: Key, modifiers: Modifiers) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }]
    }

    #[test]
    #[ignore = "opens a local workspace listener"]
    fn pages_lists_and_search_follow_the_keyboard() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let file = dir.path().join("vendredi.toml");
        fs::write(&file, fs::read_to_string(&file)?.replace(":8180", ":0"))?;
        let ctx = egui::Context::default();
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        frame(&mut app, &ctx, key(Key::Num3, Modifiers::COMMAND));
        assert!(app.view == View::Cases);
        frame(&mut app, &ctx, vec![]);
        let first = app.case.clone();
        frame(&mut app, &ctx, key(Key::ArrowDown, Modifiers::NONE));
        assert_ne!(app.case, first);
        frame(&mut app, &ctx, key(Key::ArrowUp, Modifiers::NONE));
        assert_eq!(app.case, first);
        frame(&mut app, &ctx, key(Key::N, Modifiers::COMMAND));
        assert!(app.case_form.is_some());
        // Shortcuts wait while a dialog is open.
        frame(&mut app, &ctx, key(Key::Num1, Modifiers::COMMAND));
        assert!(app.view == View::Cases);
        app.case_form = None;
        frame(&mut app, &ctx, key(Key::F, Modifiers::COMMAND));
        frame(&mut app, &ctx, vec![]);
        assert!(app.view == View::All);
        assert!(ctx.memory(|m| m.has_focus(egui::Id::new(SEARCH_ID))));
        frame(&mut app, &ctx, vec![egui::Event::Text("ORDERS".into())]);
        assert_eq!(app.search, "ORDERS");
        Ok(())
    }

    #[test]
    fn neighbours_stop_at_the_ends() {
        let items = [1, 2, 3];
        assert_eq!(neighbour(&items, Some(&2), 1), Some(3));
        assert_eq!(neighbour(&items, Some(&3), 1), Some(3));
        assert_eq!(neighbour(&items, Some(&1), -1), Some(1));
        assert_eq!(neighbour(&items, None, 1), Some(1));
        assert_eq!(neighbour::<i32>(&[], None, 1), None);
    }
}
