import { Tooltip } from "@base-ui/react/tooltip";
import { useMemo, useState, type ReactNode } from "react";
import type {
  ChannelGroup,
  ChannelId,
  ChannelSummary,
  ClientError,
  GuideWindowChannel,
} from "../../client/contracts";
import { ChannelGroupLane } from "./channel-group-lane";
import {
  guideFamilies,
  preferredVariant,
  type VariantPreferences,
} from "./guide-families";
import {
  clockLabel,
  clockMarks,
  playheadPercent,
  type ClockWindow,
} from "./guide-window";
import { ProgrammeGuideRow } from "./programme-guide-row";
import "./programme-guide.css";

/** Inputs for the dense shell timetable. */
export interface ProgrammeGuideProps {
  readonly rows: readonly GuideWindowChannel[];
  readonly groups: readonly ChannelGroup[];
  readonly activeGroup: string | null;
  readonly window: ClockWindow;
  readonly now: Date;
  readonly playingChannel: ChannelId | null;
  /** The picture quality chosen per guide row. */
  readonly variantPreferences: VariantPreferences;
  /** Theater docks the guide under the picture; stacked puts it below the stage. */
  readonly layout: "theater" | "stacked";
  readonly loading: boolean;
  readonly replacing: boolean;
  readonly error: ClientError | null;
  readonly hasMore: boolean;
  readonly loadingMore: boolean;
  readonly emptyState?: {
    readonly title: string;
    readonly detail: string;
  };
  readonly onSelectGroup: (group: string | null) => void;
  readonly onPrefetchGroup: (group: string | null) => void;
  readonly excludedGroups: ReadonlySet<string>;
  readonly onSetGroupExcluded: (name: string, exclude: boolean) => void;
  readonly onRestoreExcludedGroups: () => void;
  readonly onPreparePlayback: () => void;
  readonly onTune: (channel: ChannelSummary) => void;
  /** Tunes one Quality Variant and keeps it as its row's choice. */
  readonly onTuneVariant: (channel: ChannelSummary) => void;
  readonly onRetry: () => void;
  readonly onLoadMore: () => void;
  /** The search field, or null when the shell shows it elsewhere. */
  readonly search: ReactNode;
  readonly feeds: ReactNode;
  /** Source freshness readouts, shown beside the guide's own controls. */
  readonly status: ReactNode;
}

/** Renders channel groups, the shared time axis, and overlapping Programme cells. */
export function ProgrammeGuide({
  rows,
  groups,
  activeGroup,
  window,
  now,
  playingChannel,
  variantPreferences,
  layout,
  loading,
  replacing,
  error,
  hasMore,
  loadingMore,
  emptyState,
  onSelectGroup,
  onPrefetchGroup,
  excludedGroups,
  onSetGroupExcluded,
  onRestoreExcludedGroups,
  onPreparePlayback,
  onTune,
  onTuneVariant,
  onRetry,
  onLoadMore,
  search,
  feeds,
  status,
}: ProgrammeGuideProps) {
  const marks = clockMarks(window);
  const families = useMemo(() => guideFamilies(rows), [rows]);
  const nowFraction = playheadPercent(window, now) / 100;
  const nowLeft = `calc(var(--guide-gutter) + (100% - var(--guide-gutter)) * ${nowFraction})`;
  const [channelNameTooltip] = useState(() => Tooltip.createHandle<string>());
  const boardEmpty =
    emptyState === undefined &&
    groups.length > 0 &&
    excludedGroups.size === groups.length;

  return (
    <section
      className="programme-guide"
      data-layout={layout}
      aria-label="Programme guide"
    >
      <header className="programme-guide__toolbar">
        {search}
        {feeds}
        <div className="programme-guide__status">{status}</div>
      </header>

      <div className="programme-guide__body">
        <ChannelGroupLane
          groups={groups}
          activeGroup={activeGroup}
          excluded={excludedGroups}
          onSelectGroup={onSelectGroup}
          onPrefetchGroup={onPrefetchGroup}
          onSetExcluded={onSetGroupExcluded}
          onRestoreAll={onRestoreExcludedGroups}
        />

        <div className="programme-guide__panel">
          <div
            className="programme-guide__board"
            aria-busy={loading || replacing}
          >
            <div className="programme-guide__ruler" aria-hidden="true">
              <span />
              <div>
                {marks.map((mark, index) => {
                  const fraction = index / marks.length;
                  return (
                    <time
                      key={mark.toISOString()}
                      className={mark.getMinutes() === 0 ? "is-hour" : undefined}
                      style={{
                        left: `${fraction * 100}%`,
                        "--from-now": fraction - nowFraction,
                      }}
                    >
                      {clockLabel(mark)}
                    </time>
                  );
                })}
              </div>
              <span className="programme-guide__now" style={{ left: nowLeft }}>
                {clockLabel(now)}
              </span>
            </div>

            {loading && rows.length === 0 ? (
              <GuideNotice tone="loading" title="Opening the guide">
                Loading channels and programme times.
              </GuideNotice>
            ) : error !== null && rows.length === 0 ? (
              <GuideNotice tone="error" title="The guide is unavailable">
                <span>{guideErrorCopy(error)}</span>
                <button type="button" onClick={onRetry}>
                  Try again
                </button>
              </GuideNotice>
            ) : rows.length === 0 ? (
              <GuideNotice
                tone="empty"
                title={
                  emptyState?.title ??
                  (boardEmpty ? "Every group is hidden" : "No channels here")
                }
              >
                {emptyState?.detail ??
                  (boardEmpty
                    ? "Open Choose groups and show a group to fill the guide."
                    : "This group has no channels right now.")}
              </GuideNotice>
            ) : (
              <Tooltip.Provider delay={400}>
                <div className="programme-guide__rows">
                  <div
                    className="programme-guide__playhead"
                    aria-hidden="true"
                    style={{ left: nowLeft }}
                  />
                  {families.map((family) => (
                    <ProgrammeGuideRow
                      key={family.number}
                      family={family}
                      preferred={preferredVariant(family, variantPreferences)}
                      window={window}
                      now={now}
                      playingChannel={
                        family.variants.some(
                          (variant) => variant.channel.id === playingChannel,
                        )
                          ? playingChannel
                          : null
                      }
                      channelNameTooltip={channelNameTooltip}
                      onPreparePlayback={onPreparePlayback}
                      onTune={onTune}
                      onTuneVariant={onTuneVariant}
                    />
                  ))}
                </div>
                <ChannelNameTooltip handle={channelNameTooltip} />
              </Tooltip.Provider>
            )}

            {error !== null && rows.length > 0 ? (
              <div className="programme-guide__retained" role="alert">
                The guide could not update. These are the last loaded channels.
                <button type="button" onClick={onRetry}>
                  Try again
                </button>
              </div>
            ) : null}

            {hasMore ? (
              <button
                className="programme-guide__more"
                type="button"
                disabled={loadingMore || replacing}
                onClick={onLoadMore}
              >
                {replacing
                  ? "Updating the guide…"
                  : loadingMore
                    ? "Loading more channels…"
                    : "More channels"}
              </button>
            ) : null}
          </div>
        </div>
      </div>
    </section>
  );
}

/** One Base UI tooltip reused across Channel name triggers. */
function ChannelNameTooltip({
  handle,
}: {
  readonly handle: Tooltip.Handle<string>;
}) {
  return (
    <Tooltip.Root disableHoverablePopup handle={handle}>
      {({ payload }) => (
        <Tooltip.Portal>
          <Tooltip.Positioner
            className="programme-guide__channel-tooltip-positioner"
            side="right"
            align="center"
            sideOffset={8}
          >
            <Tooltip.Popup
              className="programme-guide__channel-tooltip"
              role="tooltip"
            >
              {payload}
            </Tooltip.Popup>
          </Tooltip.Positioner>
        </Tooltip.Portal>
      )}
    </Tooltip.Root>
  );
}

function GuideNotice({
  children,
  title,
  tone,
}: {
  readonly children: ReactNode;
  readonly title: string;
  readonly tone: "loading" | "error" | "empty";
}) {
  return (
    <div
      className="programme-guide__notice"
      data-tone={tone}
      role={tone === "error" ? "alert" : "status"}
    >
      <strong>{title}</strong>
      <p>{children}</p>
    </div>
  );
}

function guideErrorCopy(error: ClientError): string {
  switch (error._tag) {
    case "cancelled":
      return "A newer guide request replaced this one.";
    case "authentication-required":
      return "Sign in again to see your channels.";
    case "not-configured":
      return "Add your sources before opening the guide.";
    case "catalog-unavailable":
      return "The channels have not loaded yet.";
    case "invalid-input":
    case "not-found":
    case "stale-cursor":
      return "The channels changed while the guide was opening.";
    case "mpv-failed":
    case "playback-failed":
    case "service-unavailable":
    case "transport":
      return "Sparrow could not reach the guide.";
  }
}
