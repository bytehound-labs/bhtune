import { describe, expect, it } from "vitest";
import { deriveTag } from "./opcTags";

describe("deriveTag", () => {
  it.each([
    ["Unit1.LIC101.PV", "OP", "Unit1.LIC101.OP"],
    ["Unit1!LIC101!PV", "OP", "Unit1!LIC101!OP"],
    ["FCS0201/Control/PV", "MV", "FCS0201/Control/MV"],
    ["A.B!C", "X", "A.X"],
    ["A!B/C", "X", "A!X"],
    ["LIC101PV", "OP", "OP"],
    ["", "OP", "OP"],
  ])("matches Rust derive_tag(%j, %j)", (tag, suffix, expected) => {
    expect(deriveTag(tag, suffix)).toBe(expected);
  });

  it.each(["", "  ", "\t\n"])(
    "returns null for a blank suffix (%j), as Rust derive_tag does",
    (suffix) => {
      expect(deriveTag("Unit1.LIC101.PV", suffix)).toBeNull();
    },
  );
});
