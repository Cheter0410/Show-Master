// Small shared panels: notes, log, status bar, go-to box, banners, toasts, PIN pad.
import { useEffect, useRef, useState } from "react";
import { run, transport } from "../commands";
import { submitPin } from "../session";
import { dismissToast, toast, useApp } from "../store";
import { formatWallClock } from "../time";
import { IconLock, IconX } from "./icons";

// ---------- stage manager notes ----------

export function Notes({ editable }: { editable: boolean }) {
  const notes = useApp((s) => s.doc?.show.notes ?? "");
  const [draft, setDraft] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const push = (text: string) => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    void run({ cmd: "set_notes", text });
  };
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);
  return (
    <section className="notes" aria-label="Stage manager notes" data-testid="notes">
      <h3 className="panel-title">Notes</h3>
      {editable ? (
        <textarea
          className="notes-text"
          value={draft ?? notes}
          placeholder="Stage manager notes…"
          spellCheck={false}
          onChange={(e) => {
            const v = e.target.value;
            setDraft(v);
            if (timer.current) clearTimeout(timer.current);
            timer.current = setTimeout(() => push(v), 800);
          }}
          onBlur={() => {
            if (draft != null && draft !== notes) push(draft);
            setDraft(null);
          }}
        />
      ) : (
        <div className="notes-text readonly">{notes || <span className="muted">No notes</span>}</div>
      )}
    </section>
  );
}

// ---------- event log ----------

export function LogView({ canClear = false }: { canClear?: boolean }) {
  const log = useApp((s) => s.log);
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  useEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [log]);
  return (
    <section className="log" aria-label="Event log" data-testid="log">
      <h3 className="panel-title">
        Log
        {canClear ? (
          <button className="btn ghost tiny" onClick={() => void run({ cmd: "clear_log" })}>
            clear
          </button>
        ) : null}
      </h3>
      <div
        className="log-lines"
        ref={ref}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
      >
        {log.length === 0 ? <div className="muted">No events yet</div> : null}
        {log.map((l, i) => (
          <div key={`${l.time_ms}-${i}`} className={`log-line lv-${l.level}`}>
            <span className="num log-time">{formatWallClock(l.time_ms)}</span>
            <span className="log-msg">{l.message}</span>
          </div>
        ))}
      </div>
    </section>
  );
}

// ---------- status bar ----------

export function StatusBar() {
  const audio = useApp((s) => s.doc?.audio);
  const midiPorts = useApp((s) => s.doc?.midi.ports.length ?? 0);
  const peers = useApp((s) => s.doc?.midi.rtp.peers);
  const clients = useApp((s) => s.doc?.remote.clients.length ?? 0);
  const cpu = useApp((s) => (s.tick ? Math.round(s.tick.cpu * 100) : null));
  const underruns = useApp((s) => s.tick?.underruns ?? 0);
  const voices = useApp((s) => s.tick?.voices ?? 0);
  const latency = useApp((s) => s.latencyMs);
  const conn = useApp((s) => s.conn);
  const connectedPeers = peers?.filter((p) => p.state === "connected").length ?? 0;
  return (
    <footer className="statusbar num" data-testid="statusbar">
      <span
        className={`${audio?.error ? "sb-err sb-click" : audio?.is_null ? "sb-warn" : ""}`}
        title={audio?.error ?? "Audio device"}
        onClick={audio?.error ? () => toast(`Audio error: ${audio.error}`, "error", 8000) : undefined}
      >
        {audio?.error ? `Audio error: ${audio.error}` : audio?.is_null ? "NO AUDIO DEVICE (silent)" : audio?.device ?? "–"}
      </span>
      <span className="sb-minor">{audio ? `${(audio.sample_rate / 1000).toFixed(audio.sample_rate % 1000 ? 1 : 0)} kHz` : ""}</span>
      <span className="sb-minor">{audio ? `${audio.buffer_frames} frames` : ""}</span>
      <span className="sb-minor">{audio ? `${audio.channels} out` : ""}</span>
      <span className={cpu != null && cpu > 70 ? "sb-warn" : ""}>CPU {cpu ?? "–"}%</span>
      <span className={underruns > 0 ? "sb-err" : ""}>Underruns {underruns}</span>
      <span className="sb-minor">Voices {voices}</span>
      <span className="sb-spacer" />
      <span className="sb-minor">MIDI ports {midiPorts}</span>
      <span className="sb-minor">RTP peers {connectedPeers}/{peers?.length ?? 0}</span>
      <span className="sb-minor">Remote clients {clients}</span>
      <span className={conn === "open" ? (latency != null && latency > 150 ? "sb-warn" : "sb-ok") : "sb-err"} data-testid="latency">
        {conn === "open" ? `Link ${latency ?? "–"} ms` : "OFFLINE"}
      </span>
    </footer>
  );
}

// ---------- go to cue ----------

export const GOTO_INPUT_ID = "goto-cue-input";

export function GoToBox() {
  const [v, setV] = useState("");
  return (
    <form
      className="goto"
      onSubmit={(e) => {
        e.preventDefault();
        const n = v.trim();
        if (!n) return;
        void transport.gotoNumber(n).then((ok) => {
          if (ok) setV("");
        });
        (document.activeElement as HTMLElement | null)?.blur?.();
      }}
    >
      <label htmlFor={GOTO_INPUT_ID}>Go to cue</label>
      <input id={GOTO_INPUT_ID} className="input mono" data-testid="goto" value={v} placeholder="#" aria-label="Go to cue number" inputMode="decimal" autoComplete="off" onChange={(e) => setV(e.target.value)} onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          setV("");
          e.currentTarget.blur();
        }
      }} />
    </form>
  );
}

// ---------- connection ----------

export function ConnectionBanner() {
  const conn = useApp((s) => s.conn);
  const ever = useApp((s) => s.everConnected);
  const kicked = useApp((s) => s.kicked);
  if (kicked) {
    return (
      <div className="conn-banner kicked" role="alert" data-testid="conn-banner">
        <b>DISCONNECTED BY THE OPERATOR</b>
        <button className="btn" onClick={() => location.reload()}>
          Reconnect
        </button>
      </div>
    );
  }
  if (conn === "open" || !ever) return null;
  return (
    <div className="conn-banner" role="alert" data-testid="conn-banner">
      <b>CONNECTION LOST – reconnecting…</b>
      <span>The show keeps running on the server. Controls on this screen do nothing until the link is back.</span>
    </div>
  );
}

export function ConnDot() {
  const conn = useApp((s) => s.conn);
  const latency = useApp((s) => s.latencyMs);
  return (
    <span className={`conn-dot ${conn}`} data-testid="conn-dot" title={conn === "open" ? `Connected, ${latency ?? "–"} ms` : "Not connected"}>
      <i />
      <span className="num">{conn === "open" ? `${latency ?? "–"} ms` : "offline"}</span>
    </span>
  );
}

export function Splash({ text }: { text: string }) {
  return (
    <div className="splash" data-testid="splash">
      <div className="splash-logo">Show Master</div>
      <div className="splash-text">{text}</div>
    </div>
  );
}

// ---------- toasts & alarms ----------

export function Toasts() {
  const toasts = useApp((s) => s.toasts);
  const alarmKey = useApp((s) => (s.tick?.alarms ?? []).join("\u0001"));
  const seen = useRef<Set<string>>(new Set());
  useEffect(() => {
    const now = new Set(alarmKey ? alarmKey.split("\u0001") : []);
    for (const a of now) if (!seen.current.has(a)) toast(a, "alarm", 10000);
    seen.current = now;
  }, [alarmKey]);
  if (toasts.length === 0) return null;
  return (
    <div className="toasts" aria-live="assertive">
      {toasts.map((t) => (
        <div key={t.id} className={`toast lv-${t.level}`} role={t.level === "error" || t.level === "alarm" ? "alert" : "status"} data-testid="toast">
          <span>{t.level === "alarm" ? "⏰ " : ""}{t.text}</span>
          {t.action ? (
            <button
              className="btn toast-action"
              data-testid="toast-action"
              onClick={() => {
                dismissToast(t.id);
                t.action?.run();
              }}
            >
              {t.action.label}
            </button>
          ) : null}
          <button className="btn ghost icon-only" aria-label="Dismiss" onClick={() => dismissToast(t.id)}>
            <IconX />
          </button>
        </div>
      ))}
    </div>
  );
}

// ---------- PIN pad ----------

export function PinPad() {
  const role = useApp((s) => s.loginRole);
  const [pin, setPin] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const pinRef = useRef(pin);
  pinRef.current = pin;

  const submit = async () => {
    if (busy) return;
    setBusy(true);
    const err = await submitPin(pinRef.current);
    setBusy(false);
    if (err) {
      setError(err);
      setPin("");
    }
  };
  const submitRef = useRef(submit);
  submitRef.current = submit;

  const press = (d: string) => {
    setError(null);
    setPin((p) => (p.length < 12 ? p + d : p));
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (/^\d$/.test(e.key)) press(e.key);
      else if (e.key === "Backspace") setPin((p) => p.slice(0, -1));
      else if (e.key === "Enter") void submitRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="pinpad-screen" data-testid="pinpad">
      <div className="pinpad">
        <div className="pinpad-title">
          <IconLock size={20} /> Show Master
        </div>
        <div className="pinpad-sub">{role === "view" ? "Enter the view PIN" : "Enter the control PIN"}</div>
        <div className={`pin-display${error ? " error" : ""}`} aria-live="polite" data-testid="pin-display">
          {error ? error : pin.length === 0 ? <span className="muted">PIN</span> : "●".repeat(pin.length)}
        </div>
        <div className="pin-keys">
          {["1", "2", "3", "4", "5", "6", "7", "8", "9"].map((d) => (
            <button key={d} type="button" className="pin-key" data-testid={`pin-${d}`} onClick={() => press(d)}>
              {d}
            </button>
          ))}
          <button type="button" className="pin-key fn" aria-label="Delete last digit" onClick={() => setPin((p) => p.slice(0, -1))}>
            ⌫
          </button>
          <button type="button" className="pin-key" data-testid="pin-0" onClick={() => press("0")}>
            0
          </button>
          <button type="button" className="pin-key ok" data-testid="pin-ok" disabled={busy} onClick={() => void submit()}>
            OK
          </button>
        </div>
      </div>
    </div>
  );
}
