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

/** Inputs for the independent hosted Source telemetry and refresh controls. */
export interface SourceStatusDeskProps {
  readonly status: CatalogStatus | null;
  readonly refreshing: boolean;
  readonly refreshResult: ClientResult<RefreshReport> | null;
  readonly latestEvent: SparrowEvent | null;
  readonly onRefresh: () => void;
}

/** Renders independent M3U/EPG state, manual refresh feedback, and safe diagnostics. */
export function SourceStatusDesk({
  status,
  refreshing,
  refreshResult,
  latestEvent,
  onRefresh,
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
        <div className="source-desk__index" aria-hidden="true">
          RX
        </div>
        <div>
          <p className="eyebrow">Independent source telemetry</p>
          <h2 id="source-desk-heading">Signal condition</h2>
        </div>
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
      </header>

      <div className="source-desk__grid" aria-live="polite">
        <SourceCard
          code="M3U"
          title="Channel source"
          state={status?.m3u ?? null}
        />
        <SourceCard
          code="EPG"
          title="Guide source"
          state={status?.epg ?? null}
          configured={status?.configuration.epgConfigured ?? null}
          catalogAvailable={status !== null && status.generation !== null}
        />
      </div>

      <RefreshFeedback result={refreshResult} />

      <details className="source-desk__diagnostics">
        <summary>Safe diagnostics / copyable</summary>
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
  code,
  title,
  state,
  configured = true,
  catalogAvailable = false,
}: {
  readonly code: "M3U" | "EPG";
  readonly title: string;
  readonly state: SourceState | null;
  readonly configured?: boolean | null;
  readonly catalogAvailable?: boolean;
}) {
  const presentation = sourcePresentation(state, configured, catalogAvailable);
  return (
    <article className="source-card" data-state={presentation.tone}>
      <div className="source-card__topline">
        <span>{code}</span>
        <strong>{presentation.label}</strong>
      </div>
      <h3>{title}</h3>
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
        <code>failure / {presentation.failure}</code>
      )}
    </article>
  );
}

function RefreshFeedback({
  result,
}: {
  readonly result: ClientResult<RefreshReport> | null;
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

  const failed = [
    result.value.m3u._tag === "failed" ? "Channel source" : null,
    result.value.epg?._tag === "failed" ? "Guide source" : null,
  ].filter((source): source is string => source !== null);
  const outcomes = [
    `Channel source: ${refreshOutcomeSummary(result.value.m3u)}`,
    `Guide source: ${
      result.value.epg === null
        ? "not configured"
        : refreshOutcomeSummary(result.value.epg)
    }`,
  ].join(" · ");
  return (
    <div
      className="refresh-feedback"
      data-tone={failed.length === 0 ? "complete" : "failed"}
      role={failed.length === 0 ? "status" : "alert"}
    >
      <strong>
        {failed.length === 0
          ? "Manual refresh complete"
          : `${failed.join(" and ")} refresh failed`}
      </strong>
      <p>
        {failed.length === 0
          ? `${outcomes}. ${refreshSuccessCopy(result.value)}`
          : `${outcomes}. Any last validated snapshot remains in service. Browsing and playback stay available when the Guide alone fails.`}
      </p>
    </div>
  );
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
): SourcePresentation {
  if (configured === false) {
    return sourcePresentationValue(
      "absent",
      "NOT CONFIGURED",
      catalogAvailable
        ? "This deployment has no Guide source. Channel browse, search, and playback remain available."
        : "This deployment has no Guide source. Browse, search, and playback require a validated Channel snapshot.",
    );
  }
  if (state === null || configured === null) {
    return sourcePresentationValue(
      "checking",
      "CHECKING",
      "Waiting for a safe status snapshot from Sparrow.",
    );
  }

  switch (state._tag) {
    case "fresh":
      return sourcePresentationValue(
        "fresh",
        "FRESH",
        "The latest validated snapshot is in service.",
        { time: { label: "Validated", value: state.validatedAt } },
      );
    case "stale":
      return sourcePresentationValue(
        "stale",
        "STALE / RETAINED",
        "The last validated snapshot remains usable while Sparrow awaits a fresh one.",
        {
          time: { label: "Last validated", value: state.validatedAt },
          nextAttemptAt: state.nextAttemptAt,
        },
      );
    case "refreshing":
      return sourcePresentationValue(
        "refreshing",
        state.validatedAt === null ? "REFRESHING" : "REFRESHING / RETAINED",
        state.validatedAt === null
          ? "The first validated snapshot is being prepared."
          : "The last validated snapshot remains in service during refresh.",
        { time: { label: "Started", value: state.startedAt } },
      );
    case "failed":
      return sourcePresentationValue(
        "failed",
        state.validatedAt === null ? "FAILED" : "FAILED / RETAINED",
        state.validatedAt === null
          ? "No validated snapshot is available yet."
          : "Refresh failed; the last validated snapshot remains in service.",
        {
          time:
            state.validatedAt === null
              ? null
              : { label: "Last validated", value: state.validatedAt },
          nextAttemptAt: state.nextAttemptAt,
          failure: safeFailureSummary(state.failure),
        },
      );
    case "unavailable":
      return sourcePresentationValue(
        "unavailable",
        "UNAVAILABLE",
        "No validated snapshot is available for this configured source.",
        {
          failure:
            state.failure === null ? null : safeFailureSummary(state.failure),
        },
      );
    case "deferred":
      return sourcePresentationValue(
        "deferred",
        state.validatedAt === null ? "DEFERRED" : "DEFERRED / RETAINED",
        state.validatedAt === null
          ? "Source work is deferred until Sparrow can safely continue."
          : "The last validated snapshot remains in service while refresh is deferred.",
        { time: { label: "Deferred", value: state.deferredAt } },
      );
  }
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
    return "The Channel source was already fresh; Guide work completed independently.";
  }
  if (report.m3u._tag === "not-modified") {
    return "The Channel source was revalidated without replacing its snapshot.";
  }
  return `Catalog generation ${report.status.generation ?? "unavailable"} now reflects the completed source outcomes.`;
}

function refreshOutcomeSummary(outcome: RefreshOutcome): string {
  switch (outcome._tag) {
    case "not-configured":
      return "not configured";
    case "updated":
      return "updated";
    case "not-modified":
      return "validated / unchanged";
    case "skipped":
      return `skipped / ${outcome.reason}`;
    case "failed":
      return `failed / ${safeFailureSummary(outcome.failure)}`;
  }
}

function safeFailureSummary(failure: SafeFailure): string {
  switch (failure._tag) {
    case "source-access":
      return `${failure.source} / ${failure.reason}${
        failure.retryAfterSeconds === null
          ? ""
          : ` / retry ${failure.retryAfterSeconds}s`
      }`;
    case "source-read":
    case "snapshot-recovery":
      return `${failure.source} / ${failure.reason}`;
    case "snapshot":
      return `${failure.source} / ${failure.operation} / ${failure.reason}`;
    case "decoded-limit-exceeded":
      return `${failure.source} / decoded limit ${failure.limitBytes} bytes`;
    case "invalid-format":
      return `m3u / ${failure.reason} / entry ${failure.entry ?? "unknown"}`;
    case "invalid-epg-format":
      return `epg / ${failure.reason}`;
    case "invalid-encoding":
    case "no-playable-channels":
    case "no-epg-channels":
      return `${failure.source} / ${failure._tag}`;
  }
}

function refreshErrorCopy(error: ClientError): {
  readonly title: string;
  readonly detail: string;
} {
  switch (error._tag) {
    case "authentication-required":
      return {
        title: "Refresh needs authentication",
        detail: "Authenticate with this Sparrow deployment, then request refresh again.",
      };
    case "not-configured":
      return {
        title: "No Channel source is configured",
        detail: "This hosted deployment has no source to refresh.",
      };
    case "service-unavailable":
      return {
        title: "Refresh did not complete",
        detail: "The current catalog is unchanged and can remain in use. Try again shortly.",
      };
    case "transport":
      return {
        title: "Refresh result was not received",
        detail:
          "The request may still have completed. Sparrow is checking source status; use the source cards above as the current record before trying again.",
      };
    case "catalog-unavailable":
      return {
        title: "No validated catalog is available",
        detail: "Sparrow retained the safe source status. Request refresh again when the source is available.",
      };
    case "invalid-input":
    case "not-found":
    case "stale-cursor":
    case "playback-failed":
      return {
        title: "Refresh returned an unexpected result",
        detail: "No private source detail was retained in the browser. Check Sparrow again.",
      };
    case "cancelled":
      return {
        title: "Refresh was cancelled",
        detail:
          "The request may still have completed. Sparrow is checking source status before another refresh.",
      };
  }
}

function formatTime(value: IsoInstant): string {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
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
