import { describe, expect, it } from "vitest";
import { clientSchemas, type ProgrammeSummary } from "../../client/contracts";
import {
  clockMarks,
  clockWindow,
  liveProgramme,
  playheadPercent,
  programmeKey,
  programmeLayout,
  type ClockWindow,
} from "./guide-window";

const WINDOW: ClockWindow = {
  startsAt: new Date("2026-09-01T20:30:00.000Z"),
  endsAt: new Date("2026-09-01T23:30:00.000Z"),
};

describe("guide window presentation", () => {
  it("anchors to a local half hour and keeps one stable three-hour window", () => {
    const window = clockWindow(new Date(2026, 8, 1, 20, 47, 59));

    expect(window.startsAt.getMinutes()).toBe(30);
    expect(window.startsAt.getSeconds()).toBe(0);
    expect(window.endsAt.getTime() - window.startsAt.getTime()).toBe(
      3 * 60 * 60 * 1_000,
    );
    expect(clockMarks(window)).toHaveLength(6);
  });

  it("clips overlaps at both edges and excludes exact non-overlaps", () => {
    const now = new Date("2026-09-01T21:00:00.000Z");
    const leadIn = programmeLayout(
      programme("Lead-in", "2026-09-01T20:00:00.000Z", "2026-09-01T21:00:00.000Z"),
      WINDOW,
      now,
    );
    const lateFilm = programmeLayout(
      programme("Late film", "2026-09-01T23:00:00.000Z", "2026-09-02T00:30:00.000Z"),
      WINDOW,
      now,
    );
    expect(leadIn).toMatchObject({ leftPercent: 0, live: false });
    expect(leadIn?.widthPercent).toBeCloseTo(100 / 6);
    expect(lateFilm?.leftPercent).toBeCloseTo(100 * (5 / 6));
    expect(lateFilm?.widthPercent).toBeCloseTo(100 / 6);
    expect(
      programmeLayout(
        programme("Already over", "2026-09-01T20:00:00.000Z", "2026-09-01T20:30:00.000Z"),
        WINDOW,
        now,
      ),
    ).toBeNull();
    expect(
      programmeLayout(
        programme("Starts after", "2026-09-01T23:30:00.000Z", "2026-09-02T00:00:00.000Z"),
        WINDOW,
        now,
      ),
    ).toBeNull();
  });

  it("uses half-open live intervals at the exact Programme end", () => {
    const ending = programme(
      "Bulletin",
      "2026-09-01T20:30:00.000Z",
      "2026-09-01T21:00:00.000Z",
    );
    const next = programme(
      "Studio",
      "2026-09-01T21:00:00.000Z",
      "2026-09-01T21:30:00.000Z",
    );
    const now = new Date("2026-09-01T21:00:00.000Z");

    expect(liveProgramme([ending, next], now)).toBe(next);
    expect(programmeLayout(ending, WINDOW, now)?.live).toBe(false);
    expect(programmeLayout(next, WINDOW, now)?.live).toBe(true);
  });

  it("finds no live Programme in a gap and keeps duplicate schedule entries distinct", () => {
    const duplicate = programme(
      "Untitled",
      "2026-09-01T21:00:00.000Z",
      "2026-09-01T21:30:00.000Z",
    );

    expect(
      liveProgramme([duplicate], new Date("2026-09-01T20:00:00.000Z")),
    ).toBeNull();
    expect(liveProgramme([], new Date())).toBeNull();
    expect(programmeKey(duplicate, 0)).not.toBe(programmeKey(duplicate, 1));
  });

  it("clamps the playhead to the visible window", () => {
    expect(playheadPercent(WINDOW, new Date("2026-09-01T20:00:00.000Z"))).toBe(0);
    expect(playheadPercent(WINDOW, new Date("2026-09-02T00:00:00.000Z"))).toBe(100);
  });
});

function programme(
  title: string,
  startsAt: string,
  endsAt: string,
): ProgrammeSummary {
  const page = clientSchemas.schedulePage.parse({
    generation: 1,
    items: [
      {
        channelId: "channel-1",
        title,
        description: null,
        startsAt,
        endsAt,
      },
    ],
    next: null,
  });
  const result = page.items[0];
  if (result === undefined) {
    throw new Error("Programme fixture did not parse");
  }
  return result;
}
