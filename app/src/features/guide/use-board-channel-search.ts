import { useInfiniteQuery } from "@tanstack/react-query";
import { useCallback, useMemo } from "react";
import type {
  CatalogGeneration,
  ChannelSummary,
  ClientError,
  Page,
  PageCursor,
  SparrowClient,
} from "../../client/contracts";
import {
  clientErrorFromQuery,
  generationBoundResult,
} from "../../client/query-result";

const CHANNEL_SEARCH_PAGE_SIZE = 40;
const IMMUTABLE_CATALOG_STALE_TIME = Number.POSITIVE_INFINITY;

type ChannelSearchPageResult = {
  readonly ok: true;
  readonly value: Page<ChannelSummary>;
};

interface ChannelSearchContinuation {
  readonly cursor: PageCursor;
  readonly previousCursors: readonly PageCursor[];
  readonly generation: CatalogGeneration;
}

/** Inputs for a generation-bound, independently paginated Channel search lane. */
export interface BoardChannelSearchInput {
  readonly client: Pick<SparrowClient, "searchChannels">;
  readonly term: string;
  readonly generation: CatalogGeneration | null;
  readonly enabled: boolean;
}

/** One Channel search lane, with continuation details kept private. */
export interface BoardChannelSearchRead {
  readonly channels: readonly ChannelSummary[];
  readonly loading: boolean;
  readonly error: ClientError | null;
  readonly hasMore: boolean;
  readonly loadingMore: boolean;
  readonly retry: () => void;
  readonly loadMore: () => void;
}

/**
 * Owns generation-bound Channel search pagination for the board search desk.
 * Results are the complete catalog ranking; board-group visibility is applied
 * by the desk after the page arrives.
 */
export function useBoardChannelSearch({
  client,
  term,
  generation,
  enabled,
}: BoardChannelSearchInput): BoardChannelSearchRead {
  const searchQuery = useInfiniteQuery({
    queryKey: [
      "catalog",
      "search",
      "desk-channels",
      term,
      generation,
      CHANNEL_SEARCH_PAGE_SIZE,
    ],
    initialPageParam: null as ChannelSearchContinuation | null,
    queryFn: ({ pageParam, signal }) =>
      generationBoundResult(
        client.searchChannels({
          term,
          limit: CHANNEL_SEARCH_PAGE_SIZE,
          ...(pageParam === null
            ? {}
            : {
                cursor: pageParam.cursor,
                previousCursors: pageParam.previousCursors,
              }),
          signal,
        }),
        generation ?? pageParam?.generation ?? null,
      ),
    getNextPageParam: (lastPage, _pages, lastPageParam) =>
      nextContinuation(lastPage, lastPageParam),
    enabled,
    retry: false,
    staleTime: IMMUTABLE_CATALOG_STALE_TIME,
  });
  const { fetchNextPage, refetch } = searchQuery;
  const error = clientErrorFromQuery(searchQuery.error);
  const channels = useMemo(
    () => collectPageItems(searchQuery.data?.pages, generation),
    [generation, searchQuery.data?.pages],
  );
  const retry = useCallback(() => {
    void refetch();
  }, [refetch]);
  const loadMore = useCallback(() => {
    void fetchNextPage();
  }, [fetchNextPage]);

  return {
    channels,
    loading: enabled && searchQuery.isPending,
    error,
    hasMore: searchQuery.hasNextPage === true,
    loadingMore: searchQuery.isFetchingNextPage,
    retry,
    loadMore,
  };
}

function nextContinuation(
  lastPage: ChannelSearchPageResult,
  lastPageParam: ChannelSearchContinuation | null,
): ChannelSearchContinuation | null {
  if (lastPage.value.next === null) {
    return null;
  }
  return {
    cursor: lastPage.value.next,
    previousCursors:
      lastPageParam === null
        ? []
        : [...lastPageParam.previousCursors, lastPageParam.cursor],
    generation: lastPage.value.generation,
  };
}

function collectPageItems(
  pages: readonly ChannelSearchPageResult[] | undefined,
  expectedGeneration: CatalogGeneration | null,
): readonly ChannelSummary[] {
  return (pages ?? []).flatMap((page) =>
    expectedGeneration === null || page.value.generation === expectedGeneration
      ? page.value.items
      : [],
  );
}
