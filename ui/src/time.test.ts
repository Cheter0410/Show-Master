import { describe, expect, it } from "vitest";
import {
  formatClock, formatCountdown, formatDb, formatDuration, formatSeconds, linToDb, normalizeTimecode, parseDb,
  parseDuration, parseTimecode, validateSysex, validateTimecode,
} from "./time";

describe("formatDuration", () => {
  it("formats minutes, seconds and tenths", () => {
    expect(formatDuration(0)).toBe("0:00.0");
    expect(formatDuration(75.3)).toBe("1:15.3");
    expect(formatDuration(59.99)).toBe("0:59.9"); // truncates, never shows 0:60.0
    expect(formatDuration(3723)).toBe("1:02:03.0");
    expect(formatDuration(75.3, false)).toBe("1:15");
  });
  it("handles float noise, negatives and missing values", () => {
    expect(formatDuration(0.3)).toBe("0:00.3");
    expect(formatDuration(1.1 + 2.2)).toBe("0:03.3");
    expect(formatDuration(-5)).toBe("-0:05.0");
    expect(formatDuration(null)).toBe("–");
    expect(formatDuration(Number.NaN)).toBe("–");
  });
});

describe("clocks", () => {
  it("formatClock is always HH:MM:SS", () => {
    expect(formatClock(0)).toBe("00:00:00");
    expect(formatClock(3661.9)).toBe("01:01:01");
    expect(formatClock(null)).toBe("--:--:--");
  });
  it("formatCountdown rounds up", () => {
    expect(formatCountdown(3.2)).toBe("0:04");
    expect(formatCountdown(0.01)).toBe("0:01");
    expect(formatCountdown(0)).toBe("0:00");
    expect(formatCountdown(60)).toBe("1:00");
    expect(formatCountdown(3600)).toBe("1:00:00");
    expect(formatCountdown(null)).toBe("–");
  });
  it("formatSeconds is compact", () => {
    expect(formatSeconds(0)).toBe("");
    expect(formatSeconds(2.5)).toBe("2.5");
    expect(formatSeconds(93.2)).toBe("1:33.2");
  });
});

describe("parseDuration", () => {
  it("accepts seconds and colon forms", () => {
    expect(parseDuration("12.5")).toBe(12.5);
    expect(parseDuration("12,5")).toBe(12.5);
    expect(parseDuration("1:30")).toBe(90);
    expect(parseDuration("1:02:03.5")).toBe(3723.5);
    expect(parseDuration(" 90 ")).toBe(90);
    expect(parseDuration(".5")).toBe(0.5);
  });
  it("rejects garbage", () => {
    expect(parseDuration("")).toBeNull();
    expect(parseDuration("abc")).toBeNull();
    expect(parseDuration("-3")).toBeNull();
    expect(parseDuration("1:75")).toBeNull();
    expect(parseDuration("1:2:3:4")).toBeNull();
    expect(parseDuration("1:")).toBeNull();
  });
});

describe("timecode", () => {
  it("parses strict HH:MM:SS:FF", () => {
    expect(parseTimecode("01:00:10:00")).toEqual({ h: 1, m: 0, s: 10, f: 0 });
    expect(parseTimecode("01:00:10;02")).toEqual({ h: 1, m: 0, s: 10, f: 2 });
    expect(parseTimecode("1:00:10:00")).toBeNull();
    expect(parseTimecode("01:00:10")).toBeNull();
  });
  it("validates ranges against the frame rate", () => {
    expect(validateTimecode("01:00:10:00", "25")).toBeNull();
    expect(validateTimecode("23:59:59:24", "25")).toBeNull();
    expect(validateTimecode("00:00:00:25", "25")).toMatch(/Frames/);
    expect(validateTimecode("00:00:00:29", "30")).toBeNull();
    expect(validateTimecode("00:00:00:24", "24")).toMatch(/Frames/);
    expect(validateTimecode("00:00:00:23", "23.976")).toBeNull();
    expect(validateTimecode("24:00:00:00", "25")).toMatch(/Hours/);
    expect(validateTimecode("00:60:00:00", "25")).toMatch(/Minutes/);
    expect(validateTimecode("00:00:60:00", "25")).toMatch(/Seconds/);
    expect(validateTimecode("nonsense", "25")).toMatch(/Format/);
  });
  it("knows drop-frame holes", () => {
    expect(validateTimecode("00:01:00:00", "29.97df")).toMatch(/drop-frame/);
    expect(validateTimecode("00:01:00:02", "29.97df")).toBeNull();
    expect(validateTimecode("00:10:00:00", "29.97df")).toBeNull();
    expect(validateTimecode("00:01:00:00", "29.97")).toBeNull();
  });
  it("normalises loose input", () => {
    expect(normalizeTimecode("1:0:10:0")).toBe("01:00:10:00");
    expect(normalizeTimecode("01001000")).toBe("01:00:10:00");
    expect(normalizeTimecode("1.2.3.4")).toBe("01:02:03:04");
    expect(normalizeTimecode("1:2:3")).toBeNull();
    expect(normalizeTimecode("a:b:c:d")).toBeNull();
  });
});

describe("dB helpers", () => {
  it("converts linear to dB", () => {
    expect(linToDb(1)).toBeCloseTo(0);
    expect(linToDb(0.5)).toBeCloseTo(-6.02, 1);
    expect(linToDb(0)).toBe(-120);
    expect(linToDb(-1)).toBe(-120);
  });
  it("formats and parses", () => {
    expect(formatDb(0)).toBe("0.0");
    expect(formatDb(3)).toBe("+3.0");
    expect(formatDb(-6.04)).toBe("-6.0");
    expect(formatDb(-120)).toBe("-∞");
    expect(formatDb(null)).toBe("–");
    expect(parseDb("-6")).toBe(-6);
    expect(parseDb("+3,5 dB")).toBe(3.5);
    expect(parseDb("-inf")).toBe(-120);
    expect(parseDb("off")).toBe(-120);
    expect(parseDb("-500")).toBe(-120);
    expect(parseDb("99")).toBe(24);
    expect(parseDb("loud")).toBeNull();
  });
});

describe("validateSysex", () => {
  it("accepts and normalises valid messages", () => {
    expect(validateSysex("f07f7f0401007ff7")).toEqual({ ok: true, hex: "F0 7F 7F 04 01 00 7F F7" });
    expect(validateSysex("F0 01, 02 F7")).toEqual({ ok: true, hex: "F0 01 02 F7" });
  });
  it("rejects malformed messages", () => {
    expect(validateSysex("")).toMatchObject({ ok: false });
    expect(validateSysex("F0 0G F7")).toMatchObject({ ok: false });
    expect(validateSysex("F0 1 F7")).toMatchObject({ ok: false });
    expect(validateSysex("01 02 F7")).toMatchObject({ ok: false, error: "Must start with F0" });
    expect(validateSysex("F0 01 02")).toMatchObject({ ok: false, error: "Must end with F7" });
    expect(validateSysex("F0 80 F7")).toMatchObject({ ok: false });
  });
});
