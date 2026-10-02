import type {
  CatalogStatus,
  IsoInstant,
  SourceState,
} from "../../client/contracts";

/** How fresh one source is: its lifecycle state and one sentence for the viewer. */
export interface SourceFreshness {
  /** The source's lifecycle tag; `unavailable` also covers a source not set up. */
  readonly state: SourceState["_tag"];
  readonly sentence: string;
}

/**
 * Describes the M3U Source ("Channels") or EPG Source ("Guide") in one
 * sentence. A null status reads as unavailable; ages are relative to `now`
 * and never negative.
 */
export function sourceFreshness(
  source: "m3u" | "epg",
  status: CatalogStatus | null,
  now: Date,
): SourceFreshness {
  const label = source === "m3u" ? "Channels" : "Guide";
  if (status === null) {
    return { state: "unavailable", sentence: `${label} unavailable` };
  }
  const state = source === "m3u" ? status.m3u : status.epg;
  const configured =
    source === "m3u"
      ? status.configuration.configured
      : status.configuration.epgConfigured;
  if (state === null || (!configured && state._tag === "unavailable")) {
    return { state: "unavailable", sentence: `${label} not set up` };
  }
  return { state: state._tag, sentence: `${label} ${freshness(state, now)}` };
}

function freshness(state: SourceState, now: Date): string {
  switch (state._tag) {
    case "fresh":
    case "stale":
      return `updated ${age(state.validatedAt, now)}`;
    case "refreshing":
      return "updating…";
    case "deferred":
      return state.validatedAt === null
        ? "waiting to update"
        : `updated ${age(state.validatedAt, now)}`;
    case "failed":
      return "update failed";
    case "unavailable":
      return "unavailable";
  }
}

function age(validatedAt: IsoInstant, now: Date): string {
  const minutes = Math.max(
    0,
    Math.floor((now.getTime() - Date.parse(validatedAt)) / 60_000),
  );
  if (minutes < 1) {
    return "just now";
  }
  if (minutes < 60) {
    return `${minutes} min ago`;
  }
  const hours = Math.floor(minutes / 60);
  return hours < 48 ? `${hours} h ago` : `${Math.floor(hours / 24)} days ago`;
}
