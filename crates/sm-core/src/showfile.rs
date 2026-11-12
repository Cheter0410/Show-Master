//! Showfile persistence: versioned JSON, migrations, atomic writes, rotating
//! backups, autosave/recovery and "collect show folder".

use crate::model::{CueKind, Show, SCHEMA_VERSION};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const EXTENSION: &str = "smshow";
const MAX_BACKUPS: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum ShowfileError {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("showfile is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("showfile was written by a newer version (schema {found}, this build supports up to {supported})")]
    TooNew { found: u64, supported: u32 },
    #[error("showfile is malformed: {0}")]
    Malformed(String),
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> ShowfileError + '_ {
    move |source| ShowfileError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// v4 added the "smooth" fade curve and made it the default: audio cues
/// still on the old default (equal power) get the softer curve.
fn migrate_v3_to_v4(doc: &mut Value) {
    fn walk(cue: &mut Value) {
        let kind = &mut cue["kind"];
        if kind["type"] == "audio" && kind["fade_curve"] == "equal_power" {
            kind["fade_curve"] = Value::from("smooth");
        }
        if let Some(kids) = kind.get_mut("children").and_then(Value::as_array_mut) {
            kids.iter_mut().for_each(walk);
        }
    }
    if let Some(lists) = doc.get_mut("cue_lists").and_then(Value::as_array_mut) {
        for list in lists {
            if let Some(cues) = list.get_mut("cues").and_then(Value::as_array_mut) {
                cues.iter_mut().for_each(walk);
            }
        }
    }
}

/// Upgrade a raw showfile document to the current schema, step by step.
pub fn migrate(mut doc: Value) -> Result<Value, ShowfileError> {
    let mut version = doc
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| ShowfileError::Malformed("missing schema_version".into()))?;
    if version > u64::from(SCHEMA_VERSION) {
        return Err(ShowfileError::TooNew {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    if version == 0 {
        return Err(ShowfileError::Malformed(
            "schema_version 0 is invalid".into(),
        ));
    }
    while version < u64::from(SCHEMA_VERSION) {
        match version {
            1 => migrate_v1_to_v2(&mut doc),
            // v3 added OSC cues and `settings.panic_send`; nothing to convert.
            // The bump makes older versions refuse such files cleanly.
            2 => {}
            3 => migrate_v3_to_v4(&mut doc),
            other => {
                return Err(ShowfileError::Malformed(format!(
                    "no migration from schema {other}"
                )))
            }
        }
        version += 1;
        doc["schema_version"] = Value::from(version);
    }
    Ok(doc)
}

/// v1 stored audio levels as linear `volume` (0..1) and the continue mode as
/// `"continue": "none" | "continue" | "follow"`. v2 uses `gain_db` and
/// `continue_mode`.
fn migrate_v1_to_v2(doc: &mut Value) {
    fn fix_cues(cues: &mut Value) {
        let Some(arr) = cues.as_array_mut() else {
            return;
        };
        for cue in arr {
            let Some(obj) = cue.as_object_mut() else {
                continue;
            };
            if let Some(old) = obj.remove("continue") {
                let mode = match old.as_str() {
                    Some("continue") => "auto_continue",
                    Some("follow") => "auto_follow",
                    _ => "do_not_continue",
                };
                obj.insert("continue_mode".into(), Value::from(mode));
            }
            if let Some(kind) = obj.get_mut("kind").and_then(Value::as_object_mut) {
                if let Some(vol) = kind.remove("volume").and_then(|v| v.as_f64()) {
                    let db = if vol <= 0.000_001 {
                        -120.0
                    } else {
                        20.0 * vol.log10()
                    };
                    kind.insert("gain_db".into(), Value::from((db * 100.0).round() / 100.0));
                }
                if let Some(children) = kind.get_mut("children") {
                    fix_cues(children);
                }
            }
        }
    }
    if let Some(lists) = doc.get_mut("cue_lists").and_then(Value::as_array_mut) {
        for list in lists {
            if let Some(cues) = list.get_mut("cues") {
                fix_cues(cues);
            }
        }
    }
}

/// Parse showfile text (any supported schema version).
pub fn parse(text: &str) -> Result<Show, ShowfileError> {
    let doc: Value = serde_json::from_str(text)?;
    let doc = migrate(doc)?;
    let mut show: Show = serde_json::from_value(doc)?;
    show.schema_version = SCHEMA_VERSION;
    show.assign_missing_ids();
    Ok(show)
}

pub fn to_string(show: &Show) -> Result<String, ShowfileError> {
    let mut s = serde_json::to_string_pretty(show)?;
    s.push('\n');
    Ok(s)
}

pub fn load(path: &Path) -> Result<Show, ShowfileError> {
    let text = fs::read_to_string(path).map_err(io_err(path))?;
    parse(&text)
}

/// Write `data` to `path` atomically: temp file in the same directory,
/// fsync, rename. A crash never leaves a half-written file behind.
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), ShowfileError> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).map_err(io_err(dir))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("show");
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp).map_err(io_err(&tmp))?;
        f.write_all(data).map_err(io_err(&tmp))?;
        f.sync_all().map_err(io_err(&tmp))?;
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(io_err(path)(e));
    }
    // Persist the rename itself where the platform allows it.
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Save the show; the previous file (if any) is moved into `backups/` first.
pub fn save(show: &Show, path: &Path) -> Result<(), ShowfileError> {
    let text = to_string(show)?;
    if path.exists() {
        backup(path)?;
    }
    atomic_write(path, text.as_bytes())?;
    let _ = fs::remove_file(autosave_path(path));
    Ok(())
}

fn backup(path: &Path) -> Result<(), ShowfileError> {
    let dir = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("backups");
    fs::create_dir_all(&dir).map_err(io_err(&dir))?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("show");
    let mut target = dir.join(format!("{stem}.{}.{EXTENSION}", unix_now()));
    let mut n = 1;
    while target.exists() {
        target = dir.join(format!("{stem}.{}-{n}.{EXTENSION}", unix_now()));
        n += 1;
    }
    fs::copy(path, &target).map_err(io_err(&target))?;
    // Rotate: keep the newest MAX_BACKUPS for this show.
    let prefix = format!("{stem}.");
    let mut olds: Vec<PathBuf> = fs::read_dir(&dir)
        .map_err(io_err(&dir))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix))
        })
        .collect();
    olds.sort_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
    while olds.len() > MAX_BACKUPS {
        let _ = fs::remove_file(olds.remove(0));
    }
    Ok(())
}

/// Path of the autosave companion of a showfile.
pub fn autosave_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".autosave");
    PathBuf::from(os)
}

pub fn autosave(show: &Show, path: &Path) -> Result<(), ShowfileError> {
    atomic_write(&autosave_path(path), to_string(show)?.as_bytes())
}

/// If an autosave newer than the showfile exists (i.e. the app did not exit
/// cleanly after unsaved changes), return it.
pub fn recoverable(path: &Path) -> Option<Show> {
    let auto = autosave_path(path);
    let auto_time = fs::metadata(&auto).and_then(|m| m.modified()).ok()?;
    if let Ok(saved) = fs::metadata(path).and_then(|m| m.modified()) {
        if saved > auto_time {
            return None;
        }
    }
    load(&auto).ok()
}

/// Resolve a media reference against the show directory.
pub fn resolve_media(show_dir: Option<&Path>, file: &str) -> PathBuf {
    let p = Path::new(file);
    match show_dir {
        Some(dir) if p.is_relative() => dir.join(p),
        _ => p.to_path_buf(),
    }
}

/// Store `file` relative to the show directory when it lives inside it.
pub fn relativize_media(show_dir: Option<&Path>, file: &Path) -> String {
    let rel = show_dir
        .and_then(|d| file.strip_prefix(d).ok())
        .unwrap_or(file);
    rel.to_string_lossy().replace('\\', "/")
}

pub fn hash_file(path: &Path) -> Result<String, ShowfileError> {
    let mut hasher = blake3::Hasher::new();
    let mut f = fs::File::open(path).map_err(io_err(path))?;
    std::io::copy(&mut f, &mut hasher).map_err(io_err(path))?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Copy all referenced media into `<show dir>/media/`, rewrite the cue paths
/// to be relative and record content hashes. Returns the number of files
/// copied; files that cannot be read are reported in the error list.
pub fn collect_media(show: &mut Show, show_dir: &Path) -> (usize, Vec<String>) {
    gather_media(show, show_dir, false)
}

/// Like [`collect_media`], but `move_files` moves the media into the show
/// folder instead of copying (the originals are gone afterwards; a file
/// used by several cues is moved once).
pub fn gather_media(show: &mut Show, show_dir: &Path, move_files: bool) -> (usize, Vec<String>) {
    let media_dir = show_dir.join("media");
    let mut done = 0;
    let mut errors = Vec::new();
    if let Err(e) = fs::create_dir_all(&media_dir) {
        return (0, vec![format!("{}: {e}", media_dir.display())]);
    }
    // Source -> (destination, hash), so a file shared by cues is handled once.
    let mut seen: std::collections::HashMap<PathBuf, (PathBuf, String)> = Default::default();
    show.for_each_cue_mut(|cue| {
        let CueKind::Audio(a) = &mut cue.kind else {
            return;
        };
        if a.file.is_empty() {
            return;
        }
        let src = resolve_media(Some(show_dir), &a.file);
        if let Some((dest, hash)) = seen.get(&src) {
            a.file = relativize_media(Some(show_dir), dest);
            a.hash = Some(hash.clone());
            return;
        }
        let hash = match hash_file(&src) {
            Ok(h) => h,
            Err(e) => {
                errors.push(e.to_string());
                return;
            }
        };
        let name = src
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("media")
            .to_string();
        let mut dest = media_dir.join(&name);
        if dest != src {
            // Same name, different content -> disambiguate with the hash.
            if dest.exists() && hash_file(&dest).ok().as_deref() != Some(hash.as_str()) {
                dest = media_dir.join(format!("{}-{name}", &hash[..8]));
            }
            let result = if dest.exists() {
                // Already there (same content).
                if move_files {
                    fs::remove_file(&src).map(|_| false)
                } else {
                    Ok(false)
                }
            } else if move_files {
                // rename fails across disks: copy, then remove.
                fs::rename(&src, &dest)
                    .or_else(|_| fs::copy(&src, &dest).and_then(|_| fs::remove_file(&src)))
                    .map(|_| true)
            } else {
                fs::copy(&src, &dest).map(|_| true)
            };
            match result {
                Ok(true) => done += 1,
                Ok(false) => {}
                Err(e) => {
                    errors.push(format!("{}: {e}", src.display()));
                    if !dest.exists() {
                        return;
                    }
                }
            }
        }
        seen.insert(src, (dest.clone(), hash.clone()));
        a.file = relativize_media(Some(show_dir), &dest);
        a.hash = Some(hash);
    });
    (done, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioCue, ContinueMode, Cue};

    #[test]
    fn roundtrip_and_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.smshow");
        let mut show = Show::default();
        let id = show.alloc_id();
        show.cue_lists[0].cues.push(Cue {
            id,
            number: "1".into(),
            kind: CueKind::Audio(AudioCue::default()),
            ..Cue::default()
        });
        save(&show, &path).unwrap();
        assert_eq!(load(&path).unwrap(), show);
        show.name = "Changed".into();
        save(&show, &path).unwrap();
        assert_eq!(fs::read_dir(dir.path().join("backups")).unwrap().count(), 1);
        assert_eq!(load(&path).unwrap().name, "Changed");
        // no temp files left behind
        assert!(fs::read_dir(dir.path()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")));
    }

    #[test]
    fn migrates_v1() {
        let v1 = r#"{
          "schema_version": 1, "name": "Old",
          "cue_lists": [{ "id": 1, "name": "Main", "cues": [
            { "id": 2, "number": "1", "continue": "follow", "kind": { "type": "audio", "file": "a.wav", "volume": 0.5 } },
            { "id": 3, "number": "2", "continue": "none", "kind": { "type": "group", "children": [
               { "id": 4, "continue": "continue", "kind": { "type": "audio", "file": "b.wav", "volume": 0.0 } } ] } }
          ]}]
        }"#;
        let show = parse(v1).unwrap();
        assert_eq!(show.schema_version, SCHEMA_VERSION);
        let c = show.cue(2).unwrap();
        assert_eq!(c.continue_mode, ContinueMode::AutoFollow);
        let CueKind::Audio(a) = &c.kind else { panic!() };
        assert!((a.gain_db + 6.02).abs() < 0.01);
        let inner = show.cue(4).unwrap();
        assert_eq!(inner.continue_mode, ContinueMode::AutoContinue);
        let CueKind::Audio(b) = &inner.kind else {
            panic!()
        };
        assert_eq!(b.gain_db, -120.0);
        assert!(show.next_id > 4);
    }

    #[test]
    fn v2_without_input_settings_loads_with_defaults() {
        // A v2 showfile written before MIDI input / timecode chase existed.
        let v2 = r#"{
          "schema_version": 2, "name": "Before inputs", "next_id": 3,
          "timecode": { "rate": "25", "start": "01:00:00:00", "mode": "freerun",
                        "ltc_enabled": true, "ltc_channel": 1, "ltc_level_db": -12.0,
                        "mtc_enabled": false, "mtc_port": "" },
          "cue_lists": [{ "id": 1, "name": "Main", "tc_jump": "skip", "cues": [] }]
        }"#;
        let show = parse(v2).unwrap();
        assert_eq!(show.midi_input, crate::model::MidiInputSettings::default());
        assert!(show.midi_input.mappings.is_empty());
        assert!(!show.midi_input.msc.enabled);
        assert_eq!(show.timecode.source, crate::model::TcSource::Generator);
        assert_eq!(
            show.timecode.input,
            crate::model::TcInputSettings::default()
        );
        assert!(show.timecode.ltc_enabled);
        // ... and the new fields survive a save / load round trip.
        let mut show = show;
        show.timecode.source = crate::model::TcSource::External;
        show.midi_input
            .mappings
            .push(crate::model::MidiMapping::default());
        assert_eq!(parse(&to_string(&show).unwrap()).unwrap(), show);
    }

    #[test]
    fn rejects_newer_and_malformed() {
        assert!(matches!(
            parse(r#"{"schema_version": 999}"#),
            Err(ShowfileError::TooNew { .. })
        ));
        assert!(matches!(
            parse(r#"{"name": "x"}"#),
            Err(ShowfileError::Malformed(_))
        ));
        assert!(parse("not json").is_err());
    }

    #[test]
    fn autosave_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.smshow");
        let mut show = Show::default();
        save(&show, &path).unwrap();
        assert!(recoverable(&path).is_none());
        show.name = "Unsaved edit".into();
        std::thread::sleep(std::time::Duration::from_millis(20));
        autosave(&show, &path).unwrap();
        assert_eq!(recoverable(&path).unwrap().name, "Unsaved edit");
        save(&show, &path).unwrap();
        assert!(recoverable(&path).is_none());
    }

    #[test]
    fn collect_copies_and_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let media = outside.path().join("song.wav");
        fs::write(&media, b"RIFFdata").unwrap();
        let mut show = Show::default();
        let id = show.alloc_id();
        let audio = AudioCue {
            file: media.to_string_lossy().into(),
            ..AudioCue::default()
        };
        show.cue_lists[0].cues.push(Cue {
            id,
            kind: CueKind::Audio(audio),
            ..Cue::default()
        });
        let (copied, errors) = collect_media(&mut show, dir.path());
        assert_eq!((copied, errors.len()), (1, 0));
        let CueKind::Audio(a) = &show.cue(id).unwrap().kind else {
            panic!()
        };
        assert_eq!(a.file, "media/song.wav");
        assert_eq!(a.hash.as_deref().unwrap().len(), 64);
        assert!(dir.path().join("media/song.wav").exists());
    }

    #[test]
    fn gather_moves_shared_file_once() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let media = outside.path().join("song.wav");
        fs::write(&media, b"RIFFdata").unwrap();
        let mut show = Show::default();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let id = show.alloc_id();
            ids.push(id);
            show.cue_lists[0].cues.push(Cue {
                id,
                kind: CueKind::Audio(AudioCue {
                    file: media.to_string_lossy().into(),
                    ..AudioCue::default()
                }),
                ..Cue::default()
            });
        }
        let (moved, errors) = gather_media(&mut show, dir.path(), true);
        assert_eq!((moved, errors.len()), (1, 0), "{errors:?}");
        assert!(!media.exists(), "moved, not copied");
        for id in ids {
            let CueKind::Audio(a) = &show.cue(id).unwrap().kind else {
                panic!()
            };
            assert_eq!(a.file, "media/song.wav");
        }
        assert_eq!(
            fs::read(dir.path().join("media/song.wav")).unwrap(),
            b"RIFFdata"
        );
    }

    #[test]
    fn v3_audio_fades_become_smooth() {
        let show = parse(
            r#"{
          "schema_version": 3, "name": "Fades", "next_id": 4,
          "cue_lists": [{ "id": 1, "name": "Main", "cues": [
            { "id": 2, "kind": { "type": "audio", "file": "a.wav", "fade_curve": "equal_power" } },
            { "id": 3, "kind": { "type": "group", "children": [
              { "id": 4, "kind": { "type": "audio", "file": "b.wav", "fade_curve": "linear" } }
            ] } }
          ] }]
        }"#,
        )
        .unwrap();
        let curve = |id| match &show.cue(id).unwrap().kind {
            CueKind::Audio(a) => a.fade_curve,
            _ => panic!(),
        };
        assert_eq!(curve(2), crate::model::Curve::Smooth);
        assert_eq!(
            curve(4),
            crate::model::Curve::Linear,
            "a deliberate choice stays"
        );
    }
}
