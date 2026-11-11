import { useSyncExternalStore } from "react";
import { confirmLive, forceEdit, getModeDialog, requestMode, setModeDialog, subscribeModeDialog } from "../commands";
import { Confirm, Modal } from "../components/Modal";
import { IconError, IconInfo, IconWarn } from "../components/icons";
import { cueLabel, findCue } from "../cues";
import { setApp, useApp } from "../store";
import type { PreflightIssue } from "../api";

export function ModeSwitch() {
  const mode = useApp((s) => s.doc?.mode ?? "edit");
  return (
    <div className="mode-switch" role="radiogroup" aria-label="Mode" data-testid="mode-switch">
      <button role="radio" aria-checked={mode === "edit"} className={mode === "edit" ? "on edit" : ""} data-testid="mode-edit" onClick={() => void requestMode("edit")}>
        EDIT
      </button>
      <button role="radio" aria-checked={mode === "live"} className={mode === "live" ? "on live" : ""} data-testid="mode-live" onClick={() => void requestMode("live")}>
        LIVE
      </button>
    </div>
  );
}

const ORDER: Record<PreflightIssue["severity"], number> = { error: 0, warning: 1, info: 2 };
const NO_ISSUES: PreflightIssue[] = [];

/** Pre-flight result (Edit→Live) and the "cues are running" confirmation (Live→Edit). */
export function ModeDialogs() {
  const dialog = useSyncExternalStore(subscribeModeDialog, getModeDialog);
  const issues = useApp((s) => s.doc?.preflight ?? NO_ISSUES);
  const show = useApp((s) => s.doc?.show ?? null);
  if (dialog === "confirm_edit") {
    return (
      <Confirm title="Cues are still running" confirmLabel="Switch to Edit anyway" danger onCancel={() => setModeDialog(null)} onConfirm={() => void forceEdit()} cancelLabel="Stay in Live">
        Leaving Live mode unlocks editing while audio is playing. Running cues keep playing.
      </Confirm>
    );
  }
  if (dialog !== "preflight") return null;
  const sorted = issues.slice().sort((a, b) => ORDER[a.severity] - ORDER[b.severity]);
  const errors = issues.filter((i) => i.severity === "error").length;
  const warnings = issues.filter((i) => i.severity === "warning").length;
  return (
    <Modal
      title="Pre-flight check"
      testId="preflight"
      onClose={() => setModeDialog(null)}
      footer={
        <>
          <button className="btn" data-testid="preflight-back" onClick={() => setModeDialog(null)}>
            Back to edit
          </button>
          <button className={`btn ${errors ? "danger" : "primary"}`} data-testid="preflight-go" data-autofocus onClick={() => void confirmLive()}>
            {errors || warnings ? "Go live anyway" : "Go live"}
          </button>
        </>
      }
    >
      <p className={`preflight-summary ${errors ? "bad" : warnings ? "warn" : "good"}`}>
        {errors === 0 && warnings === 0 ? "No problems found." : `${errors} error${errors === 1 ? "" : "s"}, ${warnings} warning${warnings === 1 ? "" : "s"}`}
      </p>
      <ul className="preflight-list">
        {sorted.map((i, idx) => {
          const cue = findCue(show, i.cue);
          return (
            <li key={idx} className={`pf-${i.severity}`}>
              <button
                className="pf-item"
                disabled={!cue}
                title={cue ? `Jump to cue ${cueLabel(cue)}` : undefined}
                onClick={() => {
                  if (!cue) return;
                  setApp({ selection: [cue.id] });
                  setModeDialog(null);
                  requestAnimationFrame(() => document.querySelector(`[data-cue="${cue.id}"]`)?.scrollIntoView({ block: "center" }));
                }}
              >
                {i.severity === "error" ? <IconError /> : i.severity === "warning" ? <IconWarn /> : <IconInfo />}
                <span>{i.message}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </Modal>
  );
}
