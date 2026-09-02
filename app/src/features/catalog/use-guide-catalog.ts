import {
  infiniteQueryOptions,
  keepPreviousData,
  useInfiniteQuery,
  useQueryClient,
  type InfiniteData,
  type QueryClient,
} from "@tanstack/react-query";
import { useCallback, useEffect, useMemo } from "react";
import type {
  CatalogGeneration,
  ChannelGroup,
  ClientError,
  GuideWindowChannel,
  IsoInstant,
  Page,
  PageCursor,
  SparrowClient,
} from "../../client/contracts";
import {
  clientErrorFromQuery,
  generationBoundResult,
} from "../../client/query-result";

const GROUP_PAGE_SIZE = 100;
const GUIDE_CHANNEL_PAGE_SIZE = 40;
const IMMUTABLE_CATALOG_STALE_TIME = Number.POSITIVE_INFINITY;

type CatalogPageResult<Value> = {
  readonly ok: true;
  readonly value: Page<Value>;
};

type CatalogInfiniteData<Value> = InfiniteData<
  CatalogPageResult<Value>,
  CatalogContinuation | null
>;

interface CatalogContinuation {
  readonly cursor: PageCursor;
  readonly previousCursors: readonly PageCursor[];
  readonly generation: CatalogGeneration;
}

interface GuideCatalogInput {
  readonly client: SparrowClient;
  readonly enabled: boolean;
  readonly group: string | null;
  readonly startsAt: IsoInstant;
  readonly endsAt: IsoInstant;
  readonly expectedGeneration: CatalogGeneration | null;
}

/** One coherent guide read, with endpoint continuation details kept private. */
export interface GuideCatalogRead {
  readonly groups: readonly ChannelGroup[];
  readonly rows: readonly GuideWindowChannel[];
  readonly loading: boolean;
  readonly replacing: boolean;
  readonly error: ClientError | null;
  readonly hasMore: boolean;
  readonly loadingMore: boolean;
  readonly retry: () => void;
  readonly loadMore: () => void;
  readonly prefetchGroup: (group: string | null) => void;
}

/** Owns generation-bound group and guide pagination for the Split Stage. */
export function useGuideCatalog({
  client,
  enabled,
  group,
  startsAt,
  endsAt,
  expectedGeneration,
}: GuideCatalogInput): GuideCatalogRead {
  const queryClient = useQueryClient();
  const groupsGeneration = bootstrapAwareGeneration(
    queryClient,
    groupsQueryKey(null),
    expectedGeneration,
  );
  const groupsQuery = useInfiniteQuery({
    queryKey: groupsQueryKey(groupsGeneration),
    initialPageParam: null as CatalogContinuation | null,
    queryFn: ({ pageParam, signal }) =>
      generationBoundResult(
        client.listGroups({
          limit: GROUP_PAGE_SIZE,
          ...(pageParam === null
            ? {}
            : {
                cursor: pageParam.cursor,
                previousCursors: pageParam.previousCursors,
              }),
          signal,
        }),
        groupsGeneration ?? pageParam?.generation ?? null,
      ),
    getNextPageParam: (lastPage, _pages, lastPageParam) =>
      nextContinuation(lastPage, lastPageParam),
    enabled,
    retry: false,
    placeholderData: keepPreviousData,
    initialData: promotableBootstrapData<ChannelGroup>(
      queryClient,
      groupsQueryKey(null),
      expectedGeneration,
    ),
    staleTime: IMMUTABLE_CATALOG_STALE_TIME,
  });
  const guideBootstrapKey = guideWindowQueryKey({
    group,
    startsAt,
    endsAt,
    expectedGeneration: null,
  });
  const guideGeneration = bootstrapAwareGeneration(
    queryClient,
    guideBootstrapKey,
    expectedGeneration,
  );
  const guideQuery = useInfiniteQuery({
    ...guideWindowQueryOptions({
      client,
      group,
      startsAt,
      endsAt,
      expectedGeneration: guideGeneration,
    }),
    enabled,
    placeholderData: keepPreviousData,
    initialData: promotableBootstrapData<GuideWindowChannel>(
      queryClient,
      guideBootstrapKey,
      expectedGeneration,
    ),
  });
  const {
    fetchNextPage: fetchNextGroupPage,
    hasNextPage: hasNextGroupPage,
    isFetchNextPageError,
    isFetchingNextPage: isFetchingNextGroupPage,
    refetch: refetchGroups,
  } = groupsQuery;
  const { refetch: refetchGuide, fetchNextPage: fetchNextGuidePage } =
    guideQuery;
  const guideError = clientErrorFromQuery(guideQuery.error);
  const groupsError = clientErrorFromQuery(groupsQuery.error);
  const error =
    [guideError, groupsError].find(
      (candidate) => candidate?._tag === "stale-cursor",
    ) ??
    guideError ??
    groupsError;

  useEffect(() => {
    if (
      hasNextGroupPage === true &&
      !isFetchingNextGroupPage &&
      !isFetchNextPageError
    ) {
      void fetchNextGroupPage();
    }
  }, [
    fetchNextGroupPage,
    hasNextGroupPage,
    isFetchNextPageError,
    isFetchingNextGroupPage,
  ]);

  const groups = useMemo(
    () => collectPageItems(groupsQuery.data?.pages, expectedGeneration),
    [expectedGeneration, groupsQuery.data?.pages],
  );
  const receivedRows = useMemo(
    () => collectPageItems(guideQuery.data?.pages, expectedGeneration),
    [expectedGeneration, guideQuery.data?.pages],
  );
  const rows = useMemo(
    () =>
      group === null
        ? receivedRows
        : receivedRows.filter((row) => row.channel.group === group),
    [group, receivedRows],
  );
  const prefetchGroup = useCallback(
    (nextGroup: string | null) => {
      if (!enabled) {
        return;
      }
      void queryClient.prefetchInfiniteQuery(
        guideWindowQueryOptions({
          client,
          group: nextGroup,
          startsAt,
          endsAt,
          expectedGeneration,
        }),
      );
    },
    [client, enabled, endsAt, expectedGeneration, queryClient, startsAt],
  );
  const retry = useCallback(() => {
    if (guideError !== null) {
      void refetchGuide();
    }
    if (groupsError !== null) {
      void refetchGroups();
    }
  }, [groupsError, guideError, refetchGroups, refetchGuide]);
  const loadMore = useCallback(() => {
    void fetchNextGuidePage();
  }, [fetchNextGuidePage]);

  return {
    groups,
    rows,
    loading:
      enabled &&
      (guideQuery.isPending ||
        (guideQuery.isPlaceholderData && rows.length === 0)),
    replacing: guideQuery.isRefetching || guideQuery.isPlaceholderData,
    error,
    hasMore: guideQuery.hasNextPage === true,
    loadingMore: guideQuery.isFetchingNextPage,
    retry,
    loadMore,
    prefetchGroup,
  };
}

function guideWindowQueryOptions({
  client,
  group,
  startsAt,
  endsAt,
  expectedGeneration,
}: Pick<
  GuideCatalogInput,
  "client" | "group" | "startsAt" | "endsAt" | "expectedGeneration"
>) {
  return infiniteQueryOptions({
    queryKey: guideWindowQueryKey({
      group,
      startsAt,
      endsAt,
      expectedGeneration,
    }),
    initialPageParam: null as CatalogContinuation | null,
    queryFn: ({ pageParam, signal }) =>
      generationBoundResult(
        client.guideWindow({
          startsAt,
          endsAt,
          channelLimit: GUIDE_CHANNEL_PAGE_SIZE,
          ...(group === null ? {} : { group }),
          ...(pageParam === null
            ? {}
            : {
                cursor: pageParam.cursor,
                previousCursors: pageParam.previousCursors,
              }),
          signal,
        }),
        expectedGeneration ?? pageParam?.generation ?? null,
      ),
    getNextPageParam: (lastPage, _pages, lastPageParam) =>
      nextContinuation(lastPage, lastPageParam),
    retry: false,
    staleTime: IMMUTABLE_CATALOG_STALE_TIME,
  });
}

function groupsQueryKey(expectedGeneration: CatalogGeneration | null) {
  return ["catalog", "groups", expectedGeneration] as const;
}

function guideWindowQueryKey({
  group,
  startsAt,
  endsAt,
  expectedGeneration,
}: Pick<
  GuideCatalogInput,
  "group" | "startsAt" | "endsAt" | "expectedGeneration"
>) {
  return [
    "catalog",
    "guide-window",
    expectedGeneration,
    group,
    startsAt,
    endsAt,
  ] as const;
}

function promotableBootstrapData<Value>(
  queryClient: QueryClient,
  bootstrapKey: readonly unknown[],
  expectedGeneration: CatalogGeneration | null,
): CatalogInfiniteData<Value> | undefined {
  if (expectedGeneration === null) {
    return undefined;
  }
  const data =
    queryClient.getQueryData<CatalogInfiniteData<Value>>(bootstrapKey);
  if (data === undefined || data.pages.length === 0) {
    return undefined;
  }
  const pagesMatch = data.pages.every(
    (page) => page.value.generation === expectedGeneration,
  );
  const continuationsMatch = data.pageParams.every(
    (continuation) =>
      continuation === null || continuation.generation === expectedGeneration,
  );
  return pagesMatch && continuationsMatch ? data : undefined;
}

function bootstrapAwareGeneration(
  queryClient: QueryClient,
  bootstrapKey: readonly unknown[],
  expectedGeneration: CatalogGeneration | null,
): CatalogGeneration | null {
  if (expectedGeneration === null) {
    return null;
  }
  const bootstrap = queryClient.getQueryState(bootstrapKey);
  return bootstrap?.fetchStatus === "fetching" && bootstrap.data === undefined
    ? null
    : expectedGeneration;
}

function nextContinuation<Value>(
  lastPage: CatalogPageResult<Value>,
  lastPageParam: CatalogContinuation | null,
): CatalogContinuation | null {
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

function collectPageItems<Value>(
  pages: readonly CatalogPageResult<Value>[] | undefined,
  expectedGeneration: CatalogGeneration | null,
): readonly Value[] {
  return (pages ?? []).flatMap((page) =>
    expectedGeneration === null || page.value.generation === expectedGeneration
      ? page.value.items
      : [],
  );
}
