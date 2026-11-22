import { describe, expect, it } from "vitest";
import { railFit, railScrollTop } from "./rail-fit";

describe("railFit", () => {
  it("counts whole rows and the room under the last one", () => {
    // Rows 46 px tall, 2 px apart: ten fit in 478 px, with 0 px to spare.
    expect(railFit(478, 46, 48)).toEqual({ rows: 10, rest: 0 });
    // Twenty px more is not room for an eleventh row.
    expect(railFit(498, 46, 48)).toEqual({ rows: 10, rest: 20 });
    expect(railFit(526, 46, 48)).toEqual({ rows: 11, rest: 0 });
  });

  it("fits nothing into a view it cannot measure", () => {
    expect(railFit(0, 46, 48)).toEqual({ rows: 0, rest: 0 });
    expect(railFit(30, 46, 48)).toEqual({ rows: 0, rest: 30 });
  });
});

describe("railScrollTop", () => {
  const fit = { rows: 10, rest: 20 };

  it("leaves a row that is in view where it is", () => {
    expect(railScrollTop(9, 12, 0, fit, 48)).toBe(0);
  });

  it("moves by whole rows to bring a row in from below or above", () => {
    expect(railScrollTop(10, 12, 0, fit, 48)).toBe(48);
    expect(railScrollTop(11, 12, 48, fit, 48)).toBe(96);
    expect(railScrollTop(0, 12, 96, fit, 48)).toBe(0);
  });

  it("puts an offset between rows back on a whole row", () => {
    expect(railScrollTop(5, 12, 30, fit, 48)).toBe(48);
  });
});
