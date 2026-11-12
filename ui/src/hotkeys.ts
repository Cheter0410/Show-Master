// Hotkey matching / conflict detection. Combos are KeyboardEvent.code values with
// Ctrl+/Shift+/Alt+/Meta+ prefixes (always in that order), e.g. "Ctrl+Shift+KeyZ".

export const HOTKEY_ACTIONS = [
  "go", "panic", "stop_all", "toggle_pause", "next", "previous", "reset", "preview",
  "toggle_mode", "undo", "redo", "save", "duplicate", "delete", "find",
] as const;
export type HotkeyAction = (typeof HOTKEY_ACTIONS)[number];

export const ACTION_LABELS: Record<HotkeyAction, string> = {
  go: "GO",
  panic: "Panic",
  stop_all: "Stop all",
  toggle_pause: "Pause / resume",
  next: "Playhead next",
  previous: "Playhead previous",
  reset: "Reset show",
  preview: "Preview selected cue",
  toggle_mode: "Toggle Edit / Live",
  undo: "Undo",
  redo: "Redo",
  save: "Save",
  duplicate: "Duplicate cue",
  delete: "Delete cue",
  find: "Go to cue…",
};

export const DEFAULT_HOTKEYS: Record<string, HotkeyAction> = {
  Space: "go",
  Escape: "panic",
  KeyS: "stop_all",
  KeyP: "toggle_pause",
  ArrowDown: "next",
  ArrowUp: "previous",
  "Ctrl+KeyR": "reset",
  KeyV: "preview",
  "Ctrl+KeyL": "toggle_mode",
  "Ctrl+KeyZ": "undo",
  "Ctrl+Shift+KeyZ": "redo",
  "Ctrl+KeyS": "save",
  "Ctrl+KeyD": "duplicate",
  Delete: "delete",
  "Ctrl+KeyF": "find",
};

const MODIFIER_CODES = new Set([
  "ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight",
  "OSLeft", "OSRight",
]);
const MOD_ORDER = ["Ctrl", "Shift", "Alt", "Meta"] as const;

export interface KeyLike {
  code: string;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

/** Combo string for a key event; null while only modifier keys are held. */
export function comboFromEvent(e: KeyLike): string | null {
  if (!e.code || MODIFIER_CODES.has(e.code)) return null;
  let out = "";
  if (e.ctrlKey) out += "Ctrl+";
  if (e.shiftKey) out += "Shift+";
  if (e.altKey) out += "Alt+";
  if (e.metaKey) out += "Meta+";
  return out + e.code;
}

/** Canonical form: modifiers de-duplicated, in Ctrl/Shift/Alt/Meta order, case-insensitive names. */
export function normalizeCombo(combo: string): string {
  const parts = combo.split("+").map((p) => p.trim()).filter(Boolean);
  if (parts.length === 0) return "";
  const code = parts[parts.length - 1];
  const mods = new Set<string>();
  for (const p of parts.slice(0, -1)) {
    const l = p.toLowerCase();
    if (l === "ctrl" || l === "control") mods.add("Ctrl");
    else if (l === "shift") mods.add("Shift");
    else if (l === "alt" || l === "option") mods.add("Alt");
    else if (l === "meta" || l === "cmd" || l === "command" || l === "super") mods.add("Meta");
  }
  return MOD_ORDER.filter((m) => mods.has(m)).map((m) => `${m}+`).join("") + code;
}

/** Looks up the action bound to a key event. */
export function matchHotkey(hotkeys: Record<string, string>, e: KeyLike): string | null {
  const combo = comboFromEvent(e);
  if (!combo) return null;
  const direct = hotkeys[combo];
  if (direct) return direct;
  for (const [k, action] of Object.entries(hotkeys)) {
    if (normalizeCombo(k) === combo) return action;
  }
  return null;
}

/**
 * matchHotkey plus two platform aliases, used only when the pressed combo itself is unbound:
 * - Backspace (Mac keyboards label it "delete") acts as the key bound to the `delete` action;
 * - Cmd (Meta) stands in for Ctrl, so Cmd+Z / Cmd+S / Cmd+D work on a Mac.
 */
export function resolveHotkey(hotkeys: Record<string, string>, e: KeyLike): string | null {
  const direct = matchHotkey(hotkeys, e);
  if (direct) return direct;
  if (e.metaKey && !e.ctrlKey) {
    // explicit copy: DOM event properties are prototype getters, a spread would lose them
    const viaCtrl = matchHotkey(hotkeys, { code: e.code, shiftKey: e.shiftKey, altKey: e.altKey, metaKey: false, ctrlKey: true });
    if (viaCtrl) return viaCtrl;
  }
  if (e.code === "Backspace" && !e.altKey && !e.shiftKey && !e.ctrlKey) {
    const del = keyForAction(hotkeys, "delete");
    // Alias only a plain Delete binding: a custom modifier combo stays the only way.
    if (del != null && normalizeCombo(del) === "Delete") return "delete";
  }
  return null;
}

/** The action already bound to `combo` (other than `exceptAction`), i.e. a conflict. */
export function findConflict(hotkeys: Record<string, string>, combo: string, exceptAction?: string): string | null {
  const n = normalizeCombo(combo);
  for (const [k, action] of Object.entries(hotkeys)) {
    if (action !== exceptAction && normalizeCombo(k) === n) return action;
  }
  return null;
}

export type AssignResult = { ok: true; hotkeys: Record<string, string> } | { ok: false; conflict: string };

/** Binds `combo` to `action` (replacing the action's previous key). Refuses duplicates. */
export function assignHotkey(hotkeys: Record<string, string>, action: string, combo: string): AssignResult {
  const conflict = findConflict(hotkeys, combo, action);
  if (conflict) return { ok: false, conflict };
  const next: Record<string, string> = {};
  for (const [k, a] of Object.entries(hotkeys)) if (a !== action) next[k] = a;
  next[normalizeCombo(combo)] = action;
  return { ok: true, hotkeys: next };
}

export function clearHotkey(hotkeys: Record<string, string>, action: string): Record<string, string> {
  const next: Record<string, string> = {};
  for (const [k, a] of Object.entries(hotkeys)) if (a !== action) next[k] = a;
  return next;
}

export function keyForAction(hotkeys: Record<string, string>, action: string): string | null {
  for (const [k, a] of Object.entries(hotkeys)) if (a === action) return k;
  return null;
}

const CODE_LABELS: Record<string, string> = {
  Space: "Space", Escape: "Esc", ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→",
  Enter: "Enter", NumpadEnter: "Num Enter", Backspace: "⌫", Delete: "Del", Tab: "Tab",
  Minus: "-", Equal: "=", Comma: ",", Period: ".", Slash: "/", Backquote: "`",
  BracketLeft: "[", BracketRight: "]", Semicolon: ";", Quote: "'", Backslash: "\\",
};

/** Human readable label: "Ctrl+KeyZ" -> "Ctrl + Z". */
export function formatCombo(combo: string): string {
  const parts = normalizeCombo(combo).split("+");
  const code = parts.pop() ?? "";
  let key = CODE_LABELS[code];
  if (!key) {
    if (code.startsWith("Key")) key = code.slice(3);
    else if (code.startsWith("Digit")) key = code.slice(5);
    else if (code.startsWith("Numpad")) key = `Num ${code.slice(6)}`;
    else key = code;
  }
  return [...parts, key].join(" + ");
}

/** True when a key event target is a text-entry element and hotkeys must stay out of the way. */
export function isEditableTarget(t: EventTarget | null): boolean {
  if (typeof HTMLElement === "undefined" || !(t instanceof HTMLElement)) return false;
  const tag = t.tagName;
  if (tag === "TEXTAREA" || tag === "SELECT") return true;
  if (tag === "INPUT") {
    const type = (t as HTMLInputElement).type;
    return type !== "checkbox" && type !== "radio" && type !== "button" && type !== "range";
  }
  return t.isContentEditable;
}
