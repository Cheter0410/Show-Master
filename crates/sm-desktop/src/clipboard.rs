//! Copying cues between shows (and into templates).
//!
//! The clipboard text is JSON: `{"showmaster": "cues", "version": 1,
//! "cues": [...], "targets": {"<id>": "<number>"}}`. Audio paths are made
//! absolute so they still resolve in another show; `targets` lets the
//! receiving show find fade / control targets that were not copied by their
//! cue number. Templates are the same JSON in files
//! (`<config>/templates/<name>.json`).

use serde_json::{json, Value};
use sm_core::model::{Cue, CueKind, Show};
use sm_core::showfile::resolve_media;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MARK: &str = "cues";

/// Top-level cues among `ids` (a cue whose group is also selected goes with the group).
fn roots<'a>(show: &'a Show, ids: &[u64]) -> Vec<&'a Cue> {
    fn contains(c: &Cue, id: u64) -> bool {
        c.children().iter().any(|k| k.id == id || contains(k, id))
    }
    let mut out = Vec::new();
    show.for_each_cue(|_, c| {
        if ids.contains(&c.id)
            && !ids
                .iter()
                .any(|o| *o != c.id && show.cue(*o).is_some_and(|oc| contains(oc, c.id)))
        {
            out.push(c);
        }
    });
    out
}

/// Clipboard text for the cues `ids` of `show` (`show_dir` resolves relative audio paths).
pub fn copy_payload(show: &Show, show_dir: Option<&Path>, ids: &[u64]) -> Option<String> {
    let mut cues: Vec<Cue> = roots(show, ids).into_iter().cloned().collect();
    if cues.is_empty() {
        return None;
    }
    let mut targets: HashMap<String, String> = HashMap::new();
    fn walk(c: &mut Cue, show: &Show, dir: Option<&Path>, targets: &mut HashMap<String, String>) {
        match &mut c.kind {
            CueKind::Audio(a) if !a.file.is_empty() => {
                a.file = resolve_media(dir, &a.file).display().to_string();
            }
            CueKind::Fade(f) => note(f.target, show, targets),
            CueKind::Control(x) => note(x.target, show, targets),
            CueKind::Group(g) => {
                for k in &mut g.children {
                    walk(k, show, dir, targets);
                }
            }
            _ => {}
        }
    }
    fn note(t: Option<u64>, show: &Show, targets: &mut HashMap<String, String>) {
        if let Some(c) = t.and_then(|t| show.cue(t)).filter(|c| !c.number.is_empty()) {
            targets.insert(c.id.to_string(), c.number.clone());
        }
    }
    for c in &mut cues {
        walk(c, show, show_dir, &mut targets);
    }
    Some(json!({ "showmaster": MARK, "version": 1, "cues": cues, "targets": targets }).to_string())
}

/// The `paste_cues` command for clipboard / template text, or `None` if it
/// is not Show Master cues.
pub fn paste_command(text: &str, list: Option<u64>, after: Option<u64>) -> Option<Value> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    if v["showmaster"] != MARK {
        return None;
    }
    // Validate before sending: every entry must be a cue.
    let cues: Vec<Cue> = serde_json::from_value(v["cues"].clone()).ok()?;
    if cues.is_empty() {
        return None;
    }
    Some(json!({
        "cmd": "paste_cues", "cues": cues, "list": list, "after": after,
        "targets": v["targets"].clone(),
    }))
}

pub fn templates_dir() -> Option<PathBuf> {
    crate::recent::config_dir().map(|d| d.join("templates"))
}

/// Template names (file stems), sorted.
pub fn templates() -> Vec<String> {
    let Some(dir) = templates_dir() else {
        return Vec::new();
    };
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| {
                    let p = e.path();
                    (p.extension().and_then(|x| x.to_str()) == Some("json"))
                        .then(|| p.file_stem()?.to_str().map(str::to_string))
                        .flatten()
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// File-name-safe version of a template name.
pub fn safe_name(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_.()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim_matches('.').trim().to_string();
    if s.is_empty() {
        "Template".into()
    } else {
        s.chars().take(60).collect()
    }
}

pub fn save_template(name: &str, payload: &str) -> Result<PathBuf, String> {
    let dir = templates_dir().ok_or("no configuration folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.json", safe_name(name)));
    std::fs::write(&path, payload).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn load_template(name: &str) -> Result<String, String> {
    let dir = templates_dir().ok_or("no configuration folder")?;
    std::fs::read_to_string(dir.join(format!("{}.json", safe_name(name))))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use sm_core::model::{AudioCue, FadeCue, GroupCue};

    #[test]
    fn payload_round_trip() {
        let mut show = Show::default();
        let mut mk = |kind: CueKind, number: &str| {
            let id = show.alloc_id();
            Cue {
                id,
                number: number.into(),
                kind,
                ..Cue::default()
            }
        };
        let a = mk(
            CueKind::Audio(AudioCue {
                file: "media/a.wav".into(),
                ..AudioCue::default()
            }),
            "1",
        );
        let target = a.id;
        let f = mk(
            CueKind::Fade(FadeCue {
                target: Some(target),
                ..FadeCue::default()
            }),
            "2",
        );
        let kid = mk(CueKind::Memo, "");
        let kid_id = kid.id;
        let g = mk(
            CueKind::Group(GroupCue {
                children: vec![kid],
                ..GroupCue::default()
            }),
            "3",
        );
        let (fid, gid) = (f.id, g.id);
        show.cue_lists[0].cues = vec![a, f, g];
        // Fade + group + one of the group's children: the child goes with its group.
        let text = copy_payload(&show, Some(Path::new("/shows/x")), &[fid, gid, kid_id]).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["cues"].as_array().unwrap().len(), 2);
        assert_eq!(v["targets"][target.to_string()], "1");
        let cmd = paste_command(&text, Some(1), Some(9)).unwrap();
        assert_eq!(cmd["cmd"], "paste_cues");
        assert_eq!(cmd["after"], 9);
        // Audio paths become absolute.
        let all = copy_payload(&show, Some(Path::new("/shows/x")), &[target]).unwrap();
        assert!(all.contains("/shows/x/media/a.wav") || all.contains("\\\\shows\\\\x"));
        assert!(paste_command("hello", None, None).is_none());
        assert!(paste_command(r#"{"showmaster":"cues","cues":[]}"#, None, None).is_none());
    }

    #[test]
    fn names_are_file_safe() {
        assert_eq!(safe_name(" Intro / Outro "), "Intro _ Outro");
        assert_eq!(safe_name("..."), "Template");
    }
}
