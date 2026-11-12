//! Show packages (`.smpack`): the show and every audio file it uses in one
//! file, to move a show to another computer (USB stick, mail, cloud).
//!
//! A ZIP archive: `show.smshow` (media paths rewritten to `media/<name>`) and
//! `media/...`. Audio is stored as is (it does not compress); the showfile is
//! deflated. Importing unpacks into a new folder and returns the showfile.

use crate::model::{CueKind, Show};
use crate::showfile::{self, resolve_media};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const EXTENSION: &str = "smpack";
const SHOW_ENTRY: &str = "show.smshow";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    pub files: usize,
    pub bytes: u64,
    /// Media that could not be read (cue keeps its path, file missing in the package).
    pub missing: Vec<String>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Write `show` (whose media paths are relative to `show_dir`) with its
/// media to `out`.
pub fn export(show: &Show, show_dir: Option<&Path>, out: &Path) -> Result<ExportReport, String> {
    let mut show = show.clone();
    let mut names: HashMap<PathBuf, String> = HashMap::new();
    let mut used: HashMap<String, PathBuf> = HashMap::new();
    let mut missing = Vec::new();
    // Decide the name inside the package for every distinct source file.
    show.for_each_cue_mut(|cue| {
        let CueKind::Audio(a) = &mut cue.kind else { return };
        if a.file.is_empty() {
            return;
        }
        let src = resolve_media(show_dir, &a.file);
        if !src.is_file() {
            missing.push(a.file.clone());
            return;
        }
        let name = names.entry(src.clone()).or_insert_with(|| {
            let base = src
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("media")
                .to_string();
            let mut name = base.clone();
            let mut i = 2;
            while used.get(&name).is_some_and(|p| *p != src) {
                name = format!("{i}-{base}");
                i += 1;
            }
            used.insert(name.clone(), src.clone());
            name
        });
        a.file = format!("media/{name}");
    });

    let tmp = out.with_extension("smpack.part");
    let result = (|| -> Result<ExportReport, String> {
        let mut zip = ZipWriter::new(BufWriter::new(File::create(&tmp).map_err(err)?));
        let deflate = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let stored = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .large_file(true);
        zip.start_file(SHOW_ENTRY, deflate).map_err(err)?;
        zip.write_all(showfile::to_string(&show).map_err(err)?.as_bytes())
            .map_err(err)?;
        let mut bytes = 0;
        let mut files: Vec<(&String, &PathBuf)> = used.iter().collect();
        files.sort();
        for (name, src) in &files {
            zip.start_file(format!("media/{name}"), stored).map_err(err)?;
            let mut f = BufReader::new(File::open(src).map_err(|e| format!("{}: {e}", src.display()))?);
            bytes += io::copy(&mut f, &mut zip).map_err(err)?;
        }
        zip.finish().map_err(err)?.flush().map_err(err)?;
        Ok(ExportReport {
            files: files.len(),
            bytes,
            missing: missing.clone(),
        })
    })();
    match result {
        Ok(r) => {
            fs::rename(&tmp, out).map_err(err)?;
            Ok(r)
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Unpack `package` into a new folder inside `dest_parent` (named after the
/// package, made unique) and return the path of the showfile.
pub fn import(package: &Path, dest_parent: &Path) -> Result<PathBuf, String> {
    let mut zip = ZipArchive::new(BufReader::new(File::open(package).map_err(err)?))
        .map_err(|e| format!("not a show package: {e}"))?;
    let has_show = (0..zip.len()).any(|i| zip.by_index(i).is_ok_and(|f| f.name().is_ok_and(|n| n == SHOW_ENTRY)));
    if !has_show {
        return Err("not a show package (no show.smshow inside)".into());
    }
    let stem = package
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Show")
        .to_string();
    let mut dir = dest_parent.join(&stem);
    let mut i = 2;
    while dir.exists() {
        dir = dest_parent.join(format!("{stem} {i}"));
        i += 1;
    }
    fs::create_dir_all(&dir).map_err(err)?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(err)?;
        // Never write outside the target folder (`../` in a crafted archive).
        let Some(rel) = f.enclosed_name() else {
            continue;
        };
        let target = dir.join(rel);
        if f.is_dir() {
            fs::create_dir_all(&target).map_err(err)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(err)?;
        }
        let mut out = BufWriter::new(File::create(&target).map_err(err)?);
        io::copy(&mut f, &mut out).map_err(err)?;
        out.flush().map_err(err)?;
    }
    let src = dir.join(SHOW_ENTRY);
    let show_path = dir.join(format!("{stem}.smshow"));
    fs::rename(&src, &show_path).map_err(err)?;
    // Validate it loads.
    showfile::load(&show_path).map_err(|e| format!("the show in the package is damaged: {e}"))?;
    Ok(show_path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::model::{AudioCue, Cue};

    #[test]
    fn export_import_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let show_dir = dir.path().join("orig");
        let other = dir.path().join("elsewhere");
        fs::create_dir_all(show_dir.join("media")).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(show_dir.join("media/a.wav"), b"AAAA").unwrap();
        fs::write(other.join("a.wav"), b"BBBB").unwrap(); // same name, other file
        let mut show = Show::default();
        show.name = "Gala".into();
        for file in [
            "media/a.wav".to_string(),
            other.join("a.wav").display().to_string(),
            "media/a.wav".to_string(),
            "gone.wav".to_string(),
        ] {
            let id = show.alloc_id();
            show.cue_lists[0].cues.push(Cue {
                id,
                kind: CueKind::Audio(AudioCue {
                    file,
                    ..AudioCue::default()
                }),
                ..Cue::default()
            });
        }
        let pkg = dir.path().join("Gala.smpack");
        let r = export(&show, Some(&show_dir), &pkg).unwrap();
        assert_eq!(r.files, 2);
        assert_eq!(r.bytes, 8);
        assert_eq!(r.missing, vec!["gone.wav".to_string()]);

        let dest = dir.path().join("import");
        fs::create_dir_all(&dest).unwrap();
        let path = import(&pkg, &dest).unwrap();
        assert_eq!(path, dest.join("Gala").join("Gala.smshow"));
        let loaded = showfile::load(&path).unwrap();
        let files: Vec<String> = loaded.cue_lists[0]
            .cues
            .iter()
            .map(|c| match &c.kind {
                CueKind::Audio(a) => a.file.clone(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(files[0], "media/a.wav");
        assert_eq!(files[1], "media/2-a.wav");
        assert_eq!(files[2], "media/a.wav");
        let base = path.parent().unwrap();
        assert_eq!(fs::read(base.join("media/a.wav")).unwrap(), b"AAAA");
        assert_eq!(fs::read(base.join("media/2-a.wav")).unwrap(), b"BBBB");
        // Importing twice does not overwrite.
        assert_eq!(import(&pkg, &dest).unwrap(), dest.join("Gala 2").join("Gala.smshow"));
    }

    #[test]
    fn rejects_non_packages_and_path_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("x.smpack");
        fs::write(&bogus, b"no zip").unwrap();
        assert!(import(&bogus, dir.path()).is_err());
        // A crafted archive with ../ is unpacked without escaping.
        let evil = dir.path().join("evil.smpack");
        let mut z = ZipWriter::new(File::create(&evil).unwrap());
        z.start_file("../../escape.txt", SimpleFileOptions::default()).unwrap();
        z.write_all(b"x").unwrap();
        z.start_file(SHOW_ENTRY, SimpleFileOptions::default()).unwrap();
        z.write_all(showfile::to_string(&Show::default()).unwrap().as_bytes()).unwrap();
        z.finish().unwrap();
        let dest = dir.path().join("in");
        fs::create_dir_all(&dest).unwrap();
        import(&evil, &dest).unwrap();
        assert!(!dir.path().join("escape.txt").exists());
    }
}
