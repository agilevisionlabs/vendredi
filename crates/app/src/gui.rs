mod inspector;
mod partners;
mod theme;
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

pub fn run(root: PathBuf) -> Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 860.0])
            .with_min_inner_size([680.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Vendredi",
        opts,
        Box::new(move |cc| Ok(Box::new(Desktop::open(root, &cc.egui_ctx)?))),
    )
    .map_err(|e| anyhow!("{e}"))
}

#[derive(Clone, PartialEq, Eq)]
enum View {
    All,
    Attention,
    Partner(usize),
    Rules,
    Scripts,
    Documents,
    Settings,
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
    detail: Value,
    view: View,
    detail_tab: DetailTab,
    segment: usize,
    search: String,
    matches: BTreeSet<i64>,
    direction: String,
    filter_status: String,
    listening: bool,
    send_dialog: bool,
    partner_form: Option<partners::PartnerForm>,
    partner: usize,
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
        install(ctx);
        let workspace = Workspace::load(&root)?;
        let (tx, events) = mpsc::channel();
        let ctx = ctx.clone();
        let output: runtime::Output = Arc::new(move |v| {
            let _ = tx.send(v);
            ctx.request_repaint();
        });
        let running = runtime::start(workspace.clone(), output.clone(), true, true)?;
        let edit_file = PathBuf::from("vendredi.toml");
        let saved = fs::read_to_string(root.join(&edit_file))?;
        let send_file = workspace.partners[0]
            .rules
            .iter()
            .flat_map(|r| &r.actions)
            .find_map(|a| match a {
                Action::Document { document } => Some(document.display().to_string()),
                _ => None,
            })
            .unwrap_or_default();
        Ok(Self {
            root,
            workspace,
            running,
            events,
            output,
            transfers: BTreeMap::new(),
            selected: None,
            detail: Value::Null,
            view: View::All,
            detail_tab: DetailTab::Message,
            segment: 0,
            search: String::new(),
            matches: BTreeSet::new(),
            direction: "Any".into(),
            filter_status: "Any".into(),
            listening: true,
            send_dialog: false,
            partner_form: None,
            partner: 0,
            send_file,
            function: String::new(),
            edit_file,
            config_source: saved.clone(),
            edit: saved.clone(),
            saved,
            status: String::new(),
        })
    }
    fn command(&mut self, c: Command) {
        if self.running.tx.send(c).is_err() {
            self.status = "Engine stopped".into();
        }
    }
    fn select(&mut self, id: i64) {
        self.selected = Some(id);
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
                        self.matches = rows.iter().map(|r| r.id).collect();
                    }
                }
                "detail" if v["value"]["transfer"].as_i64() == self.selected => {
                    self.detail = v["value"].clone()
                }
                "detail" | "ready" => {}
                "listening" => self.listening = v["active"] == true,
                "error" => self.status = v["error"].as_str().unwrap_or("Command failed").into(),
                _ => {
                    refresh = true;
                    if let Some(error) = v["error"].as_str() {
                        self.status = error.into();
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
        (self.search.is_empty() || self.matches.contains(&t.id))
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
    fn sample(&self) -> Result<()> {
        let p = &self.workspace.partners[self.partner];
        let path = p
            .receive
            .iter()
            .find_map(|r| {
                if let Receive::Http { path } = r {
                    Some(path)
                } else {
                    None
                }
            })
            .context("this partner has no HTTP receive path")?;
        let addr = self.workspace.listen;
        let url = format!("http://{addr}{path}");
        let d = Delivery {
            id: 0,
            channel: SendChannel::Http {
                url,
                headers: Default::default(),
            },
            bytes: fs::read(self.root.join("order.edi"))?,
        };
        let root = self.root.clone();
        let out = self.output.clone();
        thread::spawn(move || {
            if let Err(e) = vendredi_transports::deliver(&root, &d) {
                out(json!({"event":"error","error":format!("{e:#}")}));
            }
        });
        Ok(())
    }
    fn files(&self) -> Vec<PathBuf> {
        if !matches!(self.view, View::Documents | View::Scripts) {
            return vec!["vendredi.toml".into()];
        }
        let mut paths = BTreeSet::new();
        for a in self
            .workspace
            .partners
            .iter()
            .flat_map(|p| &p.rules)
            .flat_map(|r| &r.actions)
        {
            match (&self.view, a) {
                (View::Documents, Action::Document { document }) => {
                    paths.insert(document.clone());
                }
                (View::Scripts, Action::Script { script, .. }) => {
                    paths.insert(script.clone());
                }
                _ => {}
            }
        }
        paths.into_iter().collect()
    }
    fn navigate(&mut self, view: View) {
        if self.edit != self.saved && self.view != view {
            self.status = "Save or reload the edited file before switching views".into();
            return;
        }
        self.view = view;
        if matches!(
            self.view,
            View::Rules | View::Scripts | View::Documents | View::Settings
        ) && let Some(path) = self.files().first()
        {
            self.open_file(path.clone());
        }
    }
    fn open_file(&mut self, path: PathBuf) {
        match fs::read_to_string(self.root.join(&path)) {
            Ok(text) => {
                self.edit_file = path;
                self.saved = text.clone();
                self.edit = text;
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    fn save(&mut self) -> Result<()> {
        let file = self.root.join(&self.edit_file);
        let before = self.saved.clone();
        let after = self.edit.clone();
        self.apply_files(&[workspace_edit::FileEdit {
            path: &file,
            before: Some(&before),
            after: &after,
        }])?;
        if self.edit_file == Path::new("vendredi.toml") {
            self.config_source.clone_from(&after);
        }
        self.saved = after;
        self.status = "Saved and reloaded workspace".into();
        Ok(())
    }
    fn editor(&mut self, ui: &mut egui::Ui) {
        let title = match self.view {
            View::Scripts => "Scripts",
            View::Documents => "Documents",
            View::Rules => "Rules",
            _ => "Settings",
        };
        ui.heading(title);
        ui.add_space(8.);
        if self.files().is_empty() {
            ui.label(format!(
                "No {} configured in this workspace.",
                title.to_lowercase()
            ));
            return;
        }
        let dirty = self.edit != self.saved;
        ui.horizontal(|ui| {
            let mut next = self.edit_file.clone();
            ui.add_enabled_ui(!dirty, |ui| {
                egui::ComboBox::from_id_salt("files")
                    .selected_text(next.display().to_string())
                    .show_ui(ui, |ui| {
                        for file in self.files() {
                            let name = file.display().to_string();
                            ui.selectable_value(&mut next, file, name);
                        }
                    });
            });
            if next != self.edit_file {
                match fs::read_to_string(self.root.join(&next)) {
                    Ok(text) => {
                        self.edit_file = next;
                        self.edit = text.clone();
                        self.saved = text;
                    }
                    Err(e) => self.status = e.to_string(),
                }
            }
            if ui
                .add_enabled(dirty, egui::Button::new("Save and reload"))
                .clicked()
                && let Err(e) = self.save()
            {
                self.status = format!("{e:#}");
            }
            if ui.button("Reload file").clicked() {
                match fs::read_to_string(self.root.join(&self.edit_file)) {
                    Ok(text) => {
                        self.saved = text.clone();
                        self.edit = text;
                    }
                    Err(e) => self.status = e.to_string(),
                }
            }
        });
        egui::ScrollArea::both().show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.edit)
                    .code_editor()
                    .desired_rows(32)
                    .desired_width(f32::INFINITY),
            );
        });
    }
}
fn replace(path: &Path, text: &str) -> Result<()> {
    let mut f = tempfile::NamedTempFile::new_in(path.parent().context("editor path")?)?;
    f.as_file()
        .set_permissions(fs::metadata(path)?.permissions())?;
    f.write_all(text.as_bytes())?;
    f.as_file().sync_all()?;
    f.persist(path)?;
    Ok(())
}
impl eframe::App for Desktop {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.show(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
    use std::{
        net::TcpListener,
        time::{Duration, Instant},
    };
    fn frame(app: &mut Desktop, ctx: &egui::Context, events: Vec<Event>) -> egui::FullOutput {
        ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1080.0, 680.0))),
                events,
                ..Default::default()
            },
            |ctx| app.show(ctx),
        )
    }
    pub(super) fn click(app: &mut Desktop, ctx: &egui::Context, label: &str) -> Result<()> {
        let output = frame(app, ctx, vec![]);
        let pos = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == label => {
                    Some(t.visual_bounding_rect().center())
                }
                _ => None,
            })
            .with_context(|| format!("missing widget {label}"))?;
        for pressed in [true, false] {
            frame(
                app,
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
            );
        }
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
        while !text_matches_reply(&app.detail) {
            ensure!(Instant::now() < deadline, "inspect timed out");
            app.drain();
        }
        assert!(app.detail["raw"].as_str().unwrap().contains("ORDRSP"));
        click(&mut app, &ctx, "BGM")?;
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
        while app.transfers.values().filter(|r| !r.outgoing).count() != 2 {
            ensure!(Instant::now() < deadline, "replay timed out");
            app.drain();
        }
        click(&mut app, &ctx, "▪  Stop")?;
        while app.listening {
            ensure!(Instant::now() < deadline, "stop timed out");
            app.drain();
        }
        click(&mut app, &ctx, "▷  Start")?;
        while !app.listening {
            ensure!(Instant::now() < deadline, "restart timed out");
            app.drain();
        }
        click(&mut app, &ctx, "Settings")?;
        // Drive the multiline editor with the same keyboard events as Select All and paste.
        let changed = app
            .edit
            .replace("message = \"ORDERS\"", "message = \"INVOIC\"");
        let edit_pos = Pos2::new(300.0, 170.0);
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
