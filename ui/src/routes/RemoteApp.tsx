// /remote – touch-first control surface for the iPad (role `control`).
import { useEffect, useState } from "react";
import { transport } from "../commands";
import { Clocks } from "../components/Clocks";
import { CueList } from "../components/CueList";
import { MasterFader, Meters } from "../components/Meters";
import { NextPanel } from "../components/NextPanel";
import { ConnDot, Notes } from "../components/Panels";
import { Transport } from "../components/Transport";
import { cueLabel, findCue } from "../cues";
import { IconChevronDown, IconUp } from "../components/icons";
import { keepScreenAwake, loadFlag, saveFlag } from "../platform";
import { currentListId, useApp } from "../store";

type Tab = "cues" | "mix" | "notes";

export function RemoteApp() {
  const [tab, setTab] = useState<Tab>("cues");
  const [follow, setFollow] = useState(true);
  const show = useApp((s) => s.doc?.show ?? null);
  const mode = useApp((s) => s.doc?.mode);
  const selectedId = useApp((s) => s.selection[0] ?? null);
  const playhead = useApp((s) => {
    const l = currentListId(s);
    return l != null ? s.doc?.playheads[String(l)] ?? null : null;
  });
  const selected = findCue(show, selectedId);
  // Phones: the four clocks fold into one line unless the user unfolds them (remembered per device).
  const [folded, setFoldedState] = useState(() => loadFlag("sm.remote.clocksFolded", true));
  const setFolded = (v: boolean) => {
    setFoldedState(v);
    saveFlag("sm.remote.clocksFolded", v);
  };
  useEffect(() => keepScreenAwake(), []);

  return (
    <div className="remote" data-testid="remote-app">
      <header className="r-head">
        <ConnDot />
        <span className="r-title ellipsis">{show?.name ?? "Show Master"}</span>
        <span className={`mode-badge ${mode}`}>{mode === "live" ? "LIVE" : "EDIT"}</span>
        <a className="btn ghost r-switch" href="/view" data-testid="to-view" title="Back to the read-only view">
          View
        </a>
        <button className="btn ghost r-clock-toggle" aria-expanded={!folded} onClick={() => setFolded(!folded)} data-testid="clocks-toggle">
          Clocks {folded ? <IconChevronDown size={14} /> : <IconUp size={14} />}
        </button>
      </header>
      <div className={`r-clocks${folded ? " folded" : ""}`}>
        <Clocks canControl={false} compact />
      </div>
      <div className="r-next">
        <NextPanel compact following={2} />
      </div>
      <div className="r-main">
        <nav className="r-tabs" role="tablist">
          {(["cues", "mix", "notes"] as Tab[]).map((t) => (
            <button key={t} role="tab" aria-selected={tab === t} className={`r-tab${tab === t ? " on" : ""}`} onClick={() => setTab(t)} data-testid={`tab-${t}`}>
              {t === "cues" ? "Cues" : t === "mix" ? "Meters" : "Notes"}
            </button>
          ))}
          {tab === "cues" ? (
            <button className={`r-tab r-follow${follow ? " on" : ""}`} aria-pressed={follow} onClick={() => setFollow(!follow)}>
              Follow
            </button>
          ) : null}
        </nav>
        <div className="r-body">
          {tab === "cues" ? <CueList variant="touch" selectable follow={follow} /> : null}
          {tab === "mix" ? (
            <div className="r-mix">
              <Meters canControl />
              <MasterFader canControl touch />
            </div>
          ) : null}
          {tab === "notes" ? <Notes editable /> : null}
        </div>
        {tab === "cues" ? (
          <div className={`r-selbar${selected ? "" : " is-empty"}`} data-testid="selbar">
            <span className="r-sel ellipsis">{selected ? <>Selected: <b>{cueLabel(selected)}</b></> : <span className="muted">Tap a cue to select it</span>}</span>
            <button className="btn big" data-testid="set-next" disabled={!selected || selected.id === playhead} onClick={() => selected && void transport.setPlayhead(selected.id)}>
              Set NEXT
            </button>
            <button className="btn big" data-testid="fire-selected" disabled={!selected} onClick={() => selected && void transport.startCue(selected.id)}>
              Fire selected
            </button>
          </div>
        ) : null}
      </div>
      <div className="r-transport">
        <Transport touch />
      </div>
    </div>
  );
}
