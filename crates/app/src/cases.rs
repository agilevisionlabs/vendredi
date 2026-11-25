//! `vendredi case` commands that work on the files of a workspace.
use anyhow::Result;
use std::path::Path;
use vendredi_core::{case, config::Workspace};

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
