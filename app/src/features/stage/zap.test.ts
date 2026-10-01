import { describe, expect, it } from "vitest";
import type {
  ChannelQuality,
  GuideWindowChannel,
} from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import { familyKey, type VariantPreferences } from "../guide/guide-families";
import { neighbouringZapStop, zapRailStops, zapStops } from "./zap";

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
      targets(zapStops(rows, news.channel.id, NO_EXCLUSIONS, NO_PREFERENCES)),
    ).toEqual(["news", "svt1-hd", "film"]);
    expect(
      targets(
        zapStops(
          rows,
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
      targets(zapStops([sd, hd], sd.channel.id, NO_EXCLUSIONS, NO_PREFERENCES)),
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
      targets(zapStops(rows, news.channel.id, excluded, NO_PREFERENCES)),
    ).toEqual(["news", "sport"]);
    expect(
      targets(zapStops(rows, film.channel.id, excluded, NO_PREFERENCES)),
    ).toEqual(["news", "film", "sport"]);
  });

  it("offers nothing when the rows do not hold the playing Channel", () => {
    expect(
      zapStops(
        [channel("news"), channel("film")],
        channel("sport").channel.id,
        NO_EXCLUSIONS,
        NO_PREFERENCES,
      ),
    ).toEqual([]);
  });

  it("steps to the row before or after, and stops at either end", () => {
    const sd = svt1("sd");
    const hd = svt1("hd");
    const stops = zapStops(
      [channel("news"), sd, hd, channel("film")],
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
      rows,
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

function row(channel: GuideWindowChannel["channel"]): GuideWindowChannel {
  return { channel, programmes: [], programmesTruncated: false };
}

function targets(
  stops: readonly { readonly target: GuideWindowChannel }[],
): readonly string[] {
  return stops.map((stop) => stop.target.channel.id);
}
