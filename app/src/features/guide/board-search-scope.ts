import type {
  ChannelSummary,
  ProgrammeSearchHit,
} from "../../client/contracts";
import {
  byQuality,
  preferredChannel,
  type VariantPreferences,
} from "./guide-families";

/**
 * One TV search result for a Channel: the guide row's Quality Variants that
 * matched, folded into one row that tunes the preferred one. A Channel
 * without variants is a row of one.
 */
export interface SearchChannelRow {
  /** The variant the row tunes; see `preferredChannel`. */
  readonly channel: ChannelSummary;
  /** The matched variants from the lowest quality up; never empty. */
  readonly variants: readonly [ChannelSummary, ...ChannelSummary[]];
}

/**
 * Folds Channel hits that share a Channel Number, the Quality Variants of one
 * guide row, into one row each, placed where the first of them ranked.
 */
export function foldSearchVariants(
  channels: readonly ChannelSummary[],
  preferences: VariantPreferences,
): readonly SearchChannelRow[] {
  const byNumber = new Map<number, ChannelSummary[]>();
  for (const channel of channels) {
    const variants = byNumber.get(channel.number);
    if (variants === undefined) {
      byNumber.set(channel.number, [channel]);
    } else {
      variants.push(channel);
    }
  }
  return Array.from(byNumber.values(), (found) => {
    const variants = [...found].sort(byQuality) as [
      ChannelSummary,
      ...ChannelSummary[],
    ];
    return { channel: preferredChannel(variants, preferences), variants };
  });
}

/**
 * Channels that remain after board-group exclusions. Passing
 * `includeExcluded` restores the complete catalog ranking.
 */
export function visibleSearchChannels(
  channels: readonly ChannelSummary[],
  excluded: ReadonlySet<string>,
  includeExcluded: boolean,
): readonly ChannelSummary[] {
  if (includeExcluded || excluded.size === 0) {
    return channels;
  }
  return channels.filter((channel) => !excluded.has(channel.group));
}

/**
 * Programme hits whose owning Channel is still on the board. Passing
 * `includeExcluded` restores hits from excluded Channel Groups.
 */
export function visibleSearchProgrammes(
  programmes: readonly ProgrammeSearchHit[],
  excluded: ReadonlySet<string>,
  includeExcluded: boolean,
): readonly ProgrammeSearchHit[] {
  if (includeExcluded || excluded.size === 0) {
    return programmes;
  }
  return programmes.filter(
    (programme) => !excluded.has(programme.channel.group),
  );
}

/**
 * True when the current search page contains only excluded Channels and
 * another catalog page can still be opened. Used to keep the desk filled
 * without making the operator page through dumps they already hid.
 */
export function shouldAdvancePastExcludedSearchHits(input: {
  readonly includeExcluded: boolean;
  readonly excludedCount: number;
  readonly receivedCount: number;
  readonly visibleCount: number;
  readonly hasMore: boolean;
  readonly loading: boolean;
}): boolean {
  return (
    !input.includeExcluded &&
    input.excludedCount > 0 &&
    input.receivedCount > 0 &&
    input.visibleCount === 0 &&
    input.hasMore &&
    !input.loading
  );
}
