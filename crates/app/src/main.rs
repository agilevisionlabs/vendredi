use anyhow::Result;
use eframe::egui;
use std::{collections::BTreeMap, path::Path};
use vendredi_core::{script, store::Store};

fn main() -> Result<()> {
    let workspace = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let state = Path::new(&workspace).join(".vendredi");
    std::fs::create_dir_all(&state)?;
    let store = Store::open(&state.join("vendredi.db"))?;
    store.transaction(|s| {
        s.exec(
            "CREATE TABLE IF NOT EXISTS events(id INTEGER PRIMARY KEY, body TEXT NOT NULL)",
            (),
        )
    })?;
    let input = script::Input {
        interchange: &serde_json::Value::Null,
        state: &BTreeMap::new(),
        reference: 0,
        now: 0,
    };
    script::run("function launch() {}", "launch", &input)?;
    eframe::run_simple_native("Vendredi", Default::default(), |ctx, _| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Vendredi");
        });
    })
    .map_err(|e| anyhow::anyhow!("{e}"))
}
