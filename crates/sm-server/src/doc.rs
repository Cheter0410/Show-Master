//! The editable show document: edit commands, undo/redo.

use serde::Deserialize;
use serde_json::{json, Value};
use sm_core::model::*;
use std::path::{Path, PathBuf};

/// Structural and content edits. Only allowed in edit mode for admins.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum EditCommand {
    AddCue {
        kind: String,
        #[serde(default)]
        list: Option<ListId>,
        #[serde(default)]
        after: Option<CueId>,
        #[serde(default)]
        parent: Option<CueId>,
        #[serde(default)]
        file: Option<String>,
        #[serde(default)]
        target: Option<CueId>,
    },
    UpdateCue {
        cue: CueId,
        patch: Value,
    },
    UpdateCues {
        cues: Vec<CueId>,
        patch: Value,
    },
    DeleteCues {
        cues: Vec<CueId>,
    },
    DuplicateCues {
        cues: Vec<CueId>,
    },
    /// Insert copied cues (clipboard, template), possibly from another show:
    /// new ids; fade / control targets inside the pasted set follow the
    /// copies, other targets are looked up by cue number (`targets`: old id
    /// -> number), otherwise cleared.
    PasteCues {
        cues: Vec<Cue>,
        #[serde(default)]
        list: Option<ListId>,
        #[serde(default)]
        after: Option<CueId>,
        #[serde(default)]
        targets: std::collections::HashMap<String, String>,
    },
    MoveCues {
        cues: Vec<CueId>,
        #[serde(default)]
        list: Option<ListId>,
        #[serde(default)]
        parent: Option<CueId>,
        index: usize,
    },
    Renumber {
        cues: Vec<CueId>,
        start: f64,
        step: f64,
    },
    GroupCues {
        cues: Vec<CueId>,
    },
    AddList {
        name: String,
    },
    UpdateList {
        list: ListId,
        patch: Value,
    },
    DeleteList {
        list: ListId,
    },
    UpdateShow {
        patch: Value,
    },
    ApplyLayout {
        layout: String,
    },
    Undo,
    Redo,
}

impl EditCommand {
    /// Edits that may be made during a show (Live mode): only the level,
    /// routing and notes of cues - they reach running cues smoothly and
    /// cannot change what GO does next.
    pub fn is_live_safe(&self) -> bool {
        fn safe(patch: &Value) -> bool {
            let Some(obj) = patch.as_object() else {
                return false;
            };
            obj.iter().all(|(k, v)| match k.as_str() {
                "notes" => true,
                "kind" => {
                    v.as_object().is_some_and(|kind| {
                        kind.keys()
                            .all(|k| matches!(k.as_str(), "gain_db" | "routing" | "type"))
                    }) && v.get("type").is_none_or(|t| t == "audio")
                }
                _ => false,
            })
        }
        match self {
            EditCommand::UpdateCue { patch, .. } | EditCommand::UpdateCues { patch, .. } => {
                safe(patch)
            }
            _ => false,
        }
    }

    /// For level / routing patches: which cues and fields they change, so
    /// that a stream of them (a fader being dragged) is one undo step.
    fn coalesce_key(&self) -> Option<String> {
        let (cues, patch) = match self {
            EditCommand::UpdateCue { cue, patch } => (vec![*cue], patch),
            EditCommand::UpdateCues { cues, patch } => (cues.clone(), patch),
            _ => return None,
        };
        let kind = patch.as_object()?.get("kind")?.as_object()?;
        if patch.as_object()?.len() != 1
            || kind.is_empty()
            || !kind
                .keys()
                .all(|k| matches!(k.as_str(), "gain_db" | "routing"))
        {
            return None;
        }
        let mut fields: Vec<&String> = kind.keys().collect();
        fields.sort();
        Some(format!("{cues:?}{fields:?}"))
    }
}

/// Media facts needed when a file is assigned to an audio cue.
pub struct MediaInfo {
    pub duration: Option<f64>,
    pub channels: u16,
    pub sample_rate: u32,
}

pub trait MediaProbe {
    fn probe(&self, path: &Path) -> Option<MediaInfo>;
}

pub struct NoProbe;
impl MediaProbe for NoProbe {
    fn probe(&self, _: &Path) -> Option<MediaInfo> {
        None
    }
}

/// Number of undo steps kept per session.
pub const UNDO_LIMIT: usize = 500;

#[derive(Default)]
pub struct Document {
    pub show: Show,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    undo: std::collections::VecDeque<Show>,
    redo: Vec<Show>,
    /// The last live-safe patch (cues + changed fields) and when: a stream
    /// of them (a fader being dragged) is one undo step.
    coalesce: Option<(String, std::time::Instant)>,
}

/// RFC 7396 JSON merge patch.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    match patch {
        Value::Object(p) => {
            if !target.is_object() {
                *target = json!({});
            }
            if let Value::Object(t) = target {
                for (k, v) in p {
                    if v.is_null() {
                        t.remove(k);
                    } else {
                        merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
                    }
                }
            }
        }
        other => *target = other.clone(),
    }
}

fn fmt_number(n: f64) -> String {
    let s = format!("{:.3}", n);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn take_from(cues: &mut Vec<Cue>, id: CueId) -> Option<Cue> {
    if let Some(i) = cues.iter().position(|c| c.id == id) {
        return Some(cues.remove(i));
    }
    for c in cues {
        if let CueKind::Group(g) = &mut c.kind {
            if let Some(found) = take_from(&mut g.children, id) {
                return Some(found);
            }
        }
    }
    None
}

fn contains(cue: &Cue, id: CueId) -> bool {
    cue.id == id || cue.children().iter().any(|c| contains(c, id))
}

/// Location of a cue: (list id, parent group id, index in its container).
fn locate(show: &Show, id: CueId) -> Option<(ListId, Option<CueId>, usize)> {
    fn rec(cues: &[Cue], parent: Option<CueId>, id: CueId) -> Option<(Option<CueId>, usize)> {
        if let Some(i) = cues.iter().position(|c| c.id == id) {
            return Some((parent, i));
        }
        cues.iter().find_map(|c| rec(c.children(), Some(c.id), id))
    }
    show.cue_lists
        .iter()
        .find_map(|l| rec(&l.cues, None, id).map(|(p, i)| (l.id, p, i)))
}

fn container_mut(show: &mut Show, list: ListId, parent: Option<CueId>) -> Option<&mut Vec<Cue>> {
    match parent {
        Some(p) => match &mut show.cue_mut(p)?.kind {
            CueKind::Group(g) => Some(&mut g.children),
            _ => None,
        },
        None => show.list_mut(list).map(|l| &mut l.cues),
    }
}

/// New ids for `cue` and its children, recording old -> new.
fn reassign_ids_mapped(
    cue: &mut Cue,
    show: &mut Show,
    map: &mut std::collections::HashMap<CueId, CueId>,
) {
    let new = show.alloc_id();
    map.insert(cue.id, new);
    cue.id = new;
    if let CueKind::Group(g) = &mut cue.kind {
        for c in &mut g.children {
            reassign_ids_mapped(c, show, map);
        }
    }
}

fn retarget(cue: &mut Cue, f: &dyn Fn(Option<CueId>) -> Option<CueId>) {
    match &mut cue.kind {
        CueKind::Fade(x) => x.target = f(x.target),
        CueKind::Control(x) => x.target = f(x.target),
        CueKind::Group(g) => {
            for c in &mut g.children {
                retarget(c, f);
            }
        }
        _ => {}
    }
}

fn reassign_ids(cue: &mut Cue, show: &mut Show) {
    cue.id = show.alloc_id();
    if let CueKind::Group(g) = &mut cue.kind {
        for c in &mut g.children {
            reassign_ids(c, show);
        }
    }
}

impl Document {
    pub fn new(show: Show, path: Option<PathBuf>) -> Self {
        Self {
            show,
            path,
            dirty: false,
            undo: std::collections::VecDeque::new(),
            redo: Vec::new(),
            coalesce: None,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn show_dir(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    fn push_undo(&mut self, show: Show) {
        self.undo.push_back(show);
        // Every step is a full copy of the show: keep memory bounded on long days.
        while self.undo.len() > UNDO_LIMIT {
            self.undo.pop_front();
        }
    }

    /// Change the show outside the undo history (runtime arm/disarm, notes).
    pub fn touch(&mut self, f: impl FnOnce(&mut Show)) {
        f(&mut self.show);
        self.dirty = true;
    }

    /// Apply an edit. On success the previous state is pushed on the undo stack.
    pub fn apply(&mut self, cmd: EditCommand, probe: &dyn MediaProbe) -> Result<Value, String> {
        if matches!(cmd, EditCommand::Undo | EditCommand::Redo) {
            self.coalesce = None;
        }
        match cmd {
            EditCommand::Undo => {
                let prev = self.undo.pop_back().ok_or("nothing to undo")?;
                self.redo.push(std::mem::replace(&mut self.show, prev));
                self.dirty = true;
                return Ok(Value::Null);
            }
            EditCommand::Redo => {
                let next = self.redo.pop().ok_or("nothing to redo")?;
                let current = std::mem::replace(&mut self.show, next);
                self.push_undo(current);
                self.dirty = true;
                return Ok(Value::Null);
            }
            _ => {}
        }
        let before = self.show.clone();
        let key = cmd.coalesce_key();
        match self.edit(cmd, probe) {
            Ok(result) => {
                if self.show != before {
                    let now = std::time::Instant::now();
                    let same = matches!((&key, &self.coalesce), (Some(k), Some((p, at)))
                        if k == p && now.duration_since(*at).as_secs_f64() < 2.0
                            && !self.undo.is_empty());
                    self.coalesce = key.map(|k| (k, now));
                    if same {
                        self.redo.clear();
                        self.dirty = true;
                        return Ok(result);
                    }
                    self.push_undo(before);
                    self.redo.clear();
                    self.dirty = true;
                }
                Ok(result)
            }
            Err(e) => {
                self.show = before;
                Err(e)
            }
        }
    }

    fn probe_audio(&self, cue: &mut Cue, probe: &dyn MediaProbe) {
        let dir = self.show_dir().map(Path::to_path_buf);
        if let CueKind::Audio(a) = &mut cue.kind {
            if a.file.is_empty() {
                a.file_duration = None;
                return;
            }
            let path = sm_core::showfile::resolve_media(dir.as_deref(), &a.file);
            a.file = sm_core::showfile::relativize_media(dir.as_deref(), &path);
            a.hash = None;
            if let Some(info) = probe.probe(&path) {
                a.file_duration = info.duration;
                a.channels = info.channels.max(1);
                a.file_rate = Some(info.sample_rate).filter(|r| *r > 0);
            }
        }
    }

    fn next_number(&self, list: ListId) -> String {
        let mut max = 0.0f64;
        if let Some(l) = self.show.list(list) {
            for c in &l.cues {
                if let Ok(n) = c.number.parse::<f64>() {
                    max = max.max(n);
                }
            }
        }
        fmt_number(max.floor() + 1.0)
    }

    fn patch_cue(
        &mut self,
        id: CueId,
        patch: &Value,
        probe: &dyn MediaProbe,
    ) -> Result<(), String> {
        let cue = self
            .show
            .cue(id)
            .ok_or_else(|| format!("cue {id} does not exist"))?
            .clone();
        let old_file = match &cue.kind {
            CueKind::Audio(a) => Some(a.file.clone()),
            _ => None,
        };
        let mut doc = serde_json::to_value(&cue).map_err(|e| e.to_string())?;
        let mut patch = patch.clone();
        if let Some(p) = patch.as_object_mut() {
            p.remove("id");
            // Changing the cue type replaces the kind instead of merging into it.
            let new_type = p
                .get("kind")
                .and_then(|k| k.get("type"))
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(t) = new_type {
                if t != cue.type_name() {
                    if let (Some(k), Some(obj)) = (p.remove("kind"), doc.as_object_mut()) {
                        obj.insert("kind".into(), k);
                    }
                }
            }
        } else {
            return Err("patch must be an object".into());
        }
        merge_patch(&mut doc, &patch);
        let mut new: Cue = serde_json::from_value(doc).map_err(|e| format!("invalid cue: {e}"))?;
        new.id = id;
        // In a merge patch `null` removes a key, which would bring back the
        // default level; for a fade it means "leave the level alone".
        if let CueKind::Fade(f) = &mut new.kind {
            if patch
                .get("kind")
                .and_then(|k| k.get("gain_db"))
                .is_some_and(Value::is_null)
            {
                f.gain_db = None;
            }
        }
        new.pre_wait = new.pre_wait.max(0.0);
        new.post_wait = new.post_wait.max(0.0);
        if let CueKind::Audio(a) = &new.kind {
            if old_file.as_deref() != Some(a.file.as_str()) {
                self.probe_audio(&mut new, probe);
            }
        }
        if let CueKind::Audio(a) = &mut new.kind {
            if let Some(v) = &mut a.vamp {
                if !(v.start.is_finite() && v.end.is_finite()) || v.end - v.start < 0.01 {
                    return Err("the loop must be at least 10 ms long".into());
                }
                v.start = v.start.max(0.0);
                // Releasing a loop exactly needs the file's own sample rate.
                if a.file_rate.is_none() && !a.file.is_empty() {
                    let dir = self.show_dir().map(Path::to_path_buf);
                    let path = sm_core::showfile::resolve_media(dir.as_deref(), &a.file);
                    a.file_rate = probe.probe(&path).map(|i| i.sample_rate).filter(|r| *r > 0);
                }
            }
        }
        if let CueKind::Group(g) = &mut new.kind {
            // Children added through a patch need ids.
            let mut pending: Vec<&mut Cue> = g.children.iter_mut().filter(|c| c.id == 0).collect();
            for c in pending.iter_mut() {
                c.id = self.show.next_id.max(1);
                self.show.next_id = c.id + 1;
            }
        }
        *self.show.cue_mut(id).ok_or("cue vanished")? = new;
        Ok(())
    }

    fn edit(&mut self, cmd: EditCommand, probe: &dyn MediaProbe) -> Result<Value, String> {
        match cmd {
            EditCommand::Undo | EditCommand::Redo => Ok(Value::Null),
            EditCommand::AddCue {
                kind,
                list,
                after,
                parent,
                file,
                target,
            } => {
                let mut k = match kind.as_str() {
                    "audio" => CueKind::Audio(AudioCue::default()),
                    "fade" => CueKind::Fade(FadeCue::default()),
                    "midi" => CueKind::Midi(MidiCue::default()),
                    "osc" => CueKind::Osc(sm_core::osc::OscMessage::default()),
                    "lantern" => CueKind::Osc(sm_core::osc::OscMessage::lantern(
                        sm_core::osc::LanternAction::Go,
                    )),
                    "http" => CueKind::Http(sm_core::http_cue::HttpRequest::default()),
                    "timecode" => CueKind::Timecode(TimecodeCue::default()),
                    "control" => CueKind::Control(ControlCue::default()),
                    "group" => CueKind::Group(GroupCue::default()),
                    "memo" => CueKind::Memo,
                    other => return Err(format!("unknown cue type '{other}'")),
                };
                let target = target.or(after).filter(|t| self.show.cue(*t).is_some());
                match &mut k {
                    CueKind::Fade(f) => {
                        f.target = target.filter(|t| {
                            matches!(
                                self.show.cue(*t).map(|c| &c.kind),
                                Some(CueKind::Audio(_) | CueKind::Group(_))
                            )
                        })
                    }
                    CueKind::Control(c) => c.target = target,
                    _ => {}
                }
                // Where does it go?
                let (list_id, parent, index) =
                    match (parent, after.and_then(|a| locate(&self.show, a))) {
                        (Some(p), Some((l, ap, i))) if ap == Some(p) => (l, Some(p), Some(i + 1)),
                        (Some(p), _) => {
                            let (l, _, _) =
                                locate(&self.show, p).ok_or("parent group does not exist")?;
                            (l, Some(p), None)
                        }
                        (None, Some((l, ap, i))) => (l, ap, Some(i + 1)),
                        (None, None) => {
                            let l = list
                                .filter(|l| self.show.list(*l).is_some())
                                .or_else(|| self.show.cue_lists.first().map(|l| l.id))
                                .ok_or("no cue list")?;
                            (l, None, None)
                        }
                    };
                let id = self.show.alloc_id();
                let mut cue = Cue {
                    id,
                    number: if parent.is_none() {
                        self.next_number(list_id)
                    } else {
                        String::new()
                    },
                    kind: k,
                    ..Cue::default()
                };
                if let (CueKind::Audio(a), Some(file)) = (&mut cue.kind, file) {
                    cue.name = Path::new(&file)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    a.file = file;
                    self.probe_audio(&mut cue, probe);
                }
                let container = container_mut(&mut self.show, list_id, parent)
                    .ok_or("target container is not a group")?;
                let index = index.unwrap_or(container.len()).min(container.len());
                container.insert(index, cue);
                Ok(json!({ "cue": id }))
            }
            EditCommand::UpdateCue { cue, patch } => {
                self.patch_cue(cue, &patch, probe)?;
                Ok(Value::Null)
            }
            EditCommand::UpdateCues { cues, patch } => {
                for c in cues {
                    self.patch_cue(c, &patch, probe)?;
                }
                Ok(Value::Null)
            }
            EditCommand::DeleteCues { cues } => {
                for id in cues {
                    for l in &mut self.show.cue_lists {
                        if take_from(&mut l.cues, id).is_some() {
                            break;
                        }
                    }
                }
                Ok(Value::Null)
            }
            EditCommand::DuplicateCues { cues } => {
                let mut new_ids = Vec::new();
                for id in cues {
                    let Some((list, parent, index)) = locate(&self.show, id) else {
                        continue;
                    };
                    let Some(mut copy) = self.show.cue(id).cloned() else {
                        continue;
                    };
                    reassign_ids(&mut copy, &mut self.show);
                    copy.number = String::new();
                    new_ids.push(copy.id);
                    // Insert after the last copy made from this position so order is kept.
                    if let Some(c) = container_mut(&mut self.show, list, parent) {
                        let at = (index + 1).min(c.len());
                        c.insert(at, copy);
                    }
                }
                Ok(json!({ "cues": new_ids }))
            }
            EditCommand::PasteCues {
                mut cues,
                list,
                after,
                targets,
            } => {
                if cues.is_empty() {
                    return Err("nothing to paste".into());
                }
                let mut map = std::collections::HashMap::new();
                for c in &mut cues {
                    reassign_ids_mapped(c, &mut self.show, &mut map);
                }
                // Targets: inside the pasted set, else by number in this show.
                let by_number = |n: &str| -> Option<CueId> {
                    let mut found = None;
                    self.show.for_each_cue(|_, c| {
                        if found.is_none() && !n.is_empty() && c.number == n {
                            found = Some(c.id);
                        }
                    });
                    found
                };
                let resolved: std::collections::HashMap<CueId, Option<CueId>> = targets
                    .iter()
                    .filter_map(|(old, num)| Some((old.parse::<CueId>().ok()?, by_number(num))))
                    .collect();
                let fix = |t: Option<CueId>| -> Option<CueId> {
                    let t = t?;
                    map.get(&t)
                        .copied()
                        .or_else(|| resolved.get(&t).copied().flatten())
                };
                // Numbers already used in this show are cleared (no duplicates).
                let mut used = std::collections::HashSet::new();
                self.show.for_each_cue(|_, c| {
                    if !c.number.is_empty() {
                        used.insert(c.number.clone());
                    }
                });
                for c in &mut cues {
                    retarget(c, &fix);
                    if used.contains(&c.number) {
                        c.number.clear();
                    }
                }
                let (list_id, parent, mut index) = match after.and_then(|a| locate(&self.show, a)) {
                    Some((l, p, i)) => (l, p, i + 1),
                    None => {
                        let l = list
                            .filter(|l| self.show.list(*l).is_some())
                            .or_else(|| self.show.cue_lists.first().map(|l| l.id))
                            .ok_or("no cue list")?;
                        let len = self.show.list(l).map_or(0, |x| x.cues.len());
                        (l, None, len)
                    }
                };
                let ids: Vec<CueId> = cues.iter().map(|c| c.id).collect();
                let container = container_mut(&mut self.show, list_id, parent)
                    .ok_or("target container is not a group")?;
                for c in cues {
                    let at = index.min(container.len());
                    container.insert(at, c);
                    index = at + 1;
                }
                Ok(json!({ "cues": ids }))
            }
            EditCommand::MoveCues {
                cues,
                list,
                parent,
                index,
            } => {
                if let Some(p) = parent {
                    let pc = self.show.cue(p).ok_or("target group does not exist")?;
                    if !matches!(pc.kind, CueKind::Group(_)) {
                        return Err("target is not a group".into());
                    }
                    if cues
                        .iter()
                        .any(|c| self.show.cue(*c).is_some_and(|c| contains(c, p)))
                    {
                        return Err("cannot move a group into itself".into());
                    }
                }
                let list = match parent.and_then(|p| locate(&self.show, p)) {
                    Some((l, _, _)) => l,
                    None => list
                        .filter(|l| self.show.list(*l).is_some())
                        .or_else(|| {
                            cues.first()
                                .and_then(|c| locate(&self.show, *c))
                                .map(|x| x.0)
                        })
                        .ok_or("no cue list")?,
                };
                // Skip cues whose ancestor is also being moved.
                let roots: Vec<CueId> = cues
                    .iter()
                    .copied()
                    .filter(|c| {
                        !cues
                            .iter()
                            .any(|o| o != c && self.show.cue(*o).is_some_and(|oc| contains(oc, *c)))
                    })
                    .collect();
                // Count how many moved cues sit before the target index in the same container.
                let before = roots.iter().filter(|c| matches!(locate(&self.show, **c), Some((l, p, i)) if l == list && p == parent && i < index)).count();
                let mut taken = Vec::new();
                for id in roots {
                    for l in &mut self.show.cue_lists {
                        if let Some(c) = take_from(&mut l.cues, id) {
                            taken.push(c);
                            break;
                        }
                    }
                }
                let container = container_mut(&mut self.show, list, parent)
                    .ok_or("target container not found")?;
                let at = index.saturating_sub(before).min(container.len());
                for (n, c) in taken.into_iter().enumerate() {
                    container.insert(at + n, c);
                }
                Ok(Value::Null)
            }
            EditCommand::Renumber { cues, start, step } => {
                let mut n = start;
                for id in cues {
                    if let Some(c) = self.show.cue_mut(id) {
                        c.number = fmt_number(n);
                        n += step;
                    }
                }
                Ok(Value::Null)
            }
            EditCommand::GroupCues { cues } => {
                let first = cues.first().copied().ok_or("no cues selected")?;
                let (list, parent, index) =
                    locate(&self.show, first).ok_or("cue does not exist")?;
                let mut children = Vec::new();
                for id in &cues {
                    if matches!(locate(&self.show, *id), Some((l, p, _)) if l == list && p == parent)
                    {
                        if let Some(c) = container_mut(&mut self.show, list, parent)
                            .and_then(|v| take_from(v, *id))
                        {
                            children.push(c);
                        }
                    }
                }
                let id = self.show.alloc_id();
                let number = children
                    .first()
                    .map(|c| c.number.clone())
                    .unwrap_or_default();
                for c in &mut children {
                    c.continue_mode = ContinueMode::DoNotContinue;
                }
                let group = Cue {
                    id,
                    number,
                    name: "Group".into(),
                    kind: CueKind::Group(GroupCue {
                        mode: GroupMode::Parallel,
                        children,
                    }),
                    ..Cue::default()
                };
                let container =
                    container_mut(&mut self.show, list, parent).ok_or("container not found")?;
                let at = index.min(container.len());
                container.insert(at, group);
                Ok(json!({ "cue": id }))
            }
            EditCommand::AddList { name } => {
                let id = self.show.alloc_id();
                self.show.cue_lists.push(CueList {
                    id,
                    name,
                    ..CueList::default()
                });
                Ok(json!({ "list": id }))
            }
            EditCommand::UpdateList { list, patch } => {
                let l = self.show.list(list).ok_or("list does not exist")?;
                let mut doc = json!({ "name": l.name, "tc_jump": l.tc_jump });
                merge_patch(&mut doc, &patch);
                let name = doc
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let tc_jump =
                    serde_json::from_value(doc.get("tc_jump").cloned().unwrap_or(Value::Null))
                        .map_err(|e| e.to_string())?;
                let l = self.show.list_mut(list).ok_or("list does not exist")?;
                l.name = name;
                l.tc_jump = tc_jump;
                Ok(Value::Null)
            }
            EditCommand::DeleteList { list } => {
                if self.show.cue_lists.len() <= 1 {
                    return Err("the last cue list cannot be deleted".into());
                }
                self.show.cue_lists.retain(|l| l.id != list);
                Ok(Value::Null)
            }
            EditCommand::UpdateShow { patch } => {
                let mut patch = patch;
                if let Some(p) = patch.as_object_mut() {
                    for k in ["cue_lists", "schema_version", "next_id"] {
                        p.remove(k);
                    }
                } else {
                    return Err("patch must be an object".into());
                }
                let lists = std::mem::take(&mut self.show.cue_lists);
                let mut doc = serde_json::to_value(&self.show).map_err(|e| e.to_string())?;
                merge_patch(&mut doc, &patch);
                let parsed: Result<Show, _> = serde_json::from_value(doc);
                match parsed {
                    Ok(mut show) => {
                        show.cue_lists = lists;
                        show.settings.audio.channels = show.settings.audio.channels.clamp(1, 128);
                        self.show = show;
                        Ok(Value::Null)
                    }
                    Err(e) => {
                        self.show.cue_lists = lists;
                        Err(format!("invalid show settings: {e}"))
                    }
                }
            }
            EditCommand::ApplyLayout { layout } => {
                self.show.patch = OutputPatch::from_layout(&layout)
                    .ok_or_else(|| format!("unknown layout '{layout}'"))?;
                let n = self.show.patch.outputs.len() as u16;
                if self.show.settings.audio.channels < n {
                    self.show.settings.audio.channels = n;
                }
                Ok(Value::Null)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(doc: &mut Document, kind: &str, after: Option<CueId>, parent: Option<CueId>) -> CueId {
        let r = doc
            .apply(
                EditCommand::AddCue {
                    kind: kind.into(),
                    list: None,
                    after,
                    parent,
                    file: None,
                    target: None,
                },
                &NoProbe,
            )
            .unwrap();
        r["cue"].as_u64().unwrap()
    }

    fn order(doc: &Document) -> Vec<CueId> {
        doc.show.cue_lists[0].cues.iter().map(|c| c.id).collect()
    }

    #[test]
    fn paste_remaps_ids_targets_and_numbers() {
        let mut doc = Document::default();
        let existing = add(&mut doc, "memo", None, None); // number "1"
        doc.show.cue_mut(existing).unwrap().number = "10".into();
        // Clipboard from another show: audio (id 500, number 1), a fade on it,
        // and a control cue on cue 10 of the source show (id 900).
        let clip: Vec<Cue> = serde_json::from_value(json!([
            { "id": 500, "number": "1", "kind": { "type": "audio", "file": "/abs/a.wav" } },
            { "id": 501, "number": "2", "kind": { "type": "fade", "target": 500 } },
            { "id": 502, "number": "10", "kind": { "type": "control", "action": "stop", "target": 900 } },
            { "id": 503, "kind": { "type": "fade", "target": 777 } }
        ]))
        .unwrap();
        let r = doc
            .apply(
                EditCommand::PasteCues {
                    cues: clip,
                    list: None,
                    after: Some(existing),
                    targets: [("900".to_string(), "10".to_string())].into(),
                },
                &NoProbe,
            )
            .unwrap();
        let ids: Vec<CueId> = r["cues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        assert_eq!(order(&doc), [vec![existing], ids.clone()].concat());
        assert!(ids.iter().all(|i| ![500, 501, 502, 503].contains(i)));
        let c = |i: usize| doc.show.cue(ids[i]).unwrap().clone();
        assert!(matches!(c(1).kind, CueKind::Fade(f) if f.target == Some(ids[0])));
        assert!(matches!(c(2).kind, CueKind::Control(x) if x.target == Some(existing)));
        assert!(matches!(c(3).kind, CueKind::Fade(f) if f.target.is_none()));
        // "10" exists already: cleared; "1" and "2" are free here.
        assert_eq!((c(0).number.as_str(), c(2).number.as_str()), ("1", ""));
        doc.apply(EditCommand::Undo, &NoProbe).unwrap();
        assert_eq!(order(&doc), vec![existing]);
    }

    #[test]
    fn add_update_undo_redo() {
        let mut doc = Document::default();
        let a = add(&mut doc, "audio", None, None);
        let b = add(&mut doc, "midi", None, None);
        let c = add(&mut doc, "memo", Some(a), None);
        assert_eq!(order(&doc), vec![a, c, b]);
        assert_eq!(doc.show.cue(b).unwrap().number, "2");
        doc.apply(
            EditCommand::UpdateCue {
                cue: a,
                patch: json!({ "name": "Intro", "pre_wait": 1.5, "kind": { "gain_db": -6.0 } }),
            },
            &NoProbe,
        )
        .unwrap();
        let cue = doc.show.cue(a).unwrap();
        assert_eq!((cue.name.as_str(), cue.pre_wait), ("Intro", 1.5));
        assert!(matches!(&cue.kind, CueKind::Audio(x) if x.gain_db == -6.0 && x.loop_count == 1));
        // invalid patches change nothing
        assert!(doc
            .apply(
                EditCommand::UpdateCue {
                    cue: a,
                    patch: json!({ "pre_wait": "soon" })
                },
                &NoProbe
            )
            .is_err());
        assert_eq!(doc.show.cue(a).unwrap().pre_wait, 1.5);
        // type change replaces the kind
        doc.apply(
            EditCommand::UpdateCue {
                cue: c,
                patch: json!({ "kind": { "type": "fade", "target": a } }),
            },
            &NoProbe,
        )
        .unwrap();
        assert!(matches!(&doc.show.cue(c).unwrap().kind, CueKind::Fade(f) if f.target == Some(a)));
        doc.apply(EditCommand::Undo, &NoProbe).unwrap();
        assert!(matches!(doc.show.cue(c).unwrap().kind, CueKind::Memo));
        doc.apply(EditCommand::Undo, &NoProbe).unwrap();
        assert_eq!(doc.show.cue(a).unwrap().name, "");
        doc.apply(EditCommand::Redo, &NoProbe).unwrap();
        assert_eq!(doc.show.cue(a).unwrap().name, "Intro");
        for _ in 0..10 {
            let _ = doc.apply(EditCommand::Undo, &NoProbe);
        }
        assert!(order(&doc).is_empty() && !doc.can_undo() && doc.can_redo());
    }

    #[test]
    fn undo_history_is_bounded() {
        let mut doc = Document::default();
        let id = add(&mut doc, "memo", None, None);
        for i in 0..UNDO_LIMIT + 50 {
            doc.apply(
                EditCommand::UpdateCue {
                    cue: id,
                    patch: json!({ "name": format!("n{i}") }),
                },
                &NoProbe,
            )
            .unwrap();
        }
        let mut steps = 0;
        while doc.apply(EditCommand::Undo, &NoProbe).is_ok() {
            steps += 1;
        }
        assert_eq!(steps, UNDO_LIMIT);
        assert_eq!(doc.show.cue(id).unwrap().name, format!("n{}", 49));
    }

    #[test]
    fn move_group_duplicate_delete() {
        let mut doc = Document::default();
        let ids: Vec<CueId> = (0..5).map(|_| add(&mut doc, "memo", None, None)).collect();
        // move first two to the end
        doc.apply(
            EditCommand::MoveCues {
                cues: vec![ids[0], ids[1]],
                list: None,
                parent: None,
                index: 5,
            },
            &NoProbe,
        )
        .unwrap();
        assert_eq!(order(&doc), vec![ids[2], ids[3], ids[4], ids[0], ids[1]]);
        // move last to the front
        doc.apply(
            EditCommand::MoveCues {
                cues: vec![ids[1]],
                list: None,
                parent: None,
                index: 0,
            },
            &NoProbe,
        )
        .unwrap();
        assert_eq!(order(&doc), vec![ids[1], ids[2], ids[3], ids[4], ids[0]]);
        let g = doc
            .apply(
                EditCommand::GroupCues {
                    cues: vec![ids[2], ids[3]],
                },
                &NoProbe,
            )
            .unwrap()["cue"]
            .as_u64()
            .unwrap();
        assert_eq!(order(&doc), vec![ids[1], g, ids[4], ids[0]]);
        assert_eq!(doc.show.cue(g).unwrap().children().len(), 2);
        doc.apply(
            EditCommand::MoveCues {
                cues: vec![ids[4]],
                list: None,
                parent: Some(g),
                index: 1,
            },
            &NoProbe,
        )
        .unwrap();
        assert_eq!(
            doc.show
                .cue(g)
                .unwrap()
                .children()
                .iter()
                .map(|c| c.id)
                .collect::<Vec<_>>(),
            vec![ids[2], ids[4], ids[3]]
        );
        assert!(doc
            .apply(
                EditCommand::MoveCues {
                    cues: vec![g],
                    list: None,
                    parent: Some(g),
                    index: 0
                },
                &NoProbe
            )
            .is_err());
        let dup = doc
            .apply(EditCommand::DuplicateCues { cues: vec![g] }, &NoProbe)
            .unwrap()["cues"][0]
            .as_u64()
            .unwrap();
        let copy = doc.show.cue(dup).unwrap();
        assert_eq!(copy.children().len(), 3);
        assert!(copy.children().iter().all(|c| !ids.contains(&c.id)));
        doc.apply(
            EditCommand::Renumber {
                cues: order(&doc),
                start: 10.0,
                step: 0.5,
            },
            &NoProbe,
        )
        .unwrap();
        assert_eq!(
            doc.show.cue_lists[0]
                .cues
                .iter()
                .map(|c| c.number.as_str())
                .collect::<Vec<_>>(),
            vec!["10", "10.5", "11", "11.5"]
        );
        doc.apply(
            EditCommand::DeleteCues {
                cues: vec![g, ids[0]],
            },
            &NoProbe,
        )
        .unwrap();
        assert_eq!(order(&doc), vec![ids[1], dup]);
    }

    #[test]
    fn show_settings_and_lists() {
        let mut doc = Document::default();
        doc.apply(EditCommand::UpdateShow { patch: json!({ "name": "X", "settings": { "go_debounce_ms": 400 }, "cue_lists": [], "timecode": { "rate": "29.97df" } }) }, &NoProbe).unwrap();
        assert_eq!(
            (
                doc.show.name.as_str(),
                doc.show.settings.go_debounce_ms,
                doc.show.cue_lists.len()
            ),
            ("X", 400, 1)
        );
        assert!(doc
            .apply(
                EditCommand::UpdateShow {
                    patch: json!({ "timecode": { "rate": "12" } })
                },
                &NoProbe
            )
            .is_err());
        doc.apply(
            EditCommand::ApplyLayout {
                layout: "5.1".into(),
            },
            &NoProbe,
        )
        .unwrap();
        assert_eq!(
            (
                doc.show.patch.outputs.len(),
                doc.show.settings.audio.channels
            ),
            (6, 6)
        );
        let l = doc
            .apply(EditCommand::AddList { name: "BG".into() }, &NoProbe)
            .unwrap()["list"]
            .as_u64()
            .unwrap();
        doc.apply(
            EditCommand::UpdateList {
                list: l,
                patch: json!({ "tc_jump": "chase" }),
            },
            &NoProbe,
        )
        .unwrap();
        assert_eq!(doc.show.list(l).unwrap().tc_jump, TcJumpPolicy::Chase);
        doc.apply(EditCommand::DeleteList { list: l }, &NoProbe)
            .unwrap();
        assert!(doc
            .apply(
                EditCommand::DeleteList {
                    list: doc.show.cue_lists[0].id
                },
                &NoProbe
            )
            .is_err());
    }
    #[test]
    fn live_safe_edits() {
        let gain: EditCommand = serde_json::from_value(
            json!({ "cmd": "update_cue", "cue": 1, "patch": { "kind": { "gain_db": -3.0, "routing": [] } } }),
        )
        .unwrap();
        assert!(gain.is_live_safe());
        let notes: EditCommand = serde_json::from_value(
            json!({ "cmd": "update_cues", "cues": [1], "patch": { "notes": "x" } }),
        )
        .unwrap();
        assert!(notes.is_live_safe());
        let file: EditCommand = serde_json::from_value(
            json!({ "cmd": "update_cue", "cue": 1, "patch": { "kind": { "file": "a.wav" } } }),
        )
        .unwrap();
        assert!(!file.is_live_safe());
        let name: EditCommand = serde_json::from_value(
            json!({ "cmd": "update_cue", "cue": 1, "patch": { "name": "x" } }),
        )
        .unwrap();
        assert!(!name.is_live_safe());
        let del: EditCommand =
            serde_json::from_value(json!({ "cmd": "delete_cues", "cues": [1] })).unwrap();
        assert!(!del.is_live_safe());
    }

    #[test]
    fn fader_drag_is_one_undo_step() {
        let mut doc = Document::default();
        let a = doc
            .apply(
                EditCommand::AddCue {
                    kind: "audio".into(),
                    list: None,
                    after: None,
                    parent: None,
                    file: None,
                    target: None,
                },
                &NoProbe,
            )
            .unwrap()["cue"]
            .as_u64()
            .unwrap();
        for db in [-1.0, -2.0, -3.0] {
            doc.apply(
                EditCommand::UpdateCue {
                    cue: a,
                    patch: json!({ "kind": { "gain_db": db } }),
                },
                &NoProbe,
            )
            .unwrap();
        }
        doc.apply(EditCommand::Undo, &NoProbe).unwrap();
        match &doc.show.cue(a).unwrap().kind {
            CueKind::Audio(x) => assert_eq!(x.gain_db, 0.0),
            _ => panic!(),
        }
    }
}
