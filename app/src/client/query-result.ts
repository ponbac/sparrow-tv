import type { ClientError, ClientResult } from "./contracts";

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
): Promise<ClientResult<Value>> {
  const result = await operation;
  if (!result.ok) {
    throw new ClientQueryError(result.error);
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
