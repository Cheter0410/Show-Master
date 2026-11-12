//! Writes the cue sheet of a showfile: `cargo run -p sm-core --example cuesheet -- show.smshow out.pdf`
use std::path::Path;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(src), Some(dst)) = (args.get(1), args.get(2)) else {
        return Err("usage: cuesheet <show.smshow> <out.pdf|out.csv>".into());
    };
    let show = sm_core::showfile::load(Path::new(src)).map_err(|e| e.to_string())?;
    let bytes = if dst.ends_with(".csv") {
        sm_core::cuesheet::to_csv(&show).into_bytes()
    } else {
        sm_core::cuesheet::to_pdf(&show, "example")
    };
    std::fs::write(dst, bytes).map_err(|e| e.to_string())
}
