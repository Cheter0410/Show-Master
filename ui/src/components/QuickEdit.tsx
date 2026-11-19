// Quick edits on the tablet (/remote): the everyday fields of one cue, big enough for touch.
// The detailed editing happens on the host computer; the server only lets the tablet change
// these fields (no new cues, files or targets). In Live mode only notes and level can change.
import { CUE_COLORS, type AudioKind, type ContinueMode, type Cue, type CuePatch, type FadeKind, type Loop } from "../api";
import { run } from "../commands";
import { cueLabel } from "../cues";
import { useApp } from "../store";
import { Check, DbField, NumField, Row, Select, TextField, TimeField } from "./fields";
import { Modal } from "./Modal";

const CONTINUE_OPTIONS: { value: ContinueMode; label: string }[] = [
  { value: "do_not_continue", label: "Do not continue" },
  { value: "auto_continue", label: "Auto-continue" },
  { value: "auto_follow", label: "Auto-follow" },
];

function patchCue(cue: Cue, patch: CuePatch): void {
  void run({ cmd: "update_cue", cue: cue.id, patch });
}

function patchKind(cue: Cue, kind: Record<string, unknown>): void {
  patchCue(cue, { kind });
}

/** The cue being edited, always fresh from the store (it may change elsewhere meanwhile). */
export function QuickEdit({ cueId, onClose }: { cueId: number; onClose: () => void }) {
  const cue = useApp((s) => {
    const show = s.doc?.show;
    if (!show) return null;
    const find = (cues: Cue[]): Cue | null => {
      for (const c of cues) {
        if (c.id === cueId) return c;
        if (c.kind.type === "group") {
          const hit = find(c.kind.children);
          if (hit) return hit;
        }
      }
      return null;
    };
    for (const l of show.cue_lists) {
      const hit = find(l.cues);
      if (hit) return hit;
    }
    return null;
  });
  const live = useApp((s) => s.doc?.mode === "live");
  if (!cue) return null;
  const k = cue.kind;
  return (
    <Modal
      title={<>Edit {cueLabel(cue)}</>}
      onClose={onClose}
      wide
      className="quick-edit"
      testId="quick-edit"
      footer={
        <button className="btn primary big" data-autofocus onClick={onClose} data-testid="quick-edit-done">
          Done
        </button>
      }
    >
      {live ? <p className="qe-live" data-testid="qe-live">Live mode: only notes and level can change. Switch to EDIT for the rest.</p> : null}
      <div className="qe-grid">
        <Row label="Number">
          <TextField value={cue.number} disabled={live} width={110} ariaLabel="Number" testId="qe-number" onCommit={(v) => patchCue(cue, { number: v.trim() })} />
        </Row>
        <Row label="Name">
          <TextField value={cue.name} disabled={live} ariaLabel="Name" testId="qe-name" width="100%" onCommit={(v) => patchCue(cue, { name: v })} />
        </Row>
        <Row label="Notes">
          <TextField value={cue.notes} ariaLabel="Notes" testId="qe-notes" width="100%" onCommit={(v) => patchCue(cue, { notes: v })} />
        </Row>
        <Row label="Colour">
          <div className="swatches" role="radiogroup" aria-label="Colour">
            {CUE_COLORS.map((c) => (
              <button
                key={c || "none"}
                role="radio"
                aria-checked={cue.color === c}
                disabled={live}
                className={`swatch sw-${c || "none"}${cue.color === c ? " on" : ""}`}
                aria-label={c || "no colour"}
                onClick={() => patchCue(cue, { color: c })}
              />
            ))}
          </div>
        </Row>
        <Row label="Armed">
          <Check checked={cue.armed} disabled={live} label="Plays on GO" testId="qe-armed" onChange={(v) => patchCue(cue, { armed: v })} />
        </Row>
        <Row label="Pre-wait">
          <TimeField value={cue.pre_wait} disabled={live} width={110} ariaLabel="Pre-wait" onCommit={(v) => patchCue(cue, { pre_wait: v ?? 0 })} />
        </Row>
        <Row label="Post-wait">
          <TimeField value={cue.post_wait} disabled={live} width={110} ariaLabel="Post-wait" onCommit={(v) => patchCue(cue, { post_wait: v ?? 0 })} />
        </Row>
        <Row label="Continue">
          <Select value={cue.continue_mode} options={CONTINUE_OPTIONS} disabled={live} ariaLabel="Continue" onChange={(v) => patchCue(cue, { continue_mode: v })} />
        </Row>
      </div>
      {k.type === "audio" ? <AudioPart cue={cue} k={k} live={live} /> : null}
      {k.type === "fade" ? <FadePart cue={cue} k={k} live={live} /> : null}
    </Modal>
  );
}

function AudioPart({ cue, k, live }: { cue: Cue; k: AudioKind; live: boolean }) {
  const length = k.end ?? k.file_duration ?? null;
  return (
    <>
      <h3 className="qe-head">Audio</h3>
      <div className="qe-grid">
        <Row label="Level">
          <DbField value={k.gain_db} width={110} ariaLabel="Level" testId="qe-gain" onCommit={(v) => patchKind(cue, { gain_db: v ?? 0 })} />
        </Row>
        <Row label="Fade in">
          <TimeField value={k.fade_in} disabled={live} width={110} ariaLabel="Fade in" onCommit={(v) => patchKind(cue, { fade_in: v ?? 0 })} />
        </Row>
        <Row label="Fade out">
          <TimeField value={k.fade_out} disabled={live} width={110} ariaLabel="Fade out" onCommit={(v) => patchKind(cue, { fade_out: v ?? 0 })} />
        </Row>
        <Row label="Start">
          <TimeField value={k.start} disabled={live} width={110} ariaLabel="Start" max={length ?? undefined} onCommit={(v) => patchKind(cue, { start: v ?? 0 })} />
        </Row>
        <Row label="End" hint="empty = end of file">
          <TimeField value={k.end} nullable disabled={live} width={110} ariaLabel="End" max={k.file_duration ?? undefined} onCommit={(v) => patchKind(cue, { end: v })} />
        </Row>
      </div>
      <Loops cue={cue} k={k} live={live} />
    </>
  );
}

function Loops({ cue, k, live }: { cue: Cue; k: AudioKind; live: boolean }) {
  const loops = k.loops ?? [];
  // Arrays are replaced as a whole by a merge patch: always send the full list.
  const save = (next: Loop[]) => patchKind(cue, { loops: next.slice().sort((a, b) => a.start - b.start) });
  const change = (i: number, part: Partial<Loop>) => save(loops.map((l, j) => (j === i ? { ...l, ...part } : l)));
  const add = () => {
    const end = k.end ?? k.file_duration ?? k.start + 4;
    const from = loops.length ? loops[loops.length - 1].end : k.start;
    const to = Math.min(end, from + 2);
    if (to - from < 0.05) return;
    let n = loops.length + 1;
    while (loops.some((l) => l.name === `Loop ${n}`)) n++;
    save([
      ...loops,
      { name: `Loop ${n}`, start: from, end: to, crossfade: 0, release_on_go: true, passes: 0, immediate: false, timecode: true },
    ]);
  };
  return (
    <>
      <h3 className="qe-head">Loops</h3>
      {loops.length === 0 ? <p className="hint">No loops.</p> : null}
      {loops.map((l, i) => (
        <div className="qe-loop" key={i} data-testid={`qe-loop-${i}`}>
          <div className="qe-loop-row">
            <TextField value={l.name} disabled={live} width={150} ariaLabel={`Loop ${i + 1} name`} placeholder={`Loop ${i + 1}`} onCommit={(v) => change(i, { name: v.trim() })} />
            <label className="qe-mini">
              from
              <TimeField value={l.start} disabled={live} width={90} ariaLabel={`Loop ${i + 1} from`} onCommit={(v) => change(i, { start: v ?? 0 })} />
            </label>
            <label className="qe-mini">
              to
              <TimeField value={l.end} disabled={live} width={90} ariaLabel={`Loop ${i + 1} to`} onCommit={(v) => change(i, { end: v ?? 0 })} />
            </label>
            <button className="btn danger" disabled={live} aria-label={`Remove loop ${i + 1}`} onClick={() => save(loops.filter((_, j) => j !== i))}>
              Remove
            </button>
          </div>
          <div className="qe-loop-row">
            <label className="qe-mini">
              crossfade
              <TimeField value={l.crossfade} disabled={live} width={80} ariaLabel={`Loop ${i + 1} crossfade`} max={10} onCommit={(v) => change(i, { crossfade: v ?? 0 })} />
            </label>
            <label className="qe-mini">
              passes
              <NumField
                value={l.passes}
                integer
                min={0}
                disabled={live}
                width={70}
                ariaLabel={`Loop ${i + 1} passes`}
                placeholder="∞"
                onCommit={(v) => change(i, { passes: v })}
              />
            </label>
            <Check checked={l.release_on_go} disabled={live} label="GO releases" onChange={(v) => change(i, { release_on_go: v })} />
            <Check checked={l.immediate} disabled={live} label="Release at once" onChange={(v) => change(i, { immediate: v })} />
          </div>
        </div>
      ))}
      <button className="btn" disabled={live} onClick={add} data-testid="qe-add-loop">
        Add loop
      </button>
      <span className="hint"> passes 0 = until released</span>
    </>
  );
}

function FadePart({ cue, k, live }: { cue: Cue; k: FadeKind; live: boolean }) {
  return (
    <>
      <h3 className="qe-head">Fade</h3>
      <div className="qe-grid">
        <Row label="Duration">
          <TimeField value={k.duration} disabled={live} width={110} ariaLabel="Duration" onCommit={(v) => patchKind(cue, { duration: v ?? 0 })} />
        </Row>
        <Row label="To level" hint="empty = level unchanged">
          <DbField value={k.gain_db} nullable disabled={live} width={110} ariaLabel="To level" onCommit={(v) => patchKind(cue, { gain_db: v })} />
        </Row>
        <Row label="Then">
          <Check checked={k.stop_target} disabled={live} label="Stop the cue" onChange={(v) => patchKind(cue, { stop_target: v })} />
        </Row>
      </div>
    </>
  );
}
