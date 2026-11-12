// Session bootstrap: GET /api/session, PIN login, token persistence, reconnect auth refresh.
import { fetchSession, HttpError, login, type Role } from "./api";
import { connect, disconnect, setBeforeReconnect } from "./connection";
import { getApp, setApp } from "./store";

export type Route = "desktop" | "remote" | "view" | "stage";

export function routeFromPath(pathname: string): Route {
  const p = pathname.replace(/\/+$/, "");
  if (p === "/remote" || p.startsWith("/remote/")) return "remote";
  if (p === "/view" || p.startsWith("/view/")) return "view";
  if (p === "/stage" || p.startsWith("/stage/")) return "stage";
  return "desktop";
}

export const route: Route = typeof location === "undefined" ? "desktop" : routeFromPath(location.pathname);
/** Role the PIN pad asks for on this route. `/` from a non-loopback machine can only ever be `control`. */
const wantedRole: "control" | "view" = route === "view" || route === "stage" ? "view" : "control";

// One stored login per page: going from /view to /remote must ask for the
// control PIN instead of reusing the view token (and must not log /view out).
const KEY = `sm.session.${wantedRole}`;

interface Stored {
  token: string;
  role: Role;
}

function loadStored(): Stored | null {
  try {
    const raw = sessionStorage.getItem(KEY);
    if (!raw) return null;
    const v = JSON.parse(raw) as Partial<Stored>;
    return typeof v.token === "string" && typeof v.role === "string" ? { token: v.token, role: v.role as Role } : null;
  } catch {
    return null;
  }
}

function store(s: Stored | null): void {
  try {
    if (s) sessionStorage.setItem(KEY, JSON.stringify(s));
    else sessionStorage.removeItem(KEY);
  } catch {
    /* private mode etc. – the token then only lives in memory */
  }
}

function ready(token: string, role: Role): void {
  store({ token, role });
  setApp({ auth: "ready", token, role, kicked: false });
  connect();
}

/** A view login is not enough for the control page. */
function usable(role: Role | null | undefined): role is Role {
  return role != null && !(wantedRole === "control" && role === "view");
}

/** One attempt to obtain a usable token. Returns true when the app is (still) ready to connect. */
async function establish(): Promise<boolean> {
  const stored = loadStored() ?? (getApp().token && getApp().role ? { token: getApp().token!, role: getApp().role! } : null);
  const info = await fetchSession(stored?.token);
  setApp({ viewNeedsPin: info.view_needs_pin, loginRole: wantedRole });
  if (usable(info.role)) {
    ready(info.token, info.role);
    return true;
  }
  if (wantedRole === "view" && !info.view_needs_pin) {
    try {
      const res = await login("", "view");
      ready(res.token, res.role);
      return true;
    } catch (e) {
      if (!(e instanceof HttpError)) throw e;
    }
  }
  if (stored && usable(stored.role)) {
    // The server did not confirm the stored token via /api/session; the WebSocket decides.
    setApp({ auth: "ready", token: stored.token, role: stored.role });
    return true;
  }
  showPin();
  return false;
}

function showPin(): void {
  store(null);
  disconnect();
  setApp({ auth: "pin", token: null, role: null, doc: null, tick: null, everConnected: false, conn: "connecting" });
}

let wsAuthFailures = 0;

export async function boot(): Promise<void> {
  setBeforeReconnect(async (helloSeen) => {
    // Socket died. If it never said hello the token may be stale (server restart, PIN changed).
    if (helloSeen) {
      wsAuthFailures = 0;
      return true;
    }
    try {
      const stored = loadStored();
      const info = await fetchSession(stored?.token ?? getApp().token);
      if (usable(info.role)) {
        wsAuthFailures = 0;
        store({ token: info.token, role: info.role });
        setApp({ token: info.token, role: info.role });
        return true;
      }
      if (wantedRole === "view" && !info.view_needs_pin) {
        const res = await login("", "view");
        store(res);
        setApp({ token: res.token, role: res.role });
        return true;
      }
      // Server reachable, token not confirmed and the socket was refused: after two tries ask for the PIN.
      if (++wsAuthFailures >= 2) {
        wsAuthFailures = 0;
        showPin();
        return false;
      }
      return true;
    } catch {
      return true; // server unreachable: keep trying, the show goes on
    }
  });

  for (let delay = 250; ; delay = Math.min(2000, delay * 2)) {
    try {
      if (await establish()) connect();
      return;
    } catch {
      setApp({ auth: "unreachable" });
      await new Promise((r) => setTimeout(r, delay));
    }
  }
}

/** PIN pad submit. Resolves to an error text or null on success. */
export async function submitPin(pin: string): Promise<string | null> {
  try {
    const res = await login(pin, getApp().loginRole);
    ready(res.token, res.role);
    return null;
  } catch (e) {
    if (e instanceof HttpError && (e.status === 401 || e.status === 403)) return "Wrong PIN";
    if (e instanceof HttpError && e.status === 429) return "Too many attempts – wait a moment";
    return "Server not reachable";
  }
}

export function logout(): void {
  showPin();
}
