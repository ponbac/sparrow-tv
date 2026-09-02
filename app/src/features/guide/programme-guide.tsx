import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { Tooltip } from "@base-ui/react/tooltip";
import { useState, type ReactNode } from "react";
import type {
  ChannelGroup,
  ChannelId,
  ChannelSummary,
  ClientError,
  GuideWindowChannel,
  GuideProgramme,
  ProgrammeSlot,
} from "../../client/contracts";
import {
  clockLabel,
  clockMarks,
  playheadPercent,
  type ClockWindow,
} from "./guide-window";
import { ProgrammeGuideRow } from "./programme-guide-row";
import "./programme-guide.css";

const ALL_GROUPS = "all";
const GROUP_PREFIX = "group:";

/** The Programme currently highlighted in the guide. */
export interface GuideSelection {
  readonly channelId: ChannelId;
  readonly programme: ProgrammeSlot | null;
}

/** Inputs for the dense Split Stage timetable. */
export interface ProgrammeGuideProps {
  readonly rows: readonly GuideWindowChannel[];
  readonly groups: readonly ChannelGroup[];
  readonly activeGroup: string | null;
  readonly window: ClockWindow;
  readonly now: Date;
  readonly selection: GuideSelection | null;
  readonly playingChannel: ChannelId | null;
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
  readonly onPreparePlayback: () => void;
  readonly onTune: (
    channel: ChannelSummary,
    programme: GuideProgramme | null,
  ) => void;
  readonly onRetry: () => void;
  readonly onLoadMore: () => void;
  readonly search: ReactNode;
  readonly feeds: ReactNode;
}

/** Renders channel groups, the shared time axis, and overlapping Programme cells. */
export function ProgrammeGuide({
  rows,
  groups,
  activeGroup,
  window,
  now,
  selection,
  playingChannel,
  loading,
  replacing,
  error,
  hasMore,
  loadingMore,
  emptyState,
  onSelectGroup,
  onPrefetchGroup,
  onPreparePlayback,
  onTune,
  onRetry,
  onLoadMore,
  search,
  feeds,
}: ProgrammeGuideProps) {
  const marks = clockMarks(window);
  const activeFilter = filterValue(activeGroup);
  const [channelNameTooltip] = useState(() => Tooltip.createHandle<string>());

  return (
    <section className="programme-guide" aria-label="Programme guide">
      <header className="programme-guide__toolbar">
        {search}
        {feeds}
      </header>

      <div className="programme-guide__body">
        <RadioGroup
          className="programme-guide__groups"
          value={activeFilter}
          onValueChange={(value) => onSelectGroup(groupFromFilterValue(value))}
          aria-label="Channel groups"
        >
          <Radio.Root
            className="programme-guide__group"
            data-acceptance-group
            value={ALL_GROUPS}
            onMouseEnter={() => onPrefetchGroup(null)}
            onFocus={() => onPrefetchGroup(null)}
          >
            All
          </Radio.Root>
          {groups.map((group) => {
            const groupValue = filterValue(group.name);
            return (
              <Radio.Root
                className="programme-guide__group"
                data-acceptance-group
                key={groupValue}
                value={groupValue}
                onMouseEnter={() => onPrefetchGroup(group.name)}
                onFocus={() => onPrefetchGroup(group.name)}
              >
                {group.name === "" ? "Ungrouped" : group.name}
                <em>{group.channelCount}</em>
              </Radio.Root>
            );
          })}
        </RadioGroup>

        <div className="programme-guide__panel">
          <div className="programme-guide__ruler" aria-hidden="true">
            <span />
            <div>
              {marks.map((mark, index) => (
                <time
                  key={mark.toISOString()}
                  className={index % 2 === 0 ? "is-hour" : undefined}
                  style={{ left: `${(index / marks.length) * 100}%` }}
                >
                  {clockLabel(mark)}
                </time>
              ))}
            </div>
          </div>

          <div
            className="programme-guide__board"
            aria-busy={loading || replacing}
          >
            {rows.length > 0 ? (
              <div
                className="programme-guide__playhead"
                aria-hidden="true"
                style={{
                  left: `calc(var(--guide-gutter) + (100% - var(--guide-gutter)) * ${playheadPercent(window, now) / 100})`,
                }}
              />
            ) : null}

            {loading && rows.length === 0 ? (
              <GuideNotice tone="loading" title="Opening the guide window">
                Resolving Channels and Programme times from one catalog generation.
              </GuideNotice>
            ) : error !== null && rows.length === 0 ? (
              <GuideNotice tone="error" title="The guide window is unavailable">
                <span>{guideErrorCopy(error)}</span>
                <button type="button" onClick={onRetry}>
                  Try again
                </button>
              </GuideNotice>
            ) : rows.length === 0 ? (
              <GuideNotice
                tone="empty"
                title={emptyState?.title ?? "Nothing is patched here"}
              >
                {emptyState?.detail ??
                  "This group has no Channels in the current catalog window."}
              </GuideNotice>
            ) : (
              <Tooltip.Provider delay={400}>
                <div className="programme-guide__rows">
                  {rows.map((row, rowIndex) => {
                    const selected = selection?.channelId === row.channel.id;
                    return (
                      <ProgrammeGuideRow
                        key={row.channel.id}
                        row={row}
                        rowIndex={rowIndex}
                        window={window}
                        now={now}
                        selected={selected}
                        selectedProgramme={
                          selected ? (selection?.programme ?? null) : null
                        }
                        playing={playingChannel === row.channel.id}
                        channelNameTooltip={channelNameTooltip}
                        onPreparePlayback={onPreparePlayback}
                        onTune={onTune}
                      />
                    );
                  })}
                </div>
                <ChannelNameTooltip handle={channelNameTooltip} />
              </Tooltip.Provider>
            )}

            {error !== null && rows.length > 0 ? (
              <div className="programme-guide__retained" role="alert">
                Guide refresh failed; the visible window is retained.
                <button type="button" onClick={onRetry}>
                  Retry
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
                  ? "Updating generation…"
                  : loadingMore
                    ? "Opening more Channels…"
                    : "More Channels"}
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
      <span aria-hidden="true">{tone === "loading" ? "◌" : "⌁"}</span>
      <strong>{title}</strong>
      <p>{children}</p>
    </div>
  );
}

function filterValue(group: string | null): string {
  return group === null ? ALL_GROUPS : `${GROUP_PREFIX}${group}`;
}

function groupFromFilterValue(value: string): string | null {
  return value === ALL_GROUPS ? null : value.slice(GROUP_PREFIX.length);
}

function guideErrorCopy(error: ClientError): string {
  switch (error._tag) {
    case "cancelled":
      return "The previous guide request was replaced.";
    case "authentication-required":
      return "Sign in again to read the private catalog.";
    case "not-configured":
      return "Configure this receiver before opening the guide.";
    case "catalog-unavailable":
      return "No validated catalog generation is available yet.";
    case "invalid-input":
    case "not-found":
    case "stale-cursor":
      return "The catalog changed while this window was opening.";
    case "mpv-failed":
    case "playback-failed":
    case "service-unavailable":
    case "transport":
      return "Sparrow could not reach the guide service.";
  }
}
