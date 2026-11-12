// Timecode generator controls for /remote (role `control`): start, pause, resume, stop, locate.
import { useState } from "react";
import { run } from "../commands";
import { useApp } from "../store";
import { TcField } from "./fields";

export function TimecodePanel() {
  const source = useApp((s) => s.doc?.show.timecode.source ?? "generator");
  const rate = useApp((s) => s.doc?.show.timecode.rate ?? "25");
  const start = useApp((s) => s.doc?.show.timecode.start ?? "00:00:00:00");
  const position = useApp((s) => s.tick?.tc.position ?? "--:--:--:--");
  const running = useApp((s) => s.tick?.tc.running ?? false);
  const [to, setTo] = useState<string | null>(null);
  const external = source === "external";
  return (
    <div className="r-tc" data-testid="tc-panel">
      <div className={`r-tc-pos num${running ? " running" : ""}`} data-testid="tc-position">
        {position}
      </div>
      <div className="r-tc-state muted">
        {external ? "Follows an external timecode – start, stop and locate come from the source" : running ? "Running" : `Stopped · start ${start}`}
      </div>
      <div className="r-tc-buttons">
        <button className="btn big" data-testid="tc-start" disabled={external} onClick={() => void run({ cmd: "tc_start" })}>
          Start
        </button>
        <button className="btn big" data-testid="tc-pause" disabled={!running} onClick={() => void run({ cmd: "tc_pause" })}>
          Pause
        </button>
        <button className="btn big" data-testid="tc-resume" disabled={running} onClick={() => void run({ cmd: "tc_resume" })}>
          Resume
        </button>
        <button className="btn big" data-testid="tc-stop" disabled={external} onClick={() => void run({ cmd: "tc_stop" })}>
          Stop
        </button>
      </div>
      <div className="r-tc-locate">
        <TcField value={to} rate={rate} nullable ariaLabel="Locate to timecode" testId="tc-locate" disabled={external} onCommit={setTo} placeholder={start} />
        <button className="btn big" data-testid="tc-locate-go" disabled={external || !to} onClick={() => to && void run({ cmd: "tc_relocate", to })}>
          Locate
        </button>
      </div>
    </div>
  );
}
