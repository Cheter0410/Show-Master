// Structural edit actions shared by the toolbar, the inspector, the row context menu and hotkeys.
import { run, send, errorText } from "./commands";
import { cueCount, findList, stepMove, topLevelSelection } from "./cues";
import { currentListId, getApp, setApp, toast } from "./store";

function editable(): boolean {
  return getApp().doc?.mode === "edit";
}

function topOf(ids: readonly number[]): number[] {
  const s = getApp();
  const top = topLevelSelection(findList(s.doc?.show, currentListId(s)), ids);
  return top.length > 0 ? top : [...ids];
}

/** Deletes the cues (children go with their group) and offers undo. */
export async function deleteCues(ids: readonly number[]): Promise<boolean> {
  if (!editable() || ids.length === 0) return false;
  const cues = topOf(ids);
  setApp({ selection: [] });
  const ok = await run({ cmd: "delete_cues", cues });
  if (ok) toast(`Deleted ${cueCount(cues.length)} — undo with Ctrl/Cmd+Z`, "info", 6000, { label: "Undo", run: () => void run({ cmd: "undo" }) });
  return ok;
}

/** Duplicates the cues and selects the copies. */
export async function duplicateCues(ids: readonly number[]): Promise<boolean> {
  if (!editable() || ids.length === 0) return false;
  try {
    const res = await send<{ cues?: number[] } | null>({ cmd: "duplicate_cues", cues: topOf(ids) });
    if (res && Array.isArray(res.cues) && res.cues.length > 0) {
      setApp({ selection: res.cues });
      const last = res.cues[res.cues.length - 1];
      requestAnimationFrame(() => document.querySelector(`[data-cue="${last}"]`)?.scrollIntoView({ block: "nearest" }));
    }
    return true;
  } catch (e) {
    toast(`Duplicate: ${errorText(e)}`, "error");
    return false;
  }
}

/** Moves one cue a step up / down among its siblings (touch screens have no drag & drop). */
export async function moveCueStep(id: number, dir: -1 | 1): Promise<boolean> {
  if (!editable()) return false;
  const s = getApp();
  const list = findList(s.doc?.show, currentListId(s));
  const to = list ? stepMove(list, id, dir) : null;
  if (!list || !to) return false;
  return run({ cmd: "move_cues", cues: [id], list: list.id, parent: to.parent, index: to.index });
}
