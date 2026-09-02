import { describe, expect, it } from "vitest";
import { clientSchemas } from "../../client/contracts";
import {
  shouldAdvancePastExcludedSearchHits,
  visibleSearchChannels,
  visibleSearchProgrammes,
} from "./board-search-scope";

const WORLD_NEWS = clientSchemas.channel.parse({
  id: "world-news",
  name: "World News",
  group: "News",
});
const CINEMA_ONE = clientSchemas.channel.parse({
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
