// Platform adapter: the only place that knows about an optional Tauri shell.
// In a plain browser none of this is active (browsers never expose real file paths).

interface TauriEventApi {
  listen: (event: string, cb: (e: { payload: unknown }) => void) => Promise<() => void>;
}
interface TauriGlobal {
  event?: TauriEventApi;
}

declare global {
  interface Window {
    __TAURI__?: TauriGlobal;
  }
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && window.__TAURI__ != null;
}

function pathsFromPayload(payload: unknown): string[] {
  // Tauri v1: payload = string[]; Tauri v2: payload = { paths: string[], position }.
  if (Array.isArray(payload)) return payload.filter((p): p is string => typeof p === "string");
  if (typeof payload === "object" && payload !== null && "paths" in payload) {
    const paths = (payload as { paths: unknown }).paths;
    if (Array.isArray(paths)) return paths.filter((p): p is string => typeof p === "string");
  }
  return [];
}

/**
 * Subscribes to OS file drops with real paths (Tauri only).
 * Returns an unsubscribe function; a no-op in the browser.
 */
export function onNativeFileDrop(cb: (paths: string[]) => void): () => void {
  const api = typeof window !== "undefined" ? window.__TAURI__?.event : undefined;
  if (!api) return () => {};
  let disposed = false;
  const unlisten: Array<() => void> = [];
  for (const name of ["tauri://drag-drop", "tauri://file-drop"]) {
    api
      .listen(name, (e) => {
        const paths = pathsFromPayload(e.payload);
        if (paths.length > 0) cb(paths);
      })
      .then((un) => {
        if (disposed) un();
        else unlisten.push(un);
      })
      .catch(() => {});
  }
  return () => {
    disposed = true;
    for (const un of unlisten) un();
  };
}

const AUDIO_EXT = /\.(wav|wave|aif|aiff|flac|mp3|ogg|oga|opus|m4a|aac|caf)$/i;
export function isAudioPath(path: string): boolean {
  return AUDIO_EXT.test(path);
}

// ---- Screen Wake Lock (keeps the iPad awake during the show) ----

interface WakeLockSentinelLike {
  released: boolean;
  release: () => Promise<void>;
  addEventListener: (type: "release", cb: () => void) => void;
}

export function keepScreenAwake(): () => void {
  const nav = navigator as Navigator & { wakeLock?: { request: (type: "screen") => Promise<WakeLockSentinelLike> } };
  if (!nav.wakeLock) return () => {};
  let sentinel: WakeLockSentinelLike | null = null;
  let stopped = false;
  const acquire = () => {
    if (stopped || document.visibilityState !== "visible" || (sentinel && !sentinel.released)) return;
    nav.wakeLock!.request("screen").then(
      (s) => {
        sentinel = s;
        if (stopped) void s.release().catch(() => {});
      },
      () => {
        /* denied (low battery, permissions) – nothing we can do */
      },
    );
  };
  const onVis = () => acquire();
  document.addEventListener("visibilitychange", onVis);
  acquire();
  return () => {
    stopped = true;
    document.removeEventListener("visibilitychange", onVis);
    if (sentinel && !sentinel.released) void sentinel.release().catch(() => {});
  };
}
