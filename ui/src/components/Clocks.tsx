import { useEffect, useState } from "react";
import { run } from "../commands";
import { useApp } from "../store";
import { formatClock, formatCountdown, formatWallClock } from "../time";
import { IconReset } from "./icons";

/** Wall clock that keeps ticking locally (offset to the server clock) even while disconnected. */
function useWallClock(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(t);
  }, []);
  return now;
}

export function Clocks({ canControl, compact = false }: { canControl: boolean; compact?: boolean }) {
  const now = useWallClock();
  const elapsed = useApp((s) => (s.tick?.show_elapsed == null ? null : Math.floor(s.tick.show_elapsed)));
  const tcPos = useApp((s) => s.tick?.tc.position ?? "--:--:--:--");
  const tcRunning = useApp((s) => s.tick?.tc.running ?? false);
  const tcRate = useApp((s) => s.tick?.tc.rate ?? "");
  const ltc = useApp((s) => s.tick?.tc.ltc ?? false);
  const mtc = useApp((s) => s.tick?.tc.mtc ?? false);
  const nextAuto = useApp((s) => (s.tick?.next_auto == null ? null : Math.ceil(s.tick.next_auto * 10) / 10));
  const stale = useApp((s) => s.conn !== "open");
  return (
    <section className={`clocks${compact ? " compact" : ""}${stale ? " stale" : ""}`} aria-label="Clocks" data-testid="clocks">
      <div className="clock">
        <span className="clock-label">Time</span>
        <span className="clock-value num">{formatWallClock(now)}</span>
      </div>
      <div className="clock">
        <span className="clock-label">
          Show
          {canControl ? (
            <button className="btn ghost tiny" title="Reset show clock" aria-label="Reset show clock" onClick={() => void run({ cmd: "reset_show_clock" })}>
              <IconReset size={12} />
            </button>
          ) : null}
        </span>
        <span className={`clock-value num${elapsed == null ? " dim" : ""}`} data-testid="show-clock">{formatClock(elapsed)}</span>
      </div>
      <div className={`clock clock-tc${tcRunning ? " running" : ""}`}>
        <span className="clock-label">
          Timecode
          <span className="tc-badges">
            <span className={`badge ${tcRunning ? "on" : ""}`}>{tcRunning ? "RUN" : "STOP"}</span>
            <span className="badge">{tcRate}</span>
            <span className={`badge ${ltc ? "on" : "off"}`}>LTC</span>
            <span className={`badge ${mtc ? "on" : "off"}`}>MTC</span>
          </span>
        </span>
        <span className="clock-value num" data-testid="tc">{tcPos}</span>
      </div>
      <div className={`clock clock-auto${nextAuto != null ? " active" : ""}`}>
        <span className="clock-label">Next auto cue</span>
        <span className={`clock-value num${nextAuto == null ? " dim" : ""}`}>{nextAuto == null ? "--:--" : formatCountdown(nextAuto)}</span>
      </div>
    </section>
  );
}
