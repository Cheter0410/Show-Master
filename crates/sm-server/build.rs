//! Copies the built web UI (`ui/dist`) into OUT_DIR so it can be embedded
//! into the binary. Without a UI build a placeholder page is embedded, so
//! `cargo build` never depends on Node being installed.

use std::fs;
use std::path::Path;

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn main() {
    let out = std::env::var("OUT_DIR").unwrap_or_default();
    let dest = Path::new(&out).join("ui");
    let _ = fs::remove_dir_all(&dest);
    let dist = Path::new("../../ui/dist");
    println!("cargo:rerun-if-changed=../../ui/dist");
    println!("cargo:rerun-if-changed=build.rs");
    let ok = dist.join("index.html").is_file() && copy_dir(dist, &dest).is_ok();
    if !ok {
        let _ = fs::create_dir_all(&dest);
        let page = "<!doctype html><meta charset=utf-8><title>Show Master</title>\
            <body style=\"font-family:sans-serif;background:#111;color:#eee;padding:2em\">\
            <h1>Show Master</h1><p>The web UI has not been built. Run <code>npm ci && npm run build</code> in <code>ui/</code> and rebuild.</p>\
            <p>The engine and the API are running: <a style=\"color:#8cf\" href=\"/api/v1/state\">/api/v1/state</a></p>";
        let _ = fs::write(dest.join("index.html"), page);
    }
}
