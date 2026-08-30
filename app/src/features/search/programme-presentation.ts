import type { ProgrammeSummary } from "../../client/contracts";

/** Builds a stable React key for one Programme in an ordered result page. */
export function programmeKey(programme: ProgrammeSummary, index: number): string {
  return `${programme.channelId}:${programme.startsAt}:${programme.endsAt}:${index}`;
}

/** Formats UTC Programme instants in the browser locale with an explicit zone. */
export function formatProgrammeTime(programme: ProgrammeSummary): string {
  const startsAt = new Date(programme.startsAt);
  const endsAt = new Date(programme.endsAt);
  const date = new Intl.DateTimeFormat(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
  }).format(startsAt);
  const time = new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    timeZoneName: "short",
  });
  return `${date} · ${time.format(startsAt)}–${time.format(endsAt)}`;
}
