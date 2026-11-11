// WebSocket connection: auto-reconnect with backoff, rev-gap detection, ping/latency,
// command acks. The server owns the show; this layer only mirrors it.
import { CommandError, type Command, type ServerMsg } from "./api";
import { applyPatch } from "./patch";
import { appendLog, getApp, setApp, toast } from "./store";

const BACKOFF_MIN = 250;
const BACKOFF_MAX = 2000;
const PING_MS = 2000;
/** No frame at all (ticks arrive at ~20 Hz) for this long => treat the socket as dead. */
const SILENCE_MS = 5000;
const ACK_TIMEOUT_MS = 8000;

interface Pending {
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

let ws: WebSocket | null = null;
let wantOpen = false;
let backoff = BACKOFF_MIN;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let pingTimer: ReturnType<typeof setInterval> | null = null;
let lastRx = 0;
let nextId = 1;
let gotHello = false;
const pending = new Map<number, Pending>();

/** Called before a reconnect attempt that follows a failed one; may refresh the token. Return false to stop. */
let beforeReconnect: (helloSeen: boolean) => Promise<boolean> = async () => true;
export function setBeforeReconnect(fn: (helloSeen: boolean) => Promise<boolean>): void {
  beforeReconnect = fn;
}

function wsUrl(token: string): string {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${location.host}/ws?token=${encodeURIComponent(token)}`;
}

function failPending(code: string): void {
  for (const [, p] of pending) {
    clearTimeout(p.timer);
    p.reject(new CommandError(code));
  }
  pending.clear();
}

function handle(msg: ServerMsg): void {
  switch (msg.t) {
    case "hello":
      gotHello = true;
      sendPing();
      setApp({ role: msg.role, clientId: msg.client_id });
      break;
    case "state":
      backoff = BACKOFF_MIN;
      setApp({ doc: msg.state, rev: msg.rev, conn: "open", everConnected: true });
      break;
    case "patch": {
      const s = getApp();
      if (!s.doc) break;
      if (msg.rev !== s.rev + 1) {
        // Missed an update: the only safe thing is a fresh state document.
        forceReconnect();
        break;
      }
      setApp({ doc: applyPatch(s.doc, msg.ops), rev: msg.rev });
      break;
    }
    case "tick":
      setApp({ tick: msg });
      break;
    case "log":
      appendLog(msg.entries);
      break;
    case "ack": {
      const p = pending.get(msg.id);
      if (!p) break;
      pending.delete(msg.id);
      clearTimeout(p.timer);
      if (msg.ok) p.resolve(msg.result ?? null);
      else p.reject(new CommandError(msg.error ?? "error"));
      break;
    }
    case "pong":
      setApp({ latencyMs: Math.max(0, Math.round(performance.now() - msg.ts)) });
      break;
    case "kicked":
      wantOpen = false;
      setApp({ kicked: true });
      break;
  }
}

function sendPing(): void {
  try {
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ t: "ping", ts: performance.now() }));
  } catch {
    /* close handler deals with it */
  }
}

function open(): void {
  const token = getApp().token;
  if (!wantOpen || token == null) return;
  gotHello = false;
  let sock: WebSocket;
  try {
    sock = new WebSocket(wsUrl(token));
  } catch {
    scheduleReconnect();
    return;
  }
  ws = sock;
  lastRx = performance.now();
  sock.onmessage = (ev) => {
    if (ws !== sock) return;
    lastRx = performance.now();
    let msg: ServerMsg;
    try {
      msg = JSON.parse(String(ev.data)) as ServerMsg; // JSON boundary
    } catch {
      return;
    }
    handle(msg);
  };
  sock.onclose = () => {
    if (ws !== sock) return;
    ws = null;
    onDown();
  };
  sock.onerror = () => {
    /* onclose follows */
  };
}

function onDown(): void {
  failPending("disconnected");
  if (getApp().conn !== "lost") setApp({ conn: getApp().everConnected ? "lost" : "connecting", latencyMs: null });
  if (wantOpen) scheduleReconnect();
}

function scheduleReconnect(): void {
  if (reconnectTimer || !wantOpen) return;
  const helloSeen = gotHello;
  const delay = backoff;
  backoff = Math.min(BACKOFF_MAX, backoff * 2);
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    void beforeReconnect(helloSeen).then(
      (go) => {
        if (go && wantOpen) open();
      },
      () => {
        if (wantOpen) open();
      },
    );
  }, delay);
}

export function forceReconnect(): void {
  const sock = ws;
  ws = null;
  if (sock) {
    sock.onclose = null;
    sock.onmessage = null;
    try {
      sock.close();
    } catch {
      /* ignore */
    }
  }
  onDown();
}

export function connect(): void {
  wantOpen = true;
  backoff = BACKOFF_MIN;
  if (!pingTimer) {
    pingTimer = setInterval(() => {
      if (!ws || ws.readyState !== WebSocket.OPEN) return;
      if (performance.now() - lastRx > SILENCE_MS) {
        forceReconnect();
        return;
      }
      sendPing();
    }, PING_MS);
  }
  if (!ws) open();
}

export function disconnect(): void {
  wantOpen = false;
  if (reconnectTimer) clearTimeout(reconnectTimer);
  reconnectTimer = null;
  const sock = ws;
  ws = null;
  if (sock) {
    sock.onclose = null;
    try {
      sock.close();
    } catch {
      /* ignore */
    }
  }
  failPending("disconnected");
}

/** Sends one command; resolves with the ack result, rejects with CommandError(code). */
export function send<T = unknown>(cmd: Command): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      reject(new CommandError("disconnected"));
      return;
    }
    const id = nextId++;
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new CommandError("timeout"));
    }, ACK_TIMEOUT_MS);
    pending.set(id, { resolve: resolve as (v: unknown) => void, reject, timer });
    try {
      ws.send(JSON.stringify({ t: "cmd", id, cmd }));
    } catch {
      pending.delete(id);
      clearTimeout(timer);
      reject(new CommandError("disconnected"));
    }
  });
}

// A page that comes back from background (iPad lock screen) should not wait for the backoff.
if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible" && wantOpen && !ws) {
      if (reconnectTimer) clearTimeout(reconnectTimer);
      reconnectTimer = null;
      backoff = BACKOFF_MIN;
      open();
    }
  });
  window.addEventListener("online", () => {
    if (wantOpen && !ws) toast("Network is back – reconnecting", "info");
  });
}
