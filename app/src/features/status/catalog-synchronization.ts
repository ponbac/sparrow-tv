import {
  queryOptions,
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import type {
  CatalogGeneration,
  CatalogStatus,
  ClientError,
  ClientResult,
  RefreshReport,
  SparrowClient,
  SparrowEvent,
} from "../../client/contracts";
import {
  clientErrorFromQuery,
  successfulQueryResult,
} from "../../client/query-result";

const TV_STARTUP_FRESH_MS = 1_000;
const STATUS_QUERY_KEY = ["catalog", "status"] as const;

/** The status read shared by TV startup and the mounted catalog. */
export function catalogStatusQueryOptions(client: SparrowClient) {
  return queryOptions({
    queryKey: STATUS_QUERY_KEY,
    queryFn: ({ signal }) => successfulQueryResult(client.status({ signal })),
    retry: false,
  });
}

/** Status, event, and optional refresh state owned by catalog synchronization. */
export interface CatalogSynchronization {
  readonly status: CatalogStatus | null;
  readonly statusError: ClientError | null;
  readonly statusPending: boolean;
  readonly refreshing: boolean;
  readonly refreshResult: ClientResult<RefreshReport> | null;
  readonly latestEvent: SparrowEvent | null;
  /** Undefined until an event or refresh report supplies an authoritative generation. */
  readonly generationHint: CatalogGeneration | null | undefined;
  readonly retryStatus: () => void;
  readonly requestRefresh: () => void;
}

/**
 * Reconciles manual refreshes and ordered transport events into React Query.
 * Immutable catalog reads select their authoritative generation in their keys.
 */
export function useCatalogSynchronization(
  client: SparrowClient,
  television: boolean,
): CatalogSynchronization {
  const queryClient = useQueryClient();
  const controllerRef = useRef<AbortController | null>(null);
  const fetchedGenerationRef = useRef<
    CatalogGeneration | null | undefined
  >(undefined);
  const [latestEvent, setLatestEvent] = useState<SparrowEvent | null>(null);
  const [generationHint, setGenerationHint] = useState<
    CatalogGeneration | null | undefined
  >(undefined);
  const statusQuery = useQuery({
    ...catalogStatusQueryOptions(client),
    // Join a just-completed TV startup read. The native subscription delivers
    // its current status on connection, including changes during this handoff.
    staleTime: television ? TV_STARTUP_FRESH_MS : 0,
  });
  const { refetch: refetchStatusQuery } = statusQuery;
  const retryStatus = useCallback(() => {
    void refetchStatusQuery();
  }, [refetchStatusQuery]);
  const refreshMutation = useMutation({
    retry: false,
    mutationFn: () => {
      const controller = new AbortController();
      controllerRef.current = controller;
      return client.refresh({ signal: controller.signal });
    },
    onSuccess: (result) => {
      if (result.ok) {
        setGenerationHint(result.value.status.generation);
        reconcileStatus(queryClient, result.value.status);
      } else if (result.error._tag !== "cancelled") {
        refetchStatus(queryClient);
      }
    },
    onError: () => refetchStatus(queryClient),
    onSettled: () => {
      controllerRef.current = null;
    },
  });

  useEffect(() => {
    return () => controllerRef.current?.abort();
  }, []);

  useEffect(() => {
    if (statusQuery.data?.ok !== true) {
      return;
    }
    const generation = statusQuery.data.value.generation;
    const previousGeneration = fetchedGenerationRef.current;
    fetchedGenerationRef.current = generation;
    setGenerationHint(generation);
    if (
      generation === null &&
      previousGeneration !== null
    ) {
      clearCatalogData(queryClient);
    }
  }, [queryClient, statusQuery.data]);

  useEffect(
    () =>
      client.subscribe((event) => {
        setLatestEvent(event);
        switch (event._tag) {
          case "catalog-status-changed":
            setGenerationHint(event.status.generation);
            reconcileStatus(queryClient, event.status);
            return;
          case "catalog-published":
            setGenerationHint(event.generation);
            refetchStatus(queryClient);
            return;
          case "refresh-completed":
            refetchStatus(queryClient);
            return;
        }
      }),
    [client, queryClient],
  );

  const defectResult: ClientResult<RefreshReport> | null = refreshMutation.isError
    ? {
        ok: false,
        error: refreshDefect(),
      }
    : null;

  return {
    status: statusQuery.data?.ok === true ? statusQuery.data.value : null,
    statusError: clientErrorFromQuery(statusQuery.error),
    statusPending: statusQuery.isPending,
    refreshing: refreshMutation.isPending,
    refreshResult: refreshMutation.data ?? defectResult,
    latestEvent,
    generationHint,
    retryStatus,
    requestRefresh: () => refreshMutation.mutate(),
  };
}

function refetchStatus(queryClient: QueryClient): void {
  settle(
    queryClient.invalidateQueries({
      queryKey: STATUS_QUERY_KEY,
      exact: true,
      refetchType: "active",
    }),
  );
}

function reconcileStatus(
  queryClient: QueryClient,
  status: CatalogStatus,
): void {
  const previous = queryClient.getQueryData<ClientResult<CatalogStatus>>(
    STATUS_QUERY_KEY,
  );
  const previousGeneration = previous?.ok === true ? previous.value.generation : null;
  settle(queryClient.cancelQueries({ queryKey: STATUS_QUERY_KEY, exact: true }));
  queryClient.setQueryData<ClientResult<CatalogStatus>>(STATUS_QUERY_KEY, {
    ok: true,
    value: status,
  });
  if (previous === undefined || previousGeneration !== status.generation) {
    if (status.generation === null) {
      clearCatalogData(queryClient);
    }
  }
}

function clearCatalogData(queryClient: QueryClient): void {
  const filters = {
    predicate: ({ queryKey }: { readonly queryKey: readonly unknown[] }) =>
      queryKey[0] === "catalog" &&
      queryKey[1] !== "status" &&
      queryKey[1] !== "capabilities",
  };
  settle(
    queryClient.cancelQueries(filters),
  );
  queryClient.removeQueries(filters);
}

function settle(operation: Promise<unknown>): void {
  operation.catch(() => undefined);
}

function refreshDefect(): ClientError {
  return {
    _tag: "transport",
    retryable: true,
    message: "The hosted refresh could not be completed.",
  };
}
