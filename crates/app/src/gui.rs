use crate::runtime::{self, Command, Running};
use anyhow::{Context, Result, anyhow, ensure};
use eframe::egui;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
};
use vendredi_core::{
    config::{Action, Receive, SendChannel, Workspace},
    engine::Delivery,
};

pub fn run(root: PathBuf) -> Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 600.0])
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

#[derive(Default)]
struct Transfer {
    partner: String,
    kind: String,
    status: String,
    outgoing: bool,
}

struct Desktop {
    root: PathBuf,
    workspace: Workspace,
    running: Running,
    events: mpsc::Receiver<Value>,
    output: runtime::Output,
    transfers: BTreeMap<i64, Transfer>,
    selected: Option<i64>,
    detail: Value,
    tab: usize,
    partner: usize,
    send_file: String,
    function: String,
    edit_file: PathBuf,
    saved: String,
    edit: String,
    status: String,
}
impl Desktop {
    fn open(root: PathBuf, ctx: &egui::Context) -> Result<Self> {
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
            tab: 0,
            partner: 0,
            send_file,
            function: String::new(),
            edit_file,
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
    fn drain(&mut self) {
        while let Ok(v) = self.events.try_recv() {
            let event = v["event"].as_str().unwrap_or("");
            match event {
                "detail" => {
                    self.detail = v["value"].clone();
                }
                "error" => self.status = v["error"].as_str().unwrap_or("command failed").into(),
                "ready" => {
                    self.status = format!("Listening on {}", v["listen"].as_str().unwrap_or(""))
                }
                "action" if v["error"].is_string() => {
                    self.status = v["error"].as_str().unwrap().into()
                }
                _ => {}
            }
            if let Some(id) = v["transfer"].as_i64() {
                let t = self.transfers.entry(id).or_default();
                if let Some(p) = v["partner"].as_str() {
                    t.partner = p.into();
                }
                match event {
                    "received" => {
                        t.status = "Received".into();
                        t.kind = "Inbound".into();
                    }
                    "outbound" | "resent" => {
                        t.status = "Pending".into();
                        t.outgoing = true;
                        t.kind = v["type"].as_str().unwrap_or("Resend").into();
                    }
                    "delivered" => t.status = "Delivered".into(),
                    "failed" => t.status = "Failed".into(),
                    "interrupted" => t.status = "Interrupted".into(),
                    "finding" => t.status = "Validation finding".into(),
                    _ => {}
                }
                if self.selected == Some(id) {
                    self.command(Command::Inspect(id));
                }
            }
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
    fn traffic(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("partner")
                .selected_text(&self.workspace.partners[self.partner].name)
                .show_ui(ui, |ui| {
                    for (index, p) in self.workspace.partners.iter().enumerate() {
                        ui.selectable_value(&mut self.partner, index, &p.name);
                    }
                });
            if self.root.join("order.edi").is_file()
                && ui.button("Send sample order").clicked()
                && let Err(e) = self.sample()
            {
                self.status = format!("{e:#}");
            }
        });
        ui.collapsing("Partner-initiated send", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Document or script");
                ui.text_edit_singleline(&mut self.send_file);
                ui.label("Function (scripts)");
                ui.text_edit_singleline(&mut self.function);
                if ui.button("Send").clicked() {
                    let f =
                        (!self.function.trim().is_empty()).then(|| self.function.trim().to_owned());
                    self.command(Command::Send(
                        self.workspace.partners[self.partner].name.clone(),
                        self.send_file.trim().into(),
                        f,
                    ));
                }
            });
        });
        ui.separator();
        ui.columns(2, |cols| {
            cols[0].heading("Traffic");
            egui::ScrollArea::vertical()
                .id_salt("traffic")
                .show(&mut cols[0], |ui| {
                    let mut selected = None;
                    for (id, t) in self.transfers.iter().rev() {
                        let label = format!("#{id}   {}   {}   {}", t.partner, t.kind, t.status);
                        if ui
                            .selectable_label(self.selected == Some(*id), label)
                            .clicked()
                        {
                            selected = Some(*id);
                        }
                    }
                    if let Some(id) = selected {
                        self.selected = Some(id);
                        self.detail = Value::Null;
                        self.command(Command::Inspect(id));
                    }
                    if self.transfers.is_empty() {
                        ui.label("Waiting for the first interchange.");
                    }
                });
            let ui = &mut cols[1];
            ui.heading("Transfer details");
            if let Some(id) = self.selected {
                if self.transfers.get(&id).is_some_and(|t| t.outgoing)
                    && ui.button("Resend transfer").clicked()
                {
                    self.command(Command::Resend(id));
                }
                egui::ScrollArea::both().id_salt("detail").show(ui, |ui| {
                    if let Some(raw) = self.detail["raw"].as_str() {
                        ui.label("Raw interchange");
                        ui.monospace(raw);
                        for (title, key) in [
                            ("Exact bytes", "bytes"),
                            ("Acknowledgements", "interchanges"),
                            ("Parsed segments and findings", "parsed"),
                            ("Execution history", "events"),
                            ("Transport metadata", "metadata"),
                        ] {
                            ui.collapsing(title, |ui| {
                                ui.monospace(
                                    serde_json::to_string_pretty(&self.detail[key])
                                        .unwrap_or_default(),
                                );
                            });
                        }
                    }
                });
            } else {
                ui.label("Select a transfer to inspect its bytes and history.");
            }
        });
    }
    fn files(&self) -> Vec<PathBuf> {
        let mut paths = vec![PathBuf::from("vendredi.toml")];
        for p in &self.workspace.partners {
            for a in p.rules.iter().flat_map(|r| &r.actions) {
                match a {
                    Action::Document { document } => paths.push(document.clone()),
                    Action::Script { script, .. } => paths.push(script.clone()),
                    _ => {}
                }
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }
    fn save(&mut self) -> Result<()> {
        let file = self.root.join(&self.edit_file);
        ensure!(
            fs::read_to_string(&file)? == self.saved,
            "file changed on disk; reopen it before saving"
        );
        replace(&file, &self.edit)?;
        let w = match Workspace::load(&self.root) {
            Ok(w) => w,
            Err(e) => {
                replace(&file, &self.saved)?;
                return Err(e);
            }
        };
        self.running.stop()?;
        match runtime::start(w.clone(), self.output.clone(), false, true) {
            Ok(r) => {
                self.running = r;
                self.workspace = w;
                self.partner = 0;
                self.saved = self.edit.clone();
                self.status = "Saved and reloaded workspace".into();
                Ok(())
            }
            Err(e) => {
                replace(&file, &self.saved)?;
                self.running =
                    runtime::start(self.workspace.clone(), self.output.clone(), false, true)?;
                Err(e)
            }
        }
    }
    fn editor(&mut self, ui: &mut egui::Ui) {
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
impl Desktop {
    fn show(&mut self, ctx: &egui::Context) {
        self.drain();
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Vendredi");
                ui.separator();
                ui.selectable_value(&mut self.tab, 0, "Exchange");
                ui.selectable_value(&mut self.tab, 1, "Workspace");
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.label(&self.status);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.tab == 0 {
                self.traffic(ui);
            } else {
                self.editor(ui);
            }
        });
    }
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
    fn click(app: &mut Desktop, ctx: &egui::Context, label: &str) -> Result<()> {
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
        let (&id, t) = app.transfers.last_key_value().unwrap();
        let label = format!("#{id}   {}   {}   {}", t.partner, t.kind, t.status);
        click(&mut app, &ctx, &label)?;
        while app.detail.is_null() {
            ensure!(Instant::now() < deadline, "inspect timed out");
            app.drain();
        }
        assert!(app.detail["raw"].as_str().unwrap().contains("ORDRSP"));
        click(&mut app, &ctx, "Workspace")?;
        // Drive the multiline editor with the same keyboard events as Select All and paste.
        let changed = app
            .edit
            .replace("message = \"ORDERS\"", "message = \"INVOIC\"");
        let edit_pos = Pos2::new(80.0, 125.0);
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
