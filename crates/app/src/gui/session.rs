//! The window's workspace: the welcome screen before one is open, the
//! open workspace, or why it could not open, and switching between them.
use super::shortcuts::Do;
use super::*;
use egui::RichText;

/// The storage key for recently opened workspaces.
pub(super) const RECENT: &str = "recent workspaces";
const RECENT_LIMIT: usize = 6;

pub(super) fn parse_recent(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// What the open workspace asks the window to do next.
pub(super) enum Switch {
    Open(PathBuf),
    Welcome,
}

/// The demo workspace in the home folder.
pub(super) fn demo_root() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join("Vendredi"))
}

fn is_workspace(dir: &Path) -> bool {
    dir.join("vendredi.toml").is_file()
}

pub(super) struct Session {
    /// The workspace shown, or `None` on the welcome screen.
    pub(super) root: Option<PathBuf>,
    pub(super) desktop: Option<Result<Desktop>>,
    pub(super) recent: Vec<PathBuf>,
    /// Shown on the welcome screen after a failed create or open.
    error: String,
    /// Kept alive for as long as the menu bar shows it.
    #[cfg(target_os = "macos")]
    pub(super) _menu: Option<muda::Menu>,
}

impl Session {
    /// Opens `root`, or the last workspace, or the demo if it exists, or
    /// shows the welcome screen.
    pub(super) fn start(
        root: Option<PathBuf>,
        mut recent: Vec<PathBuf>,
        ctx: &egui::Context,
    ) -> Self {
        // The welcome screen needs the fonts before any workspace opens.
        install(ctx);
        // Workspaces that were moved or deleted drop off the list.
        recent.retain(|r| is_workspace(r));
        let root = root.or_else(|| {
            recent
                .iter()
                .cloned()
                .chain(demo_root())
                .find(|r| is_workspace(r))
        });
        let mut session = Self {
            root: None,
            desktop: None,
            recent,
            error: String::new(),
            #[cfg(target_os = "macos")]
            _menu: None,
        };
        if let Some(root) = root {
            session.open(root, ctx);
        }
        session
    }

    /// The welcome screen, without looking for a workspace.
    #[cfg(test)]
    pub(super) fn blank() -> Self {
        Self {
            root: None,
            desktop: None,
            recent: vec![],
            error: String::new(),
            #[cfg(target_os = "macos")]
            _menu: None,
        }
    }

    pub(super) fn open(&mut self, root: PathBuf, ctx: &egui::Context) {
        // Stop the previous engine first: it holds its workspace's lock.
        self.desktop = None;
        let desktop = Desktop::open(root.clone(), ctx);
        if desktop.is_ok() {
            self.recent.retain(|r| r != &root);
            self.recent.insert(0, root.clone());
            self.recent.truncate(RECENT_LIMIT);
        }
        self.root = Some(root);
        self.desktop = Some(desktop);
        self.error.clear();
    }

    /// Sets up a new workspace in `dir` and opens it, ready to describe the
    /// first partner.
    fn create(&mut self, dir: PathBuf, ctx: &egui::Context) {
        if is_workspace(&dir) {
            self.open(dir, ctx);
            return;
        }
        match crate::sample(&dir) {
            Ok(()) => {
                self.open(dir, ctx);
                if let Some(Ok(app)) = &mut self.desktop {
                    app.open_partner_setup(Some(0));
                }
            }
            Err(e) => self.error = format!("Setting up {}: {e:#}", dir.display()),
        }
    }

    fn welcome(&mut self) {
        self.desktop = None;
        self.root = None;
    }

    fn choose_folder(&self) -> Option<PathBuf> {
        let start = self
            .root
            .clone()
            .or_else(|| self.recent.first().cloned())
            .or_else(demo_root)
            .unwrap_or_default();
        form::pick_folder(&start)
    }

    fn perform(&mut self, action: Do, ctx: &egui::Context) {
        match action {
            Do::OpenWorkspace => {
                if let Some(dir) = self.choose_folder() {
                    if is_workspace(&dir) {
                        self.open(dir, ctx);
                    } else {
                        self.error = format!(
                            "{} has no vendredi.toml. Choose a workspace folder, or create a workspace there.",
                            dir.display()
                        );
                    }
                }
            }
            Do::NewWorkspace => {
                if let Some(dir) = self.choose_folder() {
                    self.create(dir, ctx);
                }
            }
            Do::CloseWorkspace => self.welcome(),
            other => {
                if let Some(Ok(app)) = &mut self.desktop {
                    app.perform(other);
                }
            }
        }
    }

    pub(super) fn show(&mut self, ctx: &egui::Context) {
        sync_theme(ctx);
        #[cfg(target_os = "macos")]
        for action in menu::pending() {
            self.perform(action, ctx);
        }
        let switch = match &mut self.desktop {
            Some(Ok(app)) => {
                app.show(ctx);
                app.switch.take()
            }
            Some(Err(_)) => self.failed(ctx),
            None => self.welcome_screen(ctx),
        };
        match switch {
            Some(Switch::Open(dir)) => self.open(dir, ctx),
            Some(Switch::Welcome) => self.welcome(),
            None => {}
        }
        // Keyboard shortcuts that change the workspace.
        let shortcut = |key| {
            ctx.input_mut(|i| {
                i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, key))
            })
        };
        if shortcut(egui::Key::O) {
            self.perform(Do::OpenWorkspace, ctx);
        }
    }

    /// The screen shown when a workspace cannot open.
    fn failed(&mut self, ctx: &egui::Context) -> Option<Switch> {
        let Some(Err(e)) = &self.desktop else {
            return None;
        };
        let root = self.root.clone().unwrap_or_default();
        let explained = explain::explain(e);
        let mut action = None;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(col::bg())
                    .inner_margin(GAP_XL as i8 * 2),
            )
            .show(ctx, |ui| {
                ui.set_max_width(640.);
                ui.label(RichText::new("Couldn’t open the workspace").font(ty::title()));
                ui.add_space(GAP_S);
                ui.label(RichText::new(&explained.text).size(ty::LEAD));
                ui.add_space(GAP_XS);
                ui.label(muted(root.display().to_string()).monospace());
                ui.add_space(GAP_L);
                ui.horizontal(|ui| {
                    if button(ui, "Try again", true).clicked() {
                        action = Some(Switch::Open(root.clone()));
                    }
                    if button(ui, "Show in Finder", false).clicked() {
                        shortcuts::open_externally(
                            &root.join("vendredi.toml").display().to_string(),
                            true,
                        );
                    }
                    if button(ui, "Open another workspace", false).clicked() {
                        action = Some(Switch::Welcome);
                    }
                });
                ui.add_space(GAP_L);
                egui::CollapsingHeader::new("Details")
                    .id_salt("open error details")
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(240.)
                            .show(ui, |ui| {
                                ui.label(RichText::new(&explained.details).monospace());
                            });
                    });
            });
        action
    }

    fn welcome_screen(&mut self, ctx: &egui::Context) -> Option<Switch> {
        let mut action = None;
        let mut pick = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(col::panel()))
            .show(ctx, |ui| {
                let width = 560.;
                let left = ((ui.available_width() - width) / 2.).max(GAP_XL);
                ui.add_space((ui.available_height() * 0.12).min(96.));
                ui.horizontal(|ui| {
                    ui.add_space(left);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        mark(ui, 72.);
                        ui.add_space(GAP_S);
                        ui.label(RichText::new("Welcome to Vendredi").font(ty::display()));
                        ui.add_space(GAP_XS);
                        ui.label(
                            RichText::new(
                                "Stand in for your trading partners. Receive EDIFACT and X12, answer with acknowledgements and replies, and rehearse every failure before it happens in production.",
                            )
                            .size(ty::LEAD)
                            .color(col::secondary_text()),
                        );
                        ui.add_space(GAP_XL);
                        let demo = demo_root();
                        let demo_exists = demo.as_deref().is_some_and(is_workspace);
                        for (glyph, title, text, choice) in [
                            (
                                icon::PLAY_CIRCLE,
                                "Try the demo",
                                if demo_exists {
                                    "Open the demo workspace in ~/Vendredi: a sample supplier and 13 starter cases."
                                } else {
                                    "Set up a demo workspace in ~/Vendredi with a sample supplier and 13 starter cases."
                                },
                                Do::Page(View::All),
                            ),
                            (
                                icon::FOLDER_PLUS,
                                "Create a workspace…",
                                "Choose an empty folder. Vendredi sets it up and helps you describe your first partner.",
                                Do::NewWorkspace,
                            ),
                            (
                                icon::FOLDER_OPEN,
                                "Open a workspace…",
                                "Choose a folder that has a vendredi.toml.",
                                Do::OpenWorkspace,
                            ),
                        ] {
                            if choice_card(ui, glyph, title, text).clicked() {
                                pick = Some(choice);
                            }
                            ui.add_space(GAP_S);
                        }
                        if !self.error.is_empty() {
                            ui.add_space(GAP_S);
                            notice(ui, &self.error);
                        }
                        let recent = self.recent.clone();
                        if !recent.is_empty() {
                            ui.add_space(GAP_L);
                            form::label(ui, "Recent");
                            for r in recent {
                                if ui
                                    .link(r.display().to_string())
                                    .on_hover_text("Open this workspace")
                                    .clicked()
                                {
                                    action = Some(Switch::Open(r));
                                }
                            }
                        }
                    });
                });
            });
        match pick {
            // The demo lives in the home folder, and is set up the first time.
            Some(Do::Page(_)) => {
                if let Some(demo) = demo_root() {
                    if is_workspace(&demo) {
                        action = Some(Switch::Open(demo));
                    } else if let Err(e) = crate::sample(&demo) {
                        self.error = format!("Setting up the demo: {e:#}");
                    } else {
                        action = Some(Switch::Open(demo));
                    }
                }
            }
            Some(other) => self.perform(other, ctx),
            None => {}
        }
        action
    }
}

/// A large choice on the welcome screen: a title and a line about it.
fn choice_card(ui: &mut egui::Ui, glyph: &str, title: &str, text: &str) -> egui::Response {
    let response = card_frame(col::bg(), col::line())
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.label(icon_job(glyph, icon_font(22.), col::blue()));
                ui.add_space(GAP_S);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).font(ty::section()).color(col::text()));
                    ui.label(RichText::new(text).color(col::secondary_text()));
                });
            });
        })
        .response
        .interact(egui::Sense::click());
    if response.hovered() {
        ui.painter().rect_stroke(
            response.rect,
            8,
            egui::Stroke::new(1.5_f32, col::blue()),
            egui::StrokeKind::Inside,
        );
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl eframe::App for Session {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.show(ctx);
    }
    /// Remembers the page per workspace and the recent workspaces. eframe
    /// keeps the window size and position itself.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(Ok(app)) = &self.desktop {
            storage.set_string(&place_key(&app.root), app.place());
        }
        let recent: Vec<String> = self
            .recent
            .iter()
            .map(|r| r.display().to_string())
            .collect();
        storage.set_string(RECENT, recent.join("\n"));
        storage.set_string(support::LAST_VERSION, env!("CARGO_PKG_VERSION").into());
    }
}
