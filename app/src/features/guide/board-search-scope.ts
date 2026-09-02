import type {
  ChannelSummary,
  ProgrammeSearchHit,
} from "../../client/contracts";

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
