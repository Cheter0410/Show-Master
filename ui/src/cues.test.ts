import { describe, expect, it } from "vitest";
import type { Cue, CueList } from "./api";
import { cueCount, stepMove, topLevelSelection } from "./cues";

const memo = (id: number): Cue => ({ id, number: String(id), name: "", notes: "", color: "", pre_wait: 0, post_wait: 0, continue_mode: "do_not_continue", armed: true, tc_trigger: null, kind: { type: "memo" } }) as Cue;
const group = (id: number, children: Cue[]): Cue => ({ ...memo(id), kind: { type: "group", mode: "parallel", children } }) as unknown as Cue;
// 1, 2, G3 [31, 32, 33], 4
const list: CueList = { id: 1, name: "Main", tc_jump: "skip", cues: [memo(1), memo(2), group(3, [memo(31), memo(32), memo(33)]), memo(4)] };

describe("topLevelSelection", () => {
  it("keeps list order and drops children of selected groups", () => {
    expect(topLevelSelection(list, [4, 31, 3, 1])).toEqual([1, 3, 4]);
    expect(topLevelSelection(list, [32, 2])).toEqual([2, 32]);
  });
  it("drops unknown ids and handles empty input", () => {
    expect(topLevelSelection(list, [99])).toEqual([]);
    expect(topLevelSelection(null, [1])).toEqual([]);
  });
});

describe("stepMove", () => {
  it("moves up among siblings (index before removal)", () => {
    expect(stepMove(list, 2, -1)).toEqual({ parent: null, index: 0 });
    expect(stepMove(list, 32, -1)).toEqual({ parent: 3, index: 0 });
  });
  it("moves down: index skips past the next sibling", () => {
    expect(stepMove(list, 1, 1)).toEqual({ parent: null, index: 2 });
    expect(stepMove(list, 31, 1)).toEqual({ parent: 3, index: 2 });
  });
  it("returns null at either end of the container", () => {
    expect(stepMove(list, 1, -1)).toBeNull();
    expect(stepMove(list, 4, 1)).toBeNull();
    expect(stepMove(list, 33, 1)).toBeNull();
    expect(stepMove(list, 31, -1)).toBeNull();
    expect(stepMove(list, 99, 1)).toBeNull();
  });
});

describe("cueCount", () => {
  it("pluralises", () => {
    expect(cueCount(1)).toBe("1 cue");
    expect(cueCount(3)).toBe("3 cues");
  });
});
