import { describe, expect, it } from "vitest";
import { clientSchemas, type ChannelGroup } from "../../client/contracts";
import {
  BOARD_GROUP_EXCLUSIONS_STORAGE_KEY,
  groupDisplayName,
  parsePersistedExclusions,
  readStoredExclusions,
  resolvedActiveGroup,
  serializeExclusions,
  setGroupExcluded,
  shouldAdvancePastExcludedPage,
  visibleChannelGroups,
  visibleGuideRows,
  writeStoredExclusions,
} from "./board-group-roster";

const NEWS: ChannelGroup = { name: "News", channelCount: 4 };
const CINEMA: ChannelGroup = { name: "Cinema", channelCount: 2 };
const UNGROUPED: ChannelGroup = { name: "", channelCount: 1 };

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

describe("board-group-roster", () => {
  it("labels the empty catalog bucket as Ungrouped", () => {
    expect(groupDisplayName("")).toBe("Ungrouped");
    expect(groupDisplayName("News")).toBe("News");
  });

  it("parses a versioned exclusion list and rejects unknown shapes", () => {
    expect(parsePersistedExclusions({ excluded: ["News", ""] })).toEqual(
      new Set(["News", ""]),
    );
    expect(parsePersistedExclusions({ excluded: ["News"], extra: true })).toEqual(
      new Set(),
    );
    expect(parsePersistedExclusions(["News"])).toEqual(new Set());
    expect(parsePersistedExclusions({ excluded: ["\u0007"] })).toEqual(
      new Set(),
    );
  });

  it("round-trips exclusions through web storage without mutating the input set", () => {
    const storage = memoryStorage();
    const excluded = new Set(["News"]);
    writeStoredExclusions(storage, excluded);
    expect(storage.getItem(BOARD_GROUP_EXCLUSIONS_STORAGE_KEY)).toBe(
      serializeExclusions(excluded),
    );
    expect(readStoredExclusions(storage)).toEqual(new Set(["News"]));
    expect(readStoredExclusions(memoryStorage())).toEqual(new Set());
    excluded.add("Cinema");
    expect(readStoredExclusions(storage)).toEqual(new Set(["News"]));
  });

  it("treats unreadable storage as no exclusions", () => {
    const storage = memoryStorage();
    storage.setItem(BOARD_GROUP_EXCLUSIONS_STORAGE_KEY, "{");
    expect(readStoredExclusions(storage)).toEqual(new Set());
    expect(
      readStoredExclusions(
        memoryStorage({
          getItem: () => {
            throw new Error("blocked");
          },
        }),
      ),
    ).toEqual(new Set());
  });

  it("adds and removes a Channel Group without mutating the previous set", () => {
    const empty = new Set<string>();
    const excluded = setGroupExcluded(empty, "News", true);
    expect(excluded).toEqual(new Set(["News"]));
    expect(empty.size).toBe(0);
    expect(setGroupExcluded(excluded, "News", true)).toBe(excluded);
    expect(setGroupExcluded(excluded, "News", false)).toEqual(new Set());
  });

  it("hides excluded Channel Groups and All-window rows, then falls back to All", () => {
    const excluded = new Set(["News"]);
    expect(visibleChannelGroups([UNGROUPED, NEWS, CINEMA], excluded)).toEqual([
      UNGROUPED,
      CINEMA,
    ]);
    expect(
      visibleGuideRows(
        [
          { channel: WORLD_NEWS, programmes: [], programmesTruncated: false },
          { channel: CINEMA_ONE, programmes: [], programmesTruncated: false },
        ],
        excluded,
        null,
      ).map((row) => row.channel.id),
    ).toEqual(["cinema-one"]);
    expect(
      visibleGuideRows(
        [{ channel: WORLD_NEWS, programmes: [], programmesTruncated: false }],
        excluded,
        "News",
      ),
    ).toHaveLength(1);
    expect(resolvedActiveGroup("News", excluded)).toBeNull();
    expect(resolvedActiveGroup("Cinema", excluded)).toBe("Cinema");
  });

  it("advances All only when a received page is entirely excluded", () => {
    expect(
      shouldAdvancePastExcludedPage({
        activeGroup: null,
        excludedCount: 1,
        receivedCount: 40,
        visibleCount: 0,
        hasMore: true,
        loading: false,
      }),
    ).toBe(true);
    expect(
      shouldAdvancePastExcludedPage({
        activeGroup: null,
        excludedCount: 1,
        receivedCount: 40,
        visibleCount: 3,
        hasMore: true,
        loading: false,
      }),
    ).toBe(false);
    expect(
      shouldAdvancePastExcludedPage({
        activeGroup: "News",
        excludedCount: 1,
        receivedCount: 40,
        visibleCount: 0,
        hasMore: true,
        loading: false,
      }),
    ).toBe(false);
  });
});

function memoryStorage(
  overrides: Partial<Storage> = {},
): Storage {
  const values = new Map<string, string>();
  return {
    get length() {
      return values.size;
    },
    clear() {
      values.clear();
    },
    getItem(key) {
      return values.get(key) ?? null;
    },
    key(index) {
      return [...values.keys()][index] ?? null;
    },
    removeItem(key) {
      values.delete(key);
    },
    setItem(key, value) {
      values.set(key, value);
    },
    ...overrides,
  };
}
