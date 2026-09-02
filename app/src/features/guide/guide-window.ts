import type { ProgrammeSlot } from "../../client/contracts";

const HALF_HOUR_MS = 30 * 60 * 1_000;
const GUIDE_SPAN_MS = 3 * 60 * 60 * 1_000;
const CLOCK_FORMATTER = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23",
});

/** One stable three-hour guide window anchored to the current half hour. */
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
}

/** Creates the guide window containing `now` without changing every minute. */
export function clockWindow(now: Date): ClockWindow {
  const startsAt = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate(),
    now.getHours(),
    Math.floor(now.getMinutes() / 30) * 30,
  );
  return {
    startsAt,
    endsAt: new Date(startsAt.getTime() + GUIDE_SPAN_MS),
  };
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
  };
}

/** Returns the current Programme, or the first visible Programme as a fallback. */
export function programmeAt<Programme extends ProgrammeSlot>(
  programmes: readonly Programme[],
  now: Date,
): Programme | null {
  const nowTime = now.getTime();
  return (
    programmes.find(
      (programme) =>
        Date.parse(programme.startsAt) <= nowTime &&
        nowTime < Date.parse(programme.endsAt),
    ) ??
    programmes[0] ??
    null
  );
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

/** Describes when a Programme airs relative to the current clock. */
export function programmeTiming(
  programme: ProgrammeSlot,
  now: Date,
): string {
  const start = Date.parse(programme.startsAt);
  const end = Date.parse(programme.endsAt);
  const slot = `${clockLabel(programme.startsAt)}–${clockLabel(programme.endsAt)}`;
  const nowTime = now.getTime();
  if (start <= nowTime && nowTime < end) {
    return `${slot} · ${Math.max(1, Math.ceil((end - nowTime) / 60_000))} min left`;
  }
  if (nowTime < start) {
    return `${slot} · starts in ${Math.max(1, Math.ceil((start - nowTime) / 60_000))} min`;
  }
  return `${slot} · earlier`;
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

/** Compares Programme identity without depending on its owning response shape. */
export function sameProgramme(
  left: ProgrammeSlot,
  right: ProgrammeSlot | null,
): boolean {
  return (
    right !== null &&
    left.startsAt === right.startsAt &&
    left.endsAt === right.endsAt &&
    left.title === right.title
  );
}

/** Locates the current clock on the guide axis, clamped to the visible window. */
export function playheadPercent(window: ClockWindow, now: Date): number {
  const start = window.startsAt.getTime();
  const end = window.endsAt.getTime();
  const clamped = Math.min(Math.max(now.getTime(), start), end);
  return ((clamped - start) / (end - start)) * 100;
}
