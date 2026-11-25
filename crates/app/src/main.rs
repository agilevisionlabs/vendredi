mod cases;
mod gui;
mod runtime;

use anyhow::{Context, Result, bail};
use runtime::Command;
use std::{
    io::{self, BufRead},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use vendredi_core::{case, config::Workspace};

const CONFIG: &str = include_str!("../../../examples/orders/vendredi.toml");
const ORDER: &[u8] = include_bytes!("../../../examples/orders/order.edi");
const CONFIRMATION: &[u8] = include_bytes!("../../../examples/orders/confirmation.edi");
/// The starter case library, installed and attached to the sample partner.
const CASES: &[&str] = &[
    include_str!("../../../examples/cases/confirm-orders.json"),
    include_str!("../../../examples/cases/reject-orders.json"),
    include_str!("../../../examples/cases/missing-acknowledgement.json"),
    include_str!("../../../examples/cases/delayed-confirmation.json"),
    include_str!("../../../examples/cases/failing-http.json"),
    include_str!("../../../examples/cases/slow-http.json"),
    include_str!("../../../examples/cases/duplicate-reference.json"),
    include_str!("../../../examples/cases/duplicate-delivery.json"),
    include_str!("../../../examples/cases/malformed-reply.json"),
    include_str!("../../../examples/cases/wrong-count.json"),
    include_str!("../../../examples/cases/send-invoice.json"),
];

fn sample(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)?;
    let mut ids = Vec::new();
    for json in CASES {
        let bundle = case::Bundle::resolve(case::Case::parse(json.as_bytes())?, |p| {
            bail!("{} is not embedded", p.display())
        })?;
        let dir = root.join(case::DIRECTORY).join(&bundle.case.id);
        if !dir.exists() {
            bundle.write(&dir)?;
        }
        ids.push(format!("{:?}", bundle.case.id));
    }
    let config = CONFIG.replace(
        "url = \"http://127.0.0.1:8181/replies\" }",
        &format!("directory = \"replies\" }}\ncases = [{}]", ids.join(", ")),
    );
    for (name, data) in [
        ("vendredi.toml", config.as_bytes()),
        ("order.edi", ORDER),
        ("confirmation.edi", CONFIRMATION),
    ] {
        use std::io::Write;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(name))
        {
            Ok(mut f) => f.write_all(data)?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn command(args: &[String]) -> Result<Command> {
    match args {
        [cmd, p, file] if cmd == "send" => Ok(Command::Send(p.clone(), file.into(), None)),
        [cmd, p, file, f] if cmd == "send" => {
            Ok(Command::Send(p.clone(), file.into(), Some(f.clone())))
        }
        [cmd, id] if cmd == "resend" => Ok(Command::Resend(id.parse()?)),
        [cmd, id] if cmd == "inspect" => Ok(Command::Inspect(id.parse()?)),
        [cmd, p, id] if cmd == "case" => Ok(Command::UseCase(p.clone(), id.clone())),
        [cmd, p, name] if cmd == "sample" => Ok(Command::Sample(p.clone(), name.clone())),
        [cmd, p, name] if cmd == "initiate" => Ok(Command::Initiate(p.clone(), name.clone())),
        _ => bail!(
            "expected send PARTNER FILE [FUNCTION], resend ID, inspect ID, case PARTNER CASE, sample PARTNER SAMPLE or initiate PARTNER NAME"
        ),
    }
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => {
            let root = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .context("missing home directory")?
                .join("Vendredi");
            if !root.join("vendredi.toml").exists() {
                sample(&root)?;
            }
            gui::run(root)
        }
        [cmd, dir] if cmd == "gui" => gui::run(dir.into()),
        [cmd, dir] if cmd == "init" => sample(Path::new(dir)),
        [cmd, opt, dir] if cmd == "run" && opt == "--headless" => {
            let w = Workspace::load(Path::new(dir))?;
            let mut runner = runtime::start(w, Arc::new(|v| println!("{v}")), false, true)?;
            // Interactive commands share the live engine. A closed stdin is normal in CI.
            let tx = runner.tx.clone();
            std::thread::spawn(move || {
                for line in io::stdin().lock().lines().map_while(Result::ok) {
                    if line.trim() == "quit" {
                        let _ = tx.send(Command::Stop);
                        break;
                    }
                    let args: Vec<String> = if line.starts_with('[') {
                        serde_json::from_str(&line).unwrap_or_default()
                    } else {
                        line.split_whitespace().map(str::to_owned).collect()
                    };
                    match command(&args) {
                        Ok(c) => {
                            if tx.send(c).is_err() {
                                break;
                            }
                        }
                        Err(e) => eprintln!("{e}"),
                    }
                }
            });
            runner.wait()
        }
        [cmd, sub, dir] if cmd == "case" && sub == "list" => cases::list(Path::new(dir)),
        [cmd, sub, source] if cmd == "case" && sub == "test" => cases::test_command(source),
        [cmd, sub, dir, source] if cmd == "case" && sub == "import" => {
            cases::import_command(Path::new(dir), source, false)
        }
        [cmd, sub, dir, source, flag]
            if cmd == "case" && sub == "import" && flag == "--replace" =>
        {
            cases::import_command(Path::new(dir), source, true)
        }
        [cmd, sub, dir, partner, id] if cmd == "case" && sub == "use" => {
            offline(dir, vec!["case".into(), partner.clone(), id.clone()])
        }
        [cmd, dir, rest @ ..]
            if matches!(
                cmd.as_str(),
                "send" | "resend" | "inspect" | "sample" | "initiate"
            ) =>
        {
            let mut words = vec![cmd.clone()];
            words.extend_from_slice(rest);
            offline(dir, words)
        }
        _ => bail!(
            "usage: vendredi [gui WORKSPACE | init WORKSPACE | run --headless WORKSPACE | send WORKSPACE PARTNER FILE [FUNCTION] | resend WORKSPACE ID | inspect WORKSPACE ID | sample WORKSPACE PARTNER SAMPLE | initiate WORKSPACE PARTNER NAME | case list WORKSPACE | case use WORKSPACE PARTNER CASE | case import WORKSPACE PATH-OR-URL [--replace] | case test PATH-OR-URL]"
        ),
    }
}

/// Runs one command against a workspace that no other engine has open.
fn offline(dir: &str, words: Vec<String>) -> Result<()> {
    let w = Workspace::load(Path::new(dir))?;
    let failed = Arc::new(AtomicBool::new(false));
    let f = failed.clone();
    let mut runner = runtime::start(
        w,
        Arc::new(move |v| {
            if v["event"] == "error" || v["event"] == "failed" || v["error"].is_string() {
                f.store(true, Ordering::SeqCst);
            }
            println!("{v}");
        }),
        false,
        false,
    )?;
    runner.tx.send(command(&words)?)?;
    runner.stop()?;
    anyhow::ensure!(!failed.load(Ordering::SeqCst), "command failed");
    Ok(())
}
