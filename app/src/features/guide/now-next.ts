import type { ProgrammeSlot } from "../../client/contracts";
import { isProgrammeLive } from "./guide-window";

const MINUTE_MS = 60_000;

/** Returns the Programme airing at `instant`, or null between Programmes. */
export function programmeAt<Programme extends ProgrammeSlot>(
  programmes: readonly Programme[],
  instant: Date,
): Programme | null {
  return (
    programmes.find((programme) => isProgrammeLive(programme, instant)) ?? null
  );
}

/**
 * Returns the first Programme that starts after `instant`, or null when none
 * does. The Programmes must be in start order.
 */
export function nextAfter<Programme extends ProgrammeSlot>(
  programmes: readonly Programme[],
  instant: Date,
): Programme | null {
  return (
    programmes.find(
      (programme) => Date.parse(programme.startsAt) > instant.getTime(),
    ) ?? null
  );
}

/** Counts the minutes until a Programme ends, rounded up and never below one. */
export function minutesLeft(programme: ProgrammeSlot, now: Date): number {
  return Math.max(
    1,
    Math.ceil((Date.parse(programme.endsAt) - now.getTime()) / MINUTE_MS),
  );
}

/** Returns how much of a Programme has aired at `now`, from 0 to 1. */
export function elapsedFraction(programme: ProgrammeSlot, now: Date): number {
  const startsAt = Date.parse(programme.startsAt);
  const endsAt = Date.parse(programme.endsAt);
  const elapsed = (now.getTime() - startsAt) / (endsAt - startsAt);
  return Math.min(Math.max(elapsed, 0), 1);
}
