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
 * The Programme on now for a search hit, from any of its guide row's
 * Quality Variants the Guide has read, or null when the Guide has not read
 * that row or nothing airs now.
 */
export function searchHitOnNow(
  variants: readonly ChannelSummary[],
  onNow: SearchOnNow | undefined,
): ProgrammeSlot | null {
  if (onNow === undefined) return null;
  for (const variant of variants) {
    const programmes = onNow.programmes.get(variant.id);
    if (programmes !== undefined) {
      return programmeAt(programmes, onNow.now);
    }
  }
  return null;
}
