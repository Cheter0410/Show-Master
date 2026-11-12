// Central client-side store (tiny external store + useSyncExternalStore; no dependency).
import { useSyncExternalStore } from "react";
import type { LogEntry, Role, State, Tick } from "./api";

export type ConnStatus = "connecting" | "open" | "lost";
export type AuthPhase = "boot" | "pin" | "ready" | "unreachable";

export interface Toast {
  id: number;
  level: "info" | "warn" | "error" | "alarm";
  text: string;
  /** optional button in the toast (e.g. "Undo" after a delete – the only undo on a touch screen) */
  action?: { label: string; run: () => void };
}

export interface AppState {
  auth: AuthPhase;
  role: Role | null;
  token: string | null;
  viewNeedsPin: boolean;
  /** role the PIN pad logs in as */
  loginRole: "control" | "view";
  conn: ConnStatus;
  /** true once a state document has been received at least once */
  everConnected: boolean;
  kicked: boolean;
  clientId: number | null;
  doc: State | null;
  rev: number;
  tick: Tick | null;
  log: LogEntry[];
  latencyMs: number | null;
  /** selected cue ids (UI-local, not part of the server state) */
  selection: number[];
  /** cue list shown in this client */
  listId: number | null;
  toasts: Toast[];
}

const initial: AppState = {
  auth: "boot",
  role: null,
  token: null,
  viewNeedsPin: true,
  loginRole: "control",
  conn: "connecting",
  everConnected: false,
  kicked: false,
  clientId: null,
  doc: null,
  rev: 0,
  tick: null,
  log: [],
  latencyMs: null,
  selection: [],
  listId: null,
  toasts: [],
};

let state: AppState = initial;
const listeners = new Set<() => void>();

export function getApp(): AppState {
  return state;
}

export function setApp(partial: Partial<AppState>): void {
  state = { ...state, ...partial };
  for (const l of listeners) l();
}

function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

/** Selector must return a referentially stable value for unchanged state (no fresh objects). */
export function useApp<T>(selector: (s: AppState) => T): T {
  return useSyncExternalStore(subscribe, () => selector(state));
}

export const MAX_LOG = 500;

export function appendLog(entries: LogEntry[]): void {
  if (entries.length === 0) return;
  const log = state.log.concat(entries);
  setApp({ log: log.length > MAX_LOG ? log.slice(log.length - MAX_LOG) : log });
}

let toastId = 1;
export function toast(text: string, level: Toast["level"] = "info", ttlMs = 4000, action?: Toast["action"]): void {
  // Collapse identical toasts (e.g. a held hotkey producing the same error).
  if (state.toasts.some((t) => t.text === text && t.level === level)) return;
  const id = toastId++;
  setApp({ toasts: [...state.toasts.slice(-4), { id, level, text, ...(action ? { action } : {}) }] });
  setTimeout(() => dismissToast(id), ttlMs);
}

export function dismissToast(id: number): void {
  if (state.toasts.some((t) => t.id === id)) setApp({ toasts: state.toasts.filter((t) => t.id !== id) });
}

// ---- convenience selectors ----
export const selDoc = (s: AppState) => s.doc;
export const selShow = (s: AppState) => s.doc?.show ?? null;
export const selSettings = (s: AppState) => s.doc?.show.settings ?? null;
export const selTick = (s: AppState) => s.tick;
export const selRole = (s: AppState) => s.role;

/** Id of the cue list this client is looking at (falls back to the first list). */
export function currentListId(s: AppState = state): number | null {
  const lists = s.doc?.show.cue_lists;
  if (!lists || lists.length === 0) return null;
  if (s.listId != null && lists.some((l) => l.id === s.listId)) return s.listId;
  return lists[0].id;
}
