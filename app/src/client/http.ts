import {
  clientSchemas,
  type Capabilities,
  type CatalogStatus,
  type ChannelDetails,
  type ChannelGroup,
  type ChannelInput,
  type ChannelSummary,
  type ClientError,
  type ClientRequestOptions,
  type ClientResult,
  type ListChannelsInput,
  type ListGroupsInput,
  type Page,
  type SparrowClient,
} from "./contracts";

const API_ROOT = "/api/v1";

interface RuntimeParser<Value> {
  safeParse(
    input: unknown,
  ):
    | { readonly success: true; readonly data: Value }
    | { readonly success: false };
}

/** Dependencies used by the hosted HTTP client adapter. */
export interface HttpSparrowClientOptions {
  /** Fetch implementation; defaults lazily to the current browser window. */
  readonly fetch?: typeof fetch;
}

/**
 * Creates a same-origin HTTP adapter for the transport-neutral Sparrow client.
 * Expected protocol, authentication, cancellation, and transport failures are
 * returned through `ClientResult` rather than rejected promises.
 */
export function createHttpSparrowClient(
  options: HttpSparrowClientOptions = {},
): SparrowClient {
  const fetchImplementation = options.fetch ?? fetchFromBrowserWindow;
  return new HttpSparrowClient(fetchImplementation);
}

function fetchFromBrowserWindow(
  input: RequestInfo | URL,
  init?: RequestInit,
): Promise<Response> {
  return window.fetch(input, init);
}

class HttpSparrowClient implements SparrowClient {
  readonly #fetch: typeof fetch;

  constructor(fetchImplementation: typeof fetch) {
    this.#fetch = fetchImplementation;
  }

  /** Reads immutable deployment capabilities. */
  capabilities(
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<Capabilities>> {
    return this.#request(
      `${API_ROOT}/capabilities`,
      clientSchemas.capabilities,
      options.signal,
    );
  }

  /** Reads the catalog and source lifecycle status. */
  status(
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<CatalogStatus>> {
    return this.#request(
      `${API_ROOT}/status`,
      clientSchemas.status,
      options.signal,
    );
  }

  /** Reads one generation-bound page of channel groups. */
  listGroups(
    input: ListGroupsInput,
  ): Promise<ClientResult<Page<ChannelGroup>>> {
    const query = new URLSearchParams();
    query.set("limit", String(input.limit));
    if (input.cursor !== undefined) {
      query.set("cursor", input.cursor);
    }

    return this.#request(
      `${API_ROOT}/groups?${query.toString()}`,
      clientSchemas.groupsPage,
      input.signal,
    );
  }

  /** Reads one generation-bound page of channels. */
  listChannels(
    input: ListChannelsInput,
  ): Promise<ClientResult<Page<ChannelSummary>>> {
    const query = new URLSearchParams();
    query.set("limit", String(input.limit));
    if (input.group !== undefined) {
      query.set("group", input.group);
    }
    if (input.cursor !== undefined) {
      query.set("cursor", input.cursor);
    }

    return this.#request(
      `${API_ROOT}/channels?${query.toString()}`,
      clientSchemas.channelsPage,
      input.signal,
    );
  }

  /** Resolves browser-safe details for one channel. */
  channel(input: ChannelInput): Promise<ClientResult<ChannelDetails>> {
    return this.#request(
      `${API_ROOT}/channels/${encodeURIComponent(input.id)}`,
      clientSchemas.channel,
      input.signal,
    );
  }

  async #request<Value>(
    endpoint: string,
    successParser: RuntimeParser<Value>,
    signal: AbortSignal | undefined,
  ): Promise<ClientResult<Value>> {
    let response: Response;
    let payload: unknown;

    try {
      response = await this.#fetch(endpoint, {
        method: "GET",
        credentials: "same-origin",
        headers: { accept: "application/json" },
        signal,
      });
    } catch (error: unknown) {
      return {
        ok: false,
        error: classifyThrownRequestFailure(error, signal),
      };
    }

    try {
      payload = await response.json();
    } catch (error: unknown) {
      if (signal?.aborted === true || isAbortError(error)) {
        return { ok: false, error: { _tag: "cancelled" } };
      }
      return invalidProtocolResponse(response.status);
    }

    if (response.ok) {
      const parsed = successParser.safeParse(payload);
      if (parsed.success) {
        return { ok: true, value: parsed.data };
      }

      return invalidProtocolResponse(response.status);
    }

    const parsed = clientSchemas.errorEnvelope.safeParse(payload);
    if (parsed.success) {
      return { ok: false, error: parsed.data.error };
    }

    return invalidProtocolResponse(response.status);
  }
}

function classifyThrownRequestFailure(
  error: unknown,
  signal: AbortSignal | undefined,
): ClientError {
  if (signal?.aborted === true || isAbortError(error)) {
    return { _tag: "cancelled" };
  }

  return {
    _tag: "transport",
    retryable: true,
    message: "The Sparrow server could not be reached.",
  };
}

function isAbortError(error: unknown): boolean {
  return error instanceof Error && error.name === "AbortError";
}

function invalidProtocolResponse(status: number): ClientResult<never> {
  return {
    ok: false,
    error: {
      _tag: "transport",
      retryable: isRetryableStatus(status),
      message: "The Sparrow server returned an invalid response.",
    },
  };
}

function isRetryableStatus(status: number): boolean {
  return status === 408 || status === 425 || status === 429 || status >= 500;
}
