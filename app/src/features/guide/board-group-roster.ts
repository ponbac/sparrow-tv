import { z } from "zod";
import type { ChannelGroup, GuideWindowChannel } from "../../client/contracts";

const STORAGE_KEY = "sparrow.board-group-exclusions:v1";
const MAX_STORED_EXCLUSIONS = 10_000;
const textEncoder = new TextEncoder();

const groupNameSchema = z
  .string()
  .max(1024)
  .refine((value) => textEncoder.encode(value).byteLength <= 1024, {
    message: "Channel Group names cannot exceed 1024 UTF-8 bytes.",
  })
  .refine((value) => !/\p{Cc}/u.test(value), {
    message: "Channel Group names cannot contain control characters.",
  });

const persistedExclusionsSchema = z.strictObject({
  excluded: z.array(groupNameSchema).max(MAX_STORED_EXCLUSIONS),
});

/** Versioned localStorage key for Channel Groups hidden from the board. */
export const BOARD_GROUP_EXCLUSIONS_STORAGE_KEY = STORAGE_KEY;

/**
 * Returns the operator-facing Channel Group label, using Ungrouped for the
 * empty catalog bucket.
 */
export function groupDisplayName(name: string): string {
  return name === "" ? "Ungrouped" : name;
}

/**
 * Parses unknown storage JSON into a set of excluded Channel Group names.
 * Unrecognized or oversized payloads become an empty set.
 */
export function parsePersistedExclusions(raw: unknown): ReadonlySet<string> {
  const parsed = persistedExclusionsSchema.safeParse(raw);
  if (!parsed.success) {
    return new Set();
  }
  return new Set(parsed.data.excluded);
}

/**
 * Projects excluded Channel Group names into the versioned persistence DTO.
 */
export function serializeExclusions(excluded: ReadonlySet<string>): string {
  return JSON.stringify({ excluded: [...excluded] });
}

/**
 * Reads excluded Channel Group names from web storage. Missing, unreadable, or
 * invalid payloads become an empty set and never throw.
 */
export function readStoredExclusions(storage: Storage): ReadonlySet<string> {
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (raw === null) {
      return new Set();
    }
    return parsePersistedExclusions(JSON.parse(raw) as unknown);
  } catch {
    return new Set();
  }
}

/**
 * Writes excluded Channel Group names to web storage. Quota, private-mode, and
 * disabled-storage failures are swallowed so the board stays usable.
 */
export function writeStoredExclusions(
  storage: Storage,
  excluded: ReadonlySet<string>,
): void {
  try {
    storage.setItem(STORAGE_KEY, serializeExclusions(excluded));
  } catch {
    return;
  }
}

/**
 * Returns a new exclusion set with `name` added or removed. The input set is
 * never mutated.
 */
export function setGroupExcluded(
  excluded: ReadonlySet<string>,
  name: string,
  exclude: boolean,
): ReadonlySet<string> {
  if (exclude === excluded.has(name)) {
    return excluded;
  }
  const next = new Set(excluded);
  if (exclude) {
    next.add(name);
  } else {
    next.delete(name);
  }
  return next;
}

/**
 * Channel Groups that remain on the board after exclusions. `All` is not a
 * Channel Group and is never included here.
 */
export function visibleChannelGroups(
  groups: readonly ChannelGroup[],
  excluded: ReadonlySet<string>,
): readonly ChannelGroup[] {
  if (excluded.size === 0) {
    return groups;
  }
  return groups.filter((group) => !excluded.has(group.name));
}

/**
 * Guide rows that remain after Channel Group exclusions. A selected group is
 * already narrowed by the catalog read, so only the unfiltered All window is
 * trimmed here.
 */
export function visibleGuideRows(
  rows: readonly GuideWindowChannel[],
  excluded: ReadonlySet<string>,
  activeGroup: string | null,
): readonly GuideWindowChannel[] {
  if (activeGroup !== null || excluded.size === 0) {
    return rows;
  }
  return rows.filter((row) => !excluded.has(row.channel.group));
}

/**
 * Drops a selected Channel Group that has been excluded so the board falls
 * back to All rather than showing a hidden filter.
 */
export function resolvedActiveGroup(
  activeGroup: string | null,
  excluded: ReadonlySet<string>,
): string | null {
  return activeGroup !== null && excluded.has(activeGroup) ? null : activeGroup;
}

/**
 * True when the current All page contains only excluded Channels and another
 * catalog page can still be opened.
 */
export function shouldAdvancePastExcludedPage(input: {
  readonly activeGroup: string | null;
  readonly excludedCount: number;
  readonly receivedCount: number;
  readonly visibleCount: number;
  readonly hasMore: boolean;
  readonly loading: boolean;
}): boolean {
  return (
    input.activeGroup === null &&
    input.excludedCount > 0 &&
    input.receivedCount > 0 &&
    input.visibleCount === 0 &&
    input.hasMore &&
    !input.loading
  );
}
