import type { ChannelId, GuideWindowChannel } from "../../client/contracts";
import {
  guideFamilies,
  preferredVariant,
  type GuideFamily,
  type VariantPreferences,
} from "../guide/guide-families";
import type { ZapNeighbourhood } from "./zap-neighbourhood";

const RAIL_LENGTH = 6;
// The current stop sits third in the rail, so two earlier ones stay in view.
const RAIL_LEAD = 2;

/** One guide row the viewer can zap to, with the Channel that would play. */
export interface ZapStop {
  readonly family: GuideFamily;
  /**
   * The variant choosing this stop plays: the playing Channel on its own row,
   * the preferred variant on every other.
   */
  readonly target: GuideWindowChannel;
}

/**
 * The guide rows a zap moves through, in Channel Catalog order: the rows
 * around the playing Channel without those of excluded Channel Groups. The
 * playing Channel's own row always stays. A row at an end of the
 * neighbourhood that does not reach the end of the Channel Catalog may be cut
 * short of some of its Quality Variants, so it is left out until a further
 * read completes it. Empty when the rows do not hold the playing Channel.
 */
export function zapStops(
  neighbourhood: ZapNeighbourhood,
  playing: ChannelId,
  excludedGroups: ReadonlySet<string>,
  preferences: VariantPreferences,
): readonly ZapStop[] {
  const families = guideFamilies(neighbourhood.rows);
  const stops: ZapStop[] = [];
  let found = false;
  families.forEach((family, index) => {
    const playingVariant = family.variants.find(
      (variant) => variant.channel.id === playing,
    );
    const cut =
      (index === 0 && !neighbourhood.startReached) ||
      (index === families.length - 1 && !neighbourhood.endReached);
    if (playingVariant !== undefined) {
      found = true;
      stops.push({ family, target: playingVariant });
    } else if (!cut && !excludedGroups.has(family.group)) {
      stops.push({ family, target: preferredVariant(family, preferences) });
    }
  });
  return found ? stops : [];
}

/**
 * The sides on which `from` has no stop beside it although the Channel
 * Catalog goes on: where the rows around the playing Channel must be read
 * further for a zap to go on. None while `from` is not among the stops.
 */
export function zapSidesToRead(
  stops: readonly ZapStop[],
  neighbourhood: ZapNeighbourhood,
  from: ChannelId,
): readonly (-1 | 1)[] {
  if (stopIndex(stops, from) === -1) {
    return [];
  }
  return ([-1, 1] as const).filter(
    (direction) =>
      neighbouringZapStop(stops, from, direction) === null &&
      !(direction === -1
        ? neighbourhood.startReached
        : neighbourhood.endReached),
  );
}

/** The stop whose guide row holds the Channel, or null. */
export function zapStopOf(
  stops: readonly ZapStop[],
  channel: ChannelId,
): ZapStop | null {
  return stops[stopIndex(stops, channel)] ?? null;
}

/**
 * The stop before (`-1`) or after (`1`) the one holding `from`. Null at
 * either end of the list, and when `from` is not among the stops.
 */
export function neighbouringZapStop(
  stops: readonly ZapStop[],
  from: ChannelId,
  direction: -1 | 1,
): ZapStop | null {
  const index = stopIndex(stops, from);
  return index === -1 ? null : (stops[index + direction] ?? null);
}

/**
 * The stops the zap rail shows: up to six, with the one holding `current`
 * third unless the list ends sooner on either side.
 */
export function zapRailStops(
  stops: readonly ZapStop[],
  current: ChannelId,
): readonly ZapStop[] {
  const index = stopIndex(stops, current);
  if (index === -1) {
    return [];
  }
  const start = Math.max(
    0,
    Math.min(index - RAIL_LEAD, stops.length - RAIL_LENGTH),
  );
  return stops.slice(start, start + RAIL_LENGTH);
}

function stopIndex(stops: readonly ZapStop[], channel: ChannelId): number {
  return stops.findIndex((stop) =>
    stop.family.variants.some((variant) => variant.channel.id === channel),
  );
}
