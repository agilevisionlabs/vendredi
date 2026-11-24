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
use vendredi_core::config::Workspace;

const CONFIG: &str = include_str!("../../../examples/orders/vendredi.toml");
const ORDER: &[u8] = include_bytes!("../../../examples/orders/order.edi");
const CONFIRMATION: &[u8] = include_bytes!("../../../examples/orders/confirmation.edi");

fn sample(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)?;
    let config = CONFIG.replace(
        "url = \"http://127.0.0.1:8181/replies\"",
        "directory = \"replies\"",
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
        _ => bail!("expected send PARTNER FILE [FUNCTION], resend ID or inspect ID"),
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
        [cmd, dir, rest @ ..] if matches!(cmd.as_str(), "send" | "resend" | "inspect") => {
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
            let mut words = vec![cmd.clone()];
            words.extend_from_slice(rest);
            runner.tx.send(command(&words)?)?;
            runner.stop()?;
            anyhow::ensure!(!failed.load(Ordering::SeqCst), "command failed");
            Ok(())
        }
        _ => bail!(
            "usage: vendredi [gui WORKSPACE | init WORKSPACE | run --headless WORKSPACE | send WORKSPACE PARTNER FILE [FUNCTION] | resend WORKSPACE ID | inspect WORKSPACE ID]"
        ),
    }
}
