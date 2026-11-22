import type {
  ChannelId,
  ChannelSummary,
  GuideWindowChannel,
  ProgrammeSlot,
} from "../../client/contracts";
import { programmeAt } from "./now-next";

/**
 * The Programmes the Guide has already read, by Channel, and the minute the
 * Guide shows. Search hits read what is on from it and never fetch.
 */
export interface SearchOnNow {
  readonly programmes: ReadonlyMap<ChannelId, readonly ProgrammeSlot[]>;
  readonly now: Date;
}

/** Indexes the guide rows read so far by Channel. */
export function guideProgrammesByChannel(
  rows: readonly GuideWindowChannel[],
): ReadonlyMap<ChannelId, readonly ProgrammeSlot[]> {
  return new Map(rows.map((row) => [row.channel.id, row.programmes]));
}

/**
 * The Programme on now for a search hit: from the Channel the hit tunes,
 * else from the first of its guide row's other Quality Variants that has
 * one, among those the Guide has read. Null when none of them has a
 * Programme on now.
 */
export function searchHitOnNow(
  channel: ChannelSummary,
  variants: readonly ChannelSummary[],
  onNow: SearchOnNow | undefined,
): ProgrammeSlot | null {
  if (onNow === undefined) return null;
  const order = [channel, ...variants.filter((variant) => variant.id !== channel.id)];
  for (const variant of order) {
    const programmes = onNow.programmes.get(variant.id);
    if (programmes === undefined) continue;
    const programme = programmeAt(programmes, onNow.now);
    if (programme !== null) return programme;
  }
  return null;
}
