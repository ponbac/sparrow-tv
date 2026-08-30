import { useInfiniteQuery } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import type {
  CatalogGeneration,
  ChannelDetails,
  ChannelId,
  ClientResult,
  IsoInstant,
  PageCursor,
  SparrowClient,
} from "../../client/contracts";
import {
  emptyScheduleCopy,
  type GuidePresentation,
} from "./guide-presentation";
import { formatProgrammeTime, programmeKey } from "./programme-presentation";
import {
  collectGenerationItems,
  firstGeneration,
  firstResultError,
  hasUnexpectedGeneration,
} from "./generated-pages";
import {
  GenerationNotice,
  SearchLaneError,
  SearchLaneLoading,
} from "./search-feedback";

const PAGE_SIZE = 8;
const FIRST_PAGE: SchedulePageParam = null;

interface ScheduleContinuation {
  readonly cursor: PageCursor;
  readonly afterStartsAt: IsoInstant;
  readonly previousCursors: readonly PageCursor[];
}

type SchedulePageParam = ScheduleContinuation | null;

/** Displays one selected Channel's generation-bound, independently paginated schedule. */
export function ScheduleDesk({
  client,
  guide,
  selectedChannel,
  selectedDetails,
  selectedLoading,
  revision,
  focusRevision,
  catalogGeneration,
  onRestart,
  onRetrySelectedDetails,
}: {
  readonly client: SparrowClient;
  readonly guide: GuidePresentation;
  readonly selectedChannel: ChannelId | null;
  readonly selectedDetails: ClientResult<ChannelDetails> | undefined;
  readonly selectedLoading: boolean;
  readonly revision: number;
  readonly focusRevision: number;
  readonly catalogGeneration: CatalogGeneration | null;
  readonly onRestart: () => void;
  readonly onRetrySelectedDetails: () => void;
}) {
  const headingRef = useRef<HTMLHeadingElement>(null);

  useEffect(() => {
    if (focusRevision === 0 || !isMobileViewport()) {
      return;
    }
    const heading = headingRef.current;
    if (heading === null) {
      return;
    }
    heading.focus({ preventScroll: true });
    heading.scrollIntoView?.({
      behavior: prefersReducedMotion() ? "auto" : "smooth",
      block: "start",
    });
  }, [focusRevision]);

  return (
    <aside className="schedule-desk" aria-labelledby="schedule-heading">
      <header>
        <p>C / selected rundown</p>
        <h3 id="schedule-heading" ref={headingRef} tabIndex={-1}>
          Programme schedule
        </h3>
      </header>
      {selectedChannel === null ? (
        <div className="schedule-idle">
          <span aria-hidden="true">⌁</span>
          <p>Select a Channel or Programme result to open its matched schedule.</p>
        </div>
      ) : (
        <ActiveSchedule
          key={`${selectedChannel}:${revision}`}
          client={client}
          guide={guide}
          selectedChannel={selectedChannel}
          selectedDetails={selectedDetails}
          selectedLoading={selectedLoading}
          revision={revision}
          catalogGeneration={catalogGeneration}
          onRestart={onRestart}
          onRetrySelectedDetails={onRetrySelectedDetails}
        />
      )}
    </aside>
  );
}

function ActiveSchedule({
  client,
  guide,
  selectedChannel,
  selectedDetails,
  selectedLoading,
  revision,
  catalogGeneration,
  onRestart,
  onRetrySelectedDetails,
}: {
  readonly client: SparrowClient;
  readonly guide: GuidePresentation;
  readonly selectedChannel: ChannelId;
  readonly selectedDetails: ClientResult<ChannelDetails> | undefined;
  readonly selectedLoading: boolean;
  readonly revision: number;
  readonly catalogGeneration: CatalogGeneration | null;
  readonly onRestart: () => void;
  readonly onRetrySelectedDetails: () => void;
}) {
  const scheduleQuery = useInfiniteQuery({
    queryKey: [
      "catalog",
      "schedule",
      selectedChannel,
      revision,
      catalogGeneration,
    ],
    initialPageParam: FIRST_PAGE,
    queryFn: ({ pageParam, signal }) =>
      client.schedule({
        id: selectedChannel,
        limit: PAGE_SIZE,
        ...(pageParam === null
          ? {}
          : {
              cursor: pageParam.cursor,
              afterStartsAt: pageParam.afterStartsAt,
              previousCursors: pageParam.previousCursors,
            }),
        signal,
      }),
    getNextPageParam: (lastPage, _pages, lastPageParam) => {
      if (!lastPage.ok || lastPage.value.next === null) {
        return null;
      }
      const lastProgramme = lastPage.value.items.at(-1);
      return lastProgramme === undefined
        ? null
        : {
            cursor: lastPage.value.next,
            afterStartsAt: lastProgramme.startsAt,
            previousCursors:
              lastPageParam === null
                ? []
                : [
                    ...lastPageParam.previousCursors,
                    lastPageParam.cursor,
                  ],
          };
    },
  });
  const expectedGeneration = firstGeneration(scheduleQuery.data?.pages);
  const generationChanged = hasUnexpectedGeneration(
    scheduleQuery.data?.pages,
    expectedGeneration,
  );
  const programmes = collectGenerationItems(
    scheduleQuery.data?.pages,
    expectedGeneration,
    (page) => page.items,
  );
  const error = firstResultError(scheduleQuery.data?.pages);
  const channelName =
    selectedDetails?.ok === true ? selectedDetails.value.name : "Selected Channel";
  const detailError =
    selectedDetails !== undefined && !selectedDetails.ok
      ? selectedDetails.error
      : null;

  const loadMore = () => {
    scheduleQuery.fetchNextPage().catch(() => undefined);
  };

  return (
    <div className="active-schedule" aria-live="polite">
      {selectedLoading ? (
        <p className="schedule-channel-name">Resolving Channel…</p>
      ) : detailError === null ? (
        <p className="schedule-channel-name">{channelName}</p>
      ) : (
        <SearchLaneError
          error={detailError}
          onRestart={onRetrySelectedDetails}
          retained={false}
        />
      )}

      {scheduleQuery.isPending ? (
        <SearchLaneLoading label="Opening schedule" />
      ) : programmes.length === 0 && error === null ? (
        <div className="schedule-empty">
          <strong>No matched Programme data</strong>
          <p>{emptyScheduleCopy(guide)}</p>
        </div>
      ) : (
        <ol
          className="schedule-list"
          tabIndex={0}
          aria-label={`Programme times for ${channelName}`}
        >
          {programmes.map((programme, index) => (
            <li key={programmeKey(programme, index)}>
              <time dateTime={programme.startsAt}>
                {formatProgrammeTime(programme)}
              </time>
              <strong>{programme.title}</strong>
              <p>{programme.description ?? "No synopsis supplied."}</p>
            </li>
          ))}
        </ol>
      )}

      {generationChanged ? <GenerationNotice onRestart={onRestart} /> : null}

      {error === null ? null : (
        <SearchLaneError
          error={error}
          onRestart={onRestart}
          retained={programmes.length > 0}
        />
      )}
      {scheduleQuery.hasNextPage && !generationChanged ? (
        <button
          className="lane-more"
          type="button"
          disabled={scheduleQuery.isFetchingNextPage}
          onClick={loadMore}
        >
          {scheduleQuery.isFetchingNextPage
            ? "Opening more Programme times…"
            : "Later Programmes +"}
        </button>
      ) : null}
    </div>
  );
}

function isMobileViewport(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia("(max-width: 760px)").matches
  );
}

function prefersReducedMotion(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}
