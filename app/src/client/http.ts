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
  type PlaybackDescriptor,
  type ProgrammeSummary,
  type RefreshReport,
  type ScheduleInput,
  type SearchInput,
  type SearchPageInput,
  type SearchResults,
  type SparrowClient,
  type SparrowEvent,
  type StartPlaybackInput,
} from "./contracts";

const API_ROOT = "/api/v1";
const MAX_EVENT_CHARACTERS = 65_536;

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
  /** EventSource constructor seam; defaults lazily to the current browser window. */
  readonly eventSource?: HttpEventSourceFactory;
}

/** The EventSource surface needed for authenticated hosted catalog events. */
export interface HttpEventSource {
  readonly addEventListener: (type: "message", listener: EventListener) => void;
  readonly removeEventListener: (type: "message", listener: EventListener) => void;
  readonly close: () => void;
}

/** Creates one reconnecting same-origin event stream. */
export type HttpEventSourceFactory = (endpoint: string) => HttpEventSource;

/**
 * Creates a same-origin HTTP adapter for the transport-neutral Sparrow client.
 * Expected protocol, authentication, cancellation, and transport failures are
 * returned through `ClientResult` rather than rejected promises.
 */
export function createHttpSparrowClient(
  options: HttpSparrowClientOptions = {},
): SparrowClient {
  const fetchImplementation = options.fetch ?? fetchFromBrowserWindow;
  const eventSourceImplementation =
    options.eventSource ?? eventSourceFromBrowserWindow;
  return new HttpSparrowClient(fetchImplementation, eventSourceImplementation);
}

function fetchFromBrowserWindow(
  input: RequestInfo | URL,
  init?: RequestInit,
): Promise<Response> {
  return window.fetch(input, init);
}

function eventSourceFromBrowserWindow(endpoint: string): HttpEventSource {
  return new window.EventSource(endpoint, { withCredentials: true });
}

class HttpSparrowClient implements SparrowClient {
  readonly #fetch: typeof fetch;
  readonly #eventSource: HttpEventSourceFactory;

  constructor(
    fetchImplementation: typeof fetch,
    eventSourceImplementation: HttpEventSourceFactory,
  ) {
    this.#fetch = fetchImplementation;
    this.#eventSource = eventSourceImplementation;
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

  /** Requests one coalesced manual refresh of every configured Source. */
  refresh(
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<RefreshReport>> {
    return this.#request(
      `${API_ROOT}/refresh`,
      clientSchemas.refreshReport,
      options.signal,
      "POST",
      { "X-Sparrow-Request": "refresh" },
    );
  }

  /** Subscribes to strict hosted catalog events; native EventSource owns reconnection. */
  subscribe(listener: (event: SparrowEvent) => void): () => void {
    let source: HttpEventSource;
    try {
      source = this.#eventSource(`${API_ROOT}/events`);
    } catch {
      return () => undefined;
    }

    let active = true;
    const receive: EventListener = (event) => {
      if (!active || !(event instanceof MessageEvent) || typeof event.data !== "string") {
        return;
      }
      const parsedJson = parseEventJson(event.data);
      if (parsedJson === undefined) {
        return;
      }
      const parsed = clientSchemas.sparrowEvent.safeParse(parsedJson);
      if (parsed.success) {
        listener(parsed.data);
      }
    };
    try {
      source.addEventListener("message", receive);
    } catch {
      closeEventSource(source);
      return () => undefined;
    }

    return () => {
      if (!active) {
        return;
      }
      active = false;
      try {
        source.removeEventListener("message", receive);
      } catch {
        // The stream is still closed below; cleanup remains idempotent.
      }
      closeEventSource(source);
    };
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
    const encodedId = encodePathSegment(input.id);
    if (encodedId === null) {
      return Promise.resolve(invalidProtocolResponse(400));
    }
    return this.#request(
      `${API_ROOT}/channels/${encodedId}`,
      clientSchemas.channel,
      input.signal,
    );
  }

  /** Reads one generation-bound page of a Channel's Programme schedule. */
  schedule(
    input: ScheduleInput,
  ): Promise<ClientResult<Page<ProgrammeSummary>>> {
    const encodedId = encodePathSegment(input.id);
    if (encodedId === null) {
      return Promise.resolve(invalidProtocolResponse(400));
    }
    const query = new URLSearchParams();
    query.set("limit", String(input.limit));
    if (input.cursor !== undefined) {
      query.set("cursor", input.cursor);
    }

    return this.#request(
      `${API_ROOT}/channels/${encodedId}/schedule?${query.toString()}`,
      clientSchemas.schedulePageFor(input),
      input.signal,
    );
  }

  /** Searches Channels and Programmes with independent pagination. */
  search(input: SearchInput): Promise<ClientResult<SearchResults>> {
    const query = new URLSearchParams();
    query.set("term", input.term);
    query.set("channelLimit", String(input.channelLimit));
    if (input.channelCursor !== undefined) {
      query.set("channelCursor", input.channelCursor);
    }
    query.set("programmeLimit", String(input.programmeLimit));
    if (input.programmeCursor !== undefined) {
      query.set("programmeCursor", input.programmeCursor);
    }

    return this.#request(
      `${API_ROOT}/search?${query.toString()}`,
      clientSchemas.searchResultsFor(input),
      input.signal,
    );
  }

  /** Searches only the Channel lane with its own continuation token. */
  searchChannels(
    input: SearchPageInput,
  ): Promise<ClientResult<Page<ChannelSummary>>> {
    return this.#request(
      searchPageEndpoint("channels", input),
      clientSchemas.searchChannelsPageFor(input),
      input.signal,
    );
  }

  /** Searches only the Programme lane with its own continuation token. */
  searchProgrammes(
    input: SearchPageInput,
  ): Promise<ClientResult<Page<ProgrammeSummary>>> {
    return this.#request(
      searchPageEndpoint("programmes", input),
      clientSchemas.searchProgrammesPageFor(input),
      input.signal,
    );
  }

  /** Resolves an opaque Channel Identifier to Sparrow's fixed same-origin route. */
  startPlayback(
    input: StartPlaybackInput,
  ): Promise<ClientResult<PlaybackDescriptor>> {
    if (input.signal?.aborted === true) {
      return Promise.resolve({
        ok: false,
        error: { _tag: "cancelled" },
      });
    }

    const encodedId = encodePathSegment(input.id);
    if (encodedId === null) {
      return Promise.resolve(invalidProtocolResponse(400));
    }
    const parsed = clientSchemas.hostedPlaybackDescriptor.safeParse({
      _tag: "same-origin-http",
      endpoint: `${API_ROOT}/play/${encodedId}`,
    });
    if (!parsed.success) {
      return Promise.resolve(invalidProtocolResponse(500));
    }
    return Promise.resolve({ ok: true, value: parsed.data });
  }

  async #request<Value>(
    endpoint: string,
    successParser: RuntimeParser<Value>,
    signal: AbortSignal | undefined,
    method: "GET" | "POST" = "GET",
    requestHeaders: Readonly<Record<string, string>> = {},
  ): Promise<ClientResult<Value>> {
    let response: Response;
    let payload: unknown;

    try {
      response = await this.#fetch(endpoint, {
        method,
        credentials: "same-origin",
        headers: { accept: "application/json", ...requestHeaders },
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

function parseEventJson(data: string): unknown | undefined {
  if (data.length > MAX_EVENT_CHARACTERS) {
    return undefined;
  }
  try {
    const parsed: unknown = JSON.parse(data);
    return parsed;
  } catch {
    return undefined;
  }
}

function closeEventSource(source: HttpEventSource): void {
  try {
    source.close();
  } catch {
    // A failed adapter cleanup must never expose transport diagnostics.
  }
}

function searchPageEndpoint(
  lane: "channels" | "programmes",
  input: SearchPageInput,
): string {
  const query = new URLSearchParams();
  query.set("term", input.term);
  query.set("limit", String(input.limit));
  if (input.cursor !== undefined) {
    query.set("cursor", input.cursor);
  }
  return `${API_ROOT}/search/${lane}?${query.toString()}`;
}

function encodePathSegment(value: string): string | null {
  try {
    return encodeURIComponent(value);
  } catch {
    return null;
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
