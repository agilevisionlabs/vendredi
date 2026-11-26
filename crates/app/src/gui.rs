mod cases;
mod explain;
mod form;
mod history;
mod inspector;
#[cfg(test)]
mod layout_tests;
#[cfg(target_os = "macos")]
mod menu;
mod onboarding;
mod partners;
mod rules;
mod scripts;
mod search;
mod send;
mod session;
mod settings;
mod shortcuts;
mod theme;
mod toast;
mod window;
mod workspace_edit;
use crate::runtime::{self, Command, Running};
use anyhow::{Context, Result, anyhow, ensure};
use eframe::egui;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
};
use theme::*;
use vendredi_core::{
    config::{Action, Receive, SendChannel, Workspace},
    engine::{Delivery, TrafficRow},
};

pub fn run(root: Option<PathBuf>) -> Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1440.0, 860.0])
        .with_min_inner_size([680.0, 420.0]);
    // Let AppKit use the bundle icon in the Dock.
    #[cfg(target_os = "macos")]
    let viewport = viewport.with_icon(egui::IconData::default());
    let opts = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Vendredi",
        opts,
        Box::new(move |cc| {
            let recent = cc
                .storage
                .and_then(|s| s.get_string(session::RECENT))
                .map(|r| session::parse_recent(&r))
                .unwrap_or_default();
            let mut session = session::Session::start(root, recent, &cc.egui_ctx);
            // AppKit menus belong to the main thread, where eframe calls this.
            #[cfg(target_os = "macos")]
            {
                session._menu = menu::install(&cc.egui_ctx).ok();
            }
            if let (Some(storage), Some(Ok(app))) = (cc.storage, &mut session.desktop) {
                app.restore(storage.get_string(&place_key(&app.root)).as_deref());
            }
            Ok(Box::new(session))
        }),
    )
    .map_err(|e| anyhow!("{e}"))
}

fn place_key(root: &Path) -> String {
    format!("place {}", root.display())
}

impl Desktop {
    /// The current page, such as `cases` or `partner Supplier`.
    fn place(&self) -> String {
        match &self.view {
            View::All => "all".into(),
            View::Attention => "attention".into(),
            View::Partner(i) => format!("partner {}", self.workspace.partners[*i].name),
            View::Cases => "cases".into(),
            View::Scripts => "scripts".into(),
            View::Documents => "documents".into(),
            View::Settings | View::Config => "settings".into(),
        }
    }
    /// Returns to a page saved by `place`. Unknown or removed pages are ignored.
    fn restore(&mut self, place: Option<&str>) {
        let view = match place {
            Some("attention") => View::Attention,
            Some("cases") => View::Cases,
            Some("scripts") => View::Scripts,
            Some("documents") => View::Documents,
            Some("settings") => View::Settings,
            Some(p) => match p
                .strip_prefix("partner ")
                .and_then(|name| self.workspace.partners.iter().position(|q| q.name == name))
            {
                Some(i) => {
                    self.partner = i;
                    View::Partner(i)
                }
                None => return,
            },
            None => return,
        };
        self.navigate(view);
    }
}

#[derive(Clone, PartialEq, Eq)]
enum View {
    All,
    Attention,
    Partner(usize),
    Cases,
    Scripts,
    Documents,
    Settings,
    Config,
}
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum DetailTab {
    #[default]
    Message,
    Findings,
    Log,
    Transport,
}
struct Desktop {
    root: PathBuf,
    workspace: Workspace,
    running: Running,
    events: mpsc::Receiver<Value>,
    output: runtime::Output,
    transfers: BTreeMap<i64, TrafficRow>,
    selected: Option<i64>,
    compact_detail: bool,
    expand_timeline: bool,
    detail: Value,
    view: View,
    detail_tab: DetailTab,
    segment: usize,
    search: String,
    search_due: Option<std::time::Instant>,
    highlight: Option<(String, vendredi_core::engine::TrafficSearch)>,
    reveal_match: bool,
    matches: BTreeSet<i64>,
    direction: String,
    filter_status: String,
    listening: bool,
    address: std::net::SocketAddr,
    send_dialog: bool,
    send_partner: usize,
    send_script: bool,
    send_error: String,
    partner_form: Option<partners::PartnerForm>,
    cleanup: Option<history::Cleanup>,
    script_form: Option<scripts::ScriptForm>,
    rule_form: Option<rules::RuleForm>,
    settings_form: Option<settings::SettingsForm>,
    file_error: Option<String>,
    file_unavailable: bool,
    partner: usize,
    /// The case selected in the Cases view.
    case: String,
    import: Option<cases::ImportForm>,
    case_form: Option<cases::CaseForm>,
    /// Set by ⌘F; the search field takes the keyboard when it is next drawn.
    focus_search: bool,
    /// Shown as toasts: the last status already shown, whether the next
    /// status is an error, and the undo for the next one.
    toasted: String,
    status_error: bool,
    pending_undo: Option<toast::Undo>,
    toast_list: Vec<toast::Toast>,
    toast_count: u64,
    /// When each transfer first appeared, in `egui` time, for the arrival glow.
    arrivals: BTreeMap<i64, f64>,
    /// A workspace change the session carries out after this frame.
    pub(super) switch: Option<session::Switch>,
    checklist: onboarding::Checklist,
    send_file: String,
    function: String,
    config_source: String,
    edit_file: PathBuf,
    saved: String,
    edit: String,
    status: String,
}
impl Desktop {
    fn open(root: PathBuf, ctx: &egui::Context) -> Result<Self> {
        let checklist = onboarding::Checklist::load(&root);
        install(ctx);
        let workspace = Workspace::load(&root)?;
        let (tx, events) = mpsc::channel();
        let ctx = ctx.clone();
        let output: runtime::Output = Arc::new(move |v| {
            let _ = tx.send(v);
            ctx.request_repaint();
        });
        let address = workspace.listen;
        let (running, listening, status) =
            match runtime::start(workspace.clone(), output.clone(), true, true) {
                Ok(r) => (r, true, String::new()),
                Err(e) => (
                    // The status below is reported as an error toast.
                    runtime::start(workspace.clone(), output.clone(), true, false)
                        .with_context(|| format!("Opening workspace: {e:#}"))?,
                    false,
                    format!("Could not start listening. {}", explain::explain(&e).text),
                ),
            };
        let edit_file = PathBuf::from("vendredi.toml");
        let saved = fs::read_to_string(root.join(&edit_file))?;
        Ok(Self {
            root,
            workspace,
            running,
            events,
            output,
            transfers: BTreeMap::new(),
            selected: None,
            compact_detail: false,
            expand_timeline: false,
            detail: Value::Null,
            view: View::All,
            detail_tab: DetailTab::Message,
            segment: 0,
            search: String::new(),
            search_due: None,
            highlight: None,
            reveal_match: false,
            matches: BTreeSet::new(),
            direction: "Any".into(),
            filter_status: "Any".into(),
            listening,
            address,
            send_dialog: false,
            send_partner: 0,
            send_script: false,
            send_error: String::new(),
            partner_form: None,
            cleanup: None,
            script_form: None,
            rule_form: None,
            settings_form: None,
            file_error: None,
            file_unavailable: false,
            partner: 0,
            case: String::new(),
            import: None,
            case_form: None,
            focus_search: false,
            toasted: String::new(),
            status_error: !listening,
            pending_undo: None,
            toast_list: Vec::new(),
            toast_count: 0,
            arrivals: BTreeMap::new(),
            switch: None,
            checklist,
            send_file: String::new(),
            function: String::new(),
            edit_file,
            config_source: saved.clone(),
            edit: saved.clone(),
            saved,
            status,
        })
    }
    fn command(&mut self, c: Command) {
        if self.running.tx.send(c).is_err() {
            self.status = "Engine stopped".into();
        }
    }
    fn select(&mut self, id: i64) {
        self.checklist_saw_selection(id);
        self.selected = Some(id);
        self.reveal_match = true;
        self.segment = 0;
        self.detail = Value::Null;
        self.detail_tab = DetailTab::Message;
        self.command(Command::Inspect(id));
    }
    fn drain(&mut self) {
        let mut refresh = false;
        for v in self.events.try_iter().collect::<Vec<_>>() {
            match v["event"].as_str().unwrap_or("") {
                "traffic" => {
                    let query = v["search"].as_str().unwrap_or("");
                    let Ok(rows) = serde_json::from_value::<Vec<TrafficRow>>(v["rows"].clone())
                    else {
                        continue;
                    };
                    if query.is_empty() {
                        self.transfers = rows.into_iter().map(|r| (r.id, r)).collect();
                        if self.selected.is_none() {
                            let first = self
                                .transfers
                                .values()
                                .rev()
                                .find(|r| !r.outgoing)
                                .or_else(|| self.transfers.values().next_back())
                                .map(|r| r.id);
                            if let Some(id) = first {
                                self.select(id);
                            }
                        }
                    } else if query == self.search {
                        self.apply_search(query, rows);
                    }
                }
                "history_cleared" => {
                    let count = v["count"].as_u64().unwrap_or(0);
                    self.status = match count {
                        0 => "No matching exchanges to clear".into(),
                        1 => "Cleared 1 exchange".into(),
                        n => format!("Cleared {n} exchanges"),
                    };
                    self.command(Command::Traffic(String::new()));
                    if !self.search.is_empty() {
                        self.command(Command::Traffic(self.search.clone()));
                    }
                }
                "detail" if v["value"]["transfer"].as_i64() == self.selected => {
                    self.detail = v["value"].clone()
                }
                // The workspace already lists its warnings.
                "detail" | "warning" => {}
                "ready" | "listening" => {
                    self.listening = v["listen"].is_string();
                    if let Some(addr) = v["listen"].as_str().and_then(|s| s.parse().ok()) {
                        self.address = addr;
                    }
                }
                "error" => self.fail(v["error"].as_str().unwrap_or("Command failed")),
                "case_switched" => {
                    self.checklist_saw_switch();
                    self.case_switched(&v);
                }
                "import_ready" => self.poll_import(),
                _ => {
                    refresh = true;
                    if let Some(error) = v["error"].as_str() {
                        self.fail(error);
                    }
                }
            }
        }
        if refresh {
            self.command(Command::Traffic(String::new()));
            if !self.search.is_empty() {
                self.command(Command::Traffic(self.search.clone()));
            }
            if let Some(id) = self.selected {
                self.command(Command::Inspect(id));
            }
        }
    }
    fn visible(&self, t: &TrafficRow) -> bool {
        (self.highlight.is_none() || self.matches.contains(&t.id))
            && (self.direction == "Any" || t.outgoing == (self.direction == "Out"))
            && (self.filter_status == "Any" || t.status == self.filter_status)
            && match self.view {
                View::Attention => t.attention,
                View::Partner(p) => t.partner == self.workspace.partners[p].name,
                _ => true,
            }
    }
    fn reconcile_selection(&mut self) {
        if self
            .selected
            .and_then(|id| self.transfers.get(&id))
            .is_some_and(|t| self.visible(t))
        {
            return;
        }
        let mut visible = self.transfers.values().rev().filter(|t| self.visible(t));
        let next = visible
            .clone()
            .find(|t| !t.outgoing)
            .or_else(|| visible.next())
            .map(|r| r.id);
        if let Some(id) = next {
            self.select(id);
        } else {
            self.selected = None;
            self.detail = Value::Null;
        }
    }
    fn files(&self) -> Vec<PathBuf> {
        if self.view != View::Documents {
            return vec!["vendredi.toml".into()];
        }
        let mut paths = BTreeSet::new();
        for a in self
            .workspace
            .partners
            .iter()
            .flat_map(|p| p.scenario())
            .flat_map(|r| &r.actions)
        {
            if let Action::Document { document } = a {
                paths.insert(document.clone());
            }
        }
        paths.into_iter().collect()
    }
    fn navigate(&mut self, view: View) {
        if self.view == view {
            return;
        }
        if self.edit != self.saved {
            self.status = "Save or discard your edits before switching views".into();
            return;
        }
        self.compact_detail = false;
        self.view = view;
        if matches!(self.view, View::Documents | View::Config)
            && let Some(path) = self.files().first()
        {
            self.open_file(path.clone());
        }
    }
    fn open_file(&mut self, path: PathBuf) {
        let result = fs::read(self.root.join(&path))
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                if self.view == View::Documents {
                    Ok(vendredi_core::edi::latin1(&bytes))
                } else {
                    Ok(String::from_utf8(bytes)?)
                }
            });
        let changed_file = self.edit_file != path;
        self.edit_file = path;
        match result {
            Ok(text) => {
                self.edit.clone_from(&text);
                self.saved = text;
                self.file_error = None;
                self.file_unavailable = false;
            }
            Err(e) => {
                self.file_error = Some(format!("Opening {}: {e:#}", self.edit_file.display()));
                if changed_file || self.edit == self.saved {
                    self.edit.clear();
                    self.saved.clear();
                    self.file_unavailable = true;
                }
                // On a failed explicit reload, keep the current draft available for recovery.
            }
        }
    }
    fn save(&mut self) -> Result<()> {
        let file = self.root.join(&self.edit_file);
        let encode = |s: &str| -> Result<Vec<u8>> {
            if self.view == View::Documents {
                s.chars()
                    .map(|c| {
                        u8::try_from(u32::from(c))
                            .context("The EDIFACT document can only store Latin-1 characters.")
                    })
                    .collect()
            } else {
                Ok(s.as_bytes().to_vec())
            }
        };
        let before = encode(&self.saved)?;
        let after = encode(&self.edit)?;
        self.apply_files(&[workspace_edit::FileEdit {
            path: &file,
            before: Some(&before),
            after: &after,
        }])?;
        self.saved.clone_from(&self.edit);
        self.file_error = None;
        self.status = "Saved and reloaded workspace".into();
        Ok(())
    }
    fn editor(&mut self, ui: &mut egui::Ui) {
        let title = match self.view {
            View::Documents => "Documents",
            _ => "Workspace configuration",
        };
        ui.heading(title);
        ui.add_space(GAP_S);
        if self.files().is_empty() {
            empty_state(
                ui,
                "No documents yet",
                "Documents are the fixed replies a case sends, such as an order response. They appear here when the active cases use them.",
                |ui| {
                    if button(ui, "Open Cases", false).clicked() {
                        self.navigate(View::Cases);
                    }
                },
            );
            return;
        }
        let dirty = self.edit != self.saved;
        ui.horizontal_wrapped(|ui| {
            if self.view == View::Documents {
                let mut next = self.edit_file.clone();
                ui.add_enabled_ui(!dirty, |ui| {
                    egui::ComboBox::from_id_salt("files")
                        .selected_text(next.display().to_string())
                        .show_ui(ui, |ui| {
                            for file in self.files() {
                                let name = file.display().to_string();
                                ui.selectable_value(&mut next, file, name);
                            }
                        })
                });
                if next != self.edit_file {
                    self.open_file(next);
                }
            } else {
                ui.monospace("vendredi.toml");
            }
            if ui
                .add_enabled(dirty, styled("Save and reload", true, BUTTON))
                .on_hover_text(shortcuts::hint(ui.ctx(), "Save", &shortcuts::SAVE))
                .clicked()
                && let Err(e) = self.save()
            {
                self.file_error = Some(format!("{e:#}"));
            }
            if button(
                ui,
                if dirty {
                    "Discard edits"
                } else {
                    "Reload file"
                },
                false,
            )
            .clicked()
            {
                self.open_file(self.edit_file.clone());
            }
            if dirty {
                ui.label(muted("Unsaved changes"));
            }
        });
        if let Some(e) = &self.file_error {
            ui.colored_label(AMBER, e);
        }
        if self.file_unavailable {
            return;
        }
        egui::ScrollArea::both().show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.edit)
                    .id(egui::Id::new("workspace-file-content"))
                    .code_editor()
                    .desired_rows(32)
                    .desired_width(f32::INFINITY),
            );
        });
    }
}
fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = tempfile::NamedTempFile::new_in(path.parent().context("editor path")?)?;
    f.as_file()
        .set_permissions(fs::metadata(path)?.permissions())?;
    f.write_all(bytes)?;
    f.as_file().sync_all()?;
    f.persist(path)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
    use std::{
        net::TcpListener,
        time::{Duration, Instant},
    };
    pub(in crate::gui) fn frame(
        app: &mut Desktop,
        ctx: &egui::Context,
        events: Vec<Event>,
    ) -> egui::FullOutput {
        render(ctx, events, |ctx| app.show(ctx))
    }
    pub(in crate::gui) fn render(
        ctx: &egui::Context,
        events: Vec<Event>,
        draw: impl FnMut(&egui::Context),
    ) -> egui::FullOutput {
        let size = ctx
            .data(|d| d.get_temp::<Vec2>(egui::Id::new("test screen size")))
            .unwrap_or(Vec2::new(1080., 680.));
        ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            draw,
        )
    }
    pub(super) fn wait_for(
        app: &mut Desktop,
        operation: &str,
        ready: impl Fn(&Desktop) -> bool,
    ) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            app.drain();
            if ready(app) {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "timed out waiting for {operation} (status {}, selected {:?}, detail {})",
                app.status,
                app.selected,
                app.detail
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    pub(super) fn click(app: &mut Desktop, ctx: &egui::Context, label: &str) -> Result<()> {
        click_in(ctx, label, |ctx| app.show(ctx))
    }
    fn click_in(
        ctx: &egui::Context,
        label: &str,
        mut draw: impl FnMut(&egui::Context),
    ) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let pos = loop {
            let output = render(ctx, vec![], &mut draw);
            // A menu or modal can cover a label with the same text.
            if let Some(pos) = output.shapes.iter().rev().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == label => {
                    Some(t.visual_bounding_rect().center())
                }
                _ => None,
            }) {
                break pos;
            }
            ensure!(Instant::now() < deadline, "missing widget {label}");
            thread::sleep(Duration::from_millis(5));
        };
        for pressed in [true, false] {
            render(
                ctx,
                vec![
                    Event::PointerMoved(pos),
                    Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed,
                        modifiers: Modifiers::NONE,
                    },
                ],
                &mut draw,
            );
        }
        Ok(())
    }
    #[test]
    #[ignore = "uses a local workspace listener"]
    fn retry_opening_repaired_workspace() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let path = dir.path().join("vendredi.toml");
        let valid = fs::read_to_string(&path)?.replace(":8180", ":0");
        fs::write(&path, "[broken")?;
        let ctx = egui::Context::default();
        let mut session = session::Session::start(Some(dir.path().into()), vec![], &ctx);
        assert!(matches!(session.desktop, Some(Err(_))));
        click_in(&ctx, "Try again", |ctx| session.show(ctx))?;
        assert!(matches!(session.desktop, Some(Err(_))));
        // A workspace that failed to open is not remembered as recent.
        assert!(session.recent.is_empty());
        fs::write(path, valid)?;
        click_in(&ctx, "Try again", |ctx| session.show(ctx))?;
        assert!(session.desktop.context("desktop")??.listening);
        Ok(())
    }

    #[test]
    #[ignore = "opens local workspace listeners"]
    fn welcome_creates_opens_and_switches_workspaces() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let ctx = egui::Context::default();
        let first = dir.path().join("first");
        crate::sample(&first)?;
        let config = first.join("vendredi.toml");
        fs::write(&config, fs::read_to_string(&config)?.replace(":8180", ":0"))?;
        // The most recent workspace that still exists opens on launch.
        let gone = dir.path().join("gone");
        let mut session = session::Session::start(None, vec![gone, first.clone()], &ctx);
        assert_eq!(session.root.as_deref(), Some(first.as_path()));
        assert!(matches!(session.desktop, Some(Ok(_))));
        let second = dir.path().join("second");
        crate::sample(&second)?;
        let config = second.join("vendredi.toml");
        fs::write(&config, fs::read_to_string(&config)?.replace(":8180", ":0"))?;
        // Switching stops the first engine, so its workspace can open again.
        session.open(second.clone(), &ctx);
        assert_eq!(session.recent, [second.clone(), first.clone()]);
        session.open(first.clone(), &ctx);
        assert!(matches!(session.desktop, Some(Ok(_))));
        assert_eq!(session.recent, [first, second]);
        Ok(())
    }

    #[test]
    #[ignore = "uses a local listener and folder watcher"]
    fn clear_removed_partner_traffic() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let config = dir.path().join("vendredi.toml");
        let mut text = fs::read_to_string(&config)?.replace(":8180", ":0");
        text.push_str(
            r#"
[[partners]]
name = "Carrier"
identity = ["CARRIER", "ZZ"]
charset = "UNOC"
acknowledge = "never"
receive = [{ directory = "freight", suffix = ".edi" }]
send = { directory = "freight-out" }
"#,
        );
        fs::write(config, text)?;
        let ctx = egui::Context::default();
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        for folder in ["inbox", "freight"] {
            fs::write(dir.path().join(folder).join("sample.edi"), b"bad")?;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.transfers.len() != 2 {
            ensure!(
                Instant::now() < deadline,
                "folder intake timed out: {}",
                app.status
            );
            app.drain();
            thread::sleep(Duration::from_millis(10));
        }
        app.navigate(View::Partner(0));
        click(&mut app, &ctx, "Remove partner")?;
        click(&mut app, &ctx, "Clear history…")?;
        click(&mut app, &ctx, "Cancel")?;
        assert_eq!(app.transfers.len(), 2);
        click(&mut app, &ctx, "Clear history…")?;
        click(&mut app, &ctx, "All partners")?;
        click(&mut app, &ctx, "Supplier (removed)")?;
        click(&mut app, &ctx, "Last hour")?;
        click(&mut app, &ctx, "Clear history")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.transfers.len() != 1 {
            ensure!(Instant::now() < deadline, "cleanup failed: {}", app.status);
            frame(&mut app, &ctx, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(app.transfers.values().next().unwrap().partner, "Carrier");
        assert_eq!(app.status, "Cleared 1 exchange");
        drop(app);
        let mut reopened = Desktop::open(dir.path().into(), &ctx)?;
        reopened.drain();
        assert_eq!(reopened.transfers.len(), 1);
        assert_eq!(
            reopened.transfers.values().next().unwrap().partner,
            "Carrier"
        );
        Ok(())
    }

    #[test]
    #[ignore = "exercises local listeners and both sample transports"]
    fn send_and_compact_inspection() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let path = dir.path().join("vendredi.toml");
        let src = fs::read_to_string(&path)?;
        let occupied = TcpListener::bind("127.0.0.1:0")?;
        fs::write(
            &path,
            src.replace("127.0.0.1:8180", &occupied.local_addr()?.to_string()),
        )?;
        let ctx = egui::Context::default();
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        assert!(!app.listening);
        assert!(app.status.contains("Could not start listening"));
        drop(occupied);
        click(&mut app, &ctx, "▷  Resume")?;
        let until = Instant::now() + Duration::from_secs(8);
        while !app.listening {
            ensure!(Instant::now() < until, "start timed out");
            app.drain();
        }
        click(&mut app, &ctx, "→  Send document…")?;
        app.send_file = "missing.edi".into();
        click(&mut app, &ctx, "Send")?;
        assert!(app.send_dialog && app.send_error.contains("missing.edi"));
        app.send_file = "confirmation.edi".into();
        click(&mut app, &ctx, "Send")?;
        while !app
            .transfers
            .values()
            .any(|t| t.outgoing && t.status == "Delivered")
        {
            ensure!(Instant::now() < until, "send timed out");
            app.drain();
        }
        click(&mut app, &ctx, "Resend")?;
        while app
            .transfers
            .values()
            .filter(|t| t.status == "Delivered")
            .count()
            < 2
        {
            ensure!(Instant::now() < until, "resend timed out");
            app.drain();
        }
        drop(app);
        fs::write(
            &path,
            src.replace(":8180", ":0")
                .replace(r#"{ path = "/supplier/orders" }, "#, "")
                .replace(r#"suffix = ".edi""#, r#"suffix = ".custom""#),
        )?;
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        app.drain();
        assert_ne!(app.address.port(), 0);
        app.sample(0)?;
        while !app
            .transfers
            .values()
            .any(|t| !t.outgoing && t.kind == "ORDERS")
        {
            ensure!(
                Instant::now() < until,
                "folder sample timed out: {}",
                app.status
            );
            app.drain();
        }
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("test screen size"), Vec2::new(680., 420.)));
        click(&mut app, &ctx, "ORDERS")?;
        assert!(app.compact_detail);
        while app.detail["transfer"].is_null() {
            ensure!(Instant::now() < until, "inspect timed out");
            app.drain();
        }
        let meta: Value = serde_json::from_str(app.detail["metadata"].as_str().unwrap())?;
        assert!(meta["file"].as_str().unwrap().ends_with(".custom"));
        click(&mut app, &ctx, "Copy raw")?;
        assert_eq!(app.status, "Raw message copied");
        click(&mut app, &ctx, "‹  Back to traffic")?;
        assert!(!app.compact_detail);
        Ok(())
    }

    #[test]
    #[ignore = "runs a local scripted HTTP exchange"]
    fn search_filter_and_follow_exchange_links() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let path = dir.path().join("vendredi.toml");
        fs::write(&path, fs::read_to_string(&path)?.replace(":8180", ":0"))?;
        // The active case runs a second script before its own.
        let folder = vendredi_core::case::folder(dir.path(), "confirm-orders");
        let file = folder.join("case.json");
        let mut case: Value = serde_json::from_slice(&fs::read(&file)?)?;
        case["rules"][0]["actions"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"script":"report.js","function":"report"}));
        fs::write(&file, serde_json::to_vec(&case)?)?;
        fs::write(
            folder.join("report.js"),
            "function report(i,api){api.log('recorded');}",
        )?;
        let ctx = egui::Context::default();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("test screen size"), Vec2::new(1440., 860.)));
        let mut app = Desktop::open(dir.path().into(), &ctx)?;
        click(&mut app, &ctx, "Send sample order")?;
        let end = Instant::now() + Duration::from_secs(8);
        while !app.detail["events"]
            .as_array()
            .is_some_and(|events| events.iter().any(|e| e["event"] == "delivered"))
        {
            ensure!(Instant::now() < end, "scripted exchange timed out");
            frame(&mut app, &ctx, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        // Keep both panes on the last completed result while the user types.
        let selected = app.selected.unwrap();
        click(&mut app, &ctx, "Search traffic and message text")?;
        frame(&mut app, &ctx, vec![Event::Text("Su".into())]);
        assert!(app.visible(&app.transfers[&selected]));
        assert_eq!(app.detail["transfer"], selected);
        while app.highlight.is_none() {
            ensure!(Instant::now() < end, "initial search timed out");
            frame(&mut app, &ctx, vec![]);
            thread::sleep(Duration::from_millis(5));
        }
        let previous = app.matches.clone();
        frame(&mut app, &ctx, vec![Event::Text("p".into())]);
        (app.output)(json!({"event":"traffic","search":"Su","rows":[]}));
        frame(&mut app, &ctx, vec![Event::Text("p".into())]);
        assert_eq!(app.search, "Supp");
        assert_eq!(app.matches, previous);
        assert!(app.visible(&app.transfers[&selected]));
        assert_eq!(app.detail["transfer"], selected);
        app.clear_search();
        assert!(app.search_due.is_none() && app.highlight.is_none());
        click(&mut app, &ctx, "Orders")?;
        assert!(app.view == View::Cases && app.rule_form.is_some());
        click(&mut app, &ctx, "Cancel")?;
        click(&mut app, &ctx, "All traffic")?;
        click(&mut app, &ctx, "cases/confirm-orders/report.js")?;
        assert!(app.view == View::Scripts && app.script_form.is_some());
        frame(
            &mut app,
            &ctx,
            vec![Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert!(app.script_form.is_none());
        click(&mut app, &ctx, "All traffic")?;
        click(&mut app, &ctx, "Script log 2")?;
        assert!(matches!(app.detail_tab, DetailTab::Log));
        click(&mut app, &ctx, "Findings 0")?;
        assert!(matches!(app.detail_tab, DetailTab::Findings));
        click(&mut app, &ctx, "Status Any")?;
        click(&mut app, &ctx, "Delivered")?;
        assert!(
            app.transfers
                .values()
                .filter(|r| app.visible(r))
                .all(|r| r.outgoing)
        );
        click(&mut app, &ctx, "Search traffic and message text")?;
        frame(
            &mut app,
            &ctx,
            vec![Event::Text("missing reference".into())],
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.highlight.is_none() || !app.matches.is_empty() {
            ensure!(Instant::now() < deadline, "no-match result timed out");
            frame(&mut app, &ctx, vec![]);
            thread::sleep(Duration::from_millis(5));
        }
        click(&mut app, &ctx, "Clear filters")?;
        assert!(app.search.is_empty() && app.filter_status == "Any" && app.direction == "Any");
        for (query, expected) in [("Supplier", "Supplier"), ("bgm+220", "220")] {
            click(&mut app, &ctx, "Search traffic and message text")?;
            frame(&mut app, &ctx, vec![Event::Text(query.into())]);
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let out = frame(&mut app, &ctx, vec![]);
                if !app.matches.is_empty()
                    && out.shapes.iter().any(|s| {
                        let egui::Shape::Text(text) = &s.shape else {
                            return false;
                        };
                        let job = &text.galley.job;
                        job.sections.iter().any(|section| {
                            section.format.background == search::HIGHLIGHT
                                && job.text[section.byte_range.clone()].contains(expected)
                        })
                    })
                {
                    break;
                }
                ensure!(
                    Instant::now() < deadline,
                    "missing rendered match for {query}"
                );
                thread::sleep(Duration::from_millis(8));
            }
            app.clear_search();
        }
        click(&mut app, &ctx, "→  Send document…")?;
        click(&mut app, &ctx, "JavaScript")?;
        // The choices are the active case's scripts, by workspace path.
        assert_eq!(app.send_file, "cases/confirm-orders/confirm.js");
        app.send_file = "cases/confirm-orders/report.js".into();
        app.function = "report".into();
        click(&mut app, &ctx, "Send")?;
        assert!(!app.send_dialog);
        drop(app);
        let engine = vendredi_core::engine::Engine::open(
            Workspace::load(dir.path())?,
            Arc::new(vendredi_core::engine::SystemClock),
        )?;
        assert_eq!(
            engine
                .history(500)?
                .iter()
                .filter(|e| e["event"] == "script_log")
                .count(),
            3
        );
        Ok(())
    }

    fn text_matches_reply(v: &Value) -> bool {
        v["raw"].as_str().is_some_and(|s| s.contains("ORDRSP"))
    }
    #[test]
    #[ignore = "opens a local loopback listener; run with the local instance suite"]
    fn exchange_edit_and_reopen() -> Result<()> {
        let dir = tempfile::tempdir()?;
        crate::sample(dir.path())?;
        let config = dir.path().join("vendredi.toml");
        let addr = TcpListener::bind("127.0.0.1:0")?.local_addr()?;
        fs::write(
            &config,
            fs::read_to_string(&config)?.replace("127.0.0.1:8180", &addr.to_string()),
        )?;
        let ctx = egui::Context::default();
        let mut app = Desktop::open(dir.path().to_owned(), &ctx)?;
        click(&mut app, &ctx, "Send sample order")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while app
            .transfers
            .values()
            .filter(|t| t.status == "Delivered")
            .count()
            != 2
        {
            ensure!(
                Instant::now() < deadline,
                "sample exchange did not finish: {}",
                app.status
            );
            frame(&mut app, &ctx, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        click(&mut app, &ctx, "ORDRSP")?;
        wait_for(&mut app, "reply inspection", |app| {
            text_matches_reply(&app.detail)
        })?;
        assert!(app.detail["raw"].as_str().unwrap().contains("ORDRSP"));
        let segment_text = app.detail["segments"][2]["raw"]
            .as_str()
            .unwrap()
            .to_owned();
        click(&mut app, &ctx, &segment_text)?;
        assert_eq!(app.segment, 2);
        click(&mut app, &ctx, "Transport")?;
        assert!(matches!(app.detail_tab, DetailTab::Transport));
        click(&mut app, &ctx, "Needs attention")?;
        frame(&mut app, &ctx, vec![]);
        assert!(app.selected.is_none());
        click(&mut app, &ctx, "All traffic")?;
        click(&mut app, &ctx, "Direction Any")?;
        click(&mut app, &ctx, "In")?;
        assert_eq!(app.transfers.values().filter(|r| app.visible(r)).count(), 1);
        click(&mut app, &ctx, "Replay")?;
        wait_for(&mut app, "replay", |app| {
            app.transfers.values().filter(|r| !r.outgoing).count() == 2
        })?;
        click(&mut app, &ctx, "⏸  Pause")?;
        wait_for(&mut app, "listener stop", |app| !app.listening)?;
        click(&mut app, &ctx, "▷  Resume")?;
        wait_for(&mut app, "listener restart", |app| app.listening)?;
        click(&mut app, &ctx, "Settings")?;
        click(&mut app, &ctx, "Edit configuration file")?;
        // Drive the multiline editor with the same keyboard events as Select All and paste.
        // Without an active case the partner only acknowledges.
        let changed = app.edit.replace("case = \"confirm-orders\"\n", "");
        assert_ne!(changed, app.edit);
        frame(&mut app, &ctx, vec![]);
        let edit_pos = ctx
            .read_response(egui::Id::new("workspace-file-content"))
            .context("configuration editor missing")?
            .rect
            .left_top()
            + egui::vec2(24., 12.);
        frame(
            &mut app,
            &ctx,
            vec![
                Event::PointerMoved(edit_pos),
                Event::PointerButton {
                    pos: edit_pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
        frame(
            &mut app,
            &ctx,
            vec![
                Event::PointerButton {
                    pos: edit_pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                },
                Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::COMMAND,
                },
                Event::Paste(changed.clone()),
            ],
        );
        assert_eq!(app.edit, changed);
        click(&mut app, &ctx, "Save and reload")?;
        assert_eq!(fs::read_to_string(config)?, changed);
        drop(app);
        let w = Workspace::load(dir.path())?;
        let (send, recv) = mpsc::channel();
        let mut run = runtime::start(
            w,
            Arc::new(move |v| {
                let _ = send.send(v);
            }),
            true,
            true,
        )?;
        assert!(recv.try_iter().any(|v| v["event"] == "delivered"));
        let d = Delivery {
            id: 0,
            bytes: crate::ORDER.to_vec(),
            channel: SendChannel::Http {
                url: format!("http://{addr}/supplier/orders"),
                headers: Default::default(),
            },
        };
        vendredi_transports::deliver(dir.path(), &d)?;
        loop {
            let event = recv.recv_timeout(Duration::from_secs(5))?;
            if event["event"] == "no_match" {
                break;
            }
        }
        run.stop()
    }
}
