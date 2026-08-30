import {
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
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

const STATUS_QUERY_KEY = ["catalog", "status"] as const;

/** Refresh and event state owned by the hosted catalog synchronization hook. */
export interface CatalogSynchronization {
  readonly status: CatalogStatus | null;
  readonly statusError: ClientError | null;
  readonly statusPending: boolean;
  readonly refreshing: boolean;
  readonly refreshResult: ClientResult<RefreshReport> | null;
  readonly latestEvent: SparrowEvent | null;
  /** Undefined until an event or refresh report supplies an authoritative generation. */
  readonly generationHint: CatalogGeneration | null | undefined;
  readonly requestRefresh: () => void;
}

/**
 * Reconciles manual refreshes and reconnecting hosted events into React Query.
 * Successful cached catalog data remains present while active reads refetch.
 */
export function useCatalogSynchronization(
  client: SparrowClient,
): CatalogSynchronization {
  const queryClient = useQueryClient();
  const controllerRef = useRef<AbortController | null>(null);
  const invalidatedGenerationRef = useRef<CatalogGeneration | null>(null);
  const fetchedGenerationRef = useRef<
    CatalogGeneration | null | undefined
  >(undefined);
  const [latestEvent, setLatestEvent] = useState<SparrowEvent | null>(null);
  const [generationHint, setGenerationHint] = useState<
    CatalogGeneration | null | undefined
  >(undefined);
  const statusQuery = useQuery({
    queryKey: STATUS_QUERY_KEY,
    queryFn: ({ signal }) => successfulQueryResult(client.status({ signal })),
    retry: false,
  });
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
        reconcileStatus(
          queryClient,
          result.value.status,
          invalidatedGenerationRef,
        );
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
      previousGeneration !== undefined &&
      previousGeneration !== generation
    ) {
      if (generation === null) {
        invalidateCatalogData(queryClient);
        invalidatedGenerationRef.current = null;
      } else {
        invalidateGeneration(
          queryClient,
          generation,
          invalidatedGenerationRef,
        );
      }
    }
  }, [queryClient, statusQuery.data]);

  useEffect(
    () =>
      client.subscribe((event) => {
        setLatestEvent(event);
        switch (event._tag) {
          case "catalog-status-changed":
            setGenerationHint(event.status.generation);
            reconcileStatus(
              queryClient,
              event.status,
              invalidatedGenerationRef,
            );
            return;
          case "catalog-published":
            setGenerationHint(event.generation);
            invalidateGeneration(
              queryClient,
              event.generation,
              invalidatedGenerationRef,
            );
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
  invalidatedGenerationRef: {
    current: CatalogGeneration | null;
  },
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
      invalidateCatalogData(queryClient);
      invalidatedGenerationRef.current = null;
    } else {
      invalidateGeneration(
        queryClient,
        status.generation,
        invalidatedGenerationRef,
      );
    }
  }
}

function invalidateGeneration(
  queryClient: QueryClient,
  generation: CatalogGeneration,
  invalidatedGenerationRef: {
    current: CatalogGeneration | null;
  },
): void {
  if (invalidatedGenerationRef.current === generation) {
    return;
  }
  invalidatedGenerationRef.current = generation;
  invalidateCatalogData(queryClient);
}

function invalidateCatalogData(queryClient: QueryClient): void {
  settle(
    queryClient.invalidateQueries({
      predicate: ({ queryKey }) =>
        queryKey[0] === "catalog" &&
        queryKey[1] !== "status" &&
        queryKey[1] !== "capabilities",
      refetchType: "active",
    }),
  );
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
