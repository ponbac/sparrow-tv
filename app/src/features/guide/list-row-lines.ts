import type { GuideProgramme } from "../../client/contracts";
import { clockLabel } from "./guide-window";
import {
  elapsedFraction,
  minutesLeft,
  nextAfter,
  programmeAt,
} from "./now-next";

const NO_GUIDE_DATA = "No guide data. The channel still plays.";
const NOT_LOADED = "The guide for this time is not loaded.";

/** What one row of the now-and-next list says under its Channel Number. */
export type ListRowLines =
  | {
      /** A Programme is on at the time the row looks at: the row is titled by it. */
      readonly _tag: "programme";
      readonly title: string;
      /** What follows it now, or its own times when the row looks ahead. */
      readonly after: string | null;
      /** How far the Programme has got; null when the row looks ahead. */
      readonly live: {
        readonly minutesLeft: number;
        /** From 0 to 1. */
        readonly elapsed: number;
      } | null;
    }
  | {
      /** Nothing is listed then: the row is titled by its Channel. */
      readonly _tag: "channel";
      readonly after: string | null;
    };

/**
 * Words one list row from its Channel's Programmes, which must be in start
 * order. `at` is the time the viewer chose to look at, or null for now.
 * While `pending`, the Programmes are being replaced by a longer read, so a
 * later time with nothing found says nothing rather than that nothing is on.
 * When the Programmes are `truncated`, the catalog kept only the earliest of
 * the window: a time past them is one the row knows nothing about, and it
 * says that instead.
 */
export function listRowLines({
  programmes,
  truncated,
  now,
  at,
  pending,
}: {
  readonly programmes: readonly GuideProgramme[];
  readonly truncated: boolean;
  readonly now: Date;
  readonly at: Date | null;
  readonly pending: boolean;
}): ListRowLines {
  if (at !== null) {
    const then = programmeAt(programmes, at);
    if (then !== null) {
      return {
        _tag: "programme",
        title: shownTitle(then),
        after: `${clockLabel(then.startsAt)} to ${clockLabel(then.endsAt)}`,
        live: null,
      };
    }
    return {
      _tag: "channel",
      after: pending
        ? null
        : programmes.length === 0
          ? NO_GUIDE_DATA
          : truncated && isPastKept(programmes, at)
            ? NOT_LOADED
            : `Nothing listed at ${clockLabel(at)}`,
    };
  }

  const next = nextAfter(programmes, now);
  const after =
    next !== null
      ? `${clockLabel(next.startsAt)} ${shownTitle(next)}`
      : programmes.length === 0
        ? NO_GUIDE_DATA
        : null;
  const live = programmeAt(programmes, now);
  return live === null
    ? { _tag: "channel", after }
    : {
        _tag: "programme",
        title: shownTitle(live),
        after,
        live: {
          minutesLeft: minutesLeft(live, now),
          elapsed: elapsedFraction(live, now),
        },
      };
}

/**
 * Reports whether `instant` lies where Programmes were left out. The kept
 * ones are the earliest by start, so every Programme that starts before the
 * last of them is known, and nothing is known from its start on.
 */
function isPastKept(
  programmes: readonly GuideProgramme[],
  instant: Date,
): boolean {
  const last = programmes[programmes.length - 1];
  return last !== undefined && instant.getTime() >= Date.parse(last.startsAt);
}

function shownTitle(programme: GuideProgramme): string {
  return programme.titleTruncated ? `${programme.title}…` : programme.title;
}
