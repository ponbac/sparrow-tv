import { Check, Clipboard, RefreshCw } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type {
  CatalogStatus,
  ClientError,
  ClientResult,
  IsoInstant,
  RefreshOutcome,
  RefreshReport,
  SafeFailure,
  SourceState,
  SparrowEvent,
} from "../../client/contracts";
import "./source-status-desk.css";

/** Inputs for independent source status and an optional manual refresh control. */
export interface SourceStatusDeskProps {
  readonly status: CatalogStatus | null;
  readonly refreshing: boolean;
  readonly refreshResult: ClientResult<RefreshReport> | null;
  readonly latestEvent: SparrowEvent | null;
  readonly onRefresh: () => void;
  readonly manualRefresh?: boolean;
  /** Whether the current runtime can play Channels after browse succeeds. */
  readonly playbackAvailable?: boolean;
  /** Ownership language for source state; defaults compatibly from playback. */
  readonly sourceScope?: "deployment" | "device";
}

/** Renders independent M3U/EPG state, manual refresh feedback, and safe diagnostics. */
export function SourceStatusDesk({
  status,
  refreshing,
  refreshResult,
  latestEvent,
  onRefresh,
  manualRefresh = true,
  playbackAvailable = true,
  sourceScope = playbackAvailable ? "deployment" : "device",
}: SourceStatusDeskProps) {
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">(
    "idle",
  );
  const diagnostics = useMemo(
    () => safeDiagnostics(status, refreshResult, latestEvent),
    [latestEvent, refreshResult, status],
  );
  const currentDiagnosticsRef = useRef(diagnostics);
  currentDiagnosticsRef.current = diagnostics;
  useEffect(() => setCopyState("idle"), [diagnostics]);
  const refreshDisabled =
    refreshing || status === null || !status.configuration.configured;

  const copyDiagnostics = () => {
    const clipboard = navigator.clipboard;
    if (clipboard === undefined) {
      setCopyState("failed");
      return;
    }
    const copiedDiagnostics = diagnostics;
    try {
      clipboard.writeText(copiedDiagnostics).then(
        () => {
          if (currentDiagnosticsRef.current === copiedDiagnostics) {
            setCopyState("copied");
          }
        },
        () => {
          if (currentDiagnosticsRef.current === copiedDiagnostics) {
            setCopyState("failed");
          }
        },
      );
    } catch {
      setCopyState("failed");
    }
  };

  return (
    <section className="source-desk" aria-labelledby="source-desk-heading">
      <header className="source-desk__heading">
        <h2 id="source-desk-heading">Source status</h2>
        {manualRefresh ? (
          <button
            className="source-desk__refresh"
            type="button"
            disabled={refreshDisabled}
            aria-busy={refreshing}
            onClick={onRefresh}
          >
            <RefreshCw aria-hidden="true" />
            {refreshing ? "Refresh in progress" : "Refresh sources"}
          </button>
        ) : null}
      </header>

      <div className="source-desk__grid" aria-live="polite">
        <SourceCard
          title="Channel source"
          state={status?.m3u ?? null}
        />
        <SourceCard
          title="Guide source"
          state={status?.epg ?? null}
          configured={status?.configuration.epgConfigured ?? null}
          catalogAvailable={status !== null && status.generation !== null}
          playbackAvailable={playbackAvailable}
          sourceScope={sourceScope}
        />
      </div>

      <RefreshFeedback
        result={refreshResult}
        playbackAvailable={playbackAvailable}
      />

      <details className="source-desk__diagnostics">
        <summary>Diagnostics you can copy</summary>
        <div>
          <pre
            role="region"
            aria-label="Safe source diagnostics"
            tabIndex={0}
          >
            {diagnostics}
          </pre>
          <button type="button" onClick={copyDiagnostics}>
            {copyState === "copied" ? (
              <Check aria-hidden="true" />
            ) : (
              <Clipboard aria-hidden="true" />
            )}
            {copyState === "copied"
              ? "Copied"
              : copyState === "failed"
                ? "Select text to copy"
                : "Copy diagnostics"}
          </button>
        </div>
      </details>
    </section>
  );
}

function SourceCard({
  title,
  state,
  configured = true,
  catalogAvailable = false,
  playbackAvailable = true,
  sourceScope,
}: {
  readonly title: string;
  readonly state: SourceState | null;
  readonly configured?: boolean | null;
  readonly catalogAvailable?: boolean;
  readonly playbackAvailable?: boolean;
  readonly sourceScope?: "deployment" | "device";
}) {
  const presentation = sourcePresentation(
    state,
    configured,
    catalogAvailable,
    playbackAvailable,
    sourceScope ?? (playbackAvailable ? "deployment" : "device"),
  );
  return (
    <article className="source-card" data-state={presentation.tone}>
      <header className="source-card__head">
        <h3>{title}</h3>
        <strong className="source-card__state">{presentation.label}</strong>
      </header>
      <p>{presentation.detail}</p>
      {presentation.time === null ? null : (
        <time dateTime={presentation.time.value}>
          {presentation.time.label} {formatTime(presentation.time.value)}
        </time>
      )}
      {presentation.nextAttemptAt === null ? null : (
        <time dateTime={presentation.nextAttemptAt}>
          Next attempt {formatTime(presentation.nextAttemptAt)}
        </time>
      )}
      {presentation.failure === null ? null : (
        <code>Reason: {presentation.failure}</code>
      )}
    </article>
  );
}

function RefreshFeedback({
  result,
  playbackAvailable,
}: {
  readonly result: ClientResult<RefreshReport> | null;
  readonly playbackAvailable: boolean;
}) {
  if (result === null) {
    return null;
  }
  if (!result.ok) {
    const copy = refreshErrorCopy(result.error);
    return (
      <div className="refresh-feedback" data-tone="failed" role="alert">
        <strong>{copy.title}</strong>
        <p>{copy.detail}</p>
      </div>
    );
  }

  const failureTitle = refreshFailureTitle(result.value);
  const outcomes = `Channel source ${refreshOutcomeSummary(result.value.m3u)}. Guide source ${
    result.value.epg === null
      ? "not set up"
      : refreshOutcomeSummary(result.value.epg)
  }.`;
  return (
    <div
      className="refresh-feedback"
      data-tone={failureTitle === null ? "complete" : "failed"}
      role={failureTitle === null ? "status" : "alert"}
    >
      <strong>{failureTitle ?? "Manual refresh complete"}</strong>
      <p>
        {failureTitle === null
          ? `${outcomes} ${refreshSuccessCopy(result.value)}`
          : `${outcomes} ${refreshFailureDetail(result.value, playbackAvailable)}`}
      </p>
    </div>
  );
}

function refreshFailureTitle(report: RefreshReport): string | null {
  const channelFailed = report.m3u._tag === "failed";
  const guideFailed = report.epg?._tag === "failed";
  if (channelFailed) {
    return guideFailed
      ? "Channel and guide source refresh failed"
      : "Channel source refresh failed";
  }
  return guideFailed ? "Guide source refresh failed" : null;
}

interface SourcePresentation {
  readonly tone:
    | "checking"
    | "fresh"
    | "stale"
    | "refreshing"
    | "failed"
    | "unavailable"
    | "deferred"
    | "absent";
  readonly label: string;
  readonly detail: string;
  readonly time: {
    readonly label: string;
    readonly value: IsoInstant;
  } | null;
  readonly nextAttemptAt: IsoInstant | null;
  readonly failure: string | null;
}

function sourcePresentation(
  state: SourceState | null,
  configured: boolean | null,
  catalogAvailable: boolean,
  playbackAvailable: boolean,
  sourceScope: "deployment" | "device",
): SourcePresentation {
  if (configured === false) {
    return sourcePresentationValue(
      "absent",
      "Not set up",
      absentGuideCopy(sourceScope, catalogAvailable, playbackAvailable),
    );
  }
  if (state === null || configured === null) {
    return sourcePresentationValue(
      "checking",
      "Checking",
      "Waiting for Sparrow to report this source.",
    );
  }

  switch (state._tag) {
    case "fresh":
      return sourcePresentationValue(
        "fresh",
        "Fresh",
        "The latest copy is in use.",
        { time: { label: "Checked", value: state.validatedAt } },
      );
    case "stale":
      return sourcePresentationValue(
        "stale",
        "Stale, showing the saved copy",
        "Sparrow keeps using the saved copy until a fresh one arrives.",
        {
          time: { label: "Last checked", value: state.validatedAt },
          nextAttemptAt: state.nextAttemptAt,
        },
      );
    case "refreshing":
      return sourcePresentationValue(
        "refreshing",
        state.validatedAt === null
          ? "Refreshing"
          : "Refreshing, showing the saved copy",
        state.validatedAt === null
          ? "Sparrow is loading this source for the first time."
          : "The saved copy stays in use while the refresh runs.",
        { time: { label: "Started", value: state.startedAt } },
      );
    case "failed":
      return sourcePresentationValue(
        "failed",
        state.validatedAt === null ? "Failed" : "Failed, showing the saved copy",
        state.validatedAt === null
          ? "This source has not loaded yet."
          : "The refresh failed. The saved copy stays in use.",
        {
          time:
            state.validatedAt === null
              ? null
              : { label: "Last checked", value: state.validatedAt },
          nextAttemptAt: state.nextAttemptAt,
          failure: safeFailureSummary(state.failure),
        },
      );
    case "unavailable":
      return sourcePresentationValue(
        "unavailable",
        "Unavailable",
        "This source has not loaded, so Sparrow has no copy of it.",
        {
          failure:
            state.failure === null ? null : safeFailureSummary(state.failure),
        },
      );
    case "deferred":
      return sourcePresentationValue(
        "deferred",
        state.validatedAt === null ? "Waiting" : "Waiting, showing the saved copy",
        state.validatedAt === null
          ? "The refresh starts when Sparrow is free to run it."
          : "The saved copy stays in use until the refresh can run.",
        { time: { label: "Waiting since", value: state.deferredAt } },
      );
  }
}

function absentGuideCopy(
  sourceScope: "deployment" | "device",
  catalogAvailable: boolean,
  playbackAvailable: boolean,
): string {
  const owner = sourceScope === "device" ? "This device" : "This server";
  if (catalogAvailable) {
    return playbackAvailable
      ? `${owner} has no guide source. You can still browse, search and play channels.`
      : `${owner} has no guide source. You can still browse and search channels.`;
  }
  return playbackAvailable
    ? `${owner} has no guide source. Browsing, search and playback start once the channel source loads.`
    : `${owner} has no guide source. Browsing and search start once the channel source loads.`;
}

function sourcePresentationValue(
  tone: SourcePresentation["tone"],
  label: string,
  detail: string,
  telemetry: Partial<
    Pick<SourcePresentation, "time" | "nextAttemptAt" | "failure">
  > = {},
): SourcePresentation {
  return {
    tone,
    label,
    detail,
    time: null,
    nextAttemptAt: null,
    failure: null,
    ...telemetry,
  };
}

function refreshSuccessCopy(report: RefreshReport): string {
  if (report.m3u._tag === "skipped" && report.m3u.reason === "fresh") {
    return "The channel source was already fresh, so only the guide was refreshed.";
  }
  if (report.m3u._tag === "not-modified") {
    return "The channel source has not changed since the last check.";
  }
  return report.status.generation === null
    ? "No catalog is available yet."
    : "The catalog is up to date.";
}

function refreshFailureDetail(
  report: RefreshReport,
  playbackAvailable: boolean,
): string {
  const availableFeatures = playbackAvailable
    ? "Browsing and playback"
    : "Channel browsing and search";
  const catalogAvailable = report.status.generation !== null;

  if (report.m3u._tag !== "failed") {
    return catalogAvailable
      ? `Any saved guide copy stays in use. ${availableFeatures} stay available because the channel source loaded.`
      : `The guide source failed and no channel source has loaded. ${availableFeatures} are unavailable.`;
  }

  return catalogAvailable
    ? `The channel source failed, but its saved copy stays in use. ${availableFeatures} stay available from that copy.`
    : `The channel source failed and there is no saved copy. ${availableFeatures} are unavailable until a refresh succeeds.`;
}

function refreshOutcomeSummary(outcome: RefreshOutcome): string {
  switch (outcome._tag) {
    case "not-configured":
      return "not set up";
    case "updated":
      return "updated";
    case "not-modified":
      return "checked, unchanged";
    case "skipped":
      return outcome.reason === "fresh"
        ? "skipped, already fresh"
        : "skipped, waiting to retry";
    case "failed":
      return `failed: ${safeFailureSummary(outcome.failure)}`;
  }
}

/** Words for one closed failure value; the source is named by the caller. */
function safeFailureSummary(failure: SafeFailure): string {
  switch (failure._tag) {
    case "source-access":
      return `${codeWords(failure.reason)}${
        failure.retryAfterSeconds === null
          ? ""
          : `, retry in ${failure.retryAfterSeconds} s`
      }`;
    case "source-read":
    case "snapshot-recovery":
    case "invalid-epg-format":
      return codeWords(failure.reason);
    case "snapshot":
      return `${codeWords(failure.reason)} during ${codeWords(failure.operation)}`;
    case "decoded-limit-exceeded":
      return `larger than ${failure.limitBytes} bytes`;
    case "invalid-format":
      return `${codeWords(failure.reason)}${
        failure.entry === null ? "" : ` at entry ${failure.entry}`
      }`;
    case "invalid-encoding":
      return "invalid encoding";
    case "no-playable-channels":
      return "no playable channels";
    case "no-epg-channels":
      return "no guide channels";
  }
}

function codeWords(code: string): string {
  return code.replace(/-/gu, " ");
}

function refreshErrorCopy(error: ClientError): {
  readonly title: string;
  readonly detail: string;
} {
  switch (error._tag) {
    case "authentication-required":
      return {
        title: "Sign in to refresh",
        detail: "Sign in to this Sparrow server, then refresh again.",
      };
    case "not-configured":
      return {
        title: "No channel source is set up",
        detail: "This server has no source to refresh.",
      };
    case "service-unavailable":
      return {
        title: "Refresh did not complete",
        detail: "The current catalog is unchanged and still in use. Try again shortly.",
      };
    case "transport":
      return {
        title: "Refresh result was not received",
        detail:
          "The refresh may still have completed. Sparrow is checking the sources; read their status above before trying again.",
      };
    case "catalog-unavailable":
      return {
        title: "No catalog is available",
        detail: "Refresh again when the source can be reached.",
      };
    case "invalid-input":
    case "not-found":
    case "stale-cursor":
    case "playback-failed":
    case "mpv-failed":
      return {
        title: "Refresh returned an unexpected result",
        detail: "Read the source status above, then try again.",
      };
    case "cancelled":
      return {
        title: "Refresh was cancelled",
        detail:
          "The refresh may still have completed. Sparrow is checking the sources before another refresh.",
      };
  }
}

function formatTime(value: IsoInstant): string {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
    hourCycle: "h23",
  }).format(new Date(value));
}

function safeDiagnostics(
  status: CatalogStatus | null,
  refreshResult: ClientResult<RefreshReport> | null,
  latestEvent: SparrowEvent | null,
): string {
  const lines = ["sparrow-safe-diagnostics/v1"];
  if (status === null) {
    lines.push("catalog.generation=unknown", "status=checking");
  } else {
    lines.push(
      `catalog.generation=${status.generation ?? "unavailable"}`,
      `configuration.m3u=${status.configuration.configured ? "configured" : "absent"}`,
      `configuration.epg=${status.configuration.epgConfigured ? "configured" : "absent"}`,
      ...sourceDiagnosticLines("m3u", status.m3u),
      ...(status.epg === null
        ? ["epg.state=not-configured"]
        : sourceDiagnosticLines("epg", status.epg)),
    );
  }

  if (refreshResult !== null) {
    if (refreshResult.ok) {
      lines.push(
        "refresh.trigger=manual",
        ...outcomeDiagnosticLines("refresh.m3u", refreshResult.value.m3u),
        ...(refreshResult.value.epg === null
          ? ["refresh.epg=not-configured"]
          : outcomeDiagnosticLines("refresh.epg", refreshResult.value.epg)),
      );
    } else {
      lines.push(`refresh.error=${refreshResult.error._tag}`);
    }
  }

  if (latestEvent !== null) {
    lines.push(
      `event.tag=${latestEvent._tag}`,
      `event.occurred-at=${latestEvent.occurredAt}`,
    );
    if (latestEvent._tag === "catalog-published") {
      lines.push(`event.generation=${latestEvent.generation}`);
    } else if (latestEvent._tag === "refresh-completed") {
      lines.push(
        `event.source=${latestEvent.source}`,
        ...outcomeDiagnosticLines("event.outcome", latestEvent.outcome),
      );
    }
  }
  return lines.join("\n");
}

function sourceDiagnosticLines(prefix: string, state: SourceState): string[] {
  const lines = [`${prefix}.state=${state._tag}`];
  switch (state._tag) {
    case "fresh":
      return [...lines, `${prefix}.validated-at=${state.validatedAt}`];
    case "stale":
      return [
        ...lines,
        `${prefix}.validated-at=${state.validatedAt}`,
        `${prefix}.next-attempt-at=${state.nextAttemptAt ?? "unscheduled"}`,
      ];
    case "unavailable":
      return state.failure === null
        ? [...lines, `${prefix}.failure=none`]
        : [...lines, ...safeFailureDiagnosticLines(prefix, state.failure)];
    case "refreshing":
      return [
        ...lines,
        `${prefix}.validated-at=${state.validatedAt ?? "none"}`,
        `${prefix}.started-at=${state.startedAt}`,
      ];
    case "deferred":
      return [
        ...lines,
        `${prefix}.validated-at=${state.validatedAt ?? "none"}`,
        `${prefix}.deferred-at=${state.deferredAt}`,
      ];
    case "failed":
      return [
        ...lines,
        `${prefix}.validated-at=${state.validatedAt ?? "none"}`,
        ...safeFailureDiagnosticLines(prefix, state.failure),
        `${prefix}.next-attempt-at=${state.nextAttemptAt}`,
      ];
  }
}

function outcomeDiagnosticLines(
  prefix: string,
  outcome: RefreshOutcome,
): string[] {
  const lines = [`${prefix}.outcome=${outcome._tag}`];
  switch (outcome._tag) {
    case "not-configured":
      return lines;
    case "updated":
    case "not-modified":
      return [...lines, `${prefix}.validated-at=${outcome.validatedAt}`];
    case "skipped":
      return [
        ...lines,
        `${prefix}.reason=${outcome.reason}`,
        `${prefix}.next-attempt-at=${outcome.nextAttemptAt}`,
      ];
    case "failed":
      return [
        ...lines,
        ...safeFailureDiagnosticLines(prefix, outcome.failure),
        `${prefix}.next-attempt-at=${outcome.nextAttemptAt}`,
      ];
  }
}

function safeFailureDiagnosticLines(
  prefix: string,
  failure: SafeFailure,
): string[] {
  const lines = [
    `${prefix}.failure=${failure._tag}`,
    `${prefix}.failure-source=${failure.source}`,
  ];
  switch (failure._tag) {
    case "source-access":
      return [
        ...lines,
        `${prefix}.failure-reason=${failure.reason}`,
        `${prefix}.retry-after-seconds=${failure.retryAfterSeconds ?? "none"}`,
      ];
    case "source-read":
    case "snapshot-recovery":
      return [...lines, `${prefix}.failure-reason=${failure.reason}`];
    case "snapshot":
      return [
        ...lines,
        `${prefix}.failure-operation=${failure.operation}`,
        `${prefix}.failure-reason=${failure.reason}`,
      ];
    case "decoded-limit-exceeded":
      return [...lines, `${prefix}.limit-bytes=${failure.limitBytes}`];
    case "invalid-format":
      return [
        ...lines,
        `${prefix}.entry=${failure.entry ?? "none"}`,
        `${prefix}.failure-reason=${failure.reason}`,
      ];
    case "invalid-epg-format":
      return [...lines, `${prefix}.failure-reason=${failure.reason}`];
    case "invalid-encoding":
    case "no-playable-channels":
    case "no-epg-channels":
      return lines;
  }
}
