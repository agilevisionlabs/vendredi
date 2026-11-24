use anyhow::{Context, Result, anyhow, bail};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tiny_http::{Method, Request, Response, Server};
use vendredi_core::{
    config::{Receive, SendChannel, Workspace},
    engine::{Delivery, Receipt},
};

pub type Recorded = Sender<Result<Receipt, String>>;
pub struct Inbound {
    pub partner: usize,
    pub bytes: Vec<u8>,
    pub metadata: Value,
    pub claim: Option<String>,
    pub recorded: Recorded,
}
pub type Ingest = Arc<dyn Fn(Inbound) -> Result<()> + Send + Sync>;
pub struct Listeners {
    server: Arc<Server>,
    _watcher: RecommendedWatcher,
    http: Option<thread::JoinHandle<()>>,
}
impl Drop for Listeners {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(t) = self.http.take() {
            let _ = t.join();
        }
    }
}

pub fn listen(w: &Workspace, ingest: Ingest) -> Result<Listeners> {
    let server = Arc::new(
        Server::http(w.listen)
            .map_err(|e| anyhow!("{e}"))
            .with_context(|| format!("listen on {}", w.listen))?,
    );
    let mut routes = HashMap::new();
    let mut folders = Vec::new();
    for (partner, p) in w.partners.iter().enumerate() {
        for channel in &p.receive {
            match channel {
                Receive::Http { path } => {
                    routes.insert(path.clone(), partner);
                }
                Receive::Folder { directory, suffix } => {
                    folders.push((partner, w.root.join(directory), suffix.clone()))
                }
            }
        }
    }
    let (wake, signals) = mpsc::channel();
    let wake_notify = wake.clone();
    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(_) => {
                let _ = wake_notify.send(());
            }
            Err(e) => eprintln!("folder watcher: {e}"),
        })?;
    for (_, dir, _) in &folders {
        watcher.watch(dir, RecursiveMode::NonRecursive)?;
    }
    wake.send(())?;
    drop(wake);
    let folder_ingest = ingest.clone();
    thread::spawn(move || watch_folders(folders, signals, folder_ingest));
    let http = server.clone();
    let handle = thread::spawn(move || {
        while let Ok(req) = http.recv() {
            let Some(&p) = routes.get(req.url()) else {
                let _ = req.respond(Response::empty(404));
                continue;
            };
            let cb = ingest.clone();
            thread::spawn(move || {
                if let Err(e) = request(req, p, &cb) {
                    eprintln!("receive: {e:#}");
                }
            });
        }
    });
    Ok(Listeners {
        server,
        _watcher: watcher,
        http: Some(handle),
    })
}

fn request(mut req: Request, partner: usize, ingest: &Ingest) -> Result<()> {
    if req.method() != &Method::Post {
        req.respond(Response::empty(405))?;
        return Ok(());
    }
    let mut bytes = Vec::new();
    req.as_reader().read_to_end(&mut bytes)?;
    let headers: Vec<_> = req
        .headers()
        .iter()
        .map(|h| (h.field.to_string(), h.value.to_string()))
        .collect();
    let metadata = json!({"path":req.url(),"headers":headers});
    let (tx, rx) = mpsc::channel();
    ingest(Inbound {
        partner,
        bytes,
        metadata,
        claim: None,
        recorded: tx,
    })?;
    let status = match rx.recv()? {
        Ok(Receipt::Accepted) => 202,
        Ok(Receipt::Error) | Err(_) => 500,
        Ok(Receipt::Slow) => {
            thread::sleep(Duration::from_secs(2));
            202
        }
        Ok(Receipt::Drop) => {
            drop(req.into_writer());
            return Ok(());
        }
    };
    req.respond(Response::empty(status))?;
    Ok(())
}

const CLAIM: &str = ".vendredi-claim-";
fn watch_folders(folders: Vec<(usize, PathBuf, String)>, signals: Receiver<()>, ingest: Ingest) {
    let mut serial = 0u64;
    while signals.recv().is_ok() {
        while signals.try_recv().is_ok() {}
        for (p, dir, suffix) in &folders {
            if let Err(e) = scan(*p, dir, suffix, &ingest, &mut serial) {
                eprintln!("scan {}: {e:#}", dir.display());
            }
        }
    }
}
fn scan(p: usize, dir: &Path, suffix: &str, ingest: &Ingest, serial: &mut u64) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let original = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow!("invalid filename"))?;
        if !original.ends_with(suffix) {
            continue;
        }
        let (file, claim, name) = if let Some(rest) = original.strip_prefix(CLAIM) {
            let Some((key, name)) = rest.split_once('_') else {
                continue;
            };
            (entry.path(), key.to_owned(), name.to_owned())
        } else {
            *serial += 1;
            let key = format!(
                "{}-{}-{serial}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            );
            let path = dir.join(format!("{CLAIM}{key}_{original}"));
            fs::rename(entry.path(), &path)?;
            (path, key, original)
        };
        let bytes = fs::read(&file)?;
        let (tx, rx) = mpsc::channel();
        ingest(Inbound {
            partner: p,
            bytes,
            metadata: json!({"file":name}),
            claim: Some(claim),
            recorded: tx,
        })?;
        rx.recv()?.map_err(|e| anyhow!("{e}"))?;
        fs::remove_file(file)?;
    }
    Ok(())
}

pub fn deliver(root: &Path, d: &Delivery) -> Result<String> {
    match &d.channel {
        SendChannel::Http { url, headers } => {
            let mut req = ureq::AgentBuilder::new()
                .redirects(0)
                .timeout(Duration::from_secs(5))
                .build()
                .post(url)
                .set("Content-Type", "application/edifact");
            for (k, v) in headers {
                req = req.set(k, v);
            }
            let res = req.send_bytes(&d.bytes)?;
            if !(200..300).contains(&res.status()) {
                bail!("HTTP {}", res.status());
            }
            Ok(format!("HTTP {}", res.status()))
        }
        SendChannel::Folder { directory } => {
            let dir = root.join(directory);
            let name = format!("vendredi-{}.edi", d.id);
            let dest = dir.join(&name);
            let tmp = dir.join(format!(".vendredi-{}.tmp", d.id));
            use std::io::Write;
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            f.write_all(&d.bytes)?;
            f.sync_all()?;
            drop(f);
            anyhow::ensure!(!dest.exists(), "destination exists: {}", dest.display());
            fs::rename(tmp, dest)?;
            Ok(name)
        }
    }
}
