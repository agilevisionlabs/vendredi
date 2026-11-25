#[path = "src/script_config.rs"]
mod script_config;

use flate2::{Compression, write::GzEncoder};
use std::{env, fs, io::Write, path::PathBuf, process::Command};

fn main() {
    for path in [
        "../script-guest",
        "../../Cargo.toml",
        "../../Cargo.lock",
        "src/script_config.rs",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let build = out.join("guest-build");
    let status = Command::new(env::var_os("CARGO").unwrap())
        .args([
            "build",
            "--locked",
            "--release",
            "--target",
            "wasm32-wasip1",
            "-p",
            "vendredi-script-guest",
            "--target-dir",
        ])
        .arg(&build)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .expect("build the Boa guest");
    assert!(
        status.success(),
        "Boa guest build failed (requires rustup target add wasm32-wasip1)"
    );
    let wasm = fs::read(build.join("wasm32-wasip1/release/vendredi_script_guest.wasm")).unwrap();
    let engine = script_config::engine().unwrap();
    let artifact = engine
        .precompile_module(&wasm)
        .expect("compile bundled Boa for Pulley");
    let mut gzip = GzEncoder::new(
        fs::File::create(out.join("boa.cwasm.gz")).unwrap(),
        Compression::best(),
    );
    gzip.write_all(&artifact).unwrap();
    gzip.finish().unwrap();
}
