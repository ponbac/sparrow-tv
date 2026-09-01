import type {
  CatalogGeneration,
  ClientError,
  ClientResult,
} from "./contracts";

/** A safe query-layer failure that never retains a thrown transport payload. */
export class ClientQueryError extends Error {
  readonly clientError: ClientError;

  constructor(clientError: ClientError) {
    super("The Sparrow query did not return usable data.");
    this.name = "ClientQueryError";
    this.clientError = clientError;
  }
}

/**
 * Converts an expected client failure into React Query's error channel.
 * React Query then keeps the complete last successful value during a failed refetch.
 */
export async function successfulQueryResult<Value>(
  operation: Promise<ClientResult<Value>>,
): Promise<{ readonly ok: true; readonly value: Value }> {
  const result = await operation;
  if (!result.ok) {
    throw new ClientQueryError(result.error);
  }
  return result;
}

/** Rejects a response that cannot belong to the catalog generation requested by its query. */
export async function generationBoundResult<
  Value extends { readonly generation: CatalogGeneration },
>(
  operation: Promise<ClientResult<Value>>,
  expectedGeneration: CatalogGeneration | null,
): Promise<{ readonly ok: true; readonly value: Value }> {
  const result = await successfulQueryResult(operation);
  if (
    expectedGeneration !== null &&
    result.value.generation !== expectedGeneration
  ) {
    throw new ClientQueryError({
      _tag: "stale-cursor",
      current: result.value.generation,
    });
  }
  return result;
}

/** Recovers only the closed client error from a query failure. */
export function clientErrorFromQuery(error: unknown): ClientError | null {
  if (error === null || error === undefined) {
    return null;
  }
  if (error instanceof ClientQueryError) {
    return error.clientError;
  }
  return {
    _tag: "transport",
    retryable: true,
    message: "The hosted desk did not complete this catalog request.",
  };
}
