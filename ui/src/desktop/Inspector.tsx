// Inspector for the selected cue(s). Edits go out as JSON merge patches on blur / Enter.
import { useState } from "react";
import {
  CONTROL_ACTIONS, CUE_COLORS, FADE_CURVES, MIDI_MSG_TYPES, MSC_COMMANDS,
  type ContinueMode, type ControlKind, type Cue, type CuePatch, type FadeKind, type GroupKind, type GroupMode,
  type MidiKind, type MidiMessage, type MidiMsgType, type OscKind, type HttpKind, type RoutePoint, type Show, type TimecodeKind,
} from "../api";
import { formatOscArgs, parseOscArgs, validateOscAddress } from "../osc";
import { run } from "../commands";
import { Check, DbField, DraftField, NumField, Row, Select, TcField, TextField, TimeField } from "../components/fields";
import { allCues, cueCount, cueLabel, findCue, TYPE_LABELS } from "../cues";
import { useApp } from "../store";
import { IconChevronDown, IconCopy, IconError, IconTrash, IconUp } from "../components/icons";
import { deleteCues, duplicateCues } from "../editing";
import { COARSE_POINTER, loadFlag, saveFlag, useMedia } from "../platform";
import { validateSysex } from "../time";
import { AudioTab, RoutingTab } from "./AudioInspector";

const CONTINUE_OPTIONS: { value: ContinueMode; label: string }[] = [
  { value: "do_not_continue", label: "Do not continue" },
  { value: "auto_continue", label: "Auto-continue (start next together)" },
  { value: "auto_follow", label: "Auto-follow (start next when finished)" },
];
const CURVE_LABELS = { smooth: "Smooth", linear: "Linear", equal_power: "Equal power", s_curve: "S-curve" } as const;
export const CURVE_OPTIONS = FADE_CURVES.map((c) => ({ value: c, label: CURVE_LABELS[c] }));

function patchCue(cue: Cue, patch: CuePatch): void {
  void run({ cmd: "update_cue", cue: cue.id, patch });
}
function patchKind(cue: Cue, kind: Record<string, unknown>): void {
  patchCue(cue, { kind });
}

function cueOptions(show: Show, self: Cue, filter?: (c: Cue) => boolean): { value: number | null; label: string }[] {
  return [{ value: null, label: "— none —" }, ...allCues(show).filter((c) => c.id !== self.id && (!filter || filter(c))).map((c) => ({ value: c.id as number | null, label: cueLabel(c) }))];
}

/** Select that works with number|null values. */
function CueSelect(p: { value: number | null; options: { value: number | null; label: string }[]; onChange: (v: number | null) => void; testId?: string }) {
  const opts = p.options.map((o) => ({ value: o.value == null ? "" : String(o.value), label: o.label }));
  return <Select value={p.value == null ? "" : String(p.value)} options={opts} onChange={(v) => p.onChange(v === "" ? null : Number(v))} testId={p.testId} width={280} />;
}

function ColorPicker({ value, onChange }: { value: string; onChange: (c: Cue["color"]) => void }) {
  return (
    <div className="swatches" role="radiogroup" aria-label="Colour">
      {CUE_COLORS.map((c) => (
        <button key={c || "none"} role="radio" aria-checked={value === c} className={`swatch sw-${c || "none"}${value === c ? " on" : ""}`} title={c || "none"} aria-label={c || "no colour"} onClick={() => onChange(c)} />
      ))}
    </div>
  );
}

// ---------------- tabs ----------------

function BasicTab({ cue }: { cue: Cue }) {
  return (
    <div className="insp-cols">
      <div className="insp-col">
        <Row label="Number">
          <TextField value={cue.number} width={110} testId="insp-number" onCommit={(v) => patchCue(cue, { number: v.trim() })} />
        </Row>
        <Row label="Name">
          <TextField value={cue.name} testId="insp-name" onCommit={(v) => patchCue(cue, { name: v })} />
        </Row>
        <Row label="Colour">
          <ColorPicker value={cue.color} onChange={(c) => patchCue(cue, { color: c })} />
        </Row>
        <Row label="Continue">
          <Select value={cue.continue_mode} options={CONTINUE_OPTIONS} onChange={(v) => patchCue(cue, { continue_mode: v })} testId="insp-continue" />
        </Row>
        <Row label="Armed">
          <Check checked={cue.armed} onChange={(v) => patchCue(cue, { armed: v })} label="Cue fires when triggered" testId="insp-armed" />
        </Row>
      </div>
      <div className="insp-col">
        <NotesField value={cue.notes} onCommit={(v) => patchCue(cue, { notes: v })} />
      </div>
    </div>
  );
}

function NotesField({ value, onCommit }: { value: string; onCommit: (v: string) => void }) {
  const [draft, setDraft] = useState<string | null>(null);
  return (
    <label className="notes-field">
      <span className="form-label">Notes</span>
      <textarea
        className="input"
        value={draft ?? value}
        data-testid="insp-notes"
        spellCheck={false}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => {
          if (draft != null && draft !== value) onCommit(draft);
          setDraft(null);
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            setDraft(null);
            e.currentTarget.blur();
          }
        }}
      />
    </label>
  );
}

function TimingTab({ cue, rate }: { cue: Cue; rate: string }) {
  return (
    <div className="insp-col">
      <Row label="Pre-wait" hint="seconds or m:ss.s">
        <TimeField value={cue.pre_wait} width={110} testId="insp-prewait" onCommit={(v) => patchCue(cue, { pre_wait: v ?? 0 })} />
      </Row>
      <Row label="Post-wait">
        <TimeField value={cue.post_wait} width={110} onCommit={(v) => patchCue(cue, { post_wait: v ?? 0 })} />
      </Row>
      <Row label="TC trigger" hint={`fires when the timecode reaches this position (${rate} fps); empty = none`}>
        <TcField value={cue.tc_trigger} rate={rate} nullable width={140} testId="insp-tc" onCommit={(v) => patchCue(cue, { tc_trigger: v })} />
        {cue.tc_trigger ? (
          <button className="btn small" onClick={() => patchCue(cue, { tc_trigger: null })}>
            Clear
          </button>
        ) : null}
      </Row>
    </div>
  );
}

function FadeTab({ cue, kind, show }: { cue: Cue; kind: FadeKind; show: Show }) {
  const outputs = show.patch.outputs;
  const setCross = (cp: RoutePoint[]) => patchKind(cue, { crosspoints: cp });
  return (
    <div className="insp-cols">
      <div className="insp-col">
        <Row label="Target">
          <CueSelect value={kind.target} options={cueOptions(show, cue, (c) => c.kind.type === "audio" || c.kind.type === "group")} onChange={(v) => patchKind(cue, { target: v })} testId="fade-target" />
        </Row>
        <Row label="Duration">
          <TimeField value={kind.duration} width={110} onCommit={(v) => patchKind(cue, { duration: v ?? 0 })} />
        </Row>
        <Row label="Curve">
          <Select value={kind.curve} options={CURVE_OPTIONS} onChange={(v) => patchKind(cue, { curve: v })} />
        </Row>
        <Row label="Level" hint="target cue level at the end of the fade">
          <Check checked={kind.gain_db != null} onChange={(v) => patchKind(cue, { gain_db: v ? -120 : null })} label="Fade level to" />
          <DbField value={kind.gain_db} nullable disabled={kind.gain_db == null} width={90} onCommit={(v) => patchKind(cue, { gain_db: v })} />
          <span className="unit">dB</span>
        </Row>
        <Row label="When done">
          <Check checked={kind.stop_target} onChange={(v) => patchKind(cue, { stop_target: v })} label="Stop target cue" />
        </Row>
      </div>
      <div className="insp-col">
        <span className="form-label">Crosspoint targets</span>
        <div className="xp-list">
          {kind.crosspoints.length === 0 ? <span className="muted">None – only the cue level is faded.</span> : null}
          {kind.crosspoints.map((cp, i) => (
            <div className="xp-row" key={i}>
              <span className="unit">src</span>
              <NumField value={cp.src + 1} integer min={1} max={64} width={56} onCommit={(v) => setCross(kind.crosspoints.map((x, j) => (j === i ? { ...x, src: v - 1 } : x)))} />
              <span className="unit">→</span>
              <Select value={cp.out} options={outputs.map((o, oi) => ({ value: oi, label: `${oi + 1} ${o.name}` }))} onChange={(v) => setCross(kind.crosspoints.map((x, j) => (j === i ? { ...x, out: v } : x)))} width={150} />
              <DbField value={cp.db} width={80} onCommit={(v) => setCross(kind.crosspoints.map((x, j) => (j === i ? { ...x, db: v ?? -120 } : x)))} />
              <span className="unit">dB</span>
              <button className="btn small" onClick={() => setCross(kind.crosspoints.filter((_, j) => j !== i))} aria-label="Remove crosspoint">
                ✕
              </button>
            </div>
          ))}
          <button className="btn small" disabled={outputs.length === 0} onClick={() => setCross([...kind.crosspoints, { src: 0, out: 0, db: -120 }])}>
            + Add crosspoint
          </button>
        </div>
      </div>
    </div>
  );
}

const MSG_LABELS: Record<MidiMsgType, string> = {
  note_on: "Note On", note_off: "Note Off", control_change: "Control Change", program_change: "Program Change",
  pitch_bend: "Pitch Bend", sysex: "SysEx", msc: "MIDI Show Control",
};

/** A full message of the new type plus `null` for every field the old type had (RFC 7396 removal). */
export function midiTypeChangePatch(old: MidiMessage, type: MidiMsgType): Record<string, unknown> {
  const channel = "channel" in old ? old.channel : 1;
  let next: MidiMessage;
  switch (type) {
    case "note_on":
    case "note_off":
      next = { msg: type, channel, note: "note" in old ? old.note : 60, velocity: "velocity" in old ? old.velocity : type === "note_on" ? 100 : 0 };
      break;
    case "control_change":
      next = { msg: type, channel, controller: 0, value: 0 };
      break;
    case "program_change":
      next = { msg: type, channel, program: 0 };
      break;
    case "pitch_bend":
      next = { msg: type, channel, value: 0 };
      break;
    case "sysex":
      next = { msg: type, hex: "F0 7F 7F 04 01 00 7F F7" };
      break;
    case "msc":
      next = { msg: type, device_id: 0, command_format: 1, command: "go", q_number: "", q_list: "", q_path: "" };
      break;
  }
  const patch: Record<string, unknown> = {};
  for (const k of Object.keys(old)) patch[k] = null;
  return { ...patch, ...next };
}

function MidiTab({ cue, kind, show }: { cue: Cue; kind: MidiKind; show: Show }) {
  const devicePorts = useApp((s) => s.doc?.midi.ports);
  const m = kind.message;
  const aliases = show.midi_ports.map((p) => ({ value: p.name, label: `${p.name}  (${p.device || "unassigned"})` }));
  const raw = (devicePorts ?? []).filter((d) => !show.midi_ports.some((p) => p.name === d)).map((d) => ({ value: d, label: `${d}  (device)` }));
  const setMsg = (patch: Record<string, unknown>) => patchKind(cue, { message: patch });
  const ch = "channel" in m ? (
    <Row label="Channel">
      <NumField value={m.channel} integer min={1} max={16} width={70} onCommit={(v) => setMsg({ channel: v })} />
    </Row>
  ) : null;
  return (
    <div className="insp-cols">
      <div className="insp-col">
        <Row label="Port">
          <Select value={kind.port} options={[{ value: "", label: "— none —" }, ...aliases, ...raw]} onChange={(v) => patchKind(cue, { port: v })} testId="midi-port" width={280} />
        </Row>
        <Row label="Message">
          <Select value={m.msg} options={MIDI_MSG_TYPES.map((t) => ({ value: t, label: MSG_LABELS[t] }))} onChange={(t) => setMsg(midiTypeChangePatch(m, t))} testId="midi-type" width={220} />
        </Row>
        {ch}
        {m.msg === "note_on" || m.msg === "note_off" ? (
          <>
            <Row label="Note">
              <NumField value={m.note} integer min={0} max={127} width={70} onCommit={(v) => setMsg({ note: v })} />
            </Row>
            <Row label="Velocity">
              <NumField value={m.velocity} integer min={0} max={127} width={70} onCommit={(v) => setMsg({ velocity: v })} />
            </Row>
          </>
        ) : null}
        {m.msg === "control_change" ? (
          <>
            <Row label="Controller">
              <NumField value={m.controller} integer min={0} max={127} width={70} onCommit={(v) => setMsg({ controller: v })} />
            </Row>
            <Row label="Value">
              <NumField value={m.value} integer min={0} max={127} width={70} onCommit={(v) => setMsg({ value: v })} />
            </Row>
          </>
        ) : null}
        {m.msg === "program_change" ? (
          <Row label="Program">
            <NumField value={m.program} integer min={0} max={127} width={70} onCommit={(v) => setMsg({ program: v })} />
          </Row>
        ) : null}
        {m.msg === "pitch_bend" ? (
          <Row label="Value" hint="-8192 … 8191 (0 = centre)">
            <NumField value={m.value} integer min={-8192} max={8191} width={90} onCommit={(v) => setMsg({ value: v })} />
          </Row>
        ) : null}
        {m.msg === "sysex" ? (
          <Row label="Hex data" hint="must start with F0 and end with F7; data bytes 00–7F">
            <DraftField<string>
              value={m.hex}
              mono
              testId="sysex-hex"
              format={(v) => v}
              parse={(t) => {
                const r = validateSysex(t);
                return r.ok ? { ok: true, value: r.hex } : { ok: false, error: r.error };
              }}
              onCommit={(v) => setMsg({ hex: v })}
            />
          </Row>
        ) : null}
      </div>
      {m.msg === "msc" ? (
        <div className="insp-col">
          <Row label="Device ID" hint="0–111, 127 = all">
            <NumField value={m.device_id} integer min={0} max={127} width={70} onCommit={(v) => setMsg({ device_id: v })} />
          </Row>
          <Row label="Cmd format" hint="1 = lighting, 16 = sound, 127 = all types">
            <NumField value={m.command_format} integer min={0} max={127} width={70} onCommit={(v) => setMsg({ command_format: v })} />
          </Row>
          <Row label="Command">
            <Select value={m.command} options={MSC_COMMANDS.map((c) => ({ value: c, label: c.replace(/_/g, " ").toUpperCase() }))} onChange={(v) => setMsg({ command: v })} width={160} />
          </Row>
          <Row label="Q number">
            <DraftField<string> value={m.q_number} mono width={110} format={(v) => v} parse={(t) => (/^[0-9.]*$/.test(t.trim()) ? { ok: true, value: t.trim() } : { ok: false, error: "Digits and dots only" })} onCommit={(v) => setMsg({ q_number: v })} />
          </Row>
          <Row label="Q list">
            <DraftField<string> value={m.q_list} mono width={110} format={(v) => v} parse={(t) => (/^[0-9.]*$/.test(t.trim()) ? { ok: true, value: t.trim() } : { ok: false, error: "Digits and dots only" })} onCommit={(v) => setMsg({ q_list: v })} />
          </Row>
          <Row label="Q path">
            <DraftField<string> value={m.q_path} mono width={110} format={(v) => v} parse={(t) => (/^[0-9.]*$/.test(t.trim()) ? { ok: true, value: t.trim() } : { ok: false, error: "Digits and dots only" })} onCommit={(v) => setMsg({ q_path: v })} />
          </Row>
        </div>
      ) : null}
    </div>
  );
}

function TimecodeTab({ cue, kind, rate, start }: { cue: Cue; kind: TimecodeKind; rate: string; start: string }) {
  const tc = kind.tc;
  type A = TimecodeKind["tc"]["action"];
  const options: { value: A; label: string }[] = [
    { value: "start", label: "Start" }, { value: "start_from", label: "Start from…" }, { value: "stop", label: "Stop" },
    { value: "pause", label: "Pause" }, { value: "resume", label: "Resume" }, { value: "relocate", label: "Relocate to…" },
  ];
  const setAction = (a: A) => {
    // null removes the fields that do not belong to the new action (merge patch)
    if (a === "start_from") patchKind(cue, { tc: { action: a, from: tc.action === "relocate" ? tc.to : start, to: null } });
    else if (a === "relocate") patchKind(cue, { tc: { action: a, to: tc.action === "start_from" ? tc.from : start, from: null } });
    else patchKind(cue, { tc: { action: a, from: null, to: null } });
  };
  return (
    <div className="insp-col">
      <Row label="Action">
        <Select value={tc.action} options={options} onChange={setAction} testId="tc-action" width={180} />
      </Row>
      {tc.action === "start_from" ? (
        <Row label="From">
          <TcField value={tc.from} rate={rate} width={140} onCommit={(v) => v && patchKind(cue, { tc: { from: v } })} />
        </Row>
      ) : null}
      {tc.action === "relocate" ? (
        <Row label="To">
          <TcField value={tc.to} rate={rate} width={140} onCommit={(v) => v && patchKind(cue, { tc: { to: v } })} />
        </Row>
      ) : null}
    </div>
  );
}

function ControlTab({ cue, kind, show }: { cue: Cue; kind: ControlKind; show: Show }) {
  const needsTarget = kind.action !== "reset";
  return (
    <div className="insp-col">
      <Row label="Action">
        <Select value={kind.action} options={CONTROL_ACTIONS.map((a) => ({ value: a, label: a.replace(/_/g, " ") }))} onChange={(v) => patchKind(cue, { action: v })} testId="control-action" width={180} />
      </Row>
      {needsTarget ? (
        <Row label="Target">
          <CueSelect value={kind.target} options={cueOptions(show, cue)} onChange={(v) => patchKind(cue, { target: v })} testId="control-target" />
        </Row>
      ) : null}
      {kind.action === "load_to_time" ? (
        <Row label="Time">
          <TimeField value={kind.time} width={110} onCommit={(v) => patchKind(cue, { time: v ?? 0 })} />
        </Row>
      ) : null}
    </div>
  );
}

function HttpTab({ cue, kind }: { cue: Cue; kind: HttpKind }) {
  const methods = ["GET", "POST", "PUT", "PATCH", "DELETE"] as const;
  return (
    <div className="insp-col">
      <Row label="Method">
        <Select value={kind.method} options={methods.map((m) => ({ value: m, label: m }))} onChange={(v) => patchKind(cue, { method: v })} testId="http-method" width={110} />
      </Row>
      <Row label="URL" hint="plain http:// on the show network">
        <DraftField<string>
          value={kind.url}
          format={(v) => v}
          parse={(t) => (/^http:\/\/[^\s/?#@]+/.test(t.trim()) ? { ok: true, value: t.trim() } : { ok: false, error: "URL must start with http://" })}
          onCommit={(v) => patchKind(cue, { url: v })}
          ariaLabel="Request URL"
          testId="http-url"
          mono
          width={320}
        />
      </Row>
      {kind.method !== "GET" || kind.body ? (
        <>
          <Row label="Body">
            <TextField value={kind.body} ariaLabel="Request body" testId="http-body" width={320} onCommit={(v) => patchKind(cue, { body: v })} />
          </Row>
          <Row label="Content type">
            <TextField value={kind.content_type} ariaLabel="Content type" testId="http-ct" width={200} onCommit={(v) => patchKind(cue, { content_type: v.trim() })} />
          </Row>
        </>
      ) : null}
    </div>
  );
}

function OscTab({ cue, kind }: { cue: Cue; kind: OscKind }) {
  return (
    <div className="insp-col">
      <Row label="Receiver">
        <TextField value={kind.host} ariaLabel="OSC receiver" testId="osc-host" width={200} onCommit={(v) => v.trim() && patchKind(cue, { host: v.trim() })} />
      </Row>
      <Row label="Port">
        <NumField value={kind.port} integer min={1} max={65535} ariaLabel="OSC port" testId="osc-port" width={90} onCommit={(v) => patchKind(cue, { port: v })} />
      </Row>
      <Row label="Address">
        <DraftField<string>
          value={kind.address}
          format={(v) => v}
          parse={(t) => {
            const err = validateOscAddress(t.trim());
            return err ? { ok: false, error: err } : { ok: true, value: t.trim() };
          }}
          onCommit={(v) => patchKind(cue, { address: v })}
          ariaLabel="OSC address"
          testId="osc-address"
          mono
          width={260}
        />
      </Row>
      <Row label="Arguments" hint='12 = int, 0.5 = float, true/false, text, "in quotes" for spaces'>
        <TextField value={formatOscArgs(kind.args)} ariaLabel="OSC arguments" testId="osc-args" width={260} onCommit={(v) => patchKind(cue, { args: parseOscArgs(v) })} />
      </Row>
    </div>
  );
}

function GroupTab({ cue, kind }: { cue: Cue; kind: GroupKind }) {
  const options: { value: GroupMode; label: string }[] = [
    { value: "parallel", label: "Parallel – fire all children together" },
    { value: "playlist", label: "Playlist – children one after another" },
    { value: "random", label: "Random – one random child per GO" },
    { value: "list", label: "List – each cue takes its own GO" },
  ];
  return (
    <div className="insp-col">
      <Row label="Mode">
        <Select value={kind.mode} options={options} onChange={(v) => patchKind(cue, { mode: v })} testId="group-mode" />
      </Row>
      <Row label="Children">
        <span>{kind.children.length} cue{kind.children.length === 1 ? "" : "s"} – drag cues onto the group row to add them.</span>
      </Row>
    </div>
  );
}

// ---------------- bulk edit ----------------

function common<T>(cues: Cue[], get: (c: Cue) => T): T | undefined {
  const first = get(cues[0]);
  return cues.every((c) => get(c) === first) ? first : undefined;
}

function BulkEdit({ cues }: { cues: Cue[] }) {
  const ids = cues.map((c) => c.id);
  const patch = (p: CuePatch) => void run({ cmd: "update_cues", cues: ids, patch: p });
  const armed = common(cues, (c) => c.armed);
  const cont = common(cues, (c) => c.continue_mode);
  const color = common(cues, (c) => c.color);
  const pre = common(cues, (c) => c.pre_wait);
  const post = common(cues, (c) => c.post_wait);
  const [start, setStart] = useState(1);
  const [step, setStep] = useState(1);
  return (
    <div className="insp-cols" data-testid="bulk-edit">
      <div className="insp-col">
        <Row label="Armed">
          <Check checked={armed ?? false} indeterminate={armed === undefined} onChange={(v) => patch({ armed: v })} label={armed === undefined ? "mixed" : "all selected cues"} />
        </Row>
        <Row label="Colour">
          <ColorPicker value={color ?? "__mixed"} onChange={(c) => patch({ color: c })} />
        </Row>
        <Row label="Continue">
          <Select<ContinueMode | "__mixed"> value={cont ?? "__mixed"} options={CONTINUE_OPTIONS} onChange={(v) => v !== "__mixed" && patch({ continue_mode: v })} />
        </Row>
        <Row label="Pre-wait">
          <TimeField value={pre ?? null} nullable placeholder="mixed" width={110} onCommit={(v) => v != null && patch({ pre_wait: v })} />
        </Row>
        <Row label="Post-wait">
          <TimeField value={post ?? null} nullable placeholder="mixed" width={110} onCommit={(v) => v != null && patch({ post_wait: v })} />
        </Row>
      </div>
      <div className="insp-col">
        <Row label="Renumber">
          <span className="unit">start</span>
          <NumField value={start} min={0} width={70} onCommit={setStart} />
          <span className="unit">step</span>
          <NumField value={step} min={0.01} width={70} onCommit={setStep} />
          <button className="btn small" onClick={() => void run({ cmd: "renumber", cues: ids, start, step })}>
            Apply
          </button>
        </Row>
        <Row label="Actions">
          <button className="btn small" onClick={() => void run({ cmd: "group_cues", cues: ids })}>Group</button>
          <button className="btn small" onClick={() => void duplicateCues(ids)}>Duplicate</button>
          <button className="btn small danger" onClick={() => void deleteCues(ids)}>Delete</button>
        </Row>
      </div>
    </div>
  );
}

// ---------------- shell ----------------

type Tab = "basic" | "timing" | "kind" | "routing";

const COLLAPSE_KEY = "sm.inspector.collapsed";

/** Duplicate / Delete for the inspected cue(s) – always visible, also on touch screens. */
function CueActions({ ids }: { ids: number[] }) {
  const n = ids.length;
  return (
    <span className="insp-actions">
      <button className="btn small" onClick={() => void duplicateCues(ids)} data-testid="insp-duplicate" aria-label={n > 1 ? `Duplicate ${cueCount(n)}` : "Duplicate cue"}>
        <IconCopy /> <span className="insp-act-text">Duplicate</span>
      </button>
      <button className="btn small danger" onClick={() => void deleteCues(ids)} data-testid="insp-delete" aria-label={n > 1 ? `Delete ${cueCount(n)}` : "Delete cue"}>
        <IconTrash /> <span className="insp-act-text">Delete</span>
      </button>
    </span>
  );
}

export function Inspector() {
  const show = useApp((s) => s.doc?.show ?? null);
  const selection = useApp((s) => s.selection);
  const errors = useApp((s) => s.doc?.errors);
  const coarse = useMedia(COARSE_POINTER);
  const [tab, setTab] = useState<Tab>("basic");
  const [collapsed, setCollapsedState] = useState(() => loadFlag(COLLAPSE_KEY, false));
  const setCollapsed = (v: boolean) => {
    setCollapsedState(v);
    saveFlag(COLLAPSE_KEY, v);
  };
  if (!show) return null;
  const cues = selection.map((id) => findCue(show, id)).filter((c): c is Cue => c != null);
  if (cues.length === 0) {
    return (
      <section className="inspector empty" data-testid="inspector">
        <span className="muted">
          {coarse
            ? "Tap a cue to edit it. Long-press a cue for more actions (move, duplicate, delete)."
            : "Select a cue to edit it. Shift / Ctrl-click selects several cues for bulk editing. Right-click a cue for more actions."}
        </span>
      </section>
    );
  }
  const toggle = (
    <button
      className="btn ghost icon-only insp-toggle"
      onClick={() => setCollapsed(!collapsed)}
      aria-expanded={!collapsed}
      aria-label={collapsed ? "Show inspector" : "Hide inspector"}
      data-testid="insp-toggle"
    >
      {collapsed ? <IconUp size={18} /> : <IconChevronDown size={18} />}
    </button>
  );
  const cls = `inspector${collapsed ? " collapsed" : ""}`;
  if (cues.length > 1) {
    return (
      <section className={cls} data-testid="inspector">
        <div className="insp-tabs">
          <span className="insp-title">{cueCount(cues.length)} selected</span>
          <span className="spacer" />
          <CueActions ids={cues.map((c) => c.id)} />
          {toggle}
        </div>
        <div className="insp-body">
          <BulkEdit cues={cues} />
        </div>
      </section>
    );
  }
  const cue = cues[0];
  const error = errors?.[String(cue.id)];
  const k = cue.kind;
  const hasKindTab = k.type !== "memo";
  const cur: Tab = (tab === "kind" && !hasKindTab) || (tab === "routing" && k.type !== "audio") ? "basic" : tab;
  const rate = show.timecode.rate;
  const tabs: { id: Tab; label: string }[] = [
    { id: "basic", label: "Basic" },
    { id: "timing", label: "Timing" },
    ...(hasKindTab ? [{ id: "kind" as Tab, label: k.type === "timecode" ? "Timecode cue" : TYPE_LABELS[k.type] }] : []),
    ...(k.type === "audio" ? [{ id: "routing" as Tab, label: "Routing" }] : []),
  ];
  return (
    <section className={cls} data-testid="inspector">
      <div className="insp-tabs">
        <span className="insp-title ellipsis">{cueLabel(cue)}</span>
        <span className="insp-tablist" role="tablist">
          {tabs.map((t) => (
            <button
              key={t.id}
              role="tab"
              aria-selected={cur === t.id}
              className={`insp-tab${cur === t.id ? " on" : ""}`}
              data-testid={`insp-tab-${t.id}`}
              onClick={() => {
                setTab(t.id);
                if (collapsed) setCollapsed(false);
              }}
            >
              {t.label}
            </button>
          ))}
        </span>
        <span className="spacer" />
        <CueActions ids={[cue.id]} />
        {toggle}
      </div>
      {error ? (
        <div className="insp-error" role="alert" data-testid="insp-error">
          <IconError /> <span>{error}</span>
        </div>
      ) : null}
      <div className="insp-body" key={cue.id}>
        {cur === "basic" ? <BasicTab cue={cue} /> : null}
        {cur === "timing" ? <TimingTab cue={cue} rate={rate} /> : null}
        {cur === "kind" && k.type === "audio" ? <AudioTab cue={cue} kind={k} /> : null}
        {cur === "kind" && k.type === "fade" ? <FadeTab cue={cue} kind={k} show={show} /> : null}
        {cur === "kind" && k.type === "midi" ? <MidiTab cue={cue} kind={k} show={show} /> : null}
        {cur === "kind" && k.type === "osc" ? <OscTab cue={cue} kind={k} /> : null}
        {cur === "kind" && k.type === "http" ? <HttpTab cue={cue} kind={k} /> : null}
        {cur === "kind" && k.type === "timecode" ? <TimecodeTab cue={cue} kind={k} rate={rate} start={show.timecode.start} /> : null}
        {cur === "kind" && k.type === "control" ? <ControlTab cue={cue} kind={k} show={show} /> : null}
        {cur === "kind" && k.type === "group" ? <GroupTab cue={cue} kind={k} /> : null}
        {cur === "routing" && k.type === "audio" ? <RoutingTab cue={cue} kind={k} show={show} /> : null}
      </div>
    </section>
  );
}
