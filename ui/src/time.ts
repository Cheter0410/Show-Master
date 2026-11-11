// Time / level formatting helpers. Pure functions, unit-tested in time.test.ts.

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

/** 75.3 -> "1:15.3", 3723 -> "1:02:03.0"; null/NaN -> "–". Tenths are truncated, never rounded up. */
export function formatDuration(seconds: number | null | undefined, tenths = true): string {
  if (seconds == null || !Number.isFinite(seconds)) return "–";
  const neg = seconds < 0;
  const total = Math.floor(Math.abs(seconds) * 10 + 1e-6);
  const t = total % 10;
  const whole = Math.floor(total / 10);
  const s = whole % 60;
  const m = Math.floor(whole / 60) % 60;
  const h = Math.floor(whole / 3600);
  let out = h > 0 ? `${h}:${pad2(m)}:${pad2(s)}` : `${m}:${pad2(s)}`;
  if (tenths) out += `.${t}`;
  return neg ? `-${out}` : out;
}

/** Always HH:MM:SS (show clock); null -> "--:--:--". */
export function formatClock(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds)) return "--:--:--";
  const whole = Math.floor(Math.max(0, seconds));
  return `${pad2(Math.floor(whole / 3600))}:${pad2(Math.floor(whole / 60) % 60)}:${pad2(whole % 60)}`;
}

/** Local wall clock HH:MM:SS from epoch milliseconds. */
export function formatWallClock(ms: number): string {
  const d = new Date(ms);
  return `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
}

/** Countdown that rounds up, so it reaches 0:00 exactly when the time is over. */
export function formatCountdown(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds)) return "–";
  const whole = Math.ceil(Math.max(0, seconds) - 1e-6);
  const h = Math.floor(whole / 3600);
  const m = Math.floor(whole / 60) % 60;
  const s = whole % 60;
  return h > 0 ? `${h}:${pad2(m)}:${pad2(s)}` : `${m}:${pad2(s)}`;
}

/** Compact value for table cells: 0 -> "", 2.5 -> "2.5", 93.2 -> "1:33.2". */
export function formatSeconds(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds === 0) return "";
  if (Math.abs(seconds) < 60) return String(Math.round(seconds * 100) / 100);
  return formatDuration(seconds);
}

/**
 * Parses user input for a duration: "12.5", "1:30", "1:02:03.5", also with comma decimals.
 * Returns seconds, or null if the text is not a valid non-negative time.
 */
export function parseDuration(text: string): number | null {
  const s = text.trim().replace(",", ".");
  if (s === "") return null;
  const parts = s.split(":");
  if (parts.length > 3) return null;
  let total = 0;
  for (let i = 0; i < parts.length; i++) {
    const part = parts[i];
    const isLast = i === parts.length - 1;
    if (!(isLast ? /^\d+(\.\d+)?$|^\.\d+$/ : /^\d+$/).test(part)) return null;
    const v = Number(part);
    if (i > 0 && parts.length > 1 && v >= 60) return null;
    total = total * 60 + v;
  }
  return Number.isFinite(total) ? total : null;
}

export function nominalFps(rate: string): number {
  switch (rate) {
    case "23.976":
    case "24":
      return 24;
    case "25":
      return 25;
    default:
      return 30; // 29.97, 29.97df, 30
  }
}

export interface ParsedTimecode {
  h: number;
  m: number;
  s: number;
  f: number;
}

/** Strict HH:MM:SS:FF (drop-frame may use ';' before frames). */
export function parseTimecode(text: string): ParsedTimecode | null {
  const m = /^(\d{2}):(\d{2}):(\d{2})[:;](\d{2})$/.exec(text.trim());
  if (!m) return null;
  return { h: Number(m[1]), m: Number(m[2]), s: Number(m[3]), f: Number(m[4]) };
}

/** Returns an error message, or null when `text` is a valid timecode for `rate`. */
export function validateTimecode(text: string, rate: string): string | null {
  const tc = parseTimecode(text);
  if (!tc) return "Format must be HH:MM:SS:FF";
  if (tc.h > 23) return "Hours must be 00–23";
  if (tc.m > 59) return "Minutes must be 00–59";
  if (tc.s > 59) return "Seconds must be 00–59";
  const fps = nominalFps(rate);
  if (tc.f >= fps) return `Frames must be 00–${pad2(fps - 1)} at ${rate} fps`;
  if (rate === "29.97df" && tc.s === 0 && tc.f < 2 && tc.m % 10 !== 0) {
    return "Frames 00 and 01 do not exist at this position in drop-frame";
  }
  return null;
}

/** Normalises loose input ("1:0:10:0", "01001000") to HH:MM:SS:FF; null if hopeless. */
export function normalizeTimecode(text: string): string | null {
  const s = text.trim();
  if (/^\d{8}$/.test(s)) return `${s.slice(0, 2)}:${s.slice(2, 4)}:${s.slice(4, 6)}:${s.slice(6, 8)}`;
  const parts = s.split(/[:;.]/);
  if (parts.length !== 4 || parts.some((p) => !/^\d{1,2}$/.test(p))) return null;
  return parts.map((p) => p.padStart(2, "0")).join(":");
}

export const DB_FLOOR = -120;

/** Linear amplitude -> dBFS, floored at -120. */
export function linToDb(lin: number): number {
  if (!(lin > 0)) return DB_FLOOR;
  return Math.max(DB_FLOOR, 20 * Math.log10(lin));
}

export function formatDb(db: number | null | undefined): string {
  if (db == null || !Number.isFinite(db)) return "–";
  if (db <= DB_FLOOR) return "-∞";
  const r = Math.round(db * 10) / 10;
  return `${r > 0 ? "+" : ""}${r.toFixed(1)}`;
}

/** Parses "-6", "+3.5", "-inf", "off"; null if invalid. Result is clamped to [-120, +24]. */
export function parseDb(text: string): number | null {
  const s = text.trim().toLowerCase().replace(",", ".").replace(/\s*db$/, "");
  if (s === "-inf" || s === "-∞" || s === "off" || s === "inf") return DB_FLOOR;
  if (!/^[+-]?(\d+(\.\d*)?|\.\d+)$/.test(s)) return null;
  return Math.min(24, Math.max(DB_FLOOR, Number(s)));
}

/** Validates a SysEx hex string: returns normalised "F0 .. F7" or an error. */
export function validateSysex(text: string): { ok: true; hex: string } | { ok: false; error: string } {
  const compact = text.replace(/[\s,]/g, "").toUpperCase();
  if (compact === "") return { ok: false, error: "Empty message" };
  if (!/^[0-9A-F]+$/.test(compact)) return { ok: false, error: "Only hex digits 0–9 / A–F allowed" };
  if (compact.length % 2 !== 0) return { ok: false, error: "Odd number of hex digits" };
  const bytes = compact.match(/../g) ?? [];
  if (bytes[0] !== "F0") return { ok: false, error: "Must start with F0" };
  if (bytes[bytes.length - 1] !== "F7") return { ok: false, error: "Must end with F7" };
  for (let i = 1; i < bytes.length - 1; i++) {
    if (parseInt(bytes[i], 16) > 0x7f) return { ok: false, error: `Data byte ${i + 1} (${bytes[i]}) is above 7F` };
  }
  return { ok: true, hex: bytes.join(" ") };
}
