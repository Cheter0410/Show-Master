// High-level command helpers shared by buttons and hotkeys.
import { CommandError, type Command, type Mode } from "./api";
import { send } from "./connection";
import { currentListId, getApp, setApp, toast } from "./store";

export { send };

const ERROR_TEXT: Record<string, string> = {
  forbidden: "Not permitted for this role",
  live_mode: "Not possible in Live mode – switch to Edit first",
  disconnected: "Not connected – command was not sent",
  timeout: "Server did not answer in time",
  unsaved_changes: "The show has unsaved changes",
  cues_running: "Cues are still running",
};

export function errorText(e: unknown): string {
  if (e instanceof CommandError) return ERROR_TEXT[e.code] ?? e.code;
  return e instanceof Error ? e.message : String(e);
}

export function errorCode(e: unknown): string {
  return e instanceof CommandError ? e.code : "error";
}

/** Fire-and-report: sends a command, shows a toast on failure, never throws. Resolves true on success. */
export async function run(cmd: Command, quiet = false): Promise<boolean> {
  try {
    await send(cmd);
    return true;
  } catch (e) {
    if (!quiet) toast(`${cmd.cmd}: ${errorText(e)}`, "error");
    return false;
  }
}

// ---------- GO (double-fire protection) ----------

let goLockedUntil = 0;
const goListeners = new Set<() => void>();

export function goLockRemaining(): number {
  return Math.max(0, goLockedUntil - performance.now());
}

export function subscribeGoLock(l: () => void): () => void {
  goListeners.add(l);
  return () => goListeners.delete(l);
}

/**
 * Fires GO unless a previous GO from this client is younger than `go_debounce_ms`.
 * The server debounces too; this guard only keeps touch bounces and key repeats local.
 */
export function fireGo(): boolean {
  const now = performance.now();
  if (now < goLockedUntil) return false;
  const debounce = getApp().doc?.show.settings.go_debounce_ms ?? 250;
  goLockedUntil = now + Math.max(0, debounce);
  for (const l of goListeners) l();
  setTimeout(() => {
    for (const l of goListeners) l();
  }, Math.max(0, debounce) + 10);
  const list = currentListId();
  void run(list != null ? { cmd: "go", list } : { cmd: "go" });
  return true;
}

// ---------- PANIC (optional double tap) ----------

let panicArmedAt = 0;
const panicListeners = new Set<() => void>();

export function subscribePanic(l: () => void): () => void {
  panicListeners.add(l);
  return () => panicListeners.delete(l);
}

export function panicArmed(): boolean {
  const ms = getApp().doc?.show.settings.double_tap_ms ?? 600;
  return panicArmedAt > 0 && performance.now() - panicArmedAt <= ms;
}

function notifyPanic(): void {
  for (const l of panicListeners) l();
}

/** Panic press honouring `panic_double_tap`. Returns true when the command was sent. */
export function pressPanic(): boolean {
  const settings = getApp().doc?.show.settings;
  if (settings?.panic_double_tap) {
    if (!panicArmed()) {
      panicArmedAt = performance.now();
      notifyPanic();
      setTimeout(notifyPanic, settings.double_tap_ms + 20);
      return false;
    }
  }
  panicArmedAt = 0;
  notifyPanic();
  void run({ cmd: "panic" });
  return true;
}

export function panicNow(): void {
  panicArmedAt = 0;
  notifyPanic();
  void run({ cmd: "panic" });
}

// ---------- transport ----------

function withList<T extends "next" | "previous">(cmd: T): Command {
  const list = currentListId();
  return list != null ? { cmd, list } : { cmd };
}

export const transport = {
  stopAll: () => run({ cmd: "stop_all" }),
  togglePause: () => run({ cmd: "toggle_pause" }),
  next: () => run(withList("next")),
  previous: () => run(withList("previous")),
  reset: () => run({ cmd: "reset" }),
  setPlayhead: (cue: number | null) => {
    const list = currentListId();
    return run(list != null ? { cmd: "set_playhead", cue, list } : { cmd: "set_playhead", cue });
  },
  gotoNumber: (number: string) => run({ cmd: "goto_number", number }),
  startCue: (cue: number) => run({ cmd: "start_cue", cue }),
  stopCue: (cue: number) => run({ cmd: "stop_cue", cue }),
};

// ---------- mode switching (desktop) ----------

export type ModeDialog = null | "preflight" | "confirm_edit";
let modeDialog: ModeDialog = null;
const modeListeners = new Set<() => void>();
export function getModeDialog(): ModeDialog {
  return modeDialog;
}
export function setModeDialog(d: ModeDialog): void {
  modeDialog = d;
  for (const l of modeListeners) l();
}
export function subscribeModeDialog(l: () => void): () => void {
  modeListeners.add(l);
  return () => modeListeners.delete(l);
}

/** Edit→Live: run the pre-flight check and show its result first. Live→Edit: confirm if cues run. */
export async function requestMode(mode: Mode): Promise<void> {
  const cur = getApp().doc?.mode;
  if (cur === mode) return;
  if (mode === "live") {
    setModeDialog("preflight");
    await run({ cmd: "run_preflight" });
    return;
  }
  try {
    await send({ cmd: "set_mode", mode: "edit" });
  } catch (e) {
    if (errorCode(e) === "cues_running") setModeDialog("confirm_edit");
    else toast(`Switch to Edit: ${errorText(e)}`, "error");
  }
}

export async function confirmLive(): Promise<void> {
  if (await run({ cmd: "set_mode", mode: "live", force: true })) {
    setModeDialog(null);
    setApp({ selection: getApp().selection.slice(0, 1) });
  }
}

export async function forceEdit(): Promise<void> {
  if (await run({ cmd: "set_mode", mode: "edit", force: true })) setModeDialog(null);
}

export function toggleMode(): void {
  const cur = getApp().doc?.mode;
  if (modeDialog) return;
  void requestMode(cur === "live" ? "edit" : "live");
}
