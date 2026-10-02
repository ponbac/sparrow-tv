import { describe, expect, it } from "vitest";
import {
  clientSchemas,
  type GuideWindowChannel,
} from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import {
  extendNeighbourhood,
  extensionAnchors,
  extensionWith,
  MAX_NEIGHBOURHOOD_EXTENSIONS,
  mergedNeighbourhood,
  neighbourhoodOf,
  type AroundRead,
} from "./zap-neighbourhood";

// A Channel Catalog of thirty Channels, at positions 0 to 29.
const CATALOG = Array.from({ length: 30 }, (_, index) => row(index));

describe("neighbourhoodOf", () => {
  it("knows a read in the middle of the catalog reaches neither end", () => {
    const neighbourhood = neighbourhoodOf(read(15, 9));

    expect(numbers(neighbourhood.rows)).toEqual([
      11, 12, 13, 14, 15, 16, 17, 18, 19,
    ]);
    expect(neighbourhood.startReached).toBe(false);
    expect(neighbourhood.endReached).toBe(false);
  });

  it("knows a read placed near the start or the end reaches it", () => {
    const start = neighbourhoodOf(read(2, 9));
    expect(numbers(start.rows)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8]);
    expect(start.startReached).toBe(true);
    expect(start.endReached).toBe(false);

    const end = neighbourhoodOf(read(28, 9));
    expect(numbers(end.rows)).toEqual([24, 25, 26, 27, 28, 29]);
    expect(end.startReached).toBe(false);
    expect(end.endReached).toBe(true);
  });
});

describe("extendNeighbourhood", () => {
  it("adds the rows a read around an end row holds beyond it", () => {
    const first = neighbourhoodOf(read(15, 9));

    const after = extendNeighbourhood(first, read(19, 9));
    expect(numbers(after.rows)).toEqual([
      11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
    ]);
    expect(after.startReached).toBe(false);
    expect(after.endReached).toBe(false);

    const before = extendNeighbourhood(after, read(11, 9));
    expect(numbers(before.rows)).toEqual([
      7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
    ]);
  });

  it("learns that an end of the catalog is reached", () => {
    const first = neighbourhoodOf(read(15, 9));

    const toEnd = extendNeighbourhood(first, read(19, 40));
    expect(numbers(toEnd.rows).at(-1)).toBe(29);
    expect(toEnd.endReached).toBe(true);
    // The same read starts the catalog too, and holds the first row.
    expect(numbers(toEnd.rows)[0]).toBe(0);
    expect(toEnd.startReached).toBe(true);
  });

  it("learns the start is reached from a read that has nothing before the first row", () => {
    const first = neighbourhoodOf(read(4, 9));
    expect(first.startReached).toBe(false);

    const extended = extendNeighbourhood(first, read(0, 9));

    expect(numbers(extended.rows)).toEqual(numbers(first.rows));
    expect(extended.startReached).toBe(true);
  });

  it("returns the same neighbourhood for a read that adds nothing", () => {
    const first = neighbourhoodOf(read(15, 9));

    // Inside the rows already held.
    expect(extendNeighbourhood(first, read(15, 3))).toBe(first);
    // Nowhere near them.
    expect(extendNeighbourhood(first, read(2, 3))).toBe(first);
  });

  it("merges a first read with its further reads in turn", () => {
    const merged = mergedNeighbourhood(read(15, 9), [read(19, 9), read(23, 9)]);

    expect(numbers(merged.rows)).toEqual([
      11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
    ]);
  });
});

describe("extensionWith", () => {
  const base = channelOf(15).id;

  it("collects the anchors asked for around one first read, each once", () => {
    const one = extensionWith(null, base, channelOf(19).id);
    const two = extensionWith(one, base, channelOf(11).id);

    expect(extensionAnchors(two, base)).toEqual([
      channelOf(19).id,
      channelOf(11).id,
    ]);
    expect(extensionWith(two, base, channelOf(19).id)).toBe(two);
  });

  it("starts over for another first read", () => {
    const old = extensionWith(null, base, channelOf(19).id);
    const other = channelOf(22).id;

    expect(extensionAnchors(old, other)).toEqual([]);
    expect(extensionWith(old, other, channelOf(26).id)).toEqual({
      base: other,
      anchors: [channelOf(26).id],
    });
  });

  it("stops growing at the limit", () => {
    let extension = extensionWith(null, base, channelOf(0).id);
    for (let index = 1; index < 20; index += 1) {
      extension = extensionWith(extension, base, channelOf(index).id);
    }

    expect(extensionAnchors(extension, base)).toHaveLength(
      MAX_NEIGHBOURHOOD_EXTENSIONS,
    );
  });
});

/** The read core answers when asked for `limit` rows around a Channel. */
function read(around: number, limit: number): AroundRead {
  const start = Math.max(0, around - Math.floor(limit / 2));
  const end = start + limit;
  return {
    around: channelOf(around).id,
    limit,
    page: clientSchemas.guideWindow.parse({
      generation: 7,
      items: CATALOG.slice(start, end),
      next: end < CATALOG.length ? `after-${end}` : null,
    }),
  };
}

function channelOf(index: number) {
  return channelFixture({
    id: `channel-${index}`,
    name: `Channel ${index}`,
    group: "General",
    number: index + 1,
  });
}

function row(index: number): GuideWindowChannel {
  return { channel: channelOf(index), programmes: [], programmesTruncated: false };
}

/** Where each row sits in the catalog, counted from 0. */
function numbers(rows: readonly GuideWindowChannel[]): readonly number[] {
  return rows.map((entry) => entry.channel.number - 1);
}
