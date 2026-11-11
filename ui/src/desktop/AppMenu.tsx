import { useState } from "react";
import { errorCode, errorText, send } from "../commands";
import { Confirm, Modal } from "../components/Modal";
import { toast, useApp } from "../store";

function megabytes(n: number): string {
  return (n / 1_000_000).toFixed(1);
}

/** "Update x.y.z" button in the top bar; opens the release notes and installs on request. */
export function UpdateButton() {
  const u = useApp((s) => s.doc?.update);
  const [open, setOpen] = useState(false);
  if (!u || !["available", "downloading", "installing", "failed", "restarting"].includes(u.state)) return null;

  const install = async () => {
    try {
      await send({ cmd: "install_update" });
    } catch (e) {
      const code = errorCode(e);
      if (code === "unsaved_changes") toast("Save the show before updating.", "error");
      else if (code === "cues_running") toast("Stop all cues before updating.", "error");
      else if (code === "live_mode") toast("Switch to Edit mode before updating.", "error");
      else toast(`Update: ${errorText(e)}`, "error");
    }
  };
  const busy = u.state === "downloading" || u.state === "installing" || u.state === "restarting";
  const label =
    u.state === "downloading"
      ? `Downloading ${u.total > 0 ? Math.round((100 * u.done) / u.total) : 0}%`
      : u.state === "installing"
        ? "Installing…"
        : u.state === "restarting"
          ? "Restarting…"
          : `Update ${u.version ?? ""}`;

  return (
    <>
      <button className="btn update-btn" onClick={() => setOpen(true)} data-testid="tb-update" title="A new version of Show Master is available">
        {label}
      </button>
      {open ? (
        <Modal
          title={`Show Master ${u.version ?? ""} is available`}
          onClose={() => setOpen(false)}
          testId="update-dialog"
          footer={
            <>
              <button className="btn" onClick={() => setOpen(false)} data-autofocus>
                Later
              </button>
              <a className="btn" href={u.download_page} target="_blank" rel="noreferrer">
                Download page
              </a>
              {u.automatic ? (
                <button className="btn primary" disabled={busy} onClick={() => void install()} data-testid="update-install">
                  {busy ? label : "Update and restart"}
                </button>
              ) : null}
            </>
          }
        >
          <p className="muted">Installed version: {u.current}</p>
          {u.notes ? <pre className="update-notes">{u.notes}</pre> : null}
          {u.state === "downloading" ? (
            <p>
              Downloading… {megabytes(u.done)} of {megabytes(u.total)} MB
            </p>
          ) : null}
          {u.state === "failed" ? <p className="error-text">{u.error}</p> : null}
          {!u.automatic ? <p className="muted">Automatic install is not available for this copy. Get the installer from the download page.</p> : null}
          <p className="muted">Show Master closes and starts again after the update. The show must be saved and no cue may be running.</p>
        </Modal>
      ) : null}
    </>
  );
}

/** Quits the application (the engine, not just this browser tab). */
export function QuitButton() {
  const [ask, setAsk] = useState<null | "plain" | "cues_running" | "unsaved_changes">(null);
  const [gone, setGone] = useState(false);
  const quit = async (force: boolean) => {
    try {
      await send({ cmd: "quit", ...(force ? { force: true } : {}) });
      setAsk(null);
      setGone(true);
    } catch (e) {
      const code = errorCode(e);
      if (code === "cues_running" || code === "unsaved_changes") setAsk(code);
      else toast(`Quit: ${errorText(e)}`, "error");
    }
  };
  return (
    <>
      <button className="btn" onClick={() => setAsk("plain")} data-testid="tb-quit" title="Quit Show Master">
        Quit
      </button>
      {ask === "plain" ? (
        <Confirm title="Quit Show Master?" confirmLabel="Quit" danger onConfirm={() => void quit(false)} onCancel={() => setAsk(null)}>
          The engine stops and all audio ends. Tablets lose their connection.
        </Confirm>
      ) : null}
      {ask === "cues_running" ? (
        <Confirm title="Cues are running" confirmLabel="Quit anyway" danger onConfirm={() => void quit(true)} onCancel={() => setAsk(null)}>
          Quitting now stops all running cues immediately.
        </Confirm>
      ) : null}
      {ask === "unsaved_changes" ? (
        <Confirm title="Unsaved changes" confirmLabel="Quit without saving" danger onConfirm={() => void quit(true)} onCancel={() => setAsk(null)}>
          The show has unsaved changes. They are kept as an autosave and offered for recovery next time.
        </Confirm>
      ) : null}
      {gone ? (
        <Modal title="Show Master has quit" testId="quit-done">
          <p>You can close this window.</p>
        </Modal>
      ) : null}
    </>
  );
}
