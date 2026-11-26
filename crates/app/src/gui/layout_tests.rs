//! Layout checks that run over every page at several window sizes, so
//! alignment problems fail a test instead of showing up in screenshots.
use super::tests::{click, render};
use super::*;
use egui::Rect;

/// The widest the sidebar gets.
const SIDEBAR_MAX: f32 = 200.;

/// The bounds of buttons, pickers and text fields, from the accessibility
/// tree, with their labels.
fn controls(output: &egui::FullOutput) -> Vec<(String, Rect)> {
    use egui::accesskit::Role;
    let Some(update) = &output.platform_output.accesskit_update else {
        return vec![];
    };
    update
        .nodes
        .iter()
        .filter(|(_, n)| {
            matches!(
                n.role(),
                Role::Button | Role::ComboBox | Role::TextInput | Role::MultilineTextInput
            )
        })
        .filter_map(|(_, n)| {
            let b = n.bounds()?;
            let rect = Rect::from_min_max(
                egui::pos2(b.x0 as f32, b.y0 as f32),
                egui::pos2(b.x1 as f32, b.y1 as f32),
            );
            (n.role() != Role::MultilineTextInput)
                .then(|| (n.label().unwrap_or_default().to_owned(), rect))
        })
        .collect()
}

/// Problems on one rendered page: controls on one line with different
/// heights or centres, controls of an unexpected height, and shapes
/// outside the window.
pub(super) fn problems(output: &egui::FullOutput, screen: Rect) -> Vec<String> {
    // The sidebar's navigation rows have their own fixed height.
    let controls: Vec<_> = controls(output)
        .into_iter()
        .filter(|(_, r)| r.left() > SIDEBAR_MAX)
        .collect();
    let mut problems = Vec::new();
    for (i, (name, a)) in controls.iter().enumerate() {
        if ![SMALL_BUTTON, BUTTON]
            .iter()
            .any(|h| (a.height() - h).abs() < 0.5)
        {
            problems.push(format!("{name:?} is {} tall at {a:?}", a.height()));
        }
        for (other, b) in &controls[i + 1..] {
            let overlap = a.top() < b.bottom() - 1. && b.top() < a.bottom() - 1.;
            let nested = a.contains_rect(*b) || b.contains_rect(*a);
            if overlap && !nested && (a.center().y - b.center().y).abs() > 0.5 {
                problems.push(format!(
                    "{name:?} and {other:?} share a line but their centres differ by {}",
                    a.center().y - b.center().y
                ));
            }
        }
    }
    for s in &output.shapes {
        let r = s.shape.visual_bounding_rect();
        if r.is_positive()
            && s.clip_rect.intersects(r)
            && !screen.expand(1.).contains_rect(r.intersect(s.clip_rect))
        {
            problems.push(format!("shape outside the window: {r:?}"));
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

#[test]
#[ignore = "opens a local workspace listener"]
fn every_page_lines_up_at_every_size() -> Result<()> {
    let dir = tempfile::tempdir()?;
    crate::sample(dir.path())?;
    let file = dir.path().join("vendredi.toml");
    fs::write(&file, fs::read_to_string(&file)?.replace(":8180", ":0"))?;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut app = Desktop::open(dir.path().into(), &ctx)?;
    let mut report = Vec::new();
    for size in [
        egui::vec2(1080., 680.),
        egui::vec2(1440., 900.),
        egui::vec2(1920., 1200.),
    ] {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("test screen size"), size));
        for page in [
            "All traffic",
            "Needs attention",
            "Supplier",
            "Cases",
            "Scripts",
            "Documents",
            "Settings",
        ] {
            click(&mut app, &ctx, page)?;
            let output = render(&ctx, vec![], |ctx| app.show(ctx));
            for p in problems(&output, Rect::from_min_size(egui::Pos2::ZERO, size)) {
                report.push(format!("{page} at {}×{}: {p}", size.x, size.y));
            }
        }
    }
    assert!(report.is_empty(), "{}", report.join("\n"));
    Ok(())
}
