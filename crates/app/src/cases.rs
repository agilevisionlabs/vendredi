//! `vendredi case` commands that work on the files of a workspace.
use anyhow::Result;
use std::path::Path;
use vendredi_core::{
    case,
    config::Workspace,
    import::{self, Change, Incoming},
};

/// Prints each installed case and the partners it is attached to.
pub fn list(dir: &Path) -> Result<()> {
    let w = Workspace::load(dir)?;
    let own: Vec<_> = w
        .partners
        .iter()
        .map(|p| {
            if p.case_id() == case::DEFAULT {
                format!("{} (active)", p.name)
            } else {
                p.name.clone()
            }
        })
        .collect();
    let active = w.partners.iter().any(|p| p.case_id() == case::DEFAULT);
    println!(
        "{} {:<28} {:<8} Partner's own rules — {}",
        if active { "*" } else { " " },
        case::DEFAULT,
        "",
        own.join(", ")
    );
    for c in &w.cases {
        let partners: Vec<_> = w
            .partners
            .iter()
            .filter(|p| p.cases.contains(&c.id))
            .map(|p| {
                if p.case_id() == c.id {
                    format!("{} (active)", p.name)
                } else {
                    p.name.clone()
                }
            })
            .collect();
        let active = w.partners.iter().any(|p| p.case_id() == c.id);
        let tags = if c.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", c.tags.join(", "))
        };
        let attached = if partners.is_empty() {
            "not attached".into()
        } else {
            partners.join(", ")
        };
        println!(
            "{} {:<28} {:<8} {}{tags} — {attached}",
            if active { "*" } else { " " },
            c.id,
            c.standard.label(),
            c.name
        );
    }
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Fetches and checks a case or catalog and describes what installing it
/// would change.
pub fn preview(root: &Path, source: &str) -> Result<(Vec<Incoming>, Vec<Change>)> {
    let incoming = import::gather(source, &mut vendredi_transports::fetch, now())?;
    let changes = import::plan(root, &incoming)?;
    Ok((incoming, changes))
}

pub fn describe(incoming: &[Incoming], changes: &[Change]) -> Vec<String> {
    let mut lines = Vec::new();
    for (i, change) in incoming.iter().zip(changes) {
        let c = &i.bundle.case;
        let state = match change {
            Change::New => "new",
            Change::Unchanged => "unchanged",
            Change::Changed(_) => "changed",
        };
        lines.push(format!("{state:<9} {:<28} {}", c.id, c.name));
        if let Change::Changed(diff) = change {
            lines.extend(diff.iter().map(|d| format!("          {d}")));
        }
    }
    lines
}

/// `vendredi case import WORKSPACE SOURCE [--replace]`
pub fn import_command(dir: &Path, source: &str, replace: bool) -> Result<()> {
    // The workspace may be unloadable until the cases it attaches are imported.
    anyhow::ensure!(
        dir.join("vendredi.toml").is_file(),
        "{} has no vendredi.toml",
        dir.display()
    );
    let root = dir.canonicalize()?;
    let (incoming, changes) = preview(&root, source)?;
    for line in describe(&incoming, &changes) {
        println!("{line}");
    }
    import::apply(&root, &incoming, replace).map_err(|e| {
        anyhow::anyhow!("{e:#}. Nothing was imported; rerun with --replace to replace them")
    })?;
    Workspace::load(&root)?;
    let written = changes.iter().filter(|c| **c != Change::Unchanged).count();
    println!(
        "Imported {written} case{} into {}",
        if written == 1 { "" } else { "s" },
        root.join(case::DIRECTORY).display()
    );
    Ok(())
}

/// `vendredi case test SOURCE`: runs every expectation of a case or catalog.
pub fn test_command(source: &str) -> Result<()> {
    let incoming = import::gather(source, &mut vendredi_transports::fetch, now())?;
    let (mut passed, mut failed, mut untested) = (0, 0, Vec::new());
    for i in &incoming {
        let outcomes = vendredi_core::selftest::run(&i.bundle)?;
        if outcomes.is_empty() {
            untested.push(i.bundle.case.id.clone());
        }
        for o in outcomes {
            if o.failures.is_empty() {
                passed += 1;
                println!("ok    {} — {}", o.case, o.check);
            } else {
                failed += 1;
                println!("FAIL  {} — {}", o.case, o.check);
                for line in o.failures.iter().flat_map(|f| f.lines()) {
                    println!("      {line}");
                }
            }
        }
    }
    if !untested.is_empty() {
        println!("no expectations: {}", untested.join(", "));
    }
    println!("{passed} passed, {failed} failed");
    anyhow::ensure!(
        failed == 0,
        "{failed} case check{} failed",
        if failed == 1 { "" } else { "s" }
    );
    Ok(())
}
