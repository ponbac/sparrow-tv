import type { CatalogStatus, SourceState } from "../../client/contracts";

/** Browser-facing Guide availability without exposing Source details. */
export type GuidePresentation =
  | { readonly tone: "live"; readonly label: "GUIDE LIVE"; readonly detail: string }
  | {
      readonly tone: "retained";
      readonly label: "GUIDE RECORDED";
      readonly detail: string;
    }
  | { readonly tone: "absent"; readonly label: "GUIDE ABSENT"; readonly detail: string }
  | { readonly tone: "failed"; readonly label: "GUIDE FAILED"; readonly detail: string }
  | {
      readonly tone: "checking";
      readonly label: "GUIDE CHECKING";
      readonly detail: string;
    };

/** Projects typed Source status into the Guide state rendered by search and schedules. */
export function guidePresentation(status: CatalogStatus | null): GuidePresentation {
  if (status === null) {
    return {
      tone: "checking",
      label: "GUIDE CHECKING",
      detail: "Guide status is still being resolved. Channel search remains available.",
    };
  }
  if (!status.configuration.epgConfigured || status.epg === null) {
    return {
      tone: "absent",
      label: "GUIDE ABSENT",
      detail: "No EPG Source is configured. Channel search remains fully available.",
    };
  }
  return guideSourcePresentation(status.epg);
}

/** Explains an empty Programme-search lane for the current Guide state. */
export function emptyProgrammeSearchCopy(
  guide: GuidePresentation,
  term: string,
): string {
  switch (guide.tone) {
    case "absent":
      return "Programme search is unavailable because no EPG Source is configured. Channels can still be searched.";
    case "failed":
      return "Programme search has no validated Guide snapshot. Channels can still be searched.";
    case "checking":
      return "No Programme results are available while the Guide snapshot is being prepared.";
    case "retained":
      return `No retained Programmes match “${term}”.`;
    case "live":
      return `No Programmes match “${term}” in this generation.`;
  }
}

/** Explains an empty schedule without implying an ambiguous Guide match. */
export function emptyScheduleCopy(guide: GuidePresentation): string {
  switch (guide.tone) {
    case "absent":
      return "No EPG Source is configured. The Channel remains available without a schedule.";
    case "failed":
      return "No validated Guide snapshot is available. The Channel remains available.";
    case "checking":
      return "Guide data is still being prepared. The Channel remains available.";
    case "retained":
      return "The retained Guide has no unambiguous match for this Channel. Unmatched records stay unassociated; Sparrow never guesses.";
    case "live":
      return "The Guide has no unambiguous match for this Channel. Unmatched records stay unassociated; Sparrow never guesses.";
  }
}

function guideSourcePresentation(state: SourceState): GuidePresentation {
  switch (state._tag) {
    case "fresh":
      return {
        tone: "live",
        label: "GUIDE LIVE",
        detail: "Programme search and matched Channel schedules use the current Guide snapshot.",
      };
    case "stale":
      return {
        tone: "retained",
        label: "GUIDE RECORDED",
        detail: "The last validated Guide remains searchable while a fresh snapshot is awaited.",
      };
    case "failed":
      return state.validatedAt === null
        ? {
            tone: "failed",
            label: "GUIDE FAILED",
            detail: "No validated Guide snapshot is available. Channel search remains available.",
          }
        : {
            tone: "retained",
            label: "GUIDE RECORDED",
            detail: "Guide refresh failed, so the last validated Programme data remains in use.",
          };
    case "unavailable":
      return {
        tone: "failed",
        label: "GUIDE FAILED",
        detail: "No validated Guide snapshot is available. Channel search remains available.",
      };
    case "refreshing":
      return state.validatedAt === null
        ? {
            tone: "checking",
            label: "GUIDE CHECKING",
            detail: "The first Guide snapshot is loading. Channel search remains available.",
          }
        : {
            tone: "retained",
            label: "GUIDE RECORDED",
            detail: "The last Guide snapshot remains searchable during refresh.",
          };
    case "deferred":
      return state.validatedAt === null
        ? {
            tone: "checking",
            label: "GUIDE CHECKING",
            detail: "Guide loading is deferred. Channel search remains available.",
          }
        : {
            tone: "retained",
            label: "GUIDE RECORDED",
            detail: "The last Guide snapshot remains searchable while refresh is deferred.",
          };
  }
}
