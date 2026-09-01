import { Channel, invoke } from "@tauri-apps/api/core";
import { z } from "zod";
import {
  clientSchemas,
  type AndroidPlaybackPresentation,
  type AndroidPlaybackStatus,
  type AndroidPlaybackViewport,
  type Capabilities,
  type CatalogStatus,
  type ChannelDetails,
  type ChannelGroup,
  type ChannelInput,
  type ChannelSummary,
  type ClientError,
  type ClientRequestOptions,
  type ClientResult,
  type CreatePlaybackSessionInput,
  type InstalledPlaybackSession,
  type InstalledPlaybackTransport,
  type InstalledSparrowClient,
  type ListChannelsInput,
  type ListGroupsInput,
  type MpvPlaybackControl,
  type Page,
  type PageCursor,
  type PlaybackDescriptor,
  type PlaybackSessionId,
  type NativeStreamHandle,
  type ProgrammeSummary,
  type ReadPlaybackInput,
  type ReadInstalledPlaybackInput,
  type RefreshReport,
  type RestartInstalledPlaybackInput,
  type ScheduleInput,
  type SearchInput,
  type SearchPageInput,
  type SearchResults,
  type SourceConfigurationInput,
  type SparrowEvent,
  type StartPlaybackInput,
  type StopPlaybackInput,
  type StartAndroidPlaybackPresentationInput,
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
  reopenPlayback: "playback_reopen",
  restartPlayback: "playback_restart",
  suspendPlayback: "playback_suspend",
  setPlaybackActivity: "playback_activity",
  stopPlayback: "playback_stop",
  startAndroidPlayback: "playback_android_start",
  androidPlaybackStatus: "playback_android_status",
  setAndroidPlaybackControls: "playback_android_controls",
  setAndroidPlaybackViewport: "playback_android_viewport",
  stopAndroidPlayback: "playback_android_stop",
  controlMpv: "playback_mpv_control",
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
const nativePlaybackIdentitySchema = z
  .object({
    _tag: z.literal("tauri-native-stream"),
    sessionId: clientSchemas.playbackSessionId,
    streamHandle: clientSchemas.nativeStreamHandle,
  })
  .passthrough();
const linuxMpvPlaybackIdentitySchema = z
  .object({
    _tag: z.literal("linux-mpv"),
    sessionId: clientSchemas.playbackSessionId,
  })
  .passthrough();
const installedPlaybackIdentitySchema = z.union([
  nativePlaybackIdentitySchema,
  linuxMpvPlaybackIdentitySchema,
]);
const mpvPlaybackControlSchema: z.ZodType<MpvPlaybackControl> =
  z.discriminatedUnion("_tag", [
    z.strictObject({ _tag: z.literal("health-check") }),
    z.strictObject({ _tag: z.literal("pause") }),
    z.strictObject({ _tag: z.literal("resume") }),
    z.strictObject({
      _tag: z.literal("set-volume"),
      percent: z.number().int().min(0).max(100),
    }),
    z.strictObject({
      _tag: z.literal("set-muted"),
      muted: z.boolean(),
    }),
    z.strictObject({
      _tag: z.literal("set-fullscreen"),
      fullscreen: z.boolean(),
    }),
  ]);
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

  createPlaybackSession(
    input: CreatePlaybackSessionInput,
  ): InstalledPlaybackSession {
    return new TauriPlaybackSession(
      this.#ipc,
      input.id,
      this.#nextPlaybackSessionId(),
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
      () => stopNativePlayback(this.#ipc, input.sessionId, input.streamHandle),
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
      {
        input: {
          sessionId: input.sessionId,
          ...(input.streamHandle === undefined
            ? {}
            : { streamHandle: input.streamHandle }),
        },
      },
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

/**
 * Owns one client-created native session identifier across transport reopenings.
 * The identifier never crosses this resource's interface.
 */
class TauriPlaybackSession implements InstalledPlaybackSession {
  readonly #ipc: NativeIpc;
  readonly #channelId: CreatePlaybackSessionInput["id"];
  readonly #sessionId: PlaybackSessionId;
  #started = false;
  #stopped = false;
  #streamHandle: NativeStreamHandle | null = null;
  #openSettlement: Promise<void> = Promise.resolve();
  #lateCleanup: Promise<ClientResult<void>> | null = null;
  #suspendFlight: Promise<ClientResult<void>> | null = null;
  #stopFlight: Promise<ClientResult<void>> | null = null;
  #androidPresentation: TauriAndroidPlaybackPresentation | null = null;
  #androidStartSettlement: Promise<void> | null = null;
  #transportTag: InstalledPlaybackTransport["_tag"] | null = null;
  #mpvControlQueue: Promise<void> = Promise.resolve();

  constructor(
    ipc: NativeIpc,
    channelId: CreatePlaybackSessionInput["id"],
    sessionId: PlaybackSessionId,
  ) {
    this.#ipc = ipc;
    this.#channelId = channelId;
    this.#sessionId = sessionId;
  }

  start(
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<InstalledPlaybackTransport>> {
    if (this.#started || this.#stopped) {
      return Promise.resolve(invalidNativeResponse());
    }
    this.#started = true;
    return this.#open(
      NATIVE_COMMANDS.startPlayback,
      {
        id: this.#channelId,
        sessionId: this.#sessionId,
      },
      options.signal,
      null,
      false,
    );
  }

  async reopen(
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<InstalledPlaybackTransport>> {
    if (!this.#started || this.#stopped) {
      return invalidNativeResponse();
    }
    await this.#openSettlement;
    await Promise.resolve();
    if (this.#lateCleanup !== null) {
      await this.#lateCleanup;
    }
    if (this.#suspendFlight !== null) {
      await this.#suspendFlight;
    }
    if (this.#stopped) {
      return { ok: false, error: { _tag: "cancelled" } };
    }
    return this.#open(
      NATIVE_COMMANDS.reopenPlayback,
      { sessionId: this.#sessionId },
      options.signal,
      this.#streamHandle,
      false,
    );
  }

  async restart(
    input: RestartInstalledPlaybackInput,
  ): Promise<ClientResult<InstalledPlaybackTransport>> {
    if (!this.#started || this.#stopped) {
      return invalidNativeResponse();
    }
    await this.#openSettlement;
    await Promise.resolve();
    if (this.#lateCleanup !== null) {
      await this.#lateCleanup;
    }
    if (this.#suspendFlight !== null) {
      await this.#suspendFlight;
    }
    if (this.#stopped) {
      return { ok: false, error: { _tag: "cancelled" } };
    }
    if (this.#streamHandle !== input.expectedStreamHandle) {
      return { ok: false, error: { _tag: "cancelled" } };
    }
    return this.#open(
      NATIVE_COMMANDS.restartPlayback,
      {
        sessionId: this.#sessionId,
        expectedStreamHandle: input.expectedStreamHandle,
        intent: input.intent,
      },
      input.signal,
      input.expectedStreamHandle,
      input.intent._tag === "select-audio",
    );
  }

  async read(
    input: ReadInstalledPlaybackInput,
  ): Promise<ClientResult<ArrayBuffer>> {
    if (this.#stopped) {
      return { ok: false, error: { _tag: "cancelled" } };
    }
    if (input.streamHandle !== this.#streamHandle) {
      return { ok: false, error: { _tag: "cancelled" } };
    }
    const outcome = await invokeWithCancellation(
      () =>
        this.#ipc.invoke(NATIVE_COMMANDS.readPlayback, {
          input: {
            sessionId: this.#sessionId,
            streamHandle: input.streamHandle,
          },
        }),
      input.signal,
    );
    return parseNativeChunk(outcome);
  }

  async startAndroidPresentation(
    input: StartAndroidPlaybackPresentationInput,
  ): Promise<ClientResult<AndroidPlaybackPresentation>> {
    if (this.#androidStartSettlement !== null) {
      await this.#androidStartSettlement;
    }
    if (
      !this.#started ||
      this.#stopped ||
      this.#transportTag !== "tauri-native-stream" ||
      input.streamHandle !== this.#streamHandle ||
      this.#androidPresentation !== null ||
      !isAndroidPlaybackViewport(input.viewport) ||
      !isUnitVolume(input.volume)
    ) {
      return invalidNativeResponse();
    }
    const commandInput = {
      sessionId: this.#sessionId,
      streamHandle: input.streamHandle,
      viewport: input.viewport,
      volume: input.volume,
      muted: input.muted,
    };
    const invocation: { raw: Promise<unknown> | null } = { raw: null };
    let cancelled = false;
    let earlyCleanup = Promise.resolve();
    const outcomeFlight = invokeWithCancellation(
      () => {
        invocation.raw = this.#ipc.invoke(NATIVE_COMMANDS.startAndroidPlayback, {
          input: commandInput,
        });
        return invocation.raw;
      },
      input.signal,
      () => {
        cancelled = true;
        earlyCleanup = requestNativeAndroidPlaybackStop(
          this.#ipc,
          this.#sessionId,
          input.streamHandle,
        );
      },
    );
    const rawFlight = invocation.raw;
    if (rawFlight === null) {
      const result = parseNativeOutcome(await outcomeFlight, voidResponseSchema);
      return result.ok ? invalidNativeResponse() : result;
    }
    const settlement = rawFlight.then(
      async () => {
        await earlyCleanup;
        if (cancelled) {
          // The abort cleanup may have reached Kotlin before start published
          // an owner. A distinct post-resolution stop is therefore required.
          await requestNativeAndroidPlaybackStop(
            this.#ipc,
            this.#sessionId,
            input.streamHandle,
          );
        }
      },
      async () => {
        await earlyCleanup;
      },
    );
    const trackedSettlement = settlement.finally(() => {
      if (this.#androidStartSettlement === trackedSettlement) {
        this.#androidStartSettlement = null;
      }
    });
    this.#androidStartSettlement = trackedSettlement;
    const result = parseNativeOutcome(
      await outcomeFlight,
      voidResponseSchema,
    );
    if (
      !result.ok ||
      this.#stopped ||
      input.streamHandle !== this.#streamHandle
    ) {
      if (result.ok) {
        await requestNativeAndroidPlaybackStop(
          this.#ipc,
          this.#sessionId,
          input.streamHandle,
        );
      }
      return result.ok ? { ok: false, error: { _tag: "cancelled" } } : result;
    }
    const presentation = new TauriAndroidPlaybackPresentation(
      this.#ipc,
      this.#sessionId,
      input.streamHandle,
      input.viewport,
      input.volume,
      input.muted,
    );
    this.#androidPresentation = presentation;
    return { ok: true, value: presentation };
  }

  controlMpv(
    control: MpvPlaybackControl,
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<void>> {
    if (
      !this.#started ||
      this.#stopped ||
      this.#transportTag !== "linux-mpv" ||
      !isMpvPlaybackControl(control)
    ) {
      return Promise.resolve(invalidNativeResponse());
    }
    const operation = async (): Promise<ClientResult<void>> => {
      if (this.#stopped || this.#transportTag !== "linux-mpv") {
        return cancelledNativeRequest();
      }
      return requestNative(
        this.#ipc,
        NATIVE_COMMANDS.controlMpv,
        { input: { sessionId: this.#sessionId, control } },
        voidResponseSchema,
        options.signal,
      );
    };
    const flight = this.#mpvControlQueue.then(operation, operation);
    this.#mpvControlQueue = flight.then(
      () => undefined,
      () => undefined,
    );
    return flight;
  }

  suspend(options: ClientRequestOptions = {}): Promise<ClientResult<void>> {
    if (this.#stopped) {
      return Promise.resolve({ ok: true, value: undefined });
    }
    if (this.#suspendFlight !== null) {
      return this.#suspendFlight;
    }
    this.#retireAndroidPresentation();
    const flight = requestNative(
      this.#ipc,
      NATIVE_COMMANDS.suspendPlayback,
      { input: { sessionId: this.#sessionId } },
      voidResponseSchema,
      options.signal,
    );
    this.#suspendFlight = flight;
    void flight.finally(() => {
      if (this.#suspendFlight === flight) {
        this.#suspendFlight = null;
      }
    });
    return flight;
  }

  setActivity(
    active: boolean,
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<void>> {
    if (this.#stopped) {
      return Promise.resolve(
        active
          ? { ok: false, error: { _tag: "cancelled" } }
          : { ok: true, value: undefined },
      );
    }
    return requestNative(
      this.#ipc,
      NATIVE_COMMANDS.setPlaybackActivity,
      { input: { sessionId: this.#sessionId, active } },
      voidResponseSchema,
      options.signal,
    );
  }

  stop(options: ClientRequestOptions = {}): Promise<ClientResult<void>> {
    if (this.#stopFlight !== null) {
      return this.#stopFlight;
    }
    this.#stopped = true;
    this.#retireAndroidPresentation();
    const streamHandle = this.#streamHandle;
    this.#stopFlight = requestNative(
      this.#ipc,
      NATIVE_COMMANDS.stopPlayback,
      {
        input: {
          sessionId: this.#sessionId,
          ...(streamHandle === null ? {} : { streamHandle }),
        },
      },
      voidResponseSchema,
      options.signal,
    );
    return this.#stopFlight;
  }

  async #open(
    command:
      | typeof NATIVE_COMMANDS.startPlayback
      | typeof NATIVE_COMMANDS.reopenPlayback
      | typeof NATIVE_COMMANDS.restartPlayback,
    input: Readonly<Record<string, unknown>>,
    signal: AbortSignal | undefined,
    expectedStreamHandle: NativeStreamHandle | null,
    requirePreferenceStatus: boolean,
  ): Promise<ClientResult<InstalledPlaybackTransport>> {
    if (this.#stopped) {
      return { ok: false, error: { _tag: "cancelled" } };
    }
    this.#retireAndroidPresentation();
    const parser = clientSchemas.installedPlaybackDescriptor.refine(
      (descriptor) =>
        descriptor.sessionId === this.#sessionId &&
        (expectedStreamHandle === null ||
          (descriptor._tag === "tauri-native-stream" &&
            descriptor.streamHandle !== expectedStreamHandle)) &&
        (!requirePreferenceStatus ||
          (descriptor._tag === "tauri-native-stream" &&
            descriptor.preferenceStatus !== undefined)),
    );
    let rawFlight: Promise<unknown> | null = null;
    const outcome = await invokeWithCancellation(
      () => {
        rawFlight = this.#ipc.invoke(command, { input });
        this.#openSettlement = rawFlight.then(
          () => undefined,
          () => undefined,
        );
        return rawFlight;
      },
      signal,
      undefined,
      (value) => {
        const lateIdentity = installedPlaybackIdentitySchema.safeParse(value);
        if (
          lateIdentity.success &&
          lateIdentity.data.sessionId === this.#sessionId
        ) {
          this.#transportTag = lateIdentity.data._tag;
          this.#streamHandle =
            lateIdentity.data._tag === "tauri-native-stream"
              ? lateIdentity.data.streamHandle
              : null;
        }
        this.#lateCleanup = this.#stopped ? this.stop() : this.suspend();
      },
    );
    void rawFlight;
    if (outcome._tag === "resolved") {
      const identity = installedPlaybackIdentitySchema.safeParse(outcome.value);
      if (identity.success && identity.data.sessionId === this.#sessionId) {
        this.#transportTag = identity.data._tag;
        this.#streamHandle =
          identity.data._tag === "tauri-native-stream"
            ? identity.data.streamHandle
            : null;
      }
    }
    const result = parseNativeOutcome(outcome, parser);
    if (
      !result.ok &&
      result.error._tag === "transport" &&
      !result.error.retryable
    ) {
      await this.stop();
    }
    if (!result.ok) {
      return result;
    }
    this.#transportTag = result.value._tag;
    if (result.value._tag === "linux-mpv") {
      this.#streamHandle = null;
      return { ok: true, value: { _tag: "linux-mpv" } };
    }
    this.#streamHandle = result.value.streamHandle;
    return {
      ok: true,
      value: {
        _tag: "tauri-native-stream",
        streamHandle: result.value.streamHandle,
        presentation: result.value.presentation,
        tracks: result.value.tracks,
        selection: result.value.selection,
        ...(result.value.preferenceStatus === undefined
          ? {}
          : { preferenceStatus: result.value.preferenceStatus }),
      },
    };
  }

  #retireAndroidPresentation(): void {
    this.#androidPresentation?.retire();
    this.#androidPresentation = null;
  }
}

/**
 * Serializes commands for one exact Android Media3 presentation. Provider
 * locations never enter this object; the two identifiers are opaque handles.
 */
class TauriAndroidPlaybackPresentation implements AndroidPlaybackPresentation {
  readonly #ipc: NativeIpc;
  readonly #sessionId: PlaybackSessionId;
  readonly #streamHandle: NativeStreamHandle;
  #controls: {
    readonly volume: number;
    readonly muted: boolean;
    readonly paused: boolean;
  };
  #viewport: AndroidPlaybackViewport;
  #queue: Promise<void> = Promise.resolve();
  #generation = 0;
  #retired = false;
  #stopFlight: Promise<ClientResult<void>> | null = null;

  constructor(
    ipc: NativeIpc,
    sessionId: PlaybackSessionId,
    streamHandle: NativeStreamHandle,
    viewport: AndroidPlaybackViewport,
    volume: number,
    muted: boolean,
  ) {
    this.#ipc = ipc;
    this.#sessionId = sessionId;
    this.#streamHandle = streamHandle;
    this.#viewport = viewport;
    this.#controls = { volume, muted, paused: false };
  }

  status(
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<AndroidPlaybackStatus>> {
    return this.#enqueueActive(() =>
      requestNative(
        this.#ipc,
        NATIVE_COMMANDS.androidPlaybackStatus,
        { input: this.#identityInput() },
        clientSchemas.androidPlaybackStatus,
        options.signal,
      ),
    );
  }

  pause(options: ClientRequestOptions = {}): Promise<ClientResult<void>> {
    return this.#setControls(
      { ...this.#controls, paused: true },
      options.signal,
    );
  }

  resume(options: ClientRequestOptions = {}): Promise<ClientResult<void>> {
    return this.#setControls(
      { ...this.#controls, paused: false },
      options.signal,
    );
  }

  setVolume(
    volume: number,
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<void>> {
    if (!isUnitVolume(volume)) {
      return Promise.resolve(invalidNativeResponse());
    }
    return this.#setControls({ ...this.#controls, volume }, options.signal);
  }

  setMuted(
    muted: boolean,
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<void>> {
    return this.#setControls({ ...this.#controls, muted }, options.signal);
  }

  setViewport(
    viewport: AndroidPlaybackViewport,
    options: ClientRequestOptions = {},
  ): Promise<ClientResult<void>> {
    if (!isAndroidPlaybackViewport(viewport)) {
      return Promise.resolve(invalidNativeResponse());
    }
    this.#viewport = viewport;
    return this.#enqueueActive(() =>
      requestNative(
        this.#ipc,
        NATIVE_COMMANDS.setAndroidPlaybackViewport,
        {
          input: {
            ...this.#identityInput(),
            viewport: this.#viewport,
          },
        },
        voidResponseSchema,
        options.signal,
      ),
    );
  }

  stop(options: ClientRequestOptions = {}): Promise<ClientResult<void>> {
    if (this.#stopFlight !== null) {
      return this.#stopFlight;
    }
    this.#retired = true;
    this.#generation += 1;
    const flight = this.#enqueue(() =>
      requestNative(
        this.#ipc,
        NATIVE_COMMANDS.stopAndroidPlayback,
        { input: this.#identityInput() },
        voidResponseSchema,
        options.signal,
        () =>
          stopNativeAndroidPlayback(
            this.#ipc,
            this.#sessionId,
            this.#streamHandle,
          ),
      ),
    );
    this.#stopFlight = flight;
    return flight;
  }

  /** Retires stale queued work when the owning Rust session performs teardown. */
  retire(): void {
    if (this.#retired) {
      return;
    }
    this.#retired = true;
    this.#generation += 1;
  }

  #setControls(
    controls: {
      readonly volume: number;
      readonly muted: boolean;
      readonly paused: boolean;
    },
    signal: AbortSignal | undefined,
  ): Promise<ClientResult<void>> {
    if (this.#retired) {
      return Promise.resolve(cancelledNativeRequest());
    }
    this.#controls = controls;
    return this.#enqueueActive(() =>
      requestNative(
        this.#ipc,
        NATIVE_COMMANDS.setAndroidPlaybackControls,
        {
          input: {
            ...this.#identityInput(),
            ...this.#controls,
          },
        },
        voidResponseSchema,
        signal,
      ),
    );
  }

  #enqueueActive<Value>(
    operation: () => Promise<ClientResult<Value>>,
  ): Promise<ClientResult<Value>> {
    if (this.#retired) {
      return Promise.resolve(cancelledNativeRequest());
    }
    const generation = this.#generation;
    return this.#enqueue(() =>
      this.#retired || generation !== this.#generation
        ? Promise.resolve(cancelledNativeRequest())
        : operation(),
    );
  }

  #enqueue<Value>(
    operation: () => Promise<ClientResult<Value>>,
  ): Promise<ClientResult<Value>> {
    const flight = this.#queue.then(operation, operation);
    this.#queue = flight.then(
      () => undefined,
      () => undefined,
    );
    return flight;
  }

  #identityInput(): {
    readonly sessionId: PlaybackSessionId;
    readonly streamHandle: NativeStreamHandle;
  } {
    return {
      sessionId: this.#sessionId,
      streamHandle: this.#streamHandle,
    };
  }
}

type InvokeOutcome =
  | { readonly _tag: "resolved"; readonly value: unknown }
  | { readonly _tag: "rejected"; readonly error: unknown }
  | { readonly _tag: "cancelled" };

function requestNative<Value>(
  ipc: NativeIpc,
  command: string,
  args: Readonly<Record<string, unknown>> | undefined,
  parser: RuntimeParser<Value>,
  signal: AbortSignal | undefined,
  cancelActive?: () => void,
  onLateResolved?: (value: unknown) => void,
): Promise<ClientResult<Value>> {
  return invokeWithCancellation(
    () => ipc.invoke(command, args),
    signal,
    cancelActive,
    onLateResolved,
  ).then((outcome) => parseNativeOutcome(outcome, parser));
}

function parseNativeOutcome<Value>(
  outcome: InvokeOutcome,
  parser: RuntimeParser<Value>,
): ClientResult<Value> {
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
      return parsed.success
        ? { ok: true, value: parsed.data }
        : invalidNativeResponse();
    }
  }
}

function parseNativeChunk(outcome: InvokeOutcome): ClientResult<ArrayBuffer> {
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

function stopNativePlayback(
  ipc: NativeIpc,
  sessionId: PlaybackSessionId,
  streamHandle?: NativeStreamHandle,
): void {
  try {
    ipc
      .invoke(NATIVE_COMMANDS.stopPlayback, {
        input: {
          sessionId,
          ...(streamHandle === undefined ? {} : { streamHandle }),
        },
      })
      .catch(() => undefined);
  } catch {
    // The caller has already released ownership; native cleanup remains best effort.
  }
}

function stopNativeAndroidPlayback(
  ipc: NativeIpc,
  sessionId: PlaybackSessionId,
  streamHandle: NativeStreamHandle,
): void {
  void requestNativeAndroidPlaybackStop(ipc, sessionId, streamHandle);
}

function requestNativeAndroidPlaybackStop(
  ipc: NativeIpc,
  sessionId: PlaybackSessionId,
  streamHandle: NativeStreamHandle,
): Promise<void> {
  try {
    return ipc
      .invoke(NATIVE_COMMANDS.stopAndroidPlayback, {
        input: { sessionId, streamHandle },
      })
      .then(
        () => undefined,
        () => undefined,
      );
  } catch {
    // The opaque presentation is already retired; cleanup remains best effort.
    return Promise.resolve();
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

function cancelledNativeRequest(): ClientResult<never> {
  return { ok: false, error: { _tag: "cancelled" } };
}

function isUnitVolume(value: number): boolean {
  return Number.isFinite(value) && value >= 0 && value <= 1;
}

function isMpvPlaybackControl(
  value: unknown,
): value is MpvPlaybackControl {
  return mpvPlaybackControlSchema.safeParse(value).success;
}

function isAndroidPlaybackViewport(viewport: AndroidPlaybackViewport): boolean {
  return (
    Number.isSafeInteger(viewport.left) &&
    viewport.left >= 0 &&
    viewport.left <= 32_768 &&
    Number.isSafeInteger(viewport.top) &&
    viewport.top >= 0 &&
    viewport.top <= 32_768 &&
    Number.isSafeInteger(viewport.width) &&
    viewport.width >= 1 &&
    viewport.width <= 32_768 &&
    Number.isSafeInteger(viewport.height) &&
    viewport.height >= 1 &&
    viewport.height <= 32_768
  );
}

function createTauriIpc(): NativeIpc {
  return {
    invoke: (command, args) => invoke<unknown>(command, args),
    createChannel: (onmessage) => new Channel<unknown>(onmessage),
  };
}
