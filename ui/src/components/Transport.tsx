// Transport controls shared by the desktop dashboard and /remote.
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { fireGo, goLockRemaining, panicArmed, panicNow, pressPanic, subscribeGoLock, subscribePanic, transport } from "../commands";
import { useApp } from "../store";
import { Confirm } from "./Modal";
import { IconNext, IconPause, IconPlay, IconPrev, IconReset, IconStop } from "./icons";

/** GO fires on pointerdown (not click) and locks itself for go_debounce_ms. */
export function GoButton({ className = "" }: { className?: string }) {
  const locked = useSyncExternalStore(subscribeGoLock, () => goLockRemaining() > 0);
  const connected = useApp((s) => s.conn === "open");
  const hasNext = useApp((s) => {
    const ph = s.doc?.playheads;
    return ph ? Object.values(ph).some((v) => v != null) : false;
  });
  return (
    <button
      type="button"
      className={`btn-go ${locked ? "is-locked" : ""} ${className}`}
      data-testid="go"
      disabled={locked || !connected}
      aria-label="GO – fire the next cue"
      onPointerDown={(e) => {
        if (e.button !== 0 && e.pointerType === "mouse") return;
        e.preventDefault();
        fireGo();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" && !e.repeat) {
          e.preventDefault();
          fireGo();
        }
      }}
      onContextMenu={(e) => e.preventDefault()}
    >
      <span className="go-label">GO</span>
      {!hasNext ? <span className="go-sub">no cue standing by</span> : null}
    </button>
  );
}

const HOLD_MS = 500;

/**
 * PANIC. Desktop: immediate on press (or double press when `panic_double_tap` is set).
 * Touch (`hold`): press and hold for 0.5 s – the fill shows the progress.
 */
export function PanicButton({ hold = false, className = "" }: { hold?: boolean; className?: string }) {
  const armed = useSyncExternalStore(subscribePanic, panicArmed);
  const doubleTap = useApp((s) => s.doc?.show.settings.panic_double_tap ?? false);
  const [holding, setHolding] = useState(false);
  const [fired, setFired] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cancel = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    setHolding(false);
  };
  useEffect(() => cancel, []);
  const flash = () => {
    setFired(true);
    setTimeout(() => setFired(false), 600);
  };
  if (hold) {
    return (
      <button
        type="button"
        className={`btn-panic hold ${holding ? "is-holding" : ""} ${fired ? "is-fired" : ""} ${className}`}
        data-testid="panic"
        aria-label="Panic – press and hold for half a second"
        onPointerDown={(e) => {
          e.preventDefault();
          try {
            e.currentTarget.setPointerCapture(e.pointerId);
          } catch {
            /* not critical */
          }
          setHolding(true);
          timer.current = setTimeout(() => {
            timer.current = null;
            setHolding(false);
            panicNow();
            flash();
          }, HOLD_MS);
        }}
        onPointerUp={cancel}
        onPointerCancel={cancel}
        onContextMenu={(e) => e.preventDefault()}
      >
        <span className="panic-fill" style={{ transitionDuration: holding ? `${HOLD_MS}ms` : "0ms" }} />
        <span className="panic-label">PANIC</span>
        <span className="panic-sub">{fired ? "sent" : "hold 0.5 s"}</span>
      </button>
    );
  }
  return (
    <button
      type="button"
      className={`btn-panic ${armed ? "is-armed" : ""} ${fired ? "is-fired" : ""} ${className}`}
      data-testid="panic"
      aria-label={doubleTap ? "Panic – press twice" : "Panic – stop everything immediately"}
      onPointerDown={(e) => {
        if (e.button !== 0 && e.pointerType === "mouse") return;
        e.preventDefault();
        if (pressPanic()) flash();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" && !e.repeat && pressPanic()) flash();
      }}
    >
      <span className="panic-label">PANIC</span>
      <span className="panic-sub">{armed ? "press again!" : doubleTap ? "press twice" : "stop everything"}</span>
    </button>
  );
}

export function Transport({ touch = false }: { touch?: boolean }) {
  const paused = useApp((s) => s.doc?.paused_all ?? false);
  const [confirmReset, setConfirmReset] = useState(false);
  return (
    <section className={`transport${touch ? " touch" : ""}`} aria-label="Transport" data-testid="transport">
      <GoButton />
      <div className="transport-row">
        <button type="button" className={`btn tbtn t-pause ${paused ? "is-paused" : ""}`} data-testid="pause" onClick={() => void transport.togglePause()}>
          {paused ? <IconPlay size={20} /> : <IconPause size={20} />}
          <span>{paused ? "Resume" : "Pause"}</span>
        </button>
        <button type="button" className="btn tbtn t-stop" data-testid="stop-all" onClick={() => void transport.stopAll()}>
          <IconStop size={20} />
          <span>Stop all</span>
        </button>
      </div>
      <div className="transport-row three">
        <button type="button" className="btn tbtn t-prev" data-testid="prev" onClick={() => void transport.previous()} aria-label="Playhead to previous cue">
          <IconPrev size={20} />
          <span>Prev</span>
        </button>
        <button type="button" className="btn tbtn t-next" data-testid="next" onClick={() => void transport.next()} aria-label="Playhead to next cue">
          <IconNext size={20} />
          <span>Next</span>
        </button>
        <button type="button" className="btn tbtn t-reset" data-testid="reset" onClick={() => setConfirmReset(true)}>
          <IconReset size={20} />
          <span>Reset</span>
        </button>
      </div>
      <div className="transport-panic">
        <PanicButton hold={touch} />
      </div>
      {confirmReset ? (
        <Confirm
          title="Reset the show?"
          confirmLabel="Reset"
          danger
          onCancel={() => setConfirmReset(false)}
          onConfirm={() => {
            setConfirmReset(false);
            void transport.reset();
          }}
        >
          Stops all cues, moves the playhead to the top, clears loads and errors and resets the show clock.
        </Confirm>
      ) : null}
    </section>
  );
}
