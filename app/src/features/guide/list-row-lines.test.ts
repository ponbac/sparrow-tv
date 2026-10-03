import { describe, expect, it } from "vitest";
import { clientSchemas, type GuideProgramme } from "../../client/contracts";
import { listRowLines } from "./list-row-lines";

const QUIZ = programme("Quiz", "20:00", "20:30");
const FILM = programme("Film", "20:30", "21:30");
const NEWS = programme("News", "22:00", "22:30");
const EVENING = [QUIZ, FILM, NEWS];

describe("listRowLines now", () => {
  it("titles the row by the live Programme, with what follows and how far it has got", () => {
    expect(lines({ programmes: EVENING, now: at("20:45") })).toEqual({
      _tag: "programme",
      title: "Film",
      after: "22:00 News",
      live: { minutesLeft: 45, elapsed: 0.25 },
    });
  });

  it("says nothing follows the last Programme", () => {
    expect(lines({ programmes: EVENING, now: at("22:10") })).toEqual({
      _tag: "programme",
      title: "News",
      after: null,
      live: { minutesLeft: 20, elapsed: 1 / 3 },
    });
  });

  it("titles the row by its Channel between Programmes, and still names the next one", () => {
    expect(lines({ programmes: EVENING, now: at("21:45") })).toEqual({
      _tag: "channel",
      after: "22:00 News",
    });
    expect(lines({ programmes: EVENING, now: at("23:00") })).toEqual({
      _tag: "channel",
      after: null,
    });
  });

  it("says a Channel without Programmes still plays, also while a longer read is pending", () => {
    const noData = {
      _tag: "channel",
      after: "No guide data. The channel still plays.",
    };
    expect(lines({ programmes: [], now: at("20:45") })).toEqual(noData);
    expect(
      lines({ programmes: [], now: at("20:45"), pending: true }),
    ).toEqual(noData);
  });

  it("marks a title the catalog cut short", () => {
    const cut = { ...FILM, titleTruncated: true };
    const cutNext = { ...NEWS, titleTruncated: true };
    expect(
      lines({ programmes: [QUIZ, cut, cutNext], now: at("20:45") }),
    ).toMatchObject({ title: "Film…", after: "22:00 News…" });
  });
});

describe("listRowLines at a later time", () => {
  it("titles the row by the Programme on then, with its times and no progress", () => {
    expect(
      lines({ programmes: EVENING, now: at("20:45"), at: at("22:00") }),
    ).toEqual({
      _tag: "programme",
      title: "News",
      after: "22:00 to 22:30",
      live: null,
    });
    // The live Programme is still what is on at a time before it ends.
    expect(
      lines({ programmes: EVENING, now: at("20:45"), at: at("21:00") }),
    ).toEqual({
      _tag: "programme",
      title: "Film",
      after: "20:30 to 21:30",
      live: null,
    });
  });

  it("says when nothing is listed at that time", () => {
    expect(
      lines({ programmes: EVENING, now: at("20:45"), at: at("23:00") }),
    ).toEqual({ _tag: "channel", after: "Nothing listed at 23:00" });
    expect(
      lines({ programmes: [], now: at("20:45"), at: at("23:00") }),
    ).toEqual({
      _tag: "channel",
      after: "No guide data. The channel still plays.",
    });
  });

  it("says the guide is not loaded for a time past the Programmes the catalog kept", () => {
    const cut = { programmes: EVENING, truncated: true, now: at("20:45") };
    const notLoaded = {
      _tag: "channel",
      after: "The guide for this time is not loaded.",
    };

    // From the last kept Programme's start on, others may have been left out.
    expect(lines({ ...cut, at: at("23:00") })).toEqual(notLoaded);
    expect(lines({ ...cut, at: at("22:30") })).toEqual(notLoaded);
    // What was kept is still shown, and a gap before the last of it is known
    // to be empty.
    expect(lines({ ...cut, at: at("22:00") })).toMatchObject({ title: "News" });
    expect(lines({ ...cut, at: at("21:45") })).toEqual({
      _tag: "channel",
      after: "Nothing listed at 21:45",
    });
    // A pending read may still bring the time: nothing is said about it yet.
    expect(lines({ ...cut, at: at("23:00"), pending: true })).toEqual({
      _tag: "channel",
      after: null,
    });
    // Now is never past the earliest Programmes.
    expect(lines({ ...cut, now: at("23:00") })).toEqual({
      _tag: "channel",
      after: null,
    });
  });

  it("claims nothing about a time the pending read may still cover", () => {
    expect(
      lines({
        programmes: EVENING,
        now: at("20:45"),
        at: at("23:00"),
        pending: true,
      }),
    ).toEqual({ _tag: "channel", after: null });
    expect(
      lines({ programmes: [], now: at("20:45"), at: at("23:00"), pending: true }),
    ).toEqual({ _tag: "channel", after: null });
    // What is already known is shown.
    expect(
      lines({
        programmes: EVENING,
        now: at("20:45"),
        at: at("22:00"),
        pending: true,
      }),
    ).toMatchObject({ title: "News" });
  });
});

function lines(input: {
  readonly programmes: readonly GuideProgramme[];
  readonly truncated?: boolean;
  readonly now: Date;
  readonly at?: Date;
  readonly pending?: boolean;
}) {
  return listRowLines({
    programmes: input.programmes,
    truncated: input.truncated ?? false,
    now: input.now,
    at: input.at ?? null,
    pending: input.pending ?? false,
  });
}

/** A local clock time on one fixed evening, so labels read the same in any zone. */
function at(time: string): Date {
  const [hours = 0, minutes = 0] = time.split(":").map(Number);
  return new Date(2026, 8, 1, hours, minutes);
}

function programme(title: string, startsAt: string, endsAt: string): GuideProgramme {
  return {
    title,
    titleTruncated: false,
    startsAt: clientSchemas.isoInstant.parse(at(startsAt).toISOString()),
    endsAt: clientSchemas.isoInstant.parse(at(endsAt).toISOString()),
  };
}
