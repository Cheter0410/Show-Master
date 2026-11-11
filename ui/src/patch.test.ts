import { describe, expect, it } from "vitest";
import { applyPatch } from "./patch";

const base = () => ({
  mode: "edit",
  show: { name: "Old", settings: { go_debounce_ms: 250 }, cue_lists: [{ id: 1, cues: [{ id: 5, name: "A" }, { id: 6, name: "B" }] }] },
  playheads: { "1": 5 } as Record<string, number | null>,
  errors: { "17": "file not found" } as Record<string, string>,
});

describe("applyPatch", () => {
  it("sets a nested value", () => {
    const doc = base();
    const out = applyPatch(doc, [{ p: ["show", "name"], v: "New" }]);
    expect(out.show.name).toBe("New");
    expect(doc.show.name).toBe("Old"); // input untouched
  });

  it("deletes a key", () => {
    const out = applyPatch(base(), [{ p: ["errors", "17"], d: true }]);
    expect(out.errors).toEqual({});
  });

  it("creates missing keys and intermediate objects", () => {
    const out = applyPatch(base(), [
      { p: ["errors", "9"], v: "broken" },
      { p: ["loaded", "9"], v: 30 },
    ]) as ReturnType<typeof base> & { loaded: Record<string, number> };
    expect(out.errors).toEqual({ "17": "file not found", "9": "broken" });
    expect(out.loaded).toEqual({ "9": 30 });
  });

  it("uses structural sharing: untouched branches keep identity", () => {
    const doc = base();
    const out = applyPatch(doc, [{ p: ["playheads", "1"], v: 6 }]);
    expect(out).not.toBe(doc);
    expect(out.playheads).not.toBe(doc.playheads);
    expect(out.show).toBe(doc.show);
    expect(out.errors).toBe(doc.errors);
  });

  it("addresses array elements by index and replaces arrays as a whole", () => {
    const doc = base();
    const a = applyPatch(doc, [{ p: ["show", "cue_lists", 0, "cues", 1, "name"], v: "B2" }]);
    expect(a.show.cue_lists[0].cues[1].name).toBe("B2");
    expect(a.show.cue_lists[0].cues[0]).toBe(doc.show.cue_lists[0].cues[0]);
    expect(Array.isArray(a.show.cue_lists[0].cues)).toBe(true);
    const b = applyPatch(doc, [{ p: ["show", "cue_lists", 0, "cues"], v: [{ id: 7, name: "C" }] }]);
    expect(b.show.cue_lists[0].cues).toEqual([{ id: 7, name: "C" }]);
  });

  it("applies ops in order", () => {
    const out = applyPatch(base(), [
      { p: ["playheads", "1"], v: null },
      { p: ["playheads", "2"], v: 9 },
      { p: ["playheads", "2"], d: true },
      { p: ["mode"], v: "live" },
    ]);
    expect(out.playheads).toEqual({ "1": null });
    expect(out.mode).toBe("live");
  });

  it("deleting something that does not exist is a no-op", () => {
    const doc = base();
    const out = applyPatch(doc, [{ p: ["errors", "404"], d: true }, { p: ["nope", "x"], d: true }]);
    expect(out.errors).toBe(doc.errors);
    expect("nope" in out).toBe(false);
  });

  it("an empty path replaces the document; null is a valid value", () => {
    expect(applyPatch({ a: 1 }, [{ p: [], v: { b: 2 } }])).toEqual({ b: 2 });
    expect(applyPatch({ a: 1 } as { a: number | null }, [{ p: ["a"], v: null }])).toEqual({ a: null });
  });

  it("ignores malformed ops", () => {
    const doc = base();
    // @ts-expect-error deliberately malformed
    expect(applyPatch(doc, [{ v: 1 }])).toBe(doc);
  });
});
