import { describe, expect, it } from "vitest";
import { formatOscArgs, parseOscArgs, validateOscAddress } from "./osc";

describe("osc args", () => {
  it("parses like the Rust side", () => {
    expect(parseOscArgs('1 -2 0.5 true false hello "two words" "12" "q\\"x"')).toEqual([1, -2, 0.5, true, false, "hello", "two words", "12", 'q"x']);
  });
  it("round-trips", () => {
    const a = [3, 0.25, false, "x y", "7"];
    expect(parseOscArgs(formatOscArgs(a))).toEqual(a);
  });
  it("validates addresses", () => {
    expect(validateOscAddress("/a/b")).toBeNull();
    expect(validateOscAddress("a")).not.toBeNull();
    expect(validateOscAddress("/a b")).not.toBeNull();
  });
});
