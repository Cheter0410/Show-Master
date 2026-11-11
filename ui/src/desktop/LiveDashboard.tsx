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

export function LiveDashboard({ canEdit }: { canEdit: boolean }) {
  const [follow, setFollow] = useState(true);
  const name = useApp((s) => s.doc?.show.name ?? "");
  const paused = useApp((s) => s.doc?.paused_all ?? false);
  return (
    <div className="live-screen" data-testid="live-dashboard">
      <header className="live-top">
        <span className="live-badge">LIVE</span>
        <span className="live-name ellipsis">{name}</span>
        {paused ? <span className="tag tag-paused big">ALL PAUSED</span> : null}
        <span className="spacer" />
        <label className="check">
          <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} />
          <span>Follow playhead</span>
        </label>
        <GoToBox />
        {canEdit ? <ModeSwitch /> : null}
        {canEdit ? <QuitButton /> : null}
      </header>
      <div className="live-grid">
        <div className="live-cues">
          <CueList variant="desktop" selectable follow={follow} gutterSetsPlayhead onPlayhead={(id) => void transport.setPlayhead(id)} />
        </div>
        <aside className="live-side">
          <NextPanel following={3} />
          <Clocks canControl />
          <Transport />
        </aside>
        <div className="live-tools">
          <div className="live-mix">
            <Meters canControl />
            <MasterFader canControl />
          </div>
          <Notes editable />
          <LogView />
        </div>
      </div>
      <StatusBar />
    </div>
  );
}
