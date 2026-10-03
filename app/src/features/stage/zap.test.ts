import { describe, expect, it } from "vitest";
import type {
  ChannelQuality,
  GuideWindowChannel,
} from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import { familyKey, type VariantPreferences } from "../guide/guide-families";
import {
  neighbouringZapStop,
  zapRailStops,
  zapSidesToRead,
  zapStops,
} from "./zap";
import type { ZapNeighbourhood } from "./zap-neighbourhood";

const NO_EXCLUSIONS: ReadonlySet<string> = new Set();
const NO_PREFERENCES: VariantPreferences = new Map();

describe("zap stops", () => {
  it("offers one stop per guide row, playing the preferred variant of each", () => {
    const news = channel("news");
    const sd = svt1("sd");
    const hd = svt1("hd");
    const film = channel("film");
    const rows = [news, sd, hd, film];

    expect(
      targets(
        zapStops(whole(rows), news.channel.id, NO_EXCLUSIONS, NO_PREFERENCES),
      ),
    ).toEqual(["news", "svt1-hd", "film"]);
    expect(
      targets(
        zapStops(
          whole(rows),
          news.channel.id,
          NO_EXCLUSIONS,
          new Map([[familyKey({ group: "Sweden", title: "SVT1" }), "sd"]]),
        ),
      ),
    ).toEqual(["news", "svt1-sd", "film"]);
  });

  it("keeps the playing Channel as the target of its own row, whatever is preferred", () => {
    const sd = svt1("sd");
    const hd = svt1("hd");

    expect(
      targets(
        zapStops(
          whole([sd, hd]),
          sd.channel.id,
          NO_EXCLUSIONS,
          NO_PREFERENCES,
        ),
      ),
    ).toEqual(["svt1-sd"]);
  });

  it("drops the rows of excluded Channel Groups, but never the playing row", () => {
    const news = channel("news", "News");
    const film = channel("film", "Cinema");
    const sport = channel("sport", "Sport");
    const late = channel("late", "Cinema");
    const rows = [news, film, sport, late];
    const excluded = new Set(["Cinema"]);

    expect(
      targets(zapStops(whole(rows), news.channel.id, excluded, NO_PREFERENCES)),
    ).toEqual(["news", "sport"]);
    expect(
      targets(zapStops(whole(rows), film.channel.id, excluded, NO_PREFERENCES)),
    ).toEqual(["news", "film", "sport"]);
  });

  it("offers nothing when the rows do not hold the playing Channel", () => {
    expect(
      zapStops(
        whole([channel("news"), channel("film")]),
        channel("sport").channel.id,
        NO_EXCLUSIONS,
        NO_PREFERENCES,
      ),
    ).toEqual([]);
  });

  it("leaves out a row at an end the rows do not reach: it may lack Quality Variants", () => {
    const news = channel("news");
    const film = channel("film");
    // The read stopped between the two variants of each end row.
    const rows = [svt1("hd"), news, film, tv4("sd")];
    const stops = (startReached: boolean, endReached: boolean) =>
      targets(
        zapStops(
          { rows, startReached, endReached },
          news.channel.id,
          NO_EXCLUSIONS,
          NO_PREFERENCES,
        ),
      );

    expect(stops(false, false)).toEqual(["news", "film"]);
    expect(stops(true, false)).toEqual(["svt1-hd", "news", "film"]);
    expect(stops(false, true)).toEqual(["news", "film", "tv4-sd"]);
    // The playing row stays wherever it is.
    expect(
      targets(
        zapStops(
          { rows, startReached: false, endReached: false },
          svt1("hd").channel.id,
          NO_EXCLUSIONS,
          NO_PREFERENCES,
        ),
      ),
    ).toEqual(["svt1-hd", "news", "film"]);
  });

  it("asks for further rows on a side with no stop while the catalog goes on there", () => {
    const news = channel("news", "News");
    const film = channel("film", "Cinema");
    const late = channel("late", "Cinema");
    const edge = channel("edge", "Sport");
    const rows = [news, film, late, edge];
    const sides = (
      neighbourhood: ZapNeighbourhood,
      from: GuideWindowChannel,
      excluded: ReadonlySet<string> = NO_EXCLUSIONS,
    ) =>
      zapSidesToRead(
        zapStops(neighbourhood, news.channel.id, excluded, NO_PREFERENCES),
        neighbourhood,
        from.channel.id,
      );
    const cut: ZapNeighbourhood = {
      rows,
      startReached: true,
      endReached: false,
    };

    // A stop lies ahead, and the catalog starts here: nothing to read.
    expect(sides(cut, news)).toEqual([]);
    // The last whole row: the next one is the cut end row.
    expect(sides(cut, late)).toEqual([1]);
    // Every row ahead is hidden.
    expect(sides(cut, news, new Set(["Cinema"]))).toEqual([1]);
    // The catalog ends with these rows.
    expect(sides(whole(rows), edge)).toEqual([]);
    expect(sides({ ...cut, startReached: false }, news)).toEqual([-1]);
    // A Channel that is not among the stops asks for nothing.
    expect(sides(cut, channel("elsewhere"))).toEqual([]);
  });

  it("steps to the row before or after, and stops at either end", () => {
    const sd = svt1("sd");
    const hd = svt1("hd");
    const stops = zapStops(
      whole([channel("news"), sd, hd, channel("film")]),
      sd.channel.id,
      NO_EXCLUSIONS,
      NO_PREFERENCES,
    );
    const step = (from: GuideWindowChannel, direction: -1 | 1) =>
      neighbouringZapStop(stops, from.channel.id, direction)?.target.channel
        .id ?? null;

    expect(step(sd, -1)).toBe("news");
    expect(step(sd, 1)).toBe("film");
    // Any variant of a row stands for the row.
    expect(step(hd, 1)).toBe("film");
    expect(step(channel("film"), -1)).toBe("svt1-sd");
    expect(step(channel("news"), -1)).toBeNull();
    expect(step(channel("film"), 1)).toBeNull();
    expect(step(channel("sport"), 1)).toBeNull();
  });

  it("shows six stops in the rail with the current one third where the list allows", () => {
    const rows = Array.from({ length: 9 }, (_, index) =>
      channel(`channel-${index}`),
    );
    const stops = zapStops(
      whole(rows),
      channel("channel-4").channel.id,
      NO_EXCLUSIONS,
      NO_PREFERENCES,
    );
    const rail = (current: string) =>
      targets(zapRailStops(stops, channel(current).channel.id)).map((id) =>
        Number(id.replace("channel-", "")),
      );

    expect(rail("channel-4")).toEqual([2, 3, 4, 5, 6, 7]);
    // Near either end the rail stays full and the current stop moves instead.
    expect(rail("channel-0")).toEqual([0, 1, 2, 3, 4, 5]);
    expect(rail("channel-1")).toEqual([0, 1, 2, 3, 4, 5]);
    expect(rail("channel-8")).toEqual([3, 4, 5, 6, 7, 8]);
    expect(
      targets(zapRailStops(stops.slice(0, 3), channel("channel-1").channel.id)),
    ).toEqual(["channel-0", "channel-1", "channel-2"]);
    expect(zapRailStops(stops, channel("elsewhere").channel.id)).toEqual([]);
  });
});

function channel(id: string, group = "General"): GuideWindowChannel {
  return row(channelFixture({ id, name: id, group }));
}

function svt1(quality: ChannelQuality): GuideWindowChannel {
  return row(
    channelFixture({
      id: `svt1-${quality}`,
      name: `SVT1 ${quality.toUpperCase()}`,
      group: "Sweden",
      number: 901,
      variant: { quality, baseName: "SVT1" },
    }),
  );
}

function tv4(quality: ChannelQuality): GuideWindowChannel {
  return row(
    channelFixture({
      id: `tv4-${quality}`,
      name: `TV4 ${quality.toUpperCase()}`,
      group: "Sweden",
      number: 904,
      variant: { quality, baseName: "TV4" },
    }),
  );
}

/** Rows that are the whole Channel Catalog. */
function whole(rows: readonly GuideWindowChannel[]): ZapNeighbourhood {
  return { rows, startReached: true, endReached: true };
}

function row(channel: GuideWindowChannel["channel"]): GuideWindowChannel {
  return { channel, programmes: [], programmesTruncated: false };
}

function targets(
  stops: readonly { readonly target: GuideWindowChannel }[],
): readonly string[] {
  return stops.map((stop) => stop.target.channel.id);
}
