import { describe, expect, it } from "vitest";
import { revealScrollTop } from "./list-scroll";

const ROW = 68;

describe("revealScrollTop", () => {
  it("puts the row in the middle of the view", () => {
    // Row 20 of a list of 68 px rows in a 500 px view: 216 px above and below it.
    expect(
      revealScrollTop({ top: 20 * ROW, height: ROW }, view(0), true),
    ).toBe(20 * ROW - 216);
    expect(
      revealScrollTop({ top: 20 * ROW, height: ROW }, view(3_000), true),
    ).toBe(20 * ROW - 216);
  });

  it("stops at the top for a row too near it to be centred", () => {
    expect(revealScrollTop({ top: ROW, height: ROW }, view(400), true)).toBe(0);
  });

  it("leaves a row that is wholly in view where it is, unless told to centre it", () => {
    const row = { top: 5 * ROW, height: ROW };
    expect(revealScrollTop(row, view(300), false)).toBe(300);
    expect(revealScrollTop(row, view(300), true)).toBe(5 * ROW - 216);
  });

  it("centres a row that is cut off at either edge of the view", () => {
    const row = { top: 5 * ROW, height: ROW };
    const centred = 5 * ROW - 216;
    // Its top is above the view, then its bottom below it.
    expect(revealScrollTop(row, view(5 * ROW + 1), false)).toBe(centred);
    expect(revealScrollTop(row, view(6 * ROW - 500 - 1), false)).toBe(centred);
  });
});

function view(scrollTop: number) {
  return { scrollTop, height: 500 };
}
