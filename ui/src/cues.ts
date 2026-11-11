// Pure helpers over the cue tree.
import type { ActiveCue, Cue, CueList, CueType, Show } from "./api";

export interface CueRow {
  cue: Cue;
  depth: number;
  /** id of the parent group, null at top level */
  parent: number | null;
  /** index within its parent container */
  index: number;
  hasChildren: boolean;
}

export function flattenCues(cues: Cue[], collapsed?: ReadonlySet<number>, depth = 0, parent: number | null = null, out: CueRow[] = []): CueRow[] {
  cues.forEach((cue, index) => {
    const isGroup = cue.kind.type === "group";
    const children = cue.kind.type === "group" ? cue.kind.children : [];
    out.push({ cue, depth, parent, index, hasChildren: isGroup && children.length > 0 });
    if (isGroup && !collapsed?.has(cue.id)) flattenCues(children, collapsed, depth + 1, cue.id, out);
  });
  return out;
}

export function allCues(show: Show): Cue[] {
  const out: Cue[] = [];
  for (const l of show.cue_lists) for (const r of flattenCues(l.cues)) out.push(r.cue);
  return out;
}

export function findCue(show: Show | null | undefined, id: number | null | undefined): Cue | null {
  if (!show || id == null) return null;
  for (const l of show.cue_lists) {
    for (const r of flattenCues(l.cues)) if (r.cue.id === id) return r.cue;
  }
  return null;
}

export function findList(show: Show | null | undefined, id: number | null): CueList | null {
  if (!show) return null;
  return show.cue_lists.find((l) => l.id === id) ?? show.cue_lists[0] ?? null;
}

export function listOfCue(show: Show, id: number): CueList | null {
  for (const l of show.cue_lists) if (flattenCues(l.cues).some((r) => r.cue.id === id)) return l;
  return null;
}

/** True if `id` is `ancestor` or nested somewhere below it. */
export function isWithin(ancestor: Cue, id: number): boolean {
  if (ancestor.id === id) return true;
  return ancestor.kind.type === "group" && ancestor.kind.children.some((c) => isWithin(c, id));
}

export function baseName(path: string): string {
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return i >= 0 ? path.slice(i + 1) : path;
}

export function cueLabel(cue: Cue | null): string {
  if (!cue) return "–";
  const num = cue.number ? `${cue.number} ` : "";
  return `${num}${cue.name || TYPE_LABELS[cue.kind.type]}`.trim();
}

export const TYPE_LABELS: Record<CueType, string> = {
  audio: "Audio",
  fade: "Fade",
  midi: "MIDI",
  timecode: "Timecode",
  control: "Control",
  group: "Group",
  memo: "Memo",
};

/** Text for the "target / file" column. */
export function cueTarget(cue: Cue, show: Show): string {
  const k = cue.kind;
  switch (k.type) {
    case "audio":
      return k.file ? baseName(k.file) : "(no file)";
    case "fade": {
      const t = findCue(show, k.target);
      return t ? `→ ${cueLabel(t)}${k.stop_target ? " · stop" : ""}` : "(no target)";
    }
    case "midi": {
      const m = k.message;
      const what =
        m.msg === "sysex" ? "SysEx" : m.msg === "msc" ? `MSC ${m.command}${m.q_number ? ` ${m.q_number}` : ""}` : m.msg.replace("_", " ");
      return `${k.port || "(no port)"} · ${what}`;
    }
    case "timecode": {
      const tc = k.tc;
      if (tc.action === "start_from") return `TC start from ${tc.from}`;
      if (tc.action === "relocate") return `TC relocate ${tc.to}`;
      return `TC ${tc.action}`;
    }
    case "control": {
      if (k.action === "reset") return "reset show";
      const t = findCue(show, k.target);
      return `${k.action.replace(/_/g, " ")} → ${t ? cueLabel(t) : "(no target)"}`;
    }
    case "group":
      return `${k.mode} · ${k.children.length} cue${k.children.length === 1 ? "" : "s"}`;
    case "memo":
      return "";
  }
}

/** Action duration in seconds; null = open-ended / not applicable. */
export function cueDuration(cue: Cue): number | null {
  const k = cue.kind;
  if (k.type === "audio") {
    const end = k.end ?? k.file_duration;
    if (end == null) return null;
    if (k.loop_count === 0) return null;
    return Math.max(0, end - k.start) * Math.max(1, k.loop_count);
  }
  if (k.type === "fade") return k.duration;
  return null;
}

/** active entries indexed by cue id (a cue appears at most once). */
export function activeMap(active: ActiveCue[] | undefined): Map<number, ActiveCue> {
  const m = new Map<number, ActiveCue>();
  if (active) for (const a of active) m.set(a.cue, a);
  return m;
}

/** The cues following `id` in flattened order (for the NEXT panel). */
export function cuesAfter(list: CueList | null, id: number | null, count: number): Cue[] {
  if (!list || id == null) return [];
  const rows = flattenCues(list.cues);
  const i = rows.findIndex((r) => r.cue.id === id);
  if (i < 0) return [];
  return rows.slice(i + 1, i + 1 + count).map((r) => r.cue);
}

export const CONTINUE_GLYPH: Record<Cue["continue_mode"], { glyph: string; title: string }> = {
  do_not_continue: { glyph: "", title: "Do not continue" },
  auto_continue: { glyph: "↓", title: "Auto-continue (next cue starts together with this one)" },
  auto_follow: { glyph: "⤓", title: "Auto-follow (next cue starts when this one ends)" },
};
