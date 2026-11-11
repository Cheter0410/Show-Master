// Audio cue inspector: file, waveform with start/end markers, levels, loops, preview, routing matrix.
import { useEffect, useRef, useState } from "react";
import { fetchWaveform, type AudioKind, type Cue, type RoutePoint, type Show, type Waveform } from "../api";
import { run, transport } from "../commands";
import { Check, DbField, NumField, Row, Select, Slider, TimeField } from "../components/fields";
import { IconPlay, IconStop } from "../components/icons";
import { getApp, useApp } from "../store";
import { DB_FLOOR, formatDb, formatDuration, parseDb } from "../time";
import { FileBrowser } from "./FileBrowser";
import { CURVE_OPTIONS } from "./Inspector";

function patchKind(cue: Cue, kind: Record<string, unknown>): void {
  void run({ cmd: "update_cue", cue: cue.id, patch: { kind } });
}

// ---------------- waveform ----------------

type Marker = "start" | "end";

function WaveformView({ cue, kind }: { cue: Cue; kind: AudioKind }) {
  const token = useApp((s) => s.token);
  const [wave, setWave] = useState<Waveform | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [drag, setDrag] = useState<{ marker: Marker; time: number } | null>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const duration = wave?.duration ?? kind.file_duration ?? 0;
  const start = drag?.marker === "start" ? drag.time : kind.start;
  const end = drag?.marker === "end" ? drag.time : kind.end ?? duration;
  const view = useRef({ start, end, duration, wave });
  view.current = { start, end, duration, wave };

  useEffect(() => {
    setWave(null);
    setError(null);
    if (!kind.file || !token) return;
    const ac = new AbortController();
    fetchWaveform(token, kind.file, 800, ac.signal).then(
      (w) => setWave(w),
      (e: unknown) => {
        if (!ac.signal.aborted) setError(e instanceof Error && /404/.test(e.message) ? "File not found" : "Waveform not available");
      },
    );
    return () => ac.abort();
  }, [kind.file, token]);

  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    let raf = 0;
    const style = getComputedStyle(c);
    const col = (n: string, f: string) => style.getPropertyValue(n).trim() || f;
    const colors = { wave: col("--blue", "#3b82f6"), dim: col("--border", "#333"), green: col("--green", "#22c55e"), red: col("--red", "#ef4444"), text: col("--text", "#fff"), bg: col("--meter-bg", "#0a0c0f") };
    const draw = () => {
      raf = requestAnimationFrame(draw);
      const dpr = Math.min(2, window.devicePixelRatio || 1);
      const w = Math.max(1, Math.round(c.clientWidth * dpr));
      const h = Math.max(1, Math.round(c.clientHeight * dpr));
      if (c.width !== w || c.height !== h) {
        c.width = w;
        c.height = h;
      }
      const g = c.getContext("2d");
      if (!g) return;
      const v = view.current;
      g.fillStyle = colors.bg;
      g.fillRect(0, 0, w, h);
      const x = (t: number) => (v.duration > 0 ? (t / v.duration) * w : 0);
      const xs = x(v.start);
      const xe = x(v.end);
      if (v.wave) {
        const peaks = v.wave.peaks;
        const mid = h / 2;
        for (let px = 0; px < w; px++) {
          const p = peaks[Math.min(peaks.length - 1, Math.floor((px / w) * peaks.length))];
          if (!p) continue;
          g.fillStyle = px >= xs && px <= xe ? colors.wave : colors.dim;
          const y0 = mid - p[1] * mid * 0.95;
          const y1 = mid - p[0] * mid * 0.95;
          g.fillRect(px, y0, 1, Math.max(1, y1 - y0));
        }
      }
      g.fillStyle = "rgba(0,0,0,0.45)";
      g.fillRect(0, 0, xs, h);
      g.fillRect(xe, 0, w - xe, h);
      const marker = (mx: number, color: string, flagRight: boolean) => {
        g.fillStyle = color;
        g.fillRect(Math.min(w - 2 * dpr, Math.max(0, mx - dpr)), 0, 2 * dpr, h);
        g.beginPath();
        const s = 9 * dpr;
        g.moveTo(mx, 0);
        g.lineTo(mx + (flagRight ? s : -s), 0);
        g.lineTo(mx, s);
        g.fill();
      };
      marker(xs, colors.green, true);
      marker(xe, colors.red, false);
      // playback cursor
      const a = getApp().tick?.active.find((t) => t.cue === cue.id && t.phase === "running");
      if (a && v.end > v.start) {
        const len = v.end - v.start;
        const pos = v.start + (a.elapsed % len);
        g.fillStyle = colors.text;
        g.fillRect(x(pos), 0, Math.max(1, dpr), h);
      }
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, [cue.id]);

  const timeAt = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    return Math.min(duration, Math.max(0, ((e.clientX - r.left) / r.width) * duration));
  };
  const round = (t: number) => Math.round(t * 100) / 100;

  return (
    <div className="wave" data-testid="waveform">
      <canvas
        ref={canvas}
        className={drag ? "dragging" : ""}
        aria-label="Waveform – drag the green start and red end markers"
        onPointerDown={(e) => {
          if (duration <= 0) return;
          const t = timeAt(e);
          const marker: Marker = Math.abs(t - start) <= Math.abs(t - end) ? "start" : "end";
          e.currentTarget.setPointerCapture(e.pointerId);
          setDrag({ marker, time: marker === "start" ? Math.min(t, end - 0.01) : Math.max(t, start + 0.01) });
        }}
        onPointerMove={(e) => {
          if (!drag) return;
          const t = timeAt(e);
          setDrag({ marker: drag.marker, time: drag.marker === "start" ? Math.max(0, Math.min(t, end - 0.01)) : Math.min(duration, Math.max(t, start + 0.01)) });
        }}
        onPointerUp={() => {
          if (!drag) return;
          const t = round(drag.time);
          if (drag.marker === "start") patchKind(cue, { start: t });
          else patchKind(cue, { end: t >= round(duration) ? null : t });
          setTimeout(() => setDrag(null), 250);
        }}
        onPointerCancel={() => setDrag(null)}
      />
      <div className="wave-info num">
        <span className="wave-start">▶ {formatDuration(start)}</span>
        <span className="muted">{error ?? (wave ? `${wave.channels} ch · ${(wave.sample_rate / 1000).toFixed(1)} kHz · ${formatDuration(duration)}` : kind.file ? "loading waveform…" : "no file")}</span>
        <span className="wave-end">{formatDuration(end)} ◀</span>
      </div>
    </div>
  );
}

// ---------------- audio tab ----------------

export function AudioTab({ cue, kind }: { cue: Cue; kind: AudioKind }) {
  const [pick, setPick] = useState(false);
  const running = useApp((s) => s.tick?.active.some((a) => a.cue === cue.id) ?? false);
  const dur = kind.file_duration;
  return (
    <div className="audio-tab">
      <div className="audio-file">
        <span className="form-label">File</span>
        <span className="mono ellipsis audio-path" title={kind.file} data-testid="audio-file">{kind.file || "(no file)"}</span>
        <button className="btn small" onClick={() => setPick(true)} data-testid="audio-choose">
          Choose…
        </button>
        <span className="spacer" />
        <button className="btn small" data-testid="preview-play" onClick={() => void transport.startCue(cue.id)} title="Preview: start this cue without moving the playhead">
          <IconPlay /> Preview
        </button>
        <button className="btn small" data-testid="preview-stop" disabled={!running} onClick={() => void transport.stopCue(cue.id)}>
          <IconStop /> Stop
        </button>
      </div>
      <WaveformView cue={cue} kind={kind} />
      <div className="insp-cols three">
        <div className="insp-col">
          <Row label="Start">
            <TimeField value={kind.start} width={100} max={dur ?? undefined} testId="audio-start" onCommit={(v) => patchKind(cue, { start: v ?? 0 })} />
          </Row>
          <Row label="End" hint="empty = end of file">
            <TimeField value={kind.end} nullable width={100} max={dur ?? undefined} placeholder={dur != null ? formatDuration(dur) : "end"} testId="audio-end" onCommit={(v) => patchKind(cue, { end: v })} />
          </Row>
        </div>
        <div className="insp-col">
          <Row label="Gain">
            <Slider value={Math.max(-60, kind.gain_db)} min={-60} max={12} step={0.5} ariaLabel="Gain in dB" onChange={(v) => patchKind(cue, { gain_db: v <= -60 ? DB_FLOOR : v })} onDoubleClick={() => patchKind(cue, { gain_db: 0 })} />
            <DbField value={kind.gain_db} width={72} max={12} testId="audio-gain" onCommit={(v) => patchKind(cue, { gain_db: v ?? 0 })} />
            <span className="unit">dB</span>
          </Row>
          <Row label="Loops">
            <NumField value={kind.loop_count === 0 ? 1 : kind.loop_count} integer min={1} max={9999} width={72} disabled={kind.loop_count === 0} onCommit={(v) => patchKind(cue, { loop_count: v })} />
            <Check checked={kind.loop_count === 0} onChange={(v) => patchKind(cue, { loop_count: v ? 0 : 1 })} label="Infinite" testId="audio-infinite" />
          </Row>
        </div>
        <div className="insp-col">
          <Row label="Fade in">
            <TimeField value={kind.fade_in} width={80} onCommit={(v) => patchKind(cue, { fade_in: v ?? 0 })} />
            <span className="unit">out</span>
            <TimeField value={kind.fade_out} width={80} ariaLabel="Fade out" onCommit={(v) => patchKind(cue, { fade_out: v ?? 0 })} />
          </Row>
          <Row label="Curve">
            <Select value={kind.fade_curve} options={CURVE_OPTIONS} onChange={(v) => patchKind(cue, { fade_curve: v })} width={150} />
          </Row>
        </div>
      </div>
      {pick ? (
        <FileBrowser
          mode="pick-audio"
          initialPath={kind.file ? kind.file.slice(0, Math.max(1, kind.file.lastIndexOf("/"))) : undefined}
          onClose={() => setPick(false)}
          onPick={(paths) => {
            setPick(false);
            patchKind(cue, { file: paths[0] });
          }}
        />
      ) : null}
    </div>
  );
}

// ---------------- routing matrix ----------------

/** Placeholder that keeps a routing explicit-but-silent (an empty array means "default routing"). */
const SILENT: RoutePoint = { src: 0, out: 0, db: DB_FLOOR };

function isOn(p: RoutePoint | undefined): p is RoutePoint {
  return p != null && p.db > DB_FLOOR;
}

export function setPoint(routing: RoutePoint[], src: number, out: number, db: number | null): RoutePoint[] {
  const rest = routing.filter((p) => !(p.src === src && p.out === out) && isOn(p));
  const next = db != null && db > DB_FLOOR ? [...rest, { src, out, db }] : rest;
  next.sort((a, b) => a.src - b.src || a.out - b.out);
  return next.length > 0 ? next : [SILENT];
}

function Cell({ point, onSet, label }: { point: RoutePoint | undefined; onSet: (db: number | null) => void; label: string }) {
  const [edit, setEdit] = useState<string | null>(null);
  const on = isOn(point);
  if (edit != null) {
    return (
      <input
        className="input mono rt-edit"
        autoFocus
        value={edit}
        aria-label={label}
        onChange={(e) => setEdit(e.target.value)}
        onBlur={() => {
          const t = edit.trim();
          const v = t === "" ? null : parseDb(t);
          if (t === "" || v != null) onSet(v == null ? null : Math.min(12, v));
          setEdit(null);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
          else if (e.key === "Escape") {
            e.stopPropagation();
            setEdit(null);
          }
        }}
      />
    );
  }
  return (
    <button
      className={`rt-cell num${on ? " on" : ""}`}
      aria-label={`${label}: ${on ? `${formatDb(point.db)} dB` : "off"}`}
      aria-pressed={on}
      title="Click: 0 dB / off · type a number to set dB"
      onClick={() => onSet(on ? null : 0)}
      onDoubleClick={() => setEdit(on ? String(point.db) : "0")}
      onKeyDown={(e) => {
        if (/^[-+0-9.,]$/.test(e.key) && !e.ctrlKey && !e.metaKey && !e.altKey) {
          e.preventDefault();
          setEdit(e.key);
        } else if (e.key === "Delete" || e.key === "Backspace") {
          e.stopPropagation();
          onSet(null);
        }
      }}
    >
      {on ? formatDb(point.db) : ""}
    </button>
  );
}

export function RoutingTab({ cue, kind, show }: { cue: Cue; kind: AudioKind; show: Show }) {
  const outputs = show.patch.outputs;
  const groups = show.patch.groups;
  const sources = Math.max(1, kind.channels);
  const isDefault = kind.routing.length === 0;
  const set = (routing: RoutePoint[]) => patchKind(cue, { routing });
  const groupOptions = [{ value: -1, label: "Send to group…" }, ...groups.map((g, i) => ({ value: i, label: `${g.name} (${g.outputs.length} out)` }))];
  return (
    <div className="routing" data-testid="routing">
      <div className="rt-actions">
        <button className="btn small" disabled={isDefault} onClick={() => set([])} data-testid="rt-default">
          Default
        </button>
        <button className="btn small" onClick={() => set([SILENT])} data-testid="rt-clear">
          Clear
        </button>
        {sources === 1 ? (
          <Select
            value={-1}
            options={groupOptions}
            width={190}
            testId="rt-group"
            onChange={(i) => {
              const g = groups[i];
              if (g) set(g.outputs.filter((o) => o < outputs.length).map((out) => ({ src: 0, out, db: 0 })));
            }}
          />
        ) : null}
        <span className="muted">{isDefault ? "Default routing – the engine maps the file channels to the first outputs." : "Custom routing"} · click a cell for 0 dB / off, type to enter dB</span>
      </div>
      {outputs.length === 0 ? (
        <div className="muted">No outputs are patched – add outputs in Setup → Outputs / Patch.</div>
      ) : (
        <div className="rt-scroll">
          <table className={`rt-table${isDefault ? " is-default" : ""}`}>
            <thead>
              <tr>
                <th />
                {outputs.map((o, i) => (
                  <th key={i} title={`Output ${i + 1} → device channel ${o.channel + 1}`}>
                    <span className="num">{i + 1}</span>
                    <span className="rt-out ellipsis">{o.name}</span>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {Array.from({ length: sources }, (_, src) => (
                <tr key={src}>
                  <th scope="row">{sources === 1 ? "Mono" : sources === 2 ? (src === 0 ? "L" : "R") : `Ch ${src + 1}`}</th>
                  {outputs.map((o, out) => (
                    <td key={out}>
                      <Cell point={kind.routing.find((p) => p.src === src && p.out === out)} label={`Source ${src + 1} to ${o.name || `output ${out + 1}`}`} onSet={(db) => set(setPoint(kind.routing, src, out, db))} />
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
