// Tablets follow the show to the backup computer: when the main computer stops
// answering, the page probes the backup (address published by the main) and
// moves there with its login, so the operator keeps working without typing a PIN.
import { getApp } from "./store";

const DOWN_BEFORE_SWITCH_MS = 2500;
const PROBE_TIMEOUT_MS = 1500;

let target: string | null = null;
let downSince: number | null = null;
let probing = false;
let timer: ReturnType<typeof setInterval> | null = null;

/** Remember where to go (called with every state document). */
export function noteFailover(url: string | null | undefined): void {
  if (url) target = url.replace(/\/+$/, "");
}

async function reachable(url: string): Promise<boolean> {
  const ctrl = new AbortController();
  const t = setTimeout(() => ctrl.abort(), PROBE_TIMEOUT_MS);
  try {
    // Opaque cross-origin request: it resolves when something answers there.
    await fetch(`${url}/api/session`, { mode: "no-cors", cache: "no-store", signal: ctrl.signal });
    return true;
  } catch {
    return false;
  } finally {
    clearTimeout(t);
  }
}

function pairToken(): string | null {
  try {
    return localStorage.getItem("sm.pair");
  } catch {
    return null;
  }
}

async function check(): Promise<void> {
  const s = getApp();
  if (s.conn === "open" || !s.everConnected) {
    downSince = null;
    return;
  }
  downSince ??= performance.now();
  if (!target || probing || performance.now() - downSince < DOWN_BEFORE_SWITCH_MS) return;
  if (target === `${location.protocol}//${location.host}`) return;
  probing = true;
  try {
    if (await reachable(target)) {
      const pair = pairToken();
      const token = s.token;
      const q = pair ? `?pair=${encodeURIComponent(pair)}` : token ? `?session=${encodeURIComponent(token)}` : "";
      location.href = `${target}${location.pathname}${q}`;
    }
  } finally {
    probing = false;
  }
}

/** Start watching the connection (tablet pages only). */
export function startFailover(): void {
  if (timer) return;
  timer = setInterval(() => void check(), 500);
}
