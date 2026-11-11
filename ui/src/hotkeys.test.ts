import { describe, expect, it } from "vitest";
import {
  assignHotkey, clearHotkey, comboFromEvent, DEFAULT_HOTKEYS, findConflict, formatCombo, HOTKEY_ACTIONS, keyForAction,
  matchHotkey, normalizeCombo, type KeyLike,
} from "./hotkeys";

const ev = (code: string, mods: Partial<KeyLike> = {}): KeyLike => ({ code, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, ...mods });

describe("comboFromEvent", () => {
  it("builds combos with modifiers in canonical order", () => {
    expect(comboFromEvent(ev("Space"))).toBe("Space");
    expect(comboFromEvent(ev("KeyZ", { ctrlKey: true }))).toBe("Ctrl+KeyZ");
    expect(comboFromEvent(ev("KeyZ", { shiftKey: true, ctrlKey: true }))).toBe("Ctrl+Shift+KeyZ");
    expect(comboFromEvent(ev("F5", { metaKey: true, altKey: true }))).toBe("Alt+Meta+F5");
  });
  it("returns null for bare modifier keys", () => {
    expect(comboFromEvent(ev("ShiftLeft", { shiftKey: true }))).toBeNull();
    expect(comboFromEvent(ev("ControlRight", { ctrlKey: true }))).toBeNull();
    expect(comboFromEvent(ev(""))).toBeNull();
  });
});

describe("normalizeCombo", () => {
  it("orders and de-duplicates modifiers", () => {
    expect(normalizeCombo("Shift+Ctrl+KeyZ")).toBe("Ctrl+Shift+KeyZ");
    expect(normalizeCombo("ctrl+Ctrl+KeyZ")).toBe("Ctrl+KeyZ");
    expect(normalizeCombo("Cmd+KeyS")).toBe("Meta+KeyS");
    expect(normalizeCombo("Space")).toBe("Space");
  });
});

describe("matchHotkey", () => {
  const keys = { Space: "go", Escape: "panic", "Ctrl+KeyZ": "undo", "Shift+Ctrl+KeyZ": "redo" };
  it("matches exact combos only", () => {
    expect(matchHotkey(keys, ev("Space"))).toBe("go");
    expect(matchHotkey(keys, ev("Space", { shiftKey: true }))).toBeNull();
    expect(matchHotkey(keys, ev("KeyZ", { ctrlKey: true }))).toBe("undo");
    expect(matchHotkey(keys, ev("KeyZ"))).toBeNull();
  });
  it("tolerates non-canonical modifier order in the show file", () => {
    expect(matchHotkey(keys, ev("KeyZ", { ctrlKey: true, shiftKey: true }))).toBe("redo");
  });
  it("ignores bare modifiers", () => {
    expect(matchHotkey(keys, ev("ControlLeft", { ctrlKey: true }))).toBeNull();
  });
});

describe("conflict detection", () => {
  it("finds the action that already owns a combo", () => {
    expect(findConflict(DEFAULT_HOTKEYS, "Space")).toBe("go");
    expect(findConflict(DEFAULT_HOTKEYS, "Space", "go")).toBeNull();
    expect(findConflict(DEFAULT_HOTKEYS, "Shift+Ctrl+KeyZ")).toBe("redo");
    expect(findConflict(DEFAULT_HOTKEYS, "F9")).toBeNull();
  });
  it("assignHotkey refuses duplicates and leaves the map alone", () => {
    const res = assignHotkey(DEFAULT_HOTKEYS, "stop_all", "Space");
    expect(res).toEqual({ ok: false, conflict: "go" });
  });
  it("assignHotkey moves an action to a new key", () => {
    const res = assignHotkey(DEFAULT_HOTKEYS, "go", "Enter");
    expect(res.ok).toBe(true);
    if (res.ok) {
      expect(res.hotkeys.Enter).toBe("go");
      expect(res.hotkeys.Space).toBeUndefined();
      expect(keyForAction(res.hotkeys, "go")).toBe("Enter");
      expect(Object.keys(res.hotkeys).length).toBe(Object.keys(DEFAULT_HOTKEYS).length);
    }
  });
  it("re-assigning the same key to the same action is fine", () => {
    expect(assignHotkey(DEFAULT_HOTKEYS, "go", "Space").ok).toBe(true);
  });
  it("clearHotkey removes the binding", () => {
    const h = clearHotkey(DEFAULT_HOTKEYS, "panic");
    expect(keyForAction(h, "panic")).toBeNull();
    expect(h.Space).toBe("go");
  });
  it("defaults are conflict-free and cover every action", () => {
    const actions = Object.values(DEFAULT_HOTKEYS);
    expect(new Set(actions).size).toBe(actions.length);
    for (const a of HOTKEY_ACTIONS) expect(keyForAction(DEFAULT_HOTKEYS, a)).not.toBeNull();
    const normalized = Object.keys(DEFAULT_HOTKEYS).map(normalizeCombo);
    expect(new Set(normalized).size).toBe(normalized.length);
  });
});

describe("formatCombo", () => {
  it("renders readable labels", () => {
    expect(formatCombo("Ctrl+KeyZ")).toBe("Ctrl + Z");
    expect(formatCombo("Space")).toBe("Space");
    expect(formatCombo("Escape")).toBe("Esc");
    expect(formatCombo("Shift+Digit1")).toBe("Shift + 1");
    expect(formatCombo("ArrowDown")).toBe("↓");
  });
});
