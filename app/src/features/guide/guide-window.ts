import type { ProgrammeSlot } from "../../client/contracts";

const QUARTER_HOUR_MS = 15 * 60 * 1_000;
const HALF_HOUR_MS = 30 * 60 * 1_000;
const HOUR_MS = 60 * 60 * 1_000;
/** How much time the guide reads: what the timeline grid shows. */
export const GUIDE_SPAN_MS = 3 * HOUR_MS;
/** A longer read that covers every time the viewer can choose to look at. */
export const LATER_SPAN_MS = 8 * HOUR_MS;
const CLOCK_FORMATTER = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23",
});

/** One stable guide window anchored to the current half hour. */
export interface ClockWindow {
  readonly startsAt: Date;
  readonly endsAt: Date;
}

/** Horizontal placement of one Programme inside a bounded guide window. */
export interface ProgrammeLayout {
  readonly leftPercent: number;
  readonly widthPercent: number;
  readonly elapsedPercent: number;
  readonly live: boolean;
  /** The visible part spans at least half an hour: room for the time range line. */
  readonly timesFit: boolean;
}

/**
 * Creates the guide window containing `now` without changing every minute.
 * It starts at the half hour and lasts `spanMs`, three hours unless given.
 */
export function clockWindow(now: Date, spanMs = GUIDE_SPAN_MS): ClockWindow {
  const startsAt = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate(),
    now.getHours(),
    Math.floor(now.getMinutes() / 30) * 30,
  );
  return {
    startsAt,
    endsAt: new Date(startsAt.getTime() + spanMs),
  };
}

/**
 * Lists the times the viewer can look ahead to: the instants after `now` and
 * before `windowEnd` at which the local clock reads a full hour, earliest
 * first, at most `limit` of them.
 */
export function laterTimes(
  now: Date,
  windowEnd: Date,
  limit = 6,
): readonly Date[] {
  // Every zone is a whole number of quarter hours from UTC, so a full hour on
  // the local clock is always a quarter hour in UTC. Asking each one what the
  // local clock reads follows a change of offset inside the window: an hour
  // the clock skips is left out, and the hours after it stay full hours.
  const first =
    (Math.floor(now.getTime() / QUARTER_HOUR_MS) + 1) * QUARTER_HOUR_MS;
  const times: Date[] = [];
  for (
    let instant = first;
    instant < windowEnd.getTime() && times.length < limit;
    instant += QUARTER_HOUR_MS
  ) {
    const time = new Date(instant);
    if (time.getMinutes() === 0) {
      times.push(time);
    }
  }
  return times;
}

/** Returns half-hour marks spanning a clock window, including its start. */
export function clockMarks(window: ClockWindow): readonly Date[] {
  const marks: Date[] = [];
  for (
    let instant = window.startsAt.getTime();
    instant < window.endsAt.getTime();
    instant += HALF_HOUR_MS
  ) {
    marks.push(new Date(instant));
  }
  return marks;
}

/** Projects a Programme onto the visible horizontal guide axis. */
export function programmeLayout(
  programme: ProgrammeSlot,
  window: ClockWindow,
  now: Date,
): ProgrammeLayout | null {
  const windowStart = window.startsAt.getTime();
  const windowEnd = window.endsAt.getTime();
  const programmeStart = Date.parse(programme.startsAt);
  const programmeEnd = Date.parse(programme.endsAt);
  const visibleStart = Math.max(programmeStart, windowStart);
  const visibleEnd = Math.min(programmeEnd, windowEnd);
  if (visibleEnd <= visibleStart) {
    return null;
  }

  const span = windowEnd - windowStart;
  const nowTime = now.getTime();
  const elapsed = Math.min(Math.max(nowTime, visibleStart), visibleEnd);
  return {
    leftPercent: ((visibleStart - windowStart) / span) * 100,
    widthPercent: ((visibleEnd - visibleStart) / span) * 100,
    elapsedPercent: ((elapsed - visibleStart) / (visibleEnd - visibleStart)) * 100,
    live: programmeStart <= nowTime && nowTime < programmeEnd,
    timesFit: visibleEnd - visibleStart >= HALF_HOUR_MS,
  };
}

/** Builds a stable identity for a Programme inside its owning Channel row. */
export function programmeKey(
  programme: ProgrammeSlot,
  occurrence: number,
): string {
  return `${programme.startsAt}:${programme.endsAt}:${programme.title}:${occurrence}`;
}

/** Formats one guide instant as a compact local clock time. */
export function clockLabel(instant: Date | string): string {
  const date = typeof instant === "string" ? new Date(instant) : instant;
  return CLOCK_FORMATTER.format(date);
}

/** Reports whether a Programme is airing at the supplied instant. */
export function isProgrammeLive(
  programme: ProgrammeSlot,
  now: Date,
): boolean {
  const instant = now.getTime();
  return (
    Date.parse(programme.startsAt) <= instant &&
    instant < Date.parse(programme.endsAt)
  );
}

/** Locates the current clock on the guide axis, clamped to the visible window. */
export function playheadPercent(window: ClockWindow, now: Date): number {
  const start = window.startsAt.getTime();
  const end = window.endsAt.getTime();
  const clamped = Math.min(Math.max(now.getTime(), start), end);
  return ((clamped - start) / (end - start)) * 100;
}
