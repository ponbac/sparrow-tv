import { describe, expect, it } from "vitest";
import { clientSchemas, type ProgrammeSummary } from "../../client/contracts";
import {
  clockMarks,
  clockWindow,
  LATER_SPAN_MS,
  laterTimes,
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

  it("keeps the same anchor and lasts as long as the span it is given", () => {
    const now = new Date(2026, 8, 1, 20, 47, 59);
    const window = clockWindow(now, LATER_SPAN_MS);

    expect(window.startsAt).toEqual(clockWindow(now).startsAt);
    expect(window.endsAt.getTime() - window.startsAt.getTime()).toBe(
      8 * 60 * 60 * 1_000,
    );
  });

  it("offers the local full hours after now and before the window's end", () => {
    const now = new Date(2026, 8, 1, 20, 47, 59);
    // The longer window ends at 04:30 the next day, the usual one at 23:30.
    const laterEnd = clockWindow(now, LATER_SPAN_MS).endsAt;

    expect(localHours(laterTimes(now, clockWindow(now).endsAt))).toEqual([
      "1 21:00:00.0",
      "1 22:00:00.0",
      "1 23:00:00.0",
    ]);
    // Six at most unless asked for more; the hours run on over midnight.
    expect(localHours(laterTimes(now, laterEnd))).toEqual([
      "1 21:00:00.0",
      "1 22:00:00.0",
      "1 23:00:00.0",
      "2 0:00:00.0",
      "2 1:00:00.0",
      "2 2:00:00.0",
    ]);
    expect(localHours(laterTimes(now, laterEnd, 8)).slice(6)).toEqual([
      "2 3:00:00.0",
      "2 4:00:00.0",
    ]);
  });

  it("leaves out the hour that is now and the hour the window ends on", () => {
    const onTheHour = new Date(2026, 8, 1, 20, 0, 0);

    expect(
      localHours(laterTimes(onTheHour, new Date(2026, 8, 1, 23, 0, 0))),
    ).toEqual(["1 21:00:00.0", "1 22:00:00.0"]);
    expect(laterTimes(onTheHour, new Date(2026, 8, 1, 21, 0, 0))).toEqual([]);
    expect(laterTimes(onTheHour, new Date(2026, 8, 1, 23, 0, 0), 0)).toEqual(
      [],
    );
  });

  it("keeps to full hours when the clocks change by half an hour", () => {
    // Lord Howe Island moves its clocks half an hour: at 02:00 on this night
    // they read 02:30, and in April 02:00 comes an hour and a half after
    // 01:00.
    withTimeZone("Australia/Lord_Howe", () => {
      const spring = new Date(2026, 9, 4, 1, 15);
      const springHours = laterTimes(
        spring,
        clockWindow(spring, LATER_SPAN_MS).endsAt,
      );
      expect(localHours(springHours)).toEqual([
        "4 3:00:00.0",
        "4 4:00:00.0",
        "4 5:00:00.0",
        "4 6:00:00.0",
        "4 7:00:00.0",
        "4 8:00:00.0",
      ]);
      expect(springHours[0]?.toISOString()).toBe("2026-10-03T16:00:00.000Z");

      const autumn = new Date(2026, 3, 5, 0, 40);
      const autumnHours = laterTimes(
        autumn,
        clockWindow(autumn, LATER_SPAN_MS).endsAt,
        3,
      );
      expect(localHours(autumnHours)).toEqual([
        "5 1:00:00.0",
        "5 2:00:00.0",
        "5 3:00:00.0",
      ]);
      expect(autumnHours.map((time) => time.toISOString())).toEqual([
        "2026-04-04T14:00:00.000Z",
        "2026-04-04T15:30:00.000Z",
        "2026-04-04T16:30:00.000Z",
      ]);
    });
  });

  it.each(["Pacific/Chatham", "Asia/Kathmandu", "America/St_Johns", "UTC"])(
    "offers real instants on the full hour in %s, in order and once each",
    (zone) => {
      withTimeZone(zone, () => {
        const now = new Date(2026, 8, 1, 20, 47, 59);
        const windowEnd = clockWindow(now, LATER_SPAN_MS).endsAt;
        const times = laterTimes(now, windowEnd, 8);

        expect(localHours(times)).toEqual([
          "1 21:00:00.0",
          "1 22:00:00.0",
          "1 23:00:00.0",
          "2 0:00:00.0",
          "2 1:00:00.0",
          "2 2:00:00.0",
          "2 3:00:00.0",
          "2 4:00:00.0",
        ]);
        const instants = times.map((time) => time.getTime());
        expect(instants).toEqual([...new Set(instants)].sort((a, b) => a - b));
        expect(instants.every((instant) => instant > now.getTime())).toBe(true);
        expect(instants.every((instant) => instant < windowEnd.getTime())).toBe(
          true,
        );
      });
    },
  );

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

  it("marks a Programme live over a half-open interval", () => {
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

    expect(programmeLayout(ending, WINDOW, now)?.live).toBe(false);
    expect(programmeLayout(next, WINDOW, now)?.live).toBe(true);
  });

  it("keeps duplicate schedule entries distinct", () => {
    const duplicate = programme(
      "Untitled",
      "2026-09-01T21:00:00.000Z",
      "2026-09-01T21:30:00.000Z",
    );

    expect(programmeKey(duplicate, 0)).not.toBe(programmeKey(duplicate, 1));
  });

  it("clamps the playhead to the visible window", () => {
    expect(playheadPercent(WINDOW, new Date("2026-09-01T20:00:00.000Z"))).toBe(0);
    expect(playheadPercent(WINDOW, new Date("2026-09-02T00:00:00.000Z"))).toBe(100);
  });
});

/** Each instant as its local day of the month and clock time, to the millisecond. */
function localHours(times: readonly Date[]): readonly string[] {
  return times.map(
    (time) =>
      `${time.getDate()} ${time.getHours()}:${pad(time.getMinutes())}:${pad(time.getSeconds())}.${time.getMilliseconds()}`,
  );
}

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

/** Runs `body` with the local clock in another zone, then puts the zone back. */
function withTimeZone(zone: string, body: () => void): void {
  const previous = process.env.TZ;
  process.env.TZ = zone;
  try {
    body();
  } finally {
    if (previous === undefined) {
      delete process.env.TZ;
    } else {
      process.env.TZ = previous;
    }
  }
}

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
