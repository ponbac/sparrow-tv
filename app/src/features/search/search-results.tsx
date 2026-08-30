import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type {
  CatalogGeneration,
  ChannelId,
  ChannelSummary,
  ClientError,
  ClientResult,
  PageCursor,
  ProgrammeSummary,
  SearchResults,
  SparrowClient,
} from "../../client/contracts";
import {
  emptyProgrammeSearchCopy,
  type GuidePresentation,
} from "./guide-presentation";
import { formatProgrammeTime, programmeKey } from "./programme-presentation";
import {
  collectGenerationItems,
  firstResultError,
  hasUnexpectedGeneration,
} from "./generated-pages";
import {
  GenerationNotice,
  SearchLaneError,
  SearchLaneLoading,
} from "./search-feedback";

const CHANNEL_PAGE_SIZE = 12;
const PROGRAMME_PAGE_SIZE = 10;
const FIRST_PAGE: PageCursor | null = null;

interface SearchContinuation {
  readonly cursor: PageCursor;
  readonly previousCursors: readonly PageCursor[];
}

/**
 * Runs independently paginated Channel and Programme searches for one submitted term.
 * A revision begins both lanes from their first page after catalog invalidation.
 */
export function SearchResultsPanel({
  client,
  guide,
  term,
  revision,
  catalogGeneration,
  onRestart,
  onSelectChannel,
}: {
  readonly client: SparrowClient;
  readonly guide: GuidePresentation;
  readonly term: string;
  readonly revision: number;
  readonly catalogGeneration: CatalogGeneration | null;
  readonly onRestart: () => void;
  readonly onSelectChannel: (id: ChannelId) => void;
}) {
  const initialQuery = useQuery({
    queryKey: [
      "catalog",
      "search",
      "initial",
      term,
      revision,
      catalogGeneration,
    ],
    queryFn: ({ signal }) =>
      client.search({
        term,
        channelLimit: CHANNEL_PAGE_SIZE,
        programmeLimit: PROGRAMME_PAGE_SIZE,
        signal,
      }),
  });
  const initialResult = initialQuery.data;
  const initialValue = initialResult?.ok === true ? initialResult.value : null;
  const channelStart = initialValue?.channels.next ?? FIRST_PAGE;
  const programmeStart = initialValue?.programmes.next ?? FIRST_PAGE;

  const channelQuery = useInfiniteQuery({
    queryKey: [
      "catalog",
      "search",
      "channel-continuations",
      term,
      revision,
      catalogGeneration,
    ],
    initialPageParam: nextContinuation(channelStart, null),
    enabled: false,
    queryFn: ({ pageParam, signal }) =>
      client.searchChannels({
        term,
        limit: CHANNEL_PAGE_SIZE,
        ...(pageParam === null
          ? {}
          : {
              cursor: pageParam.cursor,
              previousCursors: pageParam.previousCursors,
            }),
        signal,
      }),
    getNextPageParam: (lastPage, _pages, lastPageParam) =>
      lastPage.ok
        ? nextContinuation(lastPage.value.next, lastPageParam)
        : null,
  });
  const programmeQuery = useInfiniteQuery({
    queryKey: [
      "catalog",
      "search",
      "programme-continuations",
      term,
      revision,
      catalogGeneration,
    ],
    initialPageParam: nextContinuation(programmeStart, null),
    enabled: false,
    queryFn: ({ pageParam, signal }) =>
      client.searchProgrammes({
        term,
        limit: PROGRAMME_PAGE_SIZE,
        ...(pageParam === null
          ? {}
          : {
              cursor: pageParam.cursor,
              previousCursors: pageParam.previousCursors,
            }),
        signal,
      }),
    getNextPageParam: (lastPage, _pages, lastPageParam) =>
      lastPage.ok
        ? nextContinuation(lastPage.value.next, lastPageParam)
        : null,
  });

  const expectedGeneration = initialValue?.generation ?? null;
  const channelGenerationChanged = hasUnexpectedGeneration(
    channelQuery.data?.pages,
    expectedGeneration,
  );
  const programmeGenerationChanged = hasUnexpectedGeneration(
    programmeQuery.data?.pages,
    expectedGeneration,
  );
  const generationChanged =
    channelGenerationChanged || programmeGenerationChanged;
  const channels = [
    ...(initialValue?.channels.items ?? []),
    ...collectGenerationItems(
      channelQuery.data?.pages,
      expectedGeneration,
      (page) => page.items,
    ),
  ];
  const programmes = [
    ...(initialValue?.programmes.items ?? []),
    ...collectGenerationItems(
      programmeQuery.data?.pages,
      expectedGeneration,
      (page) => page.items,
    ),
  ];
  const initialError = resultError(initialResult);
  const channelError = firstResultError(channelQuery.data?.pages);
  const programmeError = firstResultError(programmeQuery.data?.pages);
  const hasMoreChannels =
    !generationChanged &&
    (channelQuery.data === undefined
      ? channelStart !== null
      : channelQuery.hasNextPage === true);
  const hasMoreProgrammes =
    !generationChanged &&
    (programmeQuery.data === undefined
      ? programmeStart !== null
      : programmeQuery.hasNextPage === true);

  const loadMoreChannels = () => {
    channelQuery.fetchNextPage().catch(() => undefined);
  };
  const loadMoreProgrammes = () => {
    programmeQuery.fetchNextPage().catch(() => undefined);
  };

  return (
    <div
      className="search-results"
      role="group"
      aria-label={`Search results for ${term}`}
    >
      {generationChanged ? (
        <GenerationNotice onRestart={onRestart} />
      ) : null}
      {initialError === null ? null : (
        <div className="search-initial-error">
          <SearchLaneError
            error={initialError}
            onRestart={onRestart}
            retained={false}
          />
        </div>
      )}
      <ResultLane
        title="Channels"
        label="A / identity matches"
        count={channels.length}
        loading={initialQuery.isPending}
        loadingMore={channelQuery.isFetching}
        hasNextPage={hasMoreChannels}
        error={channelError}
        onLoadMore={loadMoreChannels}
        onRestart={onRestart}
      >
        {initialError === null &&
        channels.length === 0 &&
        !initialQuery.isPending &&
        channelError === null ? (
          <SearchEmpty>
            No Channels match <q>{term}</q> in this generation.
          </SearchEmpty>
        ) : (
          <div className="channel-search-list">
            {channels.map((channel) => (
              <ChannelSearchResult
                key={channel.id}
                channel={channel}
                onSelect={onSelectChannel}
              />
            ))}
          </div>
        )}
      </ResultLane>

      <ResultLane
        title="Programmes"
        label="B / Guide matches"
        count={programmes.length}
        loading={initialQuery.isPending}
        loadingMore={programmeQuery.isFetching}
        hasNextPage={hasMoreProgrammes}
        error={programmeError}
        onLoadMore={loadMoreProgrammes}
        onRestart={onRestart}
      >
        {initialError === null &&
        programmes.length === 0 &&
        !initialQuery.isPending &&
        programmeError === null ? (
          <SearchEmpty>{emptyProgrammeSearchCopy(guide, term)}</SearchEmpty>
        ) : (
          <div className="programme-search-list">
            {programmes.map((programme, index) => (
              <ProgrammeResult
                key={programmeKey(programme, index)}
                programme={programme}
                onSelect={onSelectChannel}
              />
            ))}
          </div>
        )}
      </ResultLane>
    </div>
  );
}

function ResultLane({
  title,
  label,
  count,
  loading,
  loadingMore,
  hasNextPage,
  error,
  onLoadMore,
  onRestart,
  children,
}: {
  readonly title: string;
  readonly label: string;
  readonly count: number;
  readonly loading: boolean;
  readonly loadingMore: boolean;
  readonly hasNextPage: boolean;
  readonly error: ClientError | null;
  readonly onLoadMore: () => void;
  readonly onRestart: () => void;
  readonly children: ReactNode;
}) {
  const headingId = `lane-${title.toLowerCase()}`;
  return (
    <section className="result-lane" aria-labelledby={headingId}>
      <header>
        <div>
          <p>{label}</p>
          <h3 id={headingId}>{title}</h3>
        </div>
        <span>
          <span aria-hidden="true">{String(count).padStart(2, "0")}</span>
          <span className="sr-only">{count} {title} loaded</span>
        </span>
      </header>
      {loading ? <SearchLaneLoading label={`Searching ${title}`} /> : children}
      {error === null ? null : (
        <SearchLaneError error={error} onRestart={onRestart} retained={count > 0} />
      )}
      {hasNextPage ? (
        <button
          className="lane-more"
          type="button"
          disabled={loadingMore}
          onClick={onLoadMore}
        >
          {loadingMore ? `Scanning more ${title}…` : `More ${title} +`}
        </button>
      ) : null}
    </section>
  );
}

function ChannelSearchResult({
  channel,
  onSelect,
}: {
  readonly channel: ChannelSummary;
  readonly onSelect: (id: ChannelId) => void;
}) {
  return (
    <button
      className="channel-search-result"
      type="button"
      title={channel.name}
      onClick={() => onSelect(channel.id)}
    >
      <span>{channel.group || "Ungrouped"}</span>
      <strong>{channel.name}</strong>
      <small>Open schedule ↗</small>
    </button>
  );
}

function ProgrammeResult({
  programme,
  onSelect,
}: {
  readonly programme: ProgrammeSummary;
  readonly onSelect: (id: ChannelId) => void;
}) {
  return (
    <button
      className="programme-result"
      type="button"
      onClick={() => onSelect(programme.channelId)}
    >
      <time dateTime={programme.startsAt}>{formatProgrammeTime(programme)}</time>
      <strong>{programme.title}</strong>
      <span>{programme.description ?? "No synopsis supplied."}</span>
      <small>Open Channel schedule ↗</small>
    </button>
  );
}

function SearchEmpty({ children }: { readonly children: ReactNode }) {
  return <p className="search-empty">{children}</p>;
}

function resultError(result: ClientResult<SearchResults> | undefined): ClientError | null {
  return result !== undefined && !result.ok ? result.error : null;
}

function nextContinuation(
  cursor: PageCursor | null,
  current: SearchContinuation | null,
): SearchContinuation | null {
  if (cursor === null) {
    return null;
  }
  return {
    cursor,
    previousCursors:
      current === null
        ? []
        : [...current.previousCursors, current.cursor],
  };
}
