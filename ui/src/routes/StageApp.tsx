// /stage – stage manager monitor: readable from a distance, nothing to press but navigation.
import { useEffect, useState } from "react";
import { ConnDot } from "../components/Panels";
import { cueLabel, findCue, findList, cuesAfter, TYPE_LABELS } from "../cues";
import { keepScreenAwake } from "../platform";
import { currentListId, useApp } from "../store";
import { formatClock, formatCountdown, formatWallClock } from "../time";

import type { ActiveCue } from "../api";

// Stable fallbacks: a new [] per render would make the store selector loop.
const NO_ACTIVE: ActiveCue[] = [];
const NO_ALARMS: string[] = [];

function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(t);
  }, []);
  return now;
}

export function StageApp() {
  const now = useNow();
  const show = useApp((s) => s.doc?.show ?? null);
  const mode = useApp((s) => s.doc?.mode);
  const paused = useApp((s) => s.doc?.paused_all ?? false);
  const listId = useApp((s) => currentListId(s));
  const playhead = useApp((s) => (listId != null ? s.doc?.playheads[String(listId)] ?? null : null));
  const active = useApp((s) => s.tick?.active ?? NO_ACTIVE);
  const nextAuto = useApp((s) => s.tick?.next_auto ?? null);
  const elapsed = useApp((s) => s.tick?.show_elapsed ?? null);
  const tc = useApp((s) => s.tick?.tc ?? null);
  const alarms = useApp((s) => s.tick?.alarms ?? NO_ALARMS);
  const token = useApp((s) => s.token);
  useEffect(() => keepScreenAwake(), []);

  const next = findCue(show, playhead);
  const after = cuesAfter(findList(show, listId), playhead, 2);
  const sheet = `/api/cuesheet.pdf${token ? `?token=${encodeURIComponent(token)}` : ""}`;
  return (
    <div className="stage" data-testid="stage-app">
      <header className="r-head">
        <ConnDot />
        <span className="r-title ellipsis">{show?.name ?? "Show Master"}</span>
        {paused ? <span className="tag tag-paused">PAUSED</span> : null}
        <span className={`mode-badge ${mode}`}>{mode === "live" ? "LIVE" : "EDIT"}</span>
        <a className="btn ghost r-switch" href={sheet} target="_blank" rel="noreferrer" data-testid="stage-sheet">
          Cue sheet
        </a>
        <a className="btn ghost r-switch" href="/view" data-testid="stage-to-view">
          View
        </a>
      </header>
      <div className="stage-clocks num">
        <div><span>Time</span>{formatWallClock(now)}</div>
        <div><span>Show</span>{formatClock(elapsed == null ? null : Math.floor(elapsed))}</div>
        {tc ? <div className={tc.running ? "on" : ""}><span>Timecode</span>{tc.position}</div> : null}
      </div>
      {alarms.length ? <div className="stage-alarm" role="alert">{alarms.join(" · ")}</div> : null}
      <section className="stage-next" aria-label="Next cue" data-testid="stage-next">
        <span className="tag tag-next">NEXT</span>
        {next ? (
          <>
            <div className="stage-next-title">
              <span className="num">{next.number || "–"}</span> {next.name || TYPE_LABELS[next.kind.type]}
            </div>
            {next.notes ? <div className="stage-next-notes">{next.notes}</div> : null}
          </>
        ) : (
          <div className="stage-next-title muted">Nothing standing by</div>
        )}
        {nextAuto != null ? (
          <div className="stage-countdown" data-testid="stage-countdown">
            next automatic cue in <b className="num">{formatCountdown(Math.ceil(nextAuto * 10) / 10)}</b>
          </div>
        ) : null}
        {after.length ? (
          <div className="stage-after">
            then {after.map((c) => cueLabel(c)).join("  ·  ")}
          </div>
        ) : null}
      </section>
      <section className="stage-running" aria-label="Running cues">
        <h3>Running</h3>
        {active.length === 0 ? <p className="muted">Nothing running</p> : null}
        <ul>
          {active.map((a) => {
            const c = findCue(show, a.cue);
            const left = a.duration != null ? Math.max(0, a.duration - a.elapsed) : null;
            return (
              <li key={`${a.cue}-${a.list}`} className={a.paused ? "is-paused" : ""}>
                <span className="ellipsis">{c ? cueLabel(c) : `#${a.cue}`}</span>
                <span className="num">{left != null ? `-${formatClock(Math.ceil(left))}` : formatClock(Math.floor(a.elapsed))}</span>
              </li>
            );
          })}
        </ul>
      </section>
      {show?.notes ? <section className="stage-notes" aria-label="Show notes">{show.notes}</section> : null}
    </div>
  );
}
