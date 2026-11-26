//! The macOS menu bar. Its items carry the same shortcuts as the window,
//! and AppKit delivers them here instead of as key presses.
use super::shortcuts::Do;
use super::*;
use muda::{
    AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu, accelerator::Accelerator,
};

const ITEMS: &[(&str, &str, Option<&str>)] = &[
    ("new", "New Case or Partner", Some("CmdOrCtrl+N")),
    ("new-partner", "New Partner…", Some("CmdOrCtrl+Shift+N")),
    ("send", "Send Document…", Some("CmdOrCtrl+Shift+S")),
    ("import", "Import Cases…", Some("CmdOrCtrl+I")),
    ("save", "Save", Some("CmdOrCtrl+S")),
    ("reveal", "Show Workspace in Finder", None),
    ("new-workspace", "New Workspace…", None),
    ("open-workspace", "Open Workspace…", Some("CmdOrCtrl+O")),
    ("close-workspace", "Close Workspace", None),
    ("all", "All Traffic", Some("CmdOrCtrl+1")),
    ("attention", "Needs Attention", Some("CmdOrCtrl+2")),
    ("cases", "Cases", Some("CmdOrCtrl+3")),
    ("scripts", "Scripts", Some("CmdOrCtrl+4")),
    ("settings", "Settings", Some("CmdOrCtrl+5")),
    ("preferences", "Settings…", Some("CmdOrCtrl+Comma")),
    ("find", "Search Traffic", Some("CmdOrCtrl+F")),
    ("guide", "Vendredi Guide", None),
    ("whats-new", "What's New", None),
    ("report", "Report a Problem…", None),
];

fn item(id: &str) -> MenuItem {
    let (_, text, key) = ITEMS.iter().find(|(i, ..)| *i == id).expect("menu item");
    let accelerator = key.and_then(|k| k.parse::<Accelerator>().ok());
    MenuItem::with_id(id, text, true, accelerator)
}

/// The command a menu item stands for.
pub(super) fn command(id: &str) -> Option<Do> {
    Some(match id {
        "new" => Do::New,
        "new-partner" => Do::NewPartner,
        "send" => Do::Send,
        "import" => Do::Import,
        "save" => Do::Save,
        "reveal" => Do::Reveal,
        "new-workspace" => Do::NewWorkspace,
        "open-workspace" => Do::OpenWorkspace,
        "close-workspace" => Do::CloseWorkspace,
        "all" => Do::Page(View::All),
        "attention" => Do::Page(View::Attention),
        "cases" => Do::Page(View::Cases),
        "scripts" => Do::Page(View::Scripts),
        "settings" | "preferences" => Do::Page(View::Settings),
        "find" => Do::Search,
        "guide" => Do::Guide,
        "whats-new" => Do::WhatsNew,
        "report" => Do::Report,
        _ => return None,
    })
}

/// Builds the menu bar and wakes the window when an item is chosen. The
/// returned menu must live as long as the app.
pub(super) fn install(ctx: &egui::Context) -> anyhow::Result<Menu> {
    let about = AboutMetadata {
        name: Some("Vendredi".into()),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        ..Default::default()
    };
    let app = Submenu::with_items(
        "Vendredi",
        true,
        &[
            &PredefinedMenuItem::about(Some("About Vendredi"), Some(about)),
            &PredefinedMenuItem::separator(),
            &item("preferences"),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::services(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(Some("Hide Vendredi")),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(Some("Quit Vendredi")),
        ],
    )?;
    let file = Submenu::with_items(
        "File",
        true,
        &[
            &item("new-workspace"),
            &item("open-workspace"),
            &item("close-workspace"),
            &PredefinedMenuItem::separator(),
            &item("new"),
            &item("new-partner"),
            &PredefinedMenuItem::separator(),
            &item("send"),
            &item("import"),
            &PredefinedMenuItem::separator(),
            &item("save"),
            &item("reveal"),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::close_window(None),
        ],
    )?;
    let view = Submenu::with_items(
        "View",
        true,
        &[
            &item("all"),
            &item("attention"),
            &item("cases"),
            &item("scripts"),
            &item("settings"),
            &PredefinedMenuItem::separator(),
            &item("find"),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::fullscreen(None),
        ],
    )?;
    let window = Submenu::with_items(
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(None),
            &PredefinedMenuItem::maximize(None),
        ],
    )?;
    let help = Submenu::with_items(
        "Help",
        true,
        &[
            &item("guide"),
            &item("whats-new"),
            &PredefinedMenuItem::separator(),
            &item("report"),
        ],
    )?;
    // The app menu names Vendredi itself; by default AppKit uses the
    // executable's name, which is lowercase outside the app bundle.
    let menu = Menu::with_items(&[&app, &file, &view, &window, &help])?;
    menu.init_for_nsapp();
    window.set_as_windows_menu_for_nsapp();
    help.set_as_help_menu_for_nsapp();
    let ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(Arc::new(move |event: MenuEvent| {
        let _ = MENU_EVENTS.0.lock().map(|tx| tx.send(event.id.0));
        ctx.request_repaint();
    })));
    Ok(menu)
}

type Channel = (
    std::sync::Mutex<mpsc::Sender<String>>,
    std::sync::Mutex<mpsc::Receiver<String>>,
);

/// The menu bar as text, one item per line, for checking the installed
/// menu of a running app (`VENDREDI_MENU_DUMP`).
pub(super) fn describe(menu: &Menu) -> String {
    use muda::MenuItemKind;
    fn walk(items: Vec<MenuItemKind>, depth: usize, out: &mut String) {
        for item in items {
            let text = match &item {
                MenuItemKind::MenuItem(i) => {
                    // muda has no accelerator getter; the table holds them.
                    let key = ITEMS
                        .iter()
                        .find(|(id, ..)| *id == i.id().0)
                        .and_then(|(_, _, k)| *k)
                        .map(|k| format!("  [{k}]"))
                        .unwrap_or_default();
                    format!("{}{key}", i.text())
                }
                MenuItemKind::Submenu(s) => s.text(),
                MenuItemKind::Predefined(p) => {
                    let t = p.text();
                    if t.is_empty() { "—".into() } else { t }
                }
                MenuItemKind::Check(c) => c.text(),
                MenuItemKind::Icon(i) => i.text(),
            };
            out.push_str(&format!("{}{text}\n", "  ".repeat(depth)));
            if let MenuItemKind::Submenu(s) = &item {
                walk(s.items(), depth + 1, out);
            }
        }
    }
    let mut out = String::new();
    walk(menu.items(), 0, &mut out);
    out
}

/// Menu choices waiting for the next frame.
static MENU_EVENTS: std::sync::LazyLock<Channel> = std::sync::LazyLock::new(|| {
    let (tx, rx) = mpsc::channel();
    (std::sync::Mutex::new(tx), std::sync::Mutex::new(rx))
});

pub(super) fn pending() -> Vec<Do> {
    MENU_EVENTS
        .1
        .lock()
        .map(|rx| rx.try_iter().filter_map(|id| command(&id)).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_item_has_a_command() {
        for (id, ..) in ITEMS {
            assert!(command(id).is_some(), "{id}");
        }
    }
}
