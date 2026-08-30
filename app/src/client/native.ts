import { Channel, invoke } from "@tauri-apps/api/core";
import { z } from "zod";
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
  type InstalledSparrowClient,
  type ListChannelsInput,
  type ListGroupsInput,
  type Page,
  type PageCursor,
  type PlaybackDescriptor,
  type PlaybackSessionId,
  type ProgrammeSummary,
  type ReadPlaybackInput,
  type RefreshReport,
  type ScheduleInput,
  type SearchInput,
  type SearchPageInput,
  type SearchResults,
  type SourceConfigurationInput,
  type SparrowEvent,
  type StartPlaybackInput,
  type StopPlaybackInput,
} from "./contracts";

/** Fixed Tauri command names shared with the installed shell composition. */
export const NATIVE_COMMANDS = Object.freeze({
  capabilities: "installed_capabilities",
  status: "catalog_status",
  refresh: "catalog_refresh",
  groups: "catalog_list_groups",
  channels: "catalog_list_channels",
  channel: "catalog_channel",
  schedule: "catalog_schedule",
  search: "catalog_search",
  searchChannels: "catalog_search_channels",
  searchProgrammes: "catalog_search_programmes",
  cancelSearch: "catalog_search_cancel",
  replaceSourceConfiguration: "source_configuration_replace",
  subscribe: "catalog_subscribe",
  unsubscribe: "catalog_unsubscribe",
  startPlayback: "playback_start",
  readPlayback: "playback_read",
  stopPlayback: "playback_stop",
} as const);

/** Channel surface required by the ordered Tauri subscription adapter. */
export interface NativeChannel {
  onmessage: (message: unknown) => void;
}

/** Narrow, injectable Tauri IPC boundary used by the installed client adapter. */
export interface NativeIpc {
  readonly invoke: (
    command: string,
    args?: Readonly<Record<string, unknown>>,
  ) => Promise<unknown>;
  readonly createChannel: (
    onmessage: (message: unknown) => void,
  ) => NativeChannel;
}

/** Options for constructing the installed Sparrow client. */
export interface NativeSparrowClientOptions {
  readonly ipc?: NativeIpc;
}

interface RuntimeParser<Value> {
  safeParse(
    input: unknown,
  ):
    | { readonly success: true; readonly data: Value }
    | { readonly success: false };
}

const subscriptionIdSchema = z.string().regex(/^sub1_[0-9a-f]{16}$/u);
const voidResponseSchema = z.null().transform(() => undefined);
const MAX_NATIVE_CHUNK_BYTES = 64 * 1024;
const SOURCE_LOCATION_MAX_UTF8_BYTES = 16_384;
const sourceTextEncoder = new TextEncoder();
const sourceLocationSchema = z
  .string()
  .trim()
  .min(1)
  .max(SOURCE_LOCATION_MAX_UTF8_BYTES)
  .refine(
    (value) =>
      sourceTextEncoder.encode(value).byteLength <=
      SOURCE_LOCATION_MAX_UTF8_BYTES,
  );

/**
 * Creates the installed IPC adapter. Every value crossing from the Tauri shell
 * is parsed from `unknown`, and expected command failures stay in `ClientResult`.
 */
export function createNativeSparrowClient(
  options: NativeSparrowClientOptions = {},
): InstalledSparrowClient {
  return new TauriSparrowClient(options.ipc ?? createTauriIpc());
}

class TauriSparrowClient implements InstalledSparrowClient {
  readonly #ipc: NativeIpc;
  readonly #nextSearchRequestId = createSearchRequestIdFactory();
  readonly #nextPlaybackSessionId = createPlaybackSessionIdFactory();

  constructor(ipc: NativeIpc) {
    this.#ipc = ipc;
  }

  capabilities(
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<Capabilities>> {
    return this.#request(
      NATIVE_COMMANDS.capabilities,
      undefined,
      clientSchemas.installedCapabilities,
      options.signal,
    );
  }

  status(
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<CatalogStatus>> {
    return this.#request(
      NATIVE_COMMANDS.status,
      undefined,
      clientSchemas.status,
      options.signal,
    );
  }

  refresh(
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<RefreshReport>> {
    return this.#request(
      NATIVE_COMMANDS.refresh,
      undefined,
      clientSchemas.refreshReport,
      options.signal,
    );
  }

  subscribe(listener: (event: SparrowEvent) => void): () => void {
    let active = true;
    let subscriptionId: string | null = null;
    const events = this.#ipc.createChannel((message) => {
      if (!active) {
        return;
      }
      const parsed = clientSchemas.sparrowEvent.safeParse(message);
      if (parsed.success) {
        listener(parsed.data);
      }
    });

    let subscription: Promise<unknown>;
    try {
      subscription = this.#ipc.invoke(NATIVE_COMMANDS.subscribe, { events });
    } catch {
      active = false;
      return () => undefined;
    }

    subscription.then(
      (value) => {
        const parsed = subscriptionIdSchema.safeParse(value);
        if (!parsed.success) {
          active = false;
          return;
        }
        if (!active) {
          releaseSubscription(this.#ipc, parsed.data);
          return;
        }
        subscriptionId = parsed.data;
      },
      () => {
        active = false;
      },
    );

    return () => {
      if (!active) {
        return;
      }
      active = false;
      if (subscriptionId !== null) {
        releaseSubscription(this.#ipc, subscriptionId);
        subscriptionId = null;
      }
    };
  }

  listGroups(
    input: ListGroupsInput,
  ): Promise<ClientResult<Page<ChannelGroup>>> {
    return this.#request(
      NATIVE_COMMANDS.groups,
      {
        input: {
          limit: input.limit,
          ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
        },
      },
      clientSchemas.groupsPage,
      input.signal,
    );
  }

  listChannels(
    input: ListChannelsInput,
  ): Promise<ClientResult<Page<ChannelSummary>>> {
    return this.#request(
      NATIVE_COMMANDS.channels,
      {
        input: {
          limit: input.limit,
          ...(input.group === undefined ? {} : { group: input.group }),
          ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
        },
      },
      clientSchemas.channelsPage,
      input.signal,
    );
  }

  channel(input: ChannelInput): Promise<ClientResult<ChannelDetails>> {
    return this.#request(
      NATIVE_COMMANDS.channel,
      { input: { id: input.id } },
      clientSchemas.channel,
      input.signal,
    );
  }

  schedule(
    input: ScheduleInput,
  ): Promise<ClientResult<Page<ProgrammeSummary>>> {
    return this.#request(
      NATIVE_COMMANDS.schedule,
      {
        input: {
          id: input.id,
          limit: input.limit,
          ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
        },
      },
      clientSchemas.schedulePageFor(input),
      input.signal,
    );
  }

  search(input: SearchInput): Promise<ClientResult<SearchResults>> {
    return this.#searchRequest(
      NATIVE_COMMANDS.search,
      {
        term: input.term,
        channelLimit: input.channelLimit,
        ...(input.channelCursor === undefined
          ? {}
          : { channelCursor: input.channelCursor }),
        programmeLimit: input.programmeLimit,
        ...(input.programmeCursor === undefined
          ? {}
          : { programmeCursor: input.programmeCursor }),
      },
      clientSchemas.searchResultsFor(input),
      input.signal,
    );
  }

  searchChannels(
    input: SearchPageInput,
  ): Promise<ClientResult<Page<ChannelSummary>>> {
    return this.#searchRequest(
      NATIVE_COMMANDS.searchChannels,
      searchPageCommandInput(input),
      clientSchemas.searchChannelsPageFor(input),
      input.signal,
    );
  }

  searchProgrammes(
    input: SearchPageInput,
  ): Promise<ClientResult<Page<ProgrammeSummary>>> {
    return this.#searchRequest(
      NATIVE_COMMANDS.searchProgrammes,
      searchPageCommandInput(input),
      clientSchemas.searchProgrammesPageFor(input),
      input.signal,
    );
  }

  startPlayback(
    input: StartPlaybackInput,
  ): Promise<ClientResult<PlaybackDescriptor>> {
    const sessionId = this.#nextPlaybackSessionId();
    const release = createNativePlaybackStop(this.#ipc, sessionId);
    const parser = clientSchemas.nativePlaybackDescriptor.refine(
      (descriptor) => descriptor.sessionId === sessionId,
    );
    return this.#request(
      NATIVE_COMMANDS.startPlayback,
      { input: { id: input.id, sessionId } },
      parser,
      input.signal,
      release,
      (value) => {
        const descriptor = parser.safeParse(value);
        if (descriptor.success) {
          release();
        }
      },
      release,
    );
  }

  async readPlayback(
    input: ReadPlaybackInput,
  ): Promise<ClientResult<ArrayBuffer>> {
    const outcome = await invokeWithCancellation(
      () =>
        this.#ipc.invoke(NATIVE_COMMANDS.readPlayback, {
          input: {
            sessionId: input.sessionId,
            streamHandle: input.streamHandle,
          },
        }),
      input.signal,
      () => stopNativePlayback(this.#ipc, input.sessionId),
    );
    switch (outcome._tag) {
      case "cancelled":
        return { ok: false, error: { _tag: "cancelled" } };
      case "rejected": {
        const parsedError = clientSchemas.serverError.safeParse(outcome.error);
        return parsedError.success
          ? { ok: false, error: parsedError.data }
          : invalidNativeResponse();
      }
      case "resolved":
        return outcome.value instanceof ArrayBuffer &&
          outcome.value.byteLength <= MAX_NATIVE_CHUNK_BYTES
          ? { ok: true, value: outcome.value }
          : invalidNativeResponse();
    }
  }

  stopPlayback(input: StopPlaybackInput): Promise<ClientResult<void>> {
    return this.#request(
      NATIVE_COMMANDS.stopPlayback,
      { input: { sessionId: input.sessionId } },
      voidResponseSchema,
      input.signal,
    );
  }

  replaceSourceConfiguration(
    input: SourceConfigurationInput,
  ): Promise<ClientResult<CatalogStatus>> {
    if (input.signal?.aborted === true) {
      return Promise.resolve({ ok: false, error: { _tag: "cancelled" } });
    }
    const m3u = sourceLocationSchema.safeParse(input.m3uLocation);
    if (!m3u.success) {
      return Promise.resolve({
        ok: false,
        error: invalidLocationError("m3u", input.m3uLocation),
      });
    }
    const epg =
      input.epgLocation === null
        ? { success: true as const, data: null }
        : sourceLocationSchema.safeParse(input.epgLocation);
    if (!epg.success) {
      return Promise.resolve({
        ok: false,
        error: invalidLocationError("epg", input.epgLocation ?? ""),
      });
    }

    return this.#request(
      NATIVE_COMMANDS.replaceSourceConfiguration,
      {
        input: {
          m3uLocation: m3u.data,
          epgLocation: epg.data,
        },
      },
      clientSchemas.status,
      input.signal,
    );
  }

  async #request<Value>(
    command: string,
    args: Readonly<Record<string, unknown>> | undefined,
    parser: RuntimeParser<Value>,
    signal: AbortSignal | undefined,
    cancelActive?: () => void,
    onLateResolved?: (value: unknown) => void,
    onInvalidResolved?: (value: unknown) => void,
  ): Promise<ClientResult<Value>> {
    const outcome = await invokeWithCancellation(
      () => this.#ipc.invoke(command, args),
      signal,
      cancelActive,
      onLateResolved,
    );
    switch (outcome._tag) {
      case "cancelled":
        return { ok: false, error: { _tag: "cancelled" } };
      case "rejected": {
        const parsedError = clientSchemas.serverError.safeParse(outcome.error);
        return parsedError.success
          ? { ok: false, error: parsedError.data }
          : invalidNativeResponse();
      }
      case "resolved": {
        const parsed = parser.safeParse(outcome.value);
        if (parsed.success) {
          return { ok: true, value: parsed.data };
        }
        onInvalidResolved?.(outcome.value);
        return invalidNativeResponse();
      }
    }
  }

  #searchRequest<Value>(
    command: string,
    input: Readonly<Record<string, unknown>>,
    parser: RuntimeParser<Value>,
    signal: AbortSignal | undefined,
  ): Promise<ClientResult<Value>> {
    const requestId = this.#nextSearchRequestId();
    return this.#request(
      command,
      { input: { requestId, ...input } },
      parser,
      signal,
      () => cancelNativeSearch(this.#ipc, requestId),
    );
  }
}

type InvokeOutcome =
  | { readonly _tag: "resolved"; readonly value: unknown }
  | { readonly _tag: "rejected"; readonly error: unknown }
  | { readonly _tag: "cancelled" };

function invokeWithCancellation(
  start: () => Promise<unknown>,
  signal: AbortSignal | undefined,
  cancelActive?: () => void,
  onLateResolved?: (value: unknown) => void,
): Promise<InvokeOutcome> {
  if (signal?.aborted === true) {
    return Promise.resolve({ _tag: "cancelled" });
  }

  return new Promise((resolve) => {
    let settled = false;
    const finish = (outcome: InvokeOutcome) => {
      if (settled) {
        return;
      }
      settled = true;
      signal?.removeEventListener("abort", cancel);
      resolve(outcome);
    };
    const cancel = () => {
      if (settled) {
        return;
      }
      finish({ _tag: "cancelled" });
      cancelActive?.();
    };
    signal?.addEventListener("abort", cancel, { once: true });

    let flight: Promise<unknown>;
    try {
      flight = start();
    } catch (error: unknown) {
      finish({ _tag: "rejected", error });
      return;
    }
    flight.then(
      (value) => {
        if (settled) {
          onLateResolved?.(value);
          return;
        }
        finish({ _tag: "resolved", value });
      },
      (error: unknown) => finish({ _tag: "rejected", error }),
    );
  });
}

function createSearchRequestIdFactory(): () => string {
  const nonceBytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(nonceBytes);
  const nonce = Array.from(nonceBytes, (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  let sequence = 0;
  return () => {
    sequence += 1;
    return `srch1_${nonce}_${sequence.toString(16)}`;
  };
}

function createPlaybackSessionIdFactory(): () => PlaybackSessionId {
  const nonceBytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(nonceBytes);
  const nonce = Array.from(nonceBytes, (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  let sequence = 0;
  return () => {
    sequence += 1;
    return clientSchemas.playbackSessionId.parse(
      `play1_${nonce}_${sequence.toString(16)}`,
    );
  };
}

function cancelNativeSearch(ipc: NativeIpc, requestId: string): void {
  try {
    ipc
      .invoke(NATIVE_COMMANDS.cancelSearch, { input: { requestId } })
      .catch(() => undefined);
  } catch {
    // Cancellation is best effort after the caller has already stopped waiting.
  }
}

function stopNativePlayback(ipc: NativeIpc, sessionId: PlaybackSessionId): void {
  try {
    ipc
      .invoke(NATIVE_COMMANDS.stopPlayback, { input: { sessionId } })
      .catch(() => undefined);
  } catch {
    // The caller has already released ownership; native cleanup remains best effort.
  }
}

function createNativePlaybackStop(
  ipc: NativeIpc,
  sessionId: PlaybackSessionId,
): () => void {
  let requested = false;
  return () => {
    if (requested) {
      return;
    }
    requested = true;
    stopNativePlayback(ipc, sessionId);
  };
}

function releaseSubscription(ipc: NativeIpc, subscriptionId: string): void {
  try {
    ipc
      .invoke(NATIVE_COMMANDS.unsubscribe, { subscriptionId })
      .catch(() => undefined);
  } catch {
    // Cleanup failure is intentionally silent and cannot expose shell context.
  }
}

function searchPageCommandInput(input: SearchPageInput): {
  readonly term: string;
  readonly limit: number;
  readonly cursor?: PageCursor;
} {
  return {
    term: input.term,
    limit: input.limit,
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
  };
}

function invalidLocationError(
  field: "m3u" | "epg",
  value: string,
): ClientError {
  return {
    _tag: "invalid-input",
    field,
    reason: value.trim().length === 0 ? "required" : "too-long",
  };
}

function invalidNativeResponse(): ClientResult<never> {
  return {
    ok: false,
    error: {
      _tag: "transport",
      retryable: false,
      message: "The installed app returned an invalid response.",
    },
  };
}

function createTauriIpc(): NativeIpc {
  return {
    invoke: (command, args) => invoke<unknown>(command, args),
    createChannel: (onmessage) => new Channel<unknown>(onmessage),
  };
}
