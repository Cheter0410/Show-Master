import { useState } from "react";
import { CUE_TYPES, type CueType } from "../api";
import { errorCode, errorText, run, send } from "../commands";
import { GoToBox } from "../components/Panels";
import { Confirm } from "../components/Modal";
import { IconCopy, IconGear, IconMore, IconPlus, IconRedo, IconSave, IconTrash, IconUndo, TypeIcon } from "../components/icons";
import { MenuButton, type MenuItem } from "../components/Menu";
import { deleteCues, duplicateCues } from "../editing";
import { COARSE_POINTER, useMedia } from "../platform";
import { GoButton, PanicButton } from "../components/Transport";
import { TYPE_LABELS } from "../cues";
import { currentListId, getApp, setApp, toast, useApp } from "../store";
import { FileBrowser, type FileMode } from "./FileBrowser";
import { ModeSwitch } from "./ModeSwitch";
import { UpdateButton, useQuit } from "./AppMenu";
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
  const selCount = useApp((s) => s.selection.length);
  const isEdit = useApp((s) => s.doc?.mode === "edit");
  const canEdit = isEdit && selCount > 0;
  const coarse = useMedia(COARSE_POINTER);
  const quit = useQuit();

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

  const fileItems: MenuItem[] = [
    { label: "New show", onSelect: () => void doNew(false), testId: "menu-new" },
    { label: "Open…", onSelect: () => p.setFileDialog("open-show"), testId: "menu-open" },
    { label: "Save", icon: <IconSave />, onSelect: () => void saveShow(() => p.setFileDialog("save-show")), testId: "menu-save" },
    { label: "Save as…", onSelect: () => p.setFileDialog("save-show"), testId: "menu-saveas" },
    { label: "Setup…", icon: <IconGear />, onSelect: p.onSetup, testId: "menu-setup", separator: true },
    { label: "Quit Show Master", onSelect: quit.ask, testId: "menu-quit", danger: true, separator: true },
  ];
  const addItems: MenuItem[] = CUE_TYPES.map((t) => ({
    label: t === "audio" ? "Audio… (choose file)" : TYPE_LABELS[t],
    icon: <TypeIcon type={t} />,
    testId: `menu-add-${t}`,
    onSelect: () => (t === "audio" ? p.setFileDialog("pick-audio") : void addCue(t)),
  }));
  const titleText = `${name || "Untitled"}${dirty ? " •" : ""}`;

  return (
    <header className="toolbar" data-testid="toolbar">
      <div className="tb-i tb-i-title tb-title" title={path ?? "Not saved yet"}>
        <span className="tb-name ellipsis" data-testid="show-name">{name || "Untitled"}</span>
        {dirty ? <span className="dirty-dot" title="Unsaved changes" data-testid="dirty" /> : null}
      </div>
      <div className="tb-i tb-i-file tb-group">
        <button className="btn" onClick={() => void doNew(false)} data-testid="tb-new">New</button>
        <button className="btn" onClick={() => p.setFileDialog("open-show")} data-testid="tb-open">Open…</button>
        <button className="btn" onClick={() => void saveShow(() => p.setFileDialog("save-show"))} data-testid="tb-save" title={path ?? undefined}>
          <IconSave /> Save
        </button>
        <button className="btn" onClick={() => p.setFileDialog("save-show")} data-testid="tb-saveas">Save as…</button>
      </div>
      <MenuButton className="tb-i tb-i-filemenu" label="File" items={fileItems} testId="tb-file-menu" heading={titleText} />
      <div className="tb-i tb-i-history tb-group">
        <button className="btn icon-only" disabled={!canUndo} onClick={() => void run({ cmd: "undo" })} aria-label="Undo" title="Undo (Ctrl/Cmd+Z)" data-testid="tb-undo">
          <IconUndo />
        </button>
        <button className="btn icon-only" disabled={!canRedo} onClick={() => void run({ cmd: "redo" })} aria-label="Redo" title="Redo (Ctrl/Cmd+Shift+Z)" data-testid="tb-redo">
          <IconRedo />
        </button>
      </div>
      <div className="tb-i tb-i-edit tb-group" role="group" aria-label="Selected cues">
        <button className="btn tb-dup" disabled={!canEdit} onClick={() => void duplicateCues(getApp().selection)} aria-label="Duplicate selected cues" title="Duplicate selected cues (Ctrl/Cmd+D)" data-testid="tb-duplicate">
          <IconCopy /> <span className="tb-text">Duplicate</span>
        </button>
        <button className="btn danger-outline" disabled={!canEdit} onClick={() => void deleteCues(getApp().selection)} aria-label="Delete selected cues" title="Delete selected cues (Delete / Backspace)" data-testid="tb-delete">
          <IconTrash /> <span>Delete{selCount > 1 ? ` ${selCount}` : ""}</span>
        </button>
      </div>
      <span className="tb-i tb-i-spacer1 spacer" />
      <span className="tb-i tb-i-update"><UpdateButton /></span>
      <span className="tb-i tb-i-mode"><ModeSwitch /></span>
      <button className="tb-i tb-i-setup btn" onClick={p.onSetup} data-testid="tb-setup">
        <IconGear /> Setup
      </button>
      <button className="tb-i tb-i-quit btn" onClick={quit.ask} data-testid="tb-quit" title="Quit Show Master">
        Quit
      </button>
      <MenuButton className="tb-i tb-i-more icon-only" label={<IconMore size={18} />} caret={false} ariaLabel="Menu" items={fileItems} testId="tb-more" heading={titleText} />
      <span className="tb-i tb-i-break" aria-hidden="true" />
      <div className="tb-i tb-i-add tb-group">
        <span className="tb-label"><IconPlus size={14} /> Add cue</span>
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
      <MenuButton className="tb-i tb-i-addmenu" label={<><IconPlus size={14} /> Add cue</>} items={addItems} testId="tb-add-menu" />
      <span className="tb-i tb-i-spacer2 spacer" />
      <span className="tb-i tb-i-goto"><GoToBox /></span>
      <div className="tb-i tb-i-transport tb-group tb-transport">
        <GoButton className="mini" />
        <button className="btn tb-stop" onClick={() => void transport.stopAll()} title="Stop all" aria-label="Stop all" data-testid="tb-stop">
          <IconStop /> <span className="tb-text">Stop</span>
        </button>
        <PanicButton className="mini" hold={coarse} />
      </div>
      {quit.dialogs}

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
