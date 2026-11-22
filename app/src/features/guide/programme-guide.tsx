import { Tooltip } from "@base-ui/react/tooltip";
import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FocusEvent,
  type ReactNode,
} from "react";
import type {
  ChannelGroup,
  ChannelId,
  ChannelSummary,
  ClientError,
  GuideWindowChannel,
} from "../../client/contracts";
import {
  ChannelGroupLane,
  type GuideTime,
  type GuideView,
} from "./channel-group-lane";
import { useMoreButtonFocusHandoff } from "./dialog-navigation";
import {
  guideFamilies,
  preferredVariant,
  type GuideFamily,
  type VariantPreferences,
} from "./guide-families";
import {
  followTimelineWindow,
  handleGuideArrowKey,
  revealTimelineCell,
  scrollBoardToReveal,
  settleGuideNavigation,
} from "./guide-navigation";
import { GuidePreview, type GuidePreviewSource } from "./guide-preview";
import { createPreviewStore, previewFocusOf } from "./guide-preview-store";
import {
  clockLabel,
  clockMarks,
  windowDays,
  playheadPercent,
  type ClockWindow,
} from "./guide-window";
import { revealScrollTop } from "./list-scroll";
import { ProgrammeGuideRow } from "./programme-guide-row";
import { ProgrammeListRow } from "./programme-list-row";
import { ProgrammeTimelineRow } from "./programme-timeline-row";
import { TelevisionGuideRows, type TelevisionRowsHandle } from "./television-guide-rows";
import "./programme-guide.css";

/** Inputs for the guide: Theater's timetable or pocket's now-and-next list. */
export interface ProgrammeGuideProps {
  /** Keeps the More channels action reachable during a TV page load. */
  readonly television?: boolean;
  /** The TV Guide shares its toolbar with the shell's wordmark and clock. */
  readonly masthead?: { readonly wordmark: ReactNode; readonly clock: ReactNode };
  readonly rows: readonly GuideWindowChannel[];
  readonly groups: readonly ChannelGroup[];
  readonly activeGroup: string | null;
  readonly window: ClockWindow;
  readonly now: Date;
  readonly playingChannel: ChannelId | null;
  /** The most recently played Channel so Stop restores focus to its row. */
  readonly lastPlayedChannel?: ChannelId | null;
  /** The picture quality chosen per guide row. */
  readonly variantPreferences: VariantPreferences;
  /**
   * Theater docks the guide under the picture as a timeline; pocket puts it
   * below the stage as a list of what each Channel has on.
   */
  readonly layout: "theater" | "pocket";
  /**
   * The shell's mode. Entering guide mode brings pocket's playing row to the
   * middle of the list.
   */
  readonly mode: "watch" | "guide";
  /** The time pocket's list looks at; null in Theater. */
  readonly time: GuideTime | null;
  /**
   * The TV Guide's layout: the now-and-next grid or the timeline, and the
   * lane's chip that swaps them. Absent elsewhere.
   */
  readonly view?: GuideView | null;
  /**
   * Where the TV band's panel about the focused Channel reads descriptions.
   * Absent where there is no such panel.
   */
  readonly preview?: GuidePreviewSource | null;
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

const AUTO_LOAD_SCROLL_THRESHOLD_PX = 480;
const AUTO_LOAD_FOCUS_TAIL_ROWS = 8;

/**
 * Renders the Channel Groups and the guide's rows: in Theater a shared time
 * axis with overlapping Programme cells, in pocket one now-and-next row per
 * Channel.
 */
export const ProgrammeGuide = memo(function ProgrammeGuide({
  television = false,
  masthead,
  rows,
  groups,
  activeGroup,
  window: guideWindow,
  now,
  playingChannel,
  lastPlayedChannel = null,
  variantPreferences,
  layout,
  mode,
  time,
  view = null,
  preview = null,
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
  const previousFamilies = useRef<readonly GuideFamily[]>([]);
  const families = useMemo(() => guideFamilies(rows, previousFamilies.current), [rows]);
  useLayoutEffect(() => {
    previousFamilies.current = families;
  }, [families]);
  const timeline = television && view?.current === "timeline";
  const windowed = television && layout === "pocket";
  const televisionRows = useRef<TelevisionRowsHandle>(null);
  const revealIndex = useCallback((index: number) => {
    televisionRows.current?.revealIndex(index);
  }, []);
  const playingIndex = useMemo(() => {
    const channel = playingChannel ?? lastPlayedChannel;
    if (!windowed || channel === null) return -1;
    return families.findIndex((family) =>
      family.variants.some((variant) => variant.channel.id === channel),
    );
  }, [families, lastPlayedChannel, playingChannel, windowed]);
  const [previewStore] = useState(createPreviewStore);
  const navigating = useRef(false);
  const duringNavigation = (move: () => void) => {
    navigating.current = true;
    try {
      move();
    } finally {
      navigating.current = false;
    }
  };
  useLayoutEffect(() => () => previewStore.cancelPending(), [activeGroup, mode, previewStore, timeline]);
  const windowMinutes =
    (guideWindow.endsAt.getTime() - guideWindow.startsAt.getTime()) / 60_000;
  const nowMinutes = (now.getTime() - guideWindow.startsAt.getTime()) / 60_000;
  const nowFraction = playheadPercent(guideWindow, now) / 100;
  const nowLeft = `calc(var(--guide-gutter) + (100% - var(--guide-gutter)) * ${nowFraction})`;
  const [channelNameTooltip] = useState(() => Tooltip.createHandle<string>());
  const sectionRef = useRef<HTMLElement>(null);
  const boardRef = useListScroll({
    layout,
    mode,
    playingChannel,
    lastPlayedChannel,
    activeGroup,
  });
  const paginationBusy = loadingMore || replacing;
  const autoLoadTriggeredForCountRef = useRef<number | null>(null);
  const canAutoLoad = hasMore && !paginationBusy && error === null;
  const requestAutoLoad = useCallback(() => {
    if (!canAutoLoad || autoLoadTriggeredForCountRef.current === rows.length) {
      return;
    }
    autoLoadTriggeredForCountRef.current = rows.length;
    onLoadMore();
  }, [canAutoLoad, onLoadMore, rows.length]);
  const { buttonProps: moreButtonHandlers, firstNewRow } = useMoreButtonFocusHandoff({
    television,
    active: mode === "guide",
    busy: paginationBusy,
    hasError: error !== null,
    scopeKey: activeGroup ?? "__all__",
    rowCount: families.length,
    canAutoLoad,
    onRequestAutoLoad: requestAutoLoad,
    onManualLoadMore: onLoadMore,
    resolveTarget: ({ existingRows, hasError, buttonConnected }) => {
      const board = boardRef.current;
      if (board === null) return null;
      const channelButtons = board.querySelectorAll<HTMLButtonElement>(
        ".programme-guide__row .programme-guide__channel",
      );
      if (hasError) {
        return (
          board.querySelector<HTMLButtonElement>(
            ".programme-guide__retained button",
          ) ?? channelButtons[channelButtons.length - 1]
        );
      }
      const appended = windowed
        ? board.querySelector<HTMLButtonElement>(`[data-guide-index="${existingRows}"] .programme-guide__channel`)
        : channelButtons[existingRows];
      if (appended) return appended;
      return buttonConnected ? null : channelButtons[channelButtons.length - 1];
    },
    revealTarget: (target) => {
      const board = boardRef.current;
      if (board !== null) {
        scrollBoardToReveal(board, target);
      }
    },
  });

  useEffect(() => {
    if (
      autoLoadTriggeredForCountRef.current !== null &&
      (autoLoadTriggeredForCountRef.current !== rows.length || !canAutoLoad)
    ) {
      autoLoadTriggeredForCountRef.current = null;
    }
  }, [canAutoLoad, rows.length]);

  useEffect(() => {
    const board = boardRef.current;
    if (board === null || !canAutoLoad) {
      return;
    }
    const onScroll = () => {
      if (board.scrollTop <= 0) {
        return;
      }
      const threshold = Math.max(
        AUTO_LOAD_SCROLL_THRESHOLD_PX,
        board.clientHeight * 1.5,
      );
      if (board.scrollHeight - board.scrollTop - board.clientHeight <= threshold) {
        requestAutoLoad();
      }
    };
    // On large TV viewports (e.g. 4K Shield), the first page can fit inside
    // the multi-column grid with no scrollbar; scrolling downward still loads.
    const onWheel = (event: WheelEvent) => {
      if (event.deltaY <= 0) {
        return;
      }
      const threshold = Math.max(
        AUTO_LOAD_SCROLL_THRESHOLD_PX,
        board.clientHeight * 1.5,
      );
      if (board.scrollHeight - board.scrollTop - board.clientHeight <= threshold) {
        requestAutoLoad();
      }
    };
    board.addEventListener("scroll", onScroll, { passive: true });
    board.addEventListener("wheel", onWheel, { passive: true });
    return () => {
      board.removeEventListener("scroll", onScroll);
      board.removeEventListener("wheel", onWheel);
    };
  }, [boardRef, canAutoLoad, requestAutoLoad]);

  const onRowsFocus = useCallback(
    (event: FocusEvent<HTMLDivElement>) => {
      const focus = previewFocusOf(event.target);
      if (focus !== null) previewStore.set(focus, navigating.current);
      const board = boardRef.current;
      if (
        board !== null &&
        event.target instanceof HTMLElement &&
        event.target.classList.contains("programme-guide__programme")
      ) {
        revealTimelineCell(board, event.target);
      }
      if (!canAutoLoad || !(event.target instanceof Element)) {
        return;
      }
      const row = event.target.closest<HTMLElement>(".programme-guide__row");
      if (row === null) {
        return;
      }
      if (row.dataset.guideIndex !== undefined) {
        if (families.length - Number(row.dataset.guideIndex) <= AUTO_LOAD_FOCUS_TAIL_ROWS) {
          requestAutoLoad();
        }
        return;
      }
      let followingRows = 0;
      let sibling = row.nextElementSibling;
      while (sibling !== null && followingRows < AUTO_LOAD_FOCUS_TAIL_ROWS) {
        if (sibling.classList.contains("programme-guide__row")) {
          followingRows += 1;
        }
        sibling = sibling.nextElementSibling;
      }
      if (followingRows < AUTO_LOAD_FOCUS_TAIL_ROWS) {
        requestAutoLoad();
      }
    },
    [boardRef, canAutoLoad, families.length, previewStore, requestAutoLoad],
  );
  // A fresh timeline starts at the window's first half hour.
  useLayoutEffect(() => {
    const board = boardRef.current;
    if (board === null) return;
    board.style.setProperty("--tl-offset", "0");
    board.dataset.nowHidden = "false";
  }, [boardRef, timeline]);
  // When the window moves on, the timeline keeps showing the same time, so a
  // focused cell does not slide out from under the focus.
  const windowStart = guideWindow.startsAt.getTime();
  const shownWindowStart = useRef(windowStart);
  useLayoutEffect(() => {
    const moved = (windowStart - shownWindowStart.current) / 60_000;
    shownWindowStart.current = windowStart;
    const board = boardRef.current;
    if (board === null || !timeline) return;
    followTimelineWindow(board, moved);
  }, [boardRef, timeline, windowStart]);
  const boardEmpty =
    emptyState === undefined &&
    groups.length > 0 &&
    excludedGroups.size === groups.length;
  // What a row is given whichever body shows it.
  const rowProps = (family: GuideFamily) => ({
    family,
    preferred: preferredVariant(family, variantPreferences),
    now,
    playingChannel: family.variants.some(
      (variant) => variant.channel.id === playingChannel,
    )
      ? playingChannel
      : null,
    lastPlayed:
      playingChannel === null &&
      lastPlayedChannel !== null &&
      family.variants.some((variant) => variant.channel.id === lastPlayedChannel),
    onPreparePlayback,
    onTune,
    onTuneVariant,
  });

  const timeOpen = time?.open === true;
  const toggleTime = time?.onToggle;

  return (
    <section
      ref={sectionRef}
      className="programme-guide"
      data-layout={layout}
      data-view={view?.current}
      aria-label="Programme guide"
      onKeyDownCapture={(event) => {
        const section = sectionRef.current;
        if (section === null) return;
        duringNavigation(() => handleGuideArrowKey(event, {
          section,
          workspace: section.closest<HTMLElement>(".shell__workspace"),
          board: boardRef.current,
          layout,
          mode,
          television,
          televisionWindow: windowed ? { columns: timeline ? 1 : 3, familyCount: families.length, revealIndex } : undefined,
          hasMore,
          onRequestMore: requestAutoLoad,
        }));
      }}
      onKeyDown={(event) => {
        if (
          !timeOpen ||
          mode !== "guide" ||
          event.defaultPrevented ||
          event.nativeEvent.isComposing ||
          event.key !== "Escape" ||
          toggleTime === undefined
        ) {
          return;
        }
        event.preventDefault();
        const section = sectionRef.current;
        const fromTimes =
          event.target instanceof Node &&
          (section
            ?.querySelector(".programme-guide__times")
            ?.contains(event.target) ??
            false);
        toggleTime();
        if (fromTimes) {
          section
            ?.querySelector<HTMLElement>(".programme-guide__when")
            ?.focus({ preventScroll: true });
        }
      }}
    >
      <header className="programme-guide__toolbar">
        {masthead?.wordmark}
        {search}
        {feeds}
        <div className="programme-guide__status">{status}</div>
        {masthead?.clock}
      </header>

      {preview === null ? null : (
        <GuidePreview
          source={preview}
          store={previewStore}
          families={families}
          variantPreferences={variantPreferences}
          playingChannel={playingChannel}
          now={now}
          windowStart={guideWindow.startsAt}
        />
      )}

      <div className="programme-guide__body">
        <ChannelGroupLane
          television={television}
          groups={groups}
          activeGroup={activeGroup}
          time={time}
          view={view}
          orientation={timeline ? "vertical" : "horizontal"}
          excluded={excludedGroups}
          onSelectGroup={onSelectGroup}
          onPrefetchGroup={onPrefetchGroup}
          onSetExcluded={onSetGroupExcluded}
          onRestoreAll={onRestoreExcludedGroups}
        />

        <div className="programme-guide__panel">
          <div
            className="programme-guide__board"
            ref={boardRef}
            aria-busy={loading || replacing}
            data-view={timeline ? "timeline" : undefined}
            data-window-minutes={timeline ? windowMinutes : undefined}
            data-now-minutes={timeline ? nowMinutes : undefined}
            style={timeline ? { "--tl-now": nowMinutes } : undefined}
          >
            {layout === "theater" ? (
              <GuideRuler
                window={guideWindow}
                now={now}
                nowFraction={nowFraction}
                nowLeft={nowLeft}
              />
            ) : timeline ? (
              <TimelineRuler window={guideWindow} now={now} />
            ) : null}

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
            ) : layout === "theater" ? (
              <Tooltip.Provider delay={400}>
                <div className="programme-guide__rows" onFocus={onRowsFocus}>
                  <div
                    className="programme-guide__playhead"
                    aria-hidden="true"
                    style={{ left: nowLeft }}
                  />
                  {families.map((family) => (
                    <ProgrammeGuideRow
                      key={family.number}
                      {...rowProps(family)}
                      window={guideWindow}
                      channelNameTooltip={channelNameTooltip}
                    />
                  ))}
                </div>
                <ChannelNameTooltip handle={channelNameTooltip} />
              </Tooltip.Provider>
            ) : windowed ? (
              <TelevisionGuideRows
                key={JSON.stringify([activeGroup, timeline])}
                families={families}
                timeline={timeline}
                playingIndex={playingIndex}
                firstNewRow={firstNewRow}
                handle={televisionRows}
                onCommit={(board) => duringNavigation(() => settleGuideNavigation(board, {
                  familyCount: families.length,
                  hasMore,
                  busy: paginationBusy,
                  hasError: error !== null,
                  onRequestMore: requestAutoLoad,
                  revealIndex,
                }))}
                onFocus={onRowsFocus}
                renderRow={(family, index) => timeline ? (
                  <ProgrammeTimelineRow
                    key={family.number}
                    guideIndex={index}
                    {...rowProps(family)}
                    window={guideWindow}
                  />
                ) : (
                  <ProgrammeListRow
                    key={family.number}
                    guideIndex={index}
                    {...rowProps(family)}
                    channelIcon
                    at={time?.chosen ?? null}
                    pending={replacing}
                  />
                )}
              />
            ) : (
              <div className="programme-guide__rows" onFocus={onRowsFocus}>
                {families.map((family) => (
                  <ProgrammeListRow
                    key={family.number}
                    {...rowProps(family)}
                    channelIcon={television}
                    at={time?.chosen ?? null}
                    pending={replacing}
                  />
                ))}
              </div>
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
                aria-disabled={paginationBusy || undefined}
                {...moreButtonHandlers}
                onFocus={(event) => {
                  if (windowed) revealIndex(families.length - 1);
                  moreButtonHandlers.onFocus(event);
                }}
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
});

/**
 * Decides where pocket's list is scrolled to, and returns the ref for the
 * board that scrolls. Another Channel Group starts at its top. Entering guide
 * mode puts the playing row in the middle of the list. A change of Channel
 * while the list is on screen moves the list only when the row is out of
 * view, so a row just chosen stays under the finger. The offset is assigned
 * directly: `scrollIntoView` may also scroll the clipped ancestors and drag
 * the picture's box with them.
 */
function useListScroll({
  layout,
  mode,
  playingChannel,
  lastPlayedChannel,
  activeGroup,
}: Pick<
  ProgrammeGuideProps,
  "layout" | "mode" | "playingChannel" | "lastPlayedChannel" | "activeGroup"
>) {
  const boardRef = useRef<HTMLDivElement>(null);
  const lastMode = useRef<ProgrammeGuideProps["mode"] | null>(null);
  useLayoutEffect(() => {
    if (layout === "pocket" && boardRef.current !== null) {
      boardRef.current.scrollTop = 0;
    }
  }, [activeGroup, layout]);
  useLayoutEffect(() => {
    const entered = mode === "guide" && lastMode.current !== "guide";
    lastMode.current = mode;
    const board = boardRef.current;
    // Watch mode takes the guide out of the layout: nothing to scroll.
    if (layout !== "pocket" || board === null || board.clientHeight === 0) {
      return;
    }
    const row =
      board.querySelector<HTMLElement>(
        '.programme-guide__row[data-playing="true"]',
      ) ??
      board.querySelector<HTMLElement>(
        '.programme-guide__row[data-last-played="true"]',
      );
    if (row === null) {
      return;
    }
    const rowBox = row.getBoundingClientRect();
    const scrollTop = revealScrollTop(
      {
        top: rowBox.top - board.getBoundingClientRect().top + board.scrollTop,
        height: rowBox.height,
      },
      { scrollTop: board.scrollTop, height: board.clientHeight },
      entered,
    );
    if (scrollTop !== board.scrollTop) {
      board.scrollTop = scrollTop;
    }
  }, [lastPlayedChannel, layout, mode, playingChannel]);
  return boardRef;
}

/** The timeline's half-hour marks and the clock on its now-line. */
function GuideRuler({
  window,
  now,
  nowFraction,
  nowLeft,
}: {
  readonly window: ClockWindow;
  readonly now: Date;
  /** Where now falls in the window, from 0 to 1. */
  readonly nowFraction: number;
  readonly nowLeft: string;
}) {
  const marks = clockMarks(window);
  return (
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
  );
}

/**
 * The TV timeline's ruler: a mark at every half hour of the window, and a
 * tab on the now line. Placed against the first minute the timeline shows.
 */
function TimelineRuler({
  window,
  now,
}: {
  readonly window: ClockWindow;
  readonly now: Date;
}) {
  const start = window.startsAt.getTime();
  return (
    <div className="programme-guide__timeline-ruler" aria-hidden="true">
      {/* The day of the first minute shown. Each day the window touches has
          its label; CSS shows the one under the board's --tl-offset. */}
      <span className="programme-guide__timeline-day">
        {windowDays(window, now).map((day, index, days) => (
          <span
            key={day.fromMinute}
            style={{
              "--day-from": index === 0 ? -1_000_000 : day.fromMinute,
              "--day-to": index === days.length - 1 ? 1_000_000 : day.toMinute,
            }}
          >
            {day.label}
          </span>
        ))}
      </span>
      <div className="programme-guide__ticks">
        {clockMarks(window).map((mark) => (
          <time
            key={mark.toISOString()}
            style={{ "--at": (mark.getTime() - start) / 60_000 }}
          >
            {clockLabel(mark)}
          </time>
        ))}
        <span className="programme-guide__now-tab">Now {clockLabel(now)}</span>
      </div>
    </div>
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
