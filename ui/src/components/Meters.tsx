// Output meters: one small canvas per device channel, driven by a single rAF loop that reads the
// latest tick straight from the store (no React re-render at 20 Hz) and applies client-side decay.
import { useEffect, useRef } from "react";
import { run } from "../commands";
import { Slider } from "./fields";
import { getApp, useApp } from "../store";
import { formatDb, linToDb } from "../time";

const DB_MIN = -60;
const DB_MAX = 6;
const DECAY_DB_PER_S = 26;
const HOLD_MS = 1200;

function frac(db: number): number {
  return Math.min(1, Math.max(0, (db - DB_MIN) / (DB_MAX - DB_MIN)));
}

interface Ballistics {
  peak: number;
  rms: number;
  hold: number;
  holdAt: number;
}

function cssVar(el: Element, name: string, fallback: string): string {
  const v = getComputedStyle(el).getPropertyValue(name).trim();
  return v || fallback;
}

export function Meters({ canControl, compact = false }: { canControl: boolean; compact?: boolean }) {
  const channels = useApp((s) => s.doc?.audio.channels ?? 0);
  const outputs = useApp((s) => s.doc?.show.patch.outputs);
  const clipKey = useApp((s) => (s.tick?.meters.clip ?? []).map((c) => (c ? "1" : "0")).join(""));
  const wrap = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const root = wrap.current;
    if (!root) return;
    const canvases = Array.from(root.querySelectorAll<HTMLCanvasElement>("canvas"));
    const state: Ballistics[] = canvases.map(() => ({ peak: DB_MIN, rms: DB_MIN, hold: DB_MIN, holdAt: 0 }));
    const colors = {
      bg: cssVar(root, "--meter-bg", "#0a0c0f"),
      green: cssVar(root, "--green", "#22c55e"),
      amber: cssVar(root, "--amber", "#f59e0b"),
      red: cssVar(root, "--red", "#ef4444"),
      line: cssVar(root, "--border", "#2a2f38"),
      text: cssVar(root, "--text", "#fff"),
    };
    let raf = 0;
    let last = performance.now();

    const size = () => {
      const dpr = Math.min(2, window.devicePixelRatio || 1);
      for (const c of canvases) {
        const w = Math.max(1, Math.round(c.clientWidth * dpr));
        const h = Math.max(1, Math.round(c.clientHeight * dpr));
        if (c.width !== w || c.height !== h) {
          c.width = w;
          c.height = h;
        }
      }
    };
    size();
    const ro = typeof ResizeObserver !== "undefined" ? new ResizeObserver(size) : null;
    ro?.observe(root);

    const draw = (now: number) => {
      raf = requestAnimationFrame(draw);
      const dt = Math.min(0.25, (now - last) / 1000);
      last = now;
      const app = getApp();
      const m = app.conn === "open" ? app.tick?.meters : undefined;
      for (let i = 0; i < canvases.length; i++) {
        const c = canvases[i];
        const b = state[i];
        const inPeak = m ? linToDb(m.peak[i] ?? 0) : DB_MIN;
        const inRms = m ? linToDb(m.rms[i] ?? 0) : DB_MIN;
        b.peak = Math.max(inPeak, b.peak - DECAY_DB_PER_S * dt, DB_MIN);
        b.rms = Math.max(inRms, b.rms - DECAY_DB_PER_S * dt, DB_MIN);
        if (b.peak >= b.hold) {
          b.hold = b.peak;
          b.holdAt = now;
        } else if (now - b.holdAt > HOLD_MS) {
          b.hold = Math.max(b.peak, b.hold - DECAY_DB_PER_S * dt);
        }
        const g = c.getContext("2d");
        if (!g) continue;
        const { width: w, height: h } = c;
        g.fillStyle = colors.bg;
        g.fillRect(0, 0, w, h);
        const y = (db: number) => Math.round(h - frac(db) * h);
        const seg = (from: number, to: number, x: number, bw: number, color: string, alpha: number) => {
          if (to <= from) return;
          g.globalAlpha = alpha;
          g.fillStyle = color;
          g.fillRect(x, y(to), bw, y(from) - y(to));
        };
        const bar = (level: number, x: number, bw: number, alpha: number) => {
          seg(DB_MIN, Math.min(level, -12), x, bw, colors.green, alpha);
          seg(-12, Math.min(level, -3), x, bw, colors.amber, alpha);
          seg(-3, Math.min(level, DB_MAX), x, bw, colors.red, alpha);
        };
        bar(b.peak, 0, w, 0.45);
        bar(b.rms, Math.round(w * 0.2), Math.round(w * 0.6), 1);
        g.globalAlpha = 1;
        // scale lines at 0 / -12 / -24 / -48 dB
        g.fillStyle = colors.line;
        for (const db of [0, -12, -24, -48]) g.fillRect(0, y(db), w, 1);
        if (b.hold > DB_MIN + 0.5) {
          g.fillStyle = b.hold > -3 ? colors.red : colors.text;
          g.fillRect(0, Math.max(0, y(b.hold) - 1), w, 2);
        }
      }
    };
    raf = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(raf);
      ro?.disconnect();
    };
  }, [channels]);

  const names = Array.from({ length: channels }, (_, ch) => outputs?.find((o) => o.channel === ch)?.name ?? "");
  return (
    <section className={`meters${compact ? " compact" : ""}`} aria-label="Output meters" data-testid="meters">
      <div className="meter-scale num" aria-hidden="true">
        <span style={{ bottom: `${frac(0) * 100}%` }}>0</span>
        <span style={{ bottom: `${frac(-12) * 100}%` }}>12</span>
        <span style={{ bottom: `${frac(-24) * 100}%` }}>24</span>
        <span style={{ bottom: `${frac(-48) * 100}%` }}>48</span>
      </div>
      <div className="meter-cols" ref={wrap}>
        {channels === 0 ? <span className="muted">No audio outputs</span> : null}
        {names.map((name, i) => {
          const clipped = clipKey[i] === "1";
          return (
            <div className="meter-col" key={i} title={`Output ${i + 1}${name ? ` – ${name}` : ""}`}>
              {canControl ? (
                <button className={`clip-led${clipped ? " on" : ""}`} aria-label={clipped ? `Channel ${i + 1} clipped – reset` : `Channel ${i + 1} clip indicator`} onClick={() => void run({ cmd: "reset_clip" })} />
              ) : (
                <span className={`clip-led${clipped ? " on" : ""}`} />
              )}
              <canvas />
              <span className="meter-ch num">{i + 1}</span>
              {!compact ? <span className="meter-name">{name}</span> : null}
            </div>
          );
        })}
      </div>
    </section>
  );
}

export function MasterFader({ canControl, touch = false }: { canControl: boolean; touch?: boolean }) {
  const db = useApp((s) => s.doc?.master_db ?? 0);
  const set = (v: number) => void run({ cmd: "set_master", db: v });
  return (
    <section className={`master${touch ? " touch" : ""}`} aria-label="Master fader" data-testid="master">
      <span className="master-label">Master</span>
      <Slider value={Math.max(-60, db)} min={-60} max={12} step={0.5} disabled={!canControl} ariaLabel="Master level in dB" onChange={(v) => set(v <= -60 ? -120 : v)} onDoubleClick={canControl ? () => set(0) : undefined} debounceMs={60} />
      <span className={`master-value num${db > 0 ? " hot" : ""}`}>{formatDb(db)} dB</span>
      {canControl ? (
        <button className="btn small" onClick={() => set(0)} disabled={db === 0} title="Master to 0 dB (also: double-click the fader)">
          0 dB
        </button>
      ) : null}
    </section>
  );
}
