// Desktop Live mode: the show dashboard. No structural editing is reachable from here.
import { QuitButton } from "./AppMenu";
import { useState } from "react";
import { transport } from "../commands";
import { Clocks } from "../components/Clocks";
import { CueList } from "../components/CueList";
import { MasterFader, Meters } from "../components/Meters";
import { NextPanel } from "../components/NextPanel";
import { GoToBox, LogView, Notes, StatusBar } from "../components/Panels";
import { Transport } from "../components/Transport";
import { useApp } from "../store";
import { ModeSwitch } from "./ModeSwitch";
import { COARSE_POINTER, useMedia } from "../platform";

type Panel = "cues" | "clocks" | "mix" | "notes" | "log";
const PANELS: { id: Panel; label: string }[] = [
  { id: "cues", label: "Cues" }, { id: "clocks", label: "Clocks" }, { id: "mix", label: "Meters" }, { id: "notes", label: "Notes" }, { id: "log", label: "Log" },
];

export function LiveDashboard({ canEdit }: { canEdit: boolean }) {
  const [follow, setFollow] = useState(true);
  // Phones show one panel at a time (tabs); wider screens show everything at once.
  const [panel, setPanel] = useState<Panel>("cues");
  const name = useApp((s) => s.doc?.show.name ?? "");
  const paused = useApp((s) => s.doc?.paused_all ?? false);
  const coarse = useMedia(COARSE_POINTER);
  const compact = useMedia("(max-width: 899px), (max-height: 560px)");
  return (
    <div className="live-screen" data-testid="live-dashboard">
      <header className="live-top">
        <span className="live-badge">LIVE</span>
        <span className="live-name ellipsis">{name}</span>
        {paused ? <span className="tag tag-paused big">ALL PAUSED</span> : null}
        <span className="spacer" />
        <label className="check live-follow">
          <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} />
          <span>Follow<span className="live-follow-long"> playhead</span></span>
        </label>
        <span className="live-goto"><GoToBox /></span>
        {canEdit ? <ModeSwitch /> : null}
        {canEdit ? <QuitButton className="live-quit" /> : null}
      </header>
      <div className="live-grid" data-panel={panel}>
        <nav className="live-tabs" role="tablist" aria-label="Panels">
          {PANELS.map((t) => (
            <button key={t.id} role="tab" aria-selected={panel === t.id} className={`r-tab${panel === t.id ? " on" : ""}`} onClick={() => setPanel(t.id)} data-testid={`live-tab-${t.id}`}>
              {t.label}
            </button>
          ))}
        </nav>
        <div className="live-cues">
          <CueList variant={coarse ? "touch" : "desktop"} selectable follow={follow} gutterSetsPlayhead onPlayhead={(id) => void transport.setPlayhead(id)} />
        </div>
        <div className="live-next">
          <NextPanel following={compact ? 1 : 3} compact={compact} />
        </div>
        <div className="live-clocks">
          <Clocks canControl compact={compact} />
        </div>
        <div className="live-transport">
          <Transport hold={coarse} grid={compact} />
        </div>
        <div className="live-tools">
          <div className="live-mix">
            <Meters canControl />
            <MasterFader canControl touch={coarse} />
          </div>
          <Notes editable />
          <LogView />
        </div>
      </div>
      <StatusBar />
    </div>
  );
}
