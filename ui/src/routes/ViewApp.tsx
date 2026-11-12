// /view – read-only monitor (what `showmaster.local` opens on a tablet). This screen must never
// contain a control that sends a command; "Control" only navigates to /remote (control PIN).
import { useEffect } from "react";
import { Clocks } from "../components/Clocks";
import { CueList } from "../components/CueList";
import { NextPanel } from "../components/NextPanel";
import { ConnDot, Notes } from "../components/Panels";
import { keepScreenAwake } from "../platform";
import { useApp } from "../store";

export function ViewApp() {
  const name = useApp((s) => s.doc?.show.name ?? "Show Master");
  const mode = useApp((s) => s.doc?.mode);
  const paused = useApp((s) => s.doc?.paused_all ?? false);
  useEffect(() => keepScreenAwake(), []);
  return (
    <div className="viewapp" data-testid="view-app">
      <header className="r-head">
        <ConnDot />
        <span className="r-title ellipsis">{name}</span>
        {paused ? <span className="tag tag-paused">PAUSED</span> : null}
        <span className={`mode-badge ${mode}`}>{mode === "live" ? "LIVE" : "EDIT"}</span>
        <span className="mode-badge view">VIEW ONLY</span>
        <a className="btn r-switch" href="/remote" data-testid="to-remote" title="Control the show (asks for the control PIN)">
          Control
        </a>
      </header>
      <div className="v-clocks">
        <Clocks canControl={false} compact />
      </div>
      <div className="v-next">
        <NextPanel following={3} />
      </div>
      <div className="v-list">
        <CueList variant="view" selectable={false} follow />
      </div>
      <div className="v-notes">
        <Notes editable={false} />
      </div>
    </div>
  );
}
