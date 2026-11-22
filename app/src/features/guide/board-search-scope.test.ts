import { describe, expect, it } from "vitest";
import { clientSchemas } from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import {
  foldSearchVariants,
  shouldAdvancePastExcludedSearchHits,
  visibleSearchChannels,
  visibleSearchProgrammes,
} from "./board-search-scope";
import { familyKey } from "./guide-families";

const WORLD_NEWS = channelFixture({
  id: "world-news",
  name: "World News",
  group: "News",
});
const CINEMA_ONE = channelFixture({
  id: "cinema-one",
  name: "Cinema One",
  group: "Cinema",
});
const PROGRAMME_RESULTS = clientSchemas.searchResults.parse({
  generation: 7,
  channels: { generation: 7, items: [], next: null },
  programmes: {
    generation: 7,
    items: [
      {
        channel: WORLD_NEWS,
        title: "Evening Studio",
        titleTruncated: false,
        startsAt: "2026-09-01T20:30:00Z",
        endsAt: "2026-09-01T21:00:00Z",
      },
      {
        channel: CINEMA_ONE,
        title: "Late Feature",
        titleTruncated: false,
        startsAt: "2026-09-01T21:00:00Z",
        endsAt: "2026-09-01T23:00:00Z",
      },
    ],
    next: null,
  },
});
const EVENING_STUDIO = PROGRAMME_RESULTS.programmes.items[0];
const LATE_FEATURE = PROGRAMME_RESULTS.programmes.items[1];

describe("board-search-scope", () => {
  it("keeps every hit when no groups are excluded or inclusion is requested", () => {
    const channels = [WORLD_NEWS, CINEMA_ONE];
    const programmes = [EVENING_STUDIO, LATE_FEATURE];
    const excluded = new Set(["News"]);
    expect(visibleSearchChannels(channels, new Set(), false)).toBe(channels);
    expect(visibleSearchProgrammes(programmes, new Set(), false)).toBe(
      programmes,
    );
    expect(visibleSearchChannels(channels, excluded, true)).toBe(channels);
    expect(visibleSearchProgrammes(programmes, excluded, true)).toBe(
      programmes,
    );
  });

  it("drops Channels and Programme hits from excluded groups", () => {
    const excluded = new Set(["News"]);
    expect(
      visibleSearchChannels([WORLD_NEWS, CINEMA_ONE], excluded, false).map(
        (channel) => channel.id,
      ),
    ).toEqual(["cinema-one"]);
    expect(
      visibleSearchProgrammes(
        [EVENING_STUDIO, LATE_FEATURE],
        excluded,
        false,
      ).map((programme) => programme.title),
    ).toEqual(["Late Feature"]);
  });

  it("advances only when a received search page is entirely excluded", () => {
    expect(
      shouldAdvancePastExcludedSearchHits({
        includeExcluded: false,
        excludedCount: 1,
        receivedCount: 40,
        visibleCount: 0,
        hasMore: true,
        loading: false,
      }),
    ).toBe(true);
    expect(
      shouldAdvancePastExcludedSearchHits({
        includeExcluded: true,
        excludedCount: 1,
        receivedCount: 40,
        visibleCount: 0,
        hasMore: true,
        loading: false,
      }),
    ).toBe(false);
    expect(
      shouldAdvancePastExcludedSearchHits({
        includeExcluded: false,
        excludedCount: 1,
        receivedCount: 40,
        visibleCount: 3,
        hasMore: true,
        loading: false,
      }),
    ).toBe(false);
  });
});

describe("foldSearchVariants", () => {
  const svt = (quality: "sd" | "hd" | "fhd") =>
    channelFixture({
      id: `svt1-${quality}`,
      name: `SVT1 ${quality.toUpperCase()}`,
      group: "Sverige",
      number: 101,
      variant: { quality, baseName: "SVT1" },
    });
  const NEWS = channelFixture({
    id: "svt24",
    name: "SVT24",
    group: "Nyheter",
    number: 136,
  });

  it("folds a guide row's Quality Variants into one row where the first ranked", () => {
    const rows = foldSearchVariants(
      [svt("hd"), NEWS, svt("sd"), svt("fhd")],
      new Map(),
    );

    expect(rows.map((row) => row.channel.id)).toEqual(["svt1-fhd", "svt24"]);
    expect(rows[0]?.variants.map((variant) => variant.id)).toEqual([
      "svt1-sd",
      "svt1-hd",
      "svt1-fhd",
    ]);
    expect(rows[1]?.variants).toEqual([NEWS]);
  });

  it("tunes the quality the viewer chose for that row", () => {
    const rows = foldSearchVariants(
      [svt("sd"), svt("hd"), svt("fhd")],
      new Map([[familyKey({ group: "Sverige", title: "SVT1" }), "hd"]]),
    );

    expect(rows[0]?.channel.id).toBe("svt1-hd");
  });
});
