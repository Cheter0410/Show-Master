import { useState } from "react";
import { CUE_TYPES, type CueType } from "../api";
import { errorCode, errorText, run, send } from "../commands";
import { GoToBox } from "../components/Panels";
import { Confirm } from "../components/Modal";
import { IconGear, IconPlus, IconRedo, IconSave, IconUndo, TypeIcon } from "../components/icons";
import { GoButton, PanicButton } from "../components/Transport";
import { TYPE_LABELS } from "../cues";
import { currentListId, getApp, setApp, toast, useApp } from "../store";
import { FileBrowser, type FileMode } from "./FileBrowser";
import { ModeSwitch } from "./ModeSwitch";
import { transport } from "../commands";
import { IconStop } from "../components/icons";

/** Adds a cue after the current selection and selects it. */
export async function addCue(kind: CueType, file?: string): Promise<void> {
  const s = getApp();
  const after = s.selection[s.selection.length - 1];
  const list = currentListId(s);
  try {
    const res = await send<{ cue?: number } | null>({ cmd: "add_cue", kind, ...(after != null ? { after } : {}), ...(list != null ? { list } : {}), ...(file ? { file } : {}) });
    if (res && typeof res.cue === "number") {
      setApp({ selection: [res.cue] });
      const id = res.cue;
      requestAnimationFrame(() => document.querySelector(`[data-cue="${id}"]`)?.scrollIntoView({ block: "nearest" }));
    }
  } catch (e) {
    toast(`Add cue: ${errorText(e)}`, "error");
  }
}

export async function saveShow(openSaveAs: () => void): Promise<void> {
  if (!getApp().doc?.show_path) {
    openSaveAs();
    return;
  }
  if (await run({ cmd: "save_show" })) toast("Show saved", "info", 1500);
}

type Pending = { kind: "new" } | { kind: "open"; path: string };

export function Toolbar(p: { onSetup: () => void; fileDialog: FileMode | null; setFileDialog: (m: FileMode | null) => void }) {
  const name = useApp((s) => s.doc?.show.name ?? "");
  const path = useApp((s) => s.doc?.show_path ?? null);
  const dirty = useApp((s) => s.doc?.dirty ?? false);
  const canUndo = useApp((s) => s.doc?.can_undo ?? false);
  const canRedo = useApp((s) => s.doc?.can_redo ?? false);
  const [pending, setPending] = useState<Pending | null>(null);

  const doNew = async (force: boolean) => {
    try {
      await send({ cmd: "new_show", ...(force ? { force: true } : {}) });
      setApp({ selection: [] });
    } catch (e) {
      if (errorCode(e) === "unsaved_changes") setPending({ kind: "new" });
      else toast(`New show: ${errorText(e)}`, "error");
    }
  };
  const doOpen = async (file: string, force: boolean) => {
    try {
      await send({ cmd: "open_show", path: file, ...(force ? { force: true } : {}) });
      setApp({ selection: [] });
    } catch (e) {
      if (errorCode(e) === "unsaved_changes") setPending({ kind: "open", path: file });
      else toast(`Open show: ${errorText(e)}`, "error");
    }
  };

  return (
    <header className="toolbar" data-testid="toolbar">
      <div className="tb-row">
        <div className="tb-title" title={path ?? "Not saved yet"}>
          <span className="tb-name ellipsis" data-testid="show-name">{name || "Untitled"}</span>
          {dirty ? <span className="dirty-dot" title="Unsaved changes" data-testid="dirty" /> : null}
        </div>
        <div className="tb-group">
          <button className="btn" onClick={() => void doNew(false)} data-testid="tb-new">New</button>
          <button className="btn" onClick={() => p.setFileDialog("open-show")} data-testid="tb-open">Open…</button>
          <button className="btn" onClick={() => void saveShow(() => p.setFileDialog("save-show"))} data-testid="tb-save" title={path ?? undefined}>
            <IconSave /> Save
          </button>
          <button className="btn" onClick={() => p.setFileDialog("save-show")} data-testid="tb-saveas">Save as…</button>
        </div>
        <div className="tb-group">
          <button className="btn icon-only" disabled={!canUndo} onClick={() => void run({ cmd: "undo" })} aria-label="Undo" title="Undo" data-testid="tb-undo">
            <IconUndo />
          </button>
          <button className="btn icon-only" disabled={!canRedo} onClick={() => void run({ cmd: "redo" })} aria-label="Redo" title="Redo" data-testid="tb-redo">
            <IconRedo />
          </button>
        </div>
        <span className="spacer" />
        <ModeSwitch />
        <button className="btn" onClick={p.onSetup} data-testid="tb-setup">
          <IconGear /> Setup
        </button>
      </div>
      <div className="tb-row tb-row2">
        <span className="tb-label"><IconPlus size={14} /> Add cue</span>
        <div className="tb-group">
          {CUE_TYPES.map((t) => (
            <button
              key={t}
              className="btn"
              data-testid={`add-${t}`}
              title={t === "audio" ? "Add audio cue (choose file…)" : `Add ${TYPE_LABELS[t]} cue`}
              onClick={() => (t === "audio" ? p.setFileDialog("pick-audio") : void addCue(t))}
            >
              <TypeIcon type={t} /> {TYPE_LABELS[t]}{t === "audio" ? "…" : ""}
            </button>
          ))}
        </div>
        <span className="spacer" />
        <GoToBox />
        <div className="tb-group tb-transport">
          <GoButton className="mini" />
          <button className="btn" onClick={() => void transport.stopAll()} title="Stop all" data-testid="tb-stop">
            <IconStop /> Stop
          </button>
          <PanicButton className="mini" />
        </div>
      </div>

      {p.fileDialog ? (
        <FileBrowser
          mode={p.fileDialog}
          multiple={p.fileDialog === "pick-audio"}
          onClose={() => p.setFileDialog(null)}
          onPick={(paths) => {
            const mode = p.fileDialog;
            p.setFileDialog(null);
            if (mode === "open-show") void doOpen(paths[0], false);
            else if (mode === "save-show") void run({ cmd: "save_show", path: paths[0] }).then((ok) => ok && toast("Show saved", "info", 1500));
            else
              void (async () => {
                for (const f of paths) await addCue("audio", f);
              })();
          }}
        />
      ) : null}
      {pending ? (
        <Confirm
          title="Discard unsaved changes?"
          confirmLabel="Discard changes"
          danger
          onCancel={() => setPending(null)}
          onConfirm={() => {
            const pd = pending;
            setPending(null);
            if (pd.kind === "new") void doNew(true);
            else void doOpen(pd.path, true);
          }}
        >
          The current show has changes that were not saved.
        </Confirm>
      ) : null}
    </header>
  );
}
