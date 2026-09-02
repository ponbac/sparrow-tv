import { describe, expect, it } from "vitest";
import { canonicalSearchTerm, searchTermFits } from "./board-search-term";

describe("board-search-term", () => {
  it("canonicalizes padding, case, and interior whitespace", () => {
    expect(canonicalSearchTerm("  News   ROOM  ")).toBe("news room");
  });

  it("rejects terms that overflow the catalog byte budget", () => {
    expect(searchTermFits("news")).toBe(true);
    expect(searchTermFits("x".repeat(256))).toBe(true);
    expect(searchTermFits("x".repeat(257))).toBe(false);
    expect(searchTermFits(`${"é".repeat(128)}x`)).toBe(false);
  });
});
