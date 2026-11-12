// Desktop app (`/`, role admin): Edit mode (table + inspector) and Live mode (dashboard).
import { useEffect, useRef, useState } from "react";
import { fireGo, pressPanic, run, toggleMode, transport } from "../commands";
import { Confirm, Modal } from "../components/Modal";
import { GOTO_INPUT_ID, StatusBar } from "../components/Panels";
import { isEditableTarget, resolveHotkey, type HotkeyAction } from "../hotkeys";
import { deleteCues, duplicateCues } from "../editing";
import { getApp, setApp, useApp } from "../store";
import { CueTable } from "./CueTable";
import type { FileMode } from "./FileBrowser";
import { Inspector } from "./Inspector";
import { LiveDashboard } from "./LiveDashboard";
import { ModeDialogs } from "./ModeSwitch";
import { SetupDialog } from "./Setup";
import { saveShow, Toolbar } from "./Toolbar";

function modalOpen(): boolean {
  return document.querySelector("[data-modal]") != null;
}

export function DesktopApp() {
  const mode = useApp((s) => s.doc?.mode ?? "edit");
  const recovery = useApp((s) => s.doc?.recovery_available ?? false);
  const quitLock = useApp((s) => s.doc?.show.settings.quit_lock ?? false);
  const lists = useApp((s) => s.doc?.show.cue_lists);
  const listId = useApp((s) => s.listId);
  const [setup, setSetup] = useState(false);
  const [fileDialog, setFileDialog] = useState<FileMode | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);
  const ui = useRef({ setFileDialog, setConfirmReset });
  ui.current = { setFileDialog, setConfirmReset };

  // ---- hotkeys (capture phase so Escape = panic also works while typing) ----
  useEffect(() => {
    const perform = (action: HotkeyAction, repeat: boolean) => {
      const s = getApp();
      const editing = s.doc?.mode === "edit";
      if (repeat && action !== "next" && action !== "previous") return;
      switch (action) {
        case "go":
          fireGo();
          break;
        case "panic":
          pressPanic();
          break;
        case "stop_all":
          void transport.stopAll();
          break;
        case "toggle_pause":
          void transport.togglePause();
          break;
        case "next":
          void transport.next();
          break;
        case "previous":
          void transport.previous();
          break;
        case "reset":
          ui.current.setConfirmReset(true);
          break;
        case "preview":
          if (s.selection.length > 0) void transport.startCue(s.selection[0]);
          break;
        case "toggle_mode":
          toggleMode();
          break;
        case "undo":
          if (editing && s.doc?.can_undo) void run({ cmd: "undo" });
          break;
        case "redo":
          if (editing && s.doc?.can_redo) void run({ cmd: "redo" });
          break;
        case "save":
          void saveShow(() => ui.current.setFileDialog("save-show"));
          break;
        case "duplicate":
          if (editing && s.selection.length > 0) void duplicateCues(s.selection);
          break;
        case "delete":
          if (editing && s.selection.length > 0) void deleteCues(s.selection);
          break;
        case "find": {
          const el = document.getElementById(GOTO_INPUT_ID) as HTMLInputElement | null;
          el?.focus();
          el?.select();
          break;
        }
      }
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (modalOpen()) return; // dialogs own the keyboard
      const editable = isEditableTarget(e.target);
      if (editable && e.code !== "Escape") return;
      // Space must never scroll the page or "click" a focused button.
      if (e.code === "Space") e.preventDefault();
      const action = resolveHotkey(getApp().doc?.show.hotkeys ?? {}, e);
      if (!action) return;
      e.preventDefault();
      perform(action as HotkeyAction, e.repeat);
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.code === "Space" && !modalOpen() && !isEditableTarget(e.target)) e.preventDefault();
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
    };
  }, []);

  // ---- quit lock ----
  useEffect(() => {
    if (!(quitLock && mode === "live")) return;
    const guard = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = "";
    };
    window.addEventListener("beforeunload", guard);
    return () => window.removeEventListener("beforeunload", guard);
  }, [quitLock, mode]);

  const listTabs =
    lists && lists.length > 1 ? (
      <nav className="list-tabs" role="tablist" aria-label="Cue lists">
        {lists.map((l, i) => (
          <button key={l.id} role="tab" aria-selected={listId === l.id || (listId == null && i === 0)} className={`list-tab${listId === l.id || (listId == null && i === 0) ? " on" : ""}`} onClick={() => setApp({ listId: l.id, selection: [] })}>
            {l.name}
          </button>
        ))}
      </nav>
    ) : null;

  return (
    <>
      {mode === "live" ? (
        <LiveDashboard canEdit />
      ) : (
        <div className="desk" data-testid="edit-screen">
          <Toolbar onSetup={() => setSetup(true)} fileDialog={fileDialog} setFileDialog={setFileDialog} />
          <main className="desk-main">
            {listTabs}
            <CueTable />
          </main>
          <Inspector />
          <StatusBar />
        </div>
      )}
      {setup ? <SetupDialog onClose={() => setSetup(false)} /> : null}
      <ModeDialogs />
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
      {recovery ? (
        <Modal
          title="Recover unsaved work?"
          testId="recovery"
          footer={
            <>
              <button className="btn danger" onClick={() => void run({ cmd: "recover", accept: false })}>
                Discard autosave
              </button>
              <button className="btn primary" data-autofocus onClick={() => void run({ cmd: "recover", accept: true })}>
                Recover
              </button>
            </>
          }
        >
          <div className="confirm-text">Show Master was not closed properly. An automatically saved copy of the show is available.</div>
        </Modal>
      ) : null}
    </>
  );
}
