// @vitest-environment node

import { afterEach, describe, expect, it, vi } from "vitest";
import { clientSchemas, type ClientResult } from "./contracts";
import {
  createNativeSparrowClient,
  NATIVE_COMMANDS,
  type NativeChannel,
  type NativeIpc,
} from "./native";

afterEach(() => vi.restoreAllMocks());

const INSTALLED_CAPABILITIES = {
  sourceConfiguration: "device-writable",
  playbackTransport: "tauri-native-stream",
  audioTrackSelection: true,
  mpvFailover: true,
} as const;

const EMPTY_AUDIO = {
  tracks: [],
  selection: { _tag: "none" },
} as const;
const ENGLISH_AUDIO_ID = clientSchemas.audioTrackId.parse(
  `atrk1_${"1".repeat(32)}`,
);
const SPANISH_AUDIO_ID = clientSchemas.audioTrackId.parse(
  `atrk1_${"2".repeat(32)}`,
);

const FRESH_STATUS = clientSchemas.status.parse({
  generation: 7,
  configuration: { configured: true, epgConfigured: true },
  m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
  epg: { _tag: "fresh", validatedAt: "2026-08-30T10:00:01Z" },
});

const GROUPS_PAGE = clientSchemas.groupsPage.parse({
  generation: 7,
  items: [{ name: "News", channelCount: 1 }],
  next: null,
});

const CHANNELS_PAGE = clientSchemas.channelsPage.parse({
  generation: 7,
  items: [{ id: "world-news", name: "World News", group: "News" }],
  next: null,
});

const CHANNEL = clientSchemas.channel.parse({
  id: "world-news",
  name: "World News",
  group: "News",
});

const PROGRAMME_PAYLOAD = {
  channelId: CHANNEL.id,
  title: "Evening Report",
  description: "Headlines and analysis.",
  startsAt: "2026-08-30T19:00:00Z",
  endsAt: "2026-08-30T20:00:00Z",
} as const;

const SCHEDULE_PAGE = clientSchemas.schedulePage.parse({
  generation: 7,
  items: [PROGRAMME_PAYLOAD],
  next: null,
});

const PROGRAMME = requireFirst(SCHEDULE_PAGE.items);

const SEARCH_RESULTS = clientSchemas.searchResults.parse({
  generation: 7,
  channels: CHANNELS_PAGE,
  programmes: SCHEDULE_PAGE,
});

const REFRESH_REPORT = clientSchemas.refreshReport.parse({
  trigger: "manual",
  m3u: {
    _tag: "not-modified",
    validatedAt: "2026-08-30T10:00:00Z",
  },
  epg: {
    _tag: "not-modified",
    validatedAt: "2026-08-30T10:00:01Z",
  },
  status: FRESH_STATUS,
});

interface RecordedInvoke {
  readonly command: string;
  readonly args: Readonly<Record<string, unknown>> | undefined;
}

class FakeNativeIpc implements NativeIpc {
  readonly invokes: RecordedInvoke[] = [];
  readonly channels: NativeChannel[] = [];

  constructor(
    private readonly handler: (
      command: string,
      args: Readonly<Record<string, unknown>> | undefined,
    ) => Promise<unknown>,
  ) {}

  invoke(
    command: string,
    args?: Readonly<Record<string, unknown>>,
  ): Promise<unknown> {
    this.invokes.push({ command, args });
    return this.handler(command, args);
  }

  createChannel(onmessage: (message: unknown) => void): NativeChannel {
    const channel = { onmessage };
    this.channels.push(channel);
    return channel;
  }
}

describe("installed Tauri Sparrow client", () => {
  it("projects narrow command inputs and parses every supported success", async () => {
    const ipc = new FakeNativeIpc((command) =>
      Promise.resolve(
        command === NATIVE_COMMANDS.capabilities
          ? INSTALLED_CAPABILITIES
          : command === NATIVE_COMMANDS.status
            ? FRESH_STATUS
            : command === NATIVE_COMMANDS.refresh
              ? REFRESH_REPORT
              : command === NATIVE_COMMANDS.groups
                ? GROUPS_PAGE
                : command === NATIVE_COMMANDS.channels
                  ? CHANNELS_PAGE
                  : command === NATIVE_COMMANDS.channel
                    ? CHANNEL
                    : command === NATIVE_COMMANDS.schedule
                      ? SCHEDULE_PAGE
                      : command === NATIVE_COMMANDS.search
                        ? SEARCH_RESULTS
                        : command === NATIVE_COMMANDS.searchChannels
                          ? CHANNELS_PAGE
                          : command === NATIVE_COMMANDS.searchProgrammes
                            ? SCHEDULE_PAGE
                            : FRESH_STATUS,
      ),
    );
    const client = createNativeSparrowClient({ ipc });

    await expect(client.capabilities()).resolves.toEqual({
      ok: true,
      value: INSTALLED_CAPABILITIES,
    });
    await expect(client.status()).resolves.toEqual({
      ok: true,
      value: FRESH_STATUS,
    });
    await expect(client.refresh()).resolves.toEqual({
      ok: true,
      value: REFRESH_REPORT,
    });
    await expect(client.listGroups({ limit: 100 })).resolves.toEqual({
      ok: true,
      value: GROUPS_PAGE,
    });
    await expect(
      client.listChannels({ limit: 24, group: "News" }),
    ).resolves.toEqual({ ok: true, value: CHANNELS_PAGE });
    await expect(client.channel({ id: CHANNEL.id })).resolves.toEqual({
      ok: true,
      value: CHANNEL,
    });
    await expect(
      client.schedule({
        id: CHANNEL.id,
        limit: 1,
        afterStartsAt: PROGRAMME.startsAt,
        previousCursors: [],
      }),
    ).resolves.toEqual({ ok: true, value: SCHEDULE_PAGE });
    await expect(
      client.search({
        term: "news",
        channelLimit: 1,
        channelPreviousCursors: [],
        programmeLimit: 1,
        programmePreviousCursors: [],
      }),
    ).resolves.toEqual({ ok: true, value: SEARCH_RESULTS });
    await expect(
      client.searchChannels({
        term: "news",
        limit: 1,
        previousCursors: [],
      }),
    ).resolves.toEqual({ ok: true, value: CHANNELS_PAGE });
    await expect(
      client.searchProgrammes({
        term: "report",
        limit: 1,
        previousCursors: [],
      }),
    ).resolves.toEqual({ ok: true, value: SCHEDULE_PAGE });

    expect(ipc.invokes).toEqual([
      { command: NATIVE_COMMANDS.capabilities, args: undefined },
      { command: NATIVE_COMMANDS.status, args: undefined },
      { command: NATIVE_COMMANDS.refresh, args: undefined },
      {
        command: NATIVE_COMMANDS.groups,
        args: { input: { limit: 100 } },
      },
      {
        command: NATIVE_COMMANDS.channels,
        args: { input: { limit: 24, group: "News" } },
      },
      {
        command: NATIVE_COMMANDS.channel,
        args: { input: { id: CHANNEL.id } },
      },
      {
        command: NATIVE_COMMANDS.schedule,
        args: { input: { id: CHANNEL.id, limit: 1 } },
      },
      {
        command: NATIVE_COMMANDS.search,
        args: {
          input: {
            requestId: searchRequestId(),
            term: "news",
            channelLimit: 1,
            programmeLimit: 1,
          },
        },
      },
      {
        command: NATIVE_COMMANDS.searchChannels,
        args: {
          input: { requestId: searchRequestId(), term: "news", limit: 1 },
        },
      },
      {
        command: NATIVE_COMMANDS.searchProgrammes,
        args: {
          input: { requestId: searchRequestId(), term: "report", limit: 1 },
        },
      },
    ]);
  });

  it("sends continuation cursors without leaking client-only correlation state", async () => {
    const scheduleCursor = parsedCursor("schedule-current");
    const channelCursor = parsedCursor("channel-current");
    const programmeCursor = parsedCursor("programme-current");
    const earlierCursor = parsedCursor("earlier-page");
    const ipc = new FakeNativeIpc((command) =>
      Promise.resolve(
        command === NATIVE_COMMANDS.schedule ||
          command === NATIVE_COMMANDS.searchProgrammes
          ? SCHEDULE_PAGE
          : command === NATIVE_COMMANDS.search
            ? SEARCH_RESULTS
            : CHANNELS_PAGE,
      ),
    );
    const client = createNativeSparrowClient({ ipc });

    await client.schedule({
      id: CHANNEL.id,
      limit: 1,
      cursor: scheduleCursor,
      afterStartsAt: PROGRAMME.startsAt,
      previousCursors: [earlierCursor],
    });
    await client.search({
      term: "news",
      channelLimit: 1,
      channelCursor,
      channelPreviousCursors: [earlierCursor],
      programmeLimit: 1,
      programmeCursor,
      programmePreviousCursors: [earlierCursor],
    });
    await client.searchChannels({
      term: "news",
      limit: 1,
      cursor: channelCursor,
      previousCursors: [earlierCursor],
    });
    await client.searchProgrammes({
      term: "report",
      limit: 1,
      cursor: programmeCursor,
      previousCursors: [earlierCursor],
    });

    expect(ipc.invokes).toEqual([
      {
        command: NATIVE_COMMANDS.schedule,
        args: {
          input: { id: CHANNEL.id, limit: 1, cursor: scheduleCursor },
        },
      },
      {
        command: NATIVE_COMMANDS.search,
        args: {
          input: {
            requestId: searchRequestId(),
            term: "news",
            channelLimit: 1,
            channelCursor,
            programmeLimit: 1,
            programmeCursor,
          },
        },
      },
      {
        command: NATIVE_COMMANDS.searchChannels,
        args: {
          input: {
            requestId: searchRequestId(),
            term: "news",
            limit: 1,
            cursor: channelCursor,
          },
        },
      },
      {
        command: NATIVE_COMMANDS.searchProgrammes,
        args: {
          input: {
            requestId: searchRequestId(),
            term: "report",
            limit: 1,
            cursor: programmeCursor,
          },
        },
      },
    ]);
  });

  it("rejects continuation responses that repeat a submitted native cursor", async () => {
    const cursor = parsedCursor("submitted-page");
    const ipc = new FakeNativeIpc((command) =>
      Promise.resolve(
        command === NATIVE_COMMANDS.schedule
          ? { ...SCHEDULE_PAGE, next: cursor }
          : { ...CHANNELS_PAGE, next: cursor },
      ),
    );
    const client = createNativeSparrowClient({ ipc });

    await expect(
      client.schedule({ id: CHANNEL.id, limit: 1, cursor }),
    ).resolves.toEqual(invalidResponse());
    await expect(
      client.searchChannels({ term: "news", limit: 1, cursor }),
    ).resolves.toEqual(invalidResponse());
  });

  it("passes transient source locations only to replace and accepts only safe status", async () => {
    const sourceSecret = "https://user:secret@provider.invalid/list.m3u";
    const guideSecret = "https://user:secret@provider.invalid/guide.xml";
    const ipc = new FakeNativeIpc(() => Promise.resolve(FRESH_STATUS));
    const client = createNativeSparrowClient({ ipc });

    const result = await client.replaceSourceConfiguration({
      m3uLocation: `  ${sourceSecret}  `,
      epgLocation: guideSecret,
    });

    expect(result).toEqual({ ok: true, value: FRESH_STATUS });
    expect(JSON.stringify(result)).not.toContain("provider.invalid");
    expect(ipc.invokes).toEqual([
      {
        command: NATIVE_COMMANDS.replaceSourceConfiguration,
        args: {
          input: {
            m3uLocation: sourceSecret,
            epgLocation: guideSecret,
          },
        },
      },
    ]);
  });

  it("matches the core's 16 KiB UTF-8 source-location boundary", async () => {
    const prefix = "https://provider.invalid/";
    const boundary = `${prefix}${"a".repeat(16_384 - prefix.length)}`;
    const oversized = `${boundary}a`;
    const ipc = new FakeNativeIpc(() => Promise.resolve(FRESH_STATUS));
    const client = createNativeSparrowClient({ ipc });

    await expect(
      client.replaceSourceConfiguration({
        m3uLocation: boundary,
        epgLocation: null,
      }),
    ).resolves.toEqual({ ok: true, value: FRESH_STATUS });
    await expect(
      client.replaceSourceConfiguration({
        m3uLocation: oversized,
        epgLocation: null,
      }),
    ).resolves.toEqual({
      ok: false,
      error: {
        _tag: "invalid-input",
        field: "m3u",
        reason: "too-long",
      },
    });
    expect(ipc.invokes).toHaveLength(1);
  });

  it("rejects malformed successes and rejections without reflecting shell payloads", async () => {
    const secret = "https://user:secret@provider.invalid/list.m3u";
    const responses: readonly (() => Promise<unknown>)[] = [
      () => Promise.resolve({ ...FRESH_STATUS, sourceLocation: secret }),
      () => Promise.reject({ _tag: "transport", message: secret }),
    ];
    let index = 0;
    const ipc = new FakeNativeIpc(() => {
      const response = responses[index];
      index += 1;
      return response === undefined
        ? Promise.reject(new Error("fixture exhausted"))
        : response();
    });
    const client = createNativeSparrowClient({ ipc });

    const malformedSuccess = await client.status();
    const malformedFailure = await client.status();

    expect(malformedSuccess).toEqual(invalidResponse());
    expect(malformedFailure).toEqual(invalidResponse());
    expect(JSON.stringify([malformedSuccess, malformedFailure])).not.toContain(
      "provider.invalid",
    );
  });

  it("rejects hosted capabilities on the installed command boundary", async () => {
    const ipc = new FakeNativeIpc(() =>
      Promise.resolve({
        sourceConfiguration: "deployment-readonly",
        playbackTransport: "same-origin-http",
        audioTrackSelection: false,
        mpvFailover: false,
      }),
    );
    const client = createNativeSparrowClient({ ipc });

    await expect(client.capabilities()).resolves.toEqual(invalidResponse());
  });

  it("parses a safe command rejection as an expected client failure", async () => {
    const ipc = new FakeNativeIpc(() =>
      Promise.reject({ _tag: "not-configured" }),
    );
    const client = createNativeSparrowClient({ ipc });

    await expect(client.status()).resolves.toEqual({
      ok: false,
      error: { _tag: "not-configured" },
    });
  });

  it("does not invoke an already-cancelled command and ignores a late result", async () => {
    const late = deferred<unknown>();
    const ipc = new FakeNativeIpc(() => late.promise);
    const client = createNativeSparrowClient({ ipc });
    const cancelledBeforeStart = new AbortController();
    cancelledBeforeStart.abort();

    await expect(
      client.status({ signal: cancelledBeforeStart.signal }),
    ).resolves.toEqual({ ok: false, error: { _tag: "cancelled" } });
    expect(ipc.invokes).toHaveLength(0);

    const active = new AbortController();
    const result = client.status({ signal: active.signal });
    active.abort();
    await expect(result).resolves.toEqual({
      ok: false,
      error: { _tag: "cancelled" },
    });
    late.resolve(FRESH_STATUS);
    await late.promise;
    expect(ipc.invokes).toHaveLength(1);
  });

  it("cancels an active native search with its exact opaque request identifier", async () => {
    const searchFlight = deferred<unknown>();
    const ipc = new FakeNativeIpc((command) =>
      command === NATIVE_COMMANDS.search
        ? searchFlight.promise
        : Promise.resolve(null),
    );
    const client = createNativeSparrowClient({ ipc });
    const controller = new AbortController();

    const result = client.search({
      term: "news",
      channelLimit: 1,
      programmeLimit: 1,
      signal: controller.signal,
    });
    controller.abort();

    await expect(result).resolves.toEqual({
      ok: false,
      error: { _tag: "cancelled" },
    });
    const searchInvoke = requireFirst(ipc.invokes);
    const requestId = requireSearchRequestId(searchInvoke);
    expect(requestId).toEqual(searchRequestId());
    expect(ipc.invokes).toEqual([
      searchInvoke,
      {
        command: NATIVE_COMMANDS.cancelSearch,
        args: { input: { requestId } },
      },
    ]);

    searchFlight.resolve(SEARCH_RESULTS);
    await searchFlight.promise;
  });

  it("opens, reads, and stops only one correlated opaque playback session", async () => {
    const chunk = Uint8Array.from([0x47, 0x40, 0x00]).buffer;
    const streamHandle = `stream1_${"b".repeat(16)}`;
    const ipc = new FakeNativeIpc((command, args) => {
      switch (command) {
        case NATIVE_COMMANDS.startPlayback:
          return Promise.resolve({
            _tag: "tauri-native-stream",
            sessionId: requirePlaybackSessionId(args),
            streamHandle,
            ...EMPTY_AUDIO,
          });
        case NATIVE_COMMANDS.readPlayback:
          return Promise.resolve(chunk);
        case NATIVE_COMMANDS.stopPlayback:
          return Promise.resolve(null);
        default:
          return Promise.reject(new Error("unexpected fixture command"));
      }
    });
    const client = createNativeSparrowClient({ ipc });

    const started = await client.startPlayback({ id: CHANNEL.id });
    if (!started.ok || started.value._tag !== "tauri-native-stream") {
      throw new Error("expected a native playback descriptor");
    }
    const descriptor = started.value;
    await expect(
      client.readPlayback({
        sessionId: descriptor.sessionId,
        streamHandle: descriptor.streamHandle,
      }),
    ).resolves.toEqual({ ok: true, value: chunk });
    await expect(
      client.stopPlayback({
        sessionId: descriptor.sessionId,
        streamHandle: descriptor.streamHandle,
      }),
    ).resolves.toEqual({ ok: true, value: undefined });

    expect(ipc.invokes).toEqual([
      {
        command: NATIVE_COMMANDS.startPlayback,
        args: {
          input: { id: CHANNEL.id, sessionId: descriptor.sessionId },
        },
      },
      {
        command: NATIVE_COMMANDS.readPlayback,
        args: {
          input: {
            sessionId: descriptor.sessionId,
            streamHandle: descriptor.streamHandle,
          },
        },
      },
      {
        command: NATIVE_COMMANDS.stopPlayback,
        args: {
          input: {
            sessionId: descriptor.sessionId,
            streamHandle: descriptor.streamHandle,
          },
        },
      },
    ]);
  });

  it("final-stops the exact handle when a legacy native read is cancelled", async () => {
    const readFlight = deferred<unknown>();
    const ipc = new FakeNativeIpc((command) =>
      command === NATIVE_COMMANDS.readPlayback
        ? readFlight.promise
        : Promise.resolve(null),
    );
    const client = createNativeSparrowClient({ ipc });
    const sessionId = clientSchemas.playbackSessionId.parse(
      `play1_${"6".repeat(32)}_1`,
    );
    const streamHandle = clientSchemas.nativeStreamHandle.parse(
      `stream1_${"6".repeat(16)}`,
    );
    const controller = new AbortController();

    const reading = client.readPlayback({
      sessionId,
      streamHandle,
      signal: controller.signal,
    });
    controller.abort();
    await expect(reading).resolves.toEqual({
      ok: false,
      error: { _tag: "cancelled" },
    });
    expect(ipc.invokes).toEqual([
      {
        command: NATIVE_COMMANDS.readPlayback,
        args: { input: { sessionId, streamHandle } },
      },
      {
        command: NATIVE_COMMANDS.stopPlayback,
        args: { input: { sessionId, streamHandle } },
      },
    ]);

    readFlight.resolve(new ArrayBuffer(0));
    await readFlight.promise;
  });

  it("stops an opening session exactly once when the caller cancels", async () => {
    const startFlight = deferred<unknown>();
    const ipc = new FakeNativeIpc((command) =>
      command === NATIVE_COMMANDS.startPlayback
        ? startFlight.promise
        : Promise.resolve(null),
    );
    const client = createNativeSparrowClient({ ipc });
    const controller = new AbortController();

    const result = client.startPlayback({
      id: CHANNEL.id,
      signal: controller.signal,
    });
    const sessionId = requirePlaybackSessionId(requireFirst(ipc.invokes).args);
    controller.abort();
    await expect(result).resolves.toEqual({
      ok: false,
      error: { _tag: "cancelled" },
    });
    expect(ipc.invokes).toEqual([
      {
        command: NATIVE_COMMANDS.startPlayback,
        args: { input: { id: CHANNEL.id, sessionId } },
      },
      {
        command: NATIVE_COMMANDS.stopPlayback,
        args: { input: { sessionId } },
      },
    ]);

    startFlight.resolve({
      _tag: "tauri-native-stream",
      sessionId,
      streamHandle: `stream1_${"c".repeat(16)}`,
      ...EMPTY_AUDIO,
    });
    await startFlight.promise;
    await Promise.resolve();
    expect(
      ipc.invokes.filter(
        ({ command }) => command === NATIVE_COMMANDS.stopPlayback,
      ),
    ).toHaveLength(1);
  });

  it("releases malformed start successes and rejects malformed stream chunks", async () => {
    const secret = "https://user:secret@provider.invalid/live";
    const oversized = new ArrayBuffer(64 * 1024 + 1);
    let readResponse: unknown = new Uint8Array([1, 2, 3]);
    const ipc = new FakeNativeIpc((command, args) => {
      switch (command) {
        case NATIVE_COMMANDS.startPlayback:
          return Promise.resolve({
            _tag: "tauri-native-stream",
            sessionId: requirePlaybackSessionId(args),
            streamHandle: `stream1_${"d".repeat(16)}`,
            source: secret,
          });
        case NATIVE_COMMANDS.readPlayback:
          return Promise.resolve(readResponse);
        case NATIVE_COMMANDS.stopPlayback:
          return Promise.resolve(null);
        default:
          return Promise.reject(new Error("unexpected fixture command"));
      }
    });
    const client = createNativeSparrowClient({ ipc });
    const invalidStart = await client.startPlayback({ id: CHANNEL.id });
    const sessionId = requirePlaybackSessionId(requireFirst(ipc.invokes).args);

    expect(invalidStart).toEqual(invalidResponse());
    expect(JSON.stringify(invalidStart)).not.toContain("provider.invalid");
    expect(ipc.invokes.at(-1)).toEqual({
      command: NATIVE_COMMANDS.stopPlayback,
      args: { input: { sessionId } },
    });

    const readInput = {
      sessionId: clientSchemas.nativePlaybackDescriptor.parse({
        _tag: "tauri-native-stream",
        sessionId,
        streamHandle: `stream1_${"e".repeat(16)}`,
        ...EMPTY_AUDIO,
      }).sessionId,
      streamHandle: clientSchemas.nativePlaybackDescriptor.parse({
        _tag: "tauri-native-stream",
        sessionId,
        streamHandle: `stream1_${"e".repeat(16)}`,
        ...EMPTY_AUDIO,
      }).streamHandle,
    };
    await expect(client.readPlayback(readInput)).resolves.toEqual(
      invalidResponse(),
    );
    readResponse = oversized;
    await expect(client.readPlayback(readInput)).resolves.toEqual(
      invalidResponse(),
    );
  });

  it("owns suspend, reopen, read, and exact-once stop inside one session resource", async () => {
    const chunk = Uint8Array.from([0x47, 0x40, 0x01]).buffer;
    let reopenSequence = 1;
    const ipc = new FakeNativeIpc((command, args) => {
      switch (command) {
        case NATIVE_COMMANDS.startPlayback:
          return Promise.resolve({
            _tag: "tauri-native-stream",
            sessionId: requirePlaybackSessionId(args),
            streamHandle: `stream1_${"1".repeat(16)}`,
            ...EMPTY_AUDIO,
          });
        case NATIVE_COMMANDS.reopenPlayback:
          reopenSequence += 1;
          return Promise.resolve({
            _tag: "tauri-native-stream",
            sessionId: requirePlaybackSessionId(args),
            streamHandle: `stream1_${String(reopenSequence).repeat(16)}`,
            ...EMPTY_AUDIO,
          });
        case NATIVE_COMMANDS.readPlayback:
          return Promise.resolve(chunk);
        case NATIVE_COMMANDS.suspendPlayback:
        case NATIVE_COMMANDS.setPlaybackActivity:
        case NATIVE_COMMANDS.stopPlayback:
          return Promise.resolve(null);
        default:
          return Promise.reject(new Error("unexpected fixture command"));
      }
    });
    const session = createNativeSparrowClient({ ipc }).createPlaybackSession({
      id: CHANNEL.id,
    });

    const started = await session.start();
    if (!started.ok) {
      throw new Error("expected the session to start");
    }
    expect(started.value).toEqual({
      _tag: "tauri-native-stream",
      streamHandle: `stream1_${"1".repeat(16)}`,
      ...EMPTY_AUDIO,
    });
    expect(JSON.stringify(started.value)).not.toContain("play1_");
    await expect(session.suspend()).resolves.toEqual({
      ok: true,
      value: undefined,
    });
    const reopened = await session.reopen();
    if (!reopened.ok) {
      throw new Error("expected the session to reopen");
    }
    expect(reopened.value.streamHandle).not.toBe(started.value.streamHandle);
    await expect(
      session.read({ streamHandle: reopened.value.streamHandle }),
    ).resolves.toEqual({ ok: true, value: chunk });
    await expect(session.setActivity(true)).resolves.toEqual({
      ok: true,
      value: undefined,
    });
    const firstStop = session.stop();
    const secondStop = session.stop();
    await expect(firstStop).resolves.toEqual({ ok: true, value: undefined });
    await expect(secondStop).resolves.toEqual({ ok: true, value: undefined });
    await expect(session.setActivity(false)).resolves.toEqual({
      ok: true,
      value: undefined,
    });
    await expect(session.setActivity(true)).resolves.toEqual({
      ok: false,
      error: { _tag: "cancelled" },
    });

    const startInvoke = requireFirst(ipc.invokes);
    const sessionId = requirePlaybackSessionId(startInvoke.args);
    expect(ipc.invokes).toEqual([
      {
        command: NATIVE_COMMANDS.startPlayback,
        args: { input: { id: CHANNEL.id, sessionId } },
      },
      {
        command: NATIVE_COMMANDS.suspendPlayback,
        args: { input: { sessionId } },
      },
      {
        command: NATIVE_COMMANDS.reopenPlayback,
        args: { input: { sessionId } },
      },
      {
        command: NATIVE_COMMANDS.readPlayback,
        args: {
          input: {
            sessionId,
            streamHandle: reopened.value.streamHandle,
          },
        },
      },
      {
        command: NATIVE_COMMANDS.setPlaybackActivity,
        args: { input: { sessionId, active: true } },
      },
      {
        command: NATIVE_COMMANDS.stopPlayback,
        args: {
          input: {
            sessionId,
            streamHandle: reopened.value.streamHandle,
          },
        },
      },
    ]);
  });

  it("atomically restarts an exact handle for Audio Track selection and rejects stale work locally", async () => {
    const firstHandle = clientSchemas.nativeStreamHandle.parse(
      `stream1_${"7".repeat(16)}`,
    );
    const secondHandle = clientSchemas.nativeStreamHandle.parse(
      `stream1_${"8".repeat(16)}`,
    );
    const ipc = new FakeNativeIpc((command, args) => {
      switch (command) {
        case NATIVE_COMMANDS.startPlayback:
          return Promise.resolve(
            audioDescriptor(
              requirePlaybackSessionId(args),
              firstHandle,
              ENGLISH_AUDIO_ID,
              {
                _tag: "selected",
                trackId: ENGLISH_AUDIO_ID,
                reason: "first-available",
              },
            ),
          );
        case NATIVE_COMMANDS.restartPlayback:
          return Promise.resolve(
            audioDescriptor(
              requirePlaybackSessionId(args),
              secondHandle,
              SPANISH_AUDIO_ID,
              {
                _tag: "selected",
                trackId: SPANISH_AUDIO_ID,
                reason: "requested",
              },
              "saved",
            ),
          );
        case NATIVE_COMMANDS.stopPlayback:
          return Promise.resolve(null);
        default:
          return Promise.reject(new Error("unexpected fixture command"));
      }
    });
    const session = createNativeSparrowClient({ ipc }).createPlaybackSession({
      id: CHANNEL.id,
    });
    const started = await session.start();
    if (!started.ok) {
      throw new Error("expected the session to start");
    }

    const restarted = await session.restart({
      expectedStreamHandle: started.value.streamHandle,
      intent: { _tag: "select-audio", audioTrackId: SPANISH_AUDIO_ID },
    });
    expect(restarted).toEqual({
      ok: true,
      value: {
        _tag: "tauri-native-stream",
        streamHandle: secondHandle,
        tracks: audioTracks(SPANISH_AUDIO_ID),
        selection: {
          _tag: "selected",
          trackId: SPANISH_AUDIO_ID,
          reason: "requested",
        },
        preferenceStatus: "saved",
      },
    });
    await expect(
      session.restart({
        expectedStreamHandle: firstHandle,
        intent: { _tag: "select-audio", audioTrackId: ENGLISH_AUDIO_ID },
      }),
    ).resolves.toEqual({ ok: false, error: { _tag: "cancelled" } });
    await expect(
      session.read({ streamHandle: firstHandle }),
    ).resolves.toEqual({ ok: false, error: { _tag: "cancelled" } });
    await session.stop();

    const sessionId = requirePlaybackSessionId(requireFirst(ipc.invokes).args);
    expect(ipc.invokes).toEqual([
      {
        command: NATIVE_COMMANDS.startPlayback,
        args: { input: { id: CHANNEL.id, sessionId } },
      },
      {
        command: NATIVE_COMMANDS.restartPlayback,
        args: {
          input: {
            sessionId,
            expectedStreamHandle: firstHandle,
            intent: {
              _tag: "select-audio",
              audioTrackId: SPANISH_AUDIO_ID,
            },
          },
        },
      },
      {
        command: NATIVE_COMMANDS.stopPlayback,
        args: { input: { sessionId, streamHandle: secondHandle } },
      },
    ]);
  });

  it("accepts bounded missing Audio Track metadata and validates visible fallback invariants", () => {
    const valid = clientSchemas.nativePlaybackDescriptor.parse(
      audioDescriptor(
        `play1_${"9".repeat(32)}_1`,
        `stream1_${"9".repeat(16)}`,
        ENGLISH_AUDIO_ID,
        {
          _tag: "fallback",
          trackId: ENGLISH_AUDIO_ID,
          missing: "saved-preference",
        },
      ),
    );
    expect(valid.tracks[1]).toEqual({
      id: SPANISH_AUDIO_ID,
      codec: "ac-3",
      selected: false,
    });
    expect(valid.selection).toEqual({
      _tag: "fallback",
      trackId: ENGLISH_AUDIO_ID,
      missing: "saved-preference",
    });

    expect(
      clientSchemas.nativePlaybackDescriptor.safeParse({
        ...valid,
        tracks: [valid.tracks[0], valid.tracks[0]],
      }).success,
    ).toBe(false);
    expect(
      clientSchemas.nativePlaybackDescriptor.safeParse({
        ...valid,
        selection: {
          _tag: "selected",
          trackId: SPANISH_AUDIO_ID,
          reason: "requested",
        },
      }).success,
    ).toBe(false);
    expect(
      clientSchemas.nativePlaybackDescriptor.safeParse({
        ...valid,
        providerLocation: "private-canary",
      }).success,
    ).toBe(false);
  });

  it("starts and stops one correlated mpv fallback only after primary stop", async () => {
    const ipc = new FakeNativeIpc((command, args) => {
      const sessionId = requirePlaybackSessionId(args);
      switch (command) {
        case NATIVE_COMMANDS.startPlayback:
          return Promise.resolve({
            _tag: "tauri-native-stream",
            sessionId,
            streamHandle: `stream1_${"7".repeat(16)}`,
            ...EMPTY_AUDIO,
          });
        case NATIVE_COMMANDS.stopPlayback:
          return Promise.resolve(null);
        case NATIVE_COMMANDS.startMpvFallback:
          return Promise.resolve({ _tag: "fallback-playing", sessionId });
        case NATIVE_COMMANDS.stopMpvFallback:
          return Promise.resolve({ _tag: "fallback-stopped", sessionId });
        default:
          return Promise.reject(new Error("unexpected fixture command"));
      }
    });
    const session = createNativeSparrowClient({ ipc }).createPlaybackSession({
      id: CHANNEL.id,
    });

    await session.start();
    await expect(session.startMpvFallback()).resolves.toEqual({
      ok: false,
      error: {
        _tag: "fallback-failed",
        reason: "primary-active",
        retryable: true,
      },
    });
    await session.stop();
    const firstStart = session.startMpvFallback();
    const secondStart = session.startMpvFallback();
    const started = await firstStart;
    await expect(secondStart).resolves.toBe(started);
    expect(started).toMatchObject({
      ok: true,
      value: { _tag: "fallback-playing" },
    });
    const firstStop = session.stopMpvFallback();
    const secondStop = session.stopMpvFallback();
    const stopped = await firstStop;
    await expect(secondStop).resolves.toBe(stopped);
    expect(stopped).toMatchObject({
      ok: true,
      value: { _tag: "fallback-stopped" },
    });

    const sessionId = requirePlaybackSessionId(requireFirst(ipc.invokes).args);
    expect(ipc.invokes.map(({ command }) => command)).toEqual([
      NATIVE_COMMANDS.startPlayback,
      NATIVE_COMMANDS.stopPlayback,
      NATIVE_COMMANDS.startMpvFallback,
      NATIVE_COMMANDS.stopMpvFallback,
    ]);
    expect(ipc.invokes.slice(1).every(({ args }) =>
      requirePlaybackSessionId(args) === sessionId,
    )).toBe(true);
  });

  it("parses typed mpv failures and rejects uncorrelated fallback responses", async () => {
    let response: "failure" | "uncorrelated" = "failure";
    const ipc = new FakeNativeIpc((command, args) => {
      const sessionId = requirePlaybackSessionId(args);
      switch (command) {
        case NATIVE_COMMANDS.startPlayback:
          return Promise.resolve({
            _tag: "tauri-native-stream",
            sessionId,
            streamHandle: `stream1_${"8".repeat(16)}`,
            ...EMPTY_AUDIO,
          });
        case NATIVE_COMMANDS.stopPlayback:
          return Promise.resolve(null);
        case NATIVE_COMMANDS.startMpvFallback:
          return response === "failure"
            ? Promise.reject({
                _tag: "fallback-failed",
                reason: "not-installed",
                retryable: false,
              })
            : Promise.resolve({
                _tag: "fallback-playing",
                sessionId: `play1_${"0".repeat(32)}_9`,
              });
        default:
          return Promise.reject(new Error("unexpected fixture command"));
      }
    });
    const session = createNativeSparrowClient({ ipc }).createPlaybackSession({
      id: CHANNEL.id,
    });
    await session.start();
    await session.stop();

    await expect(session.startMpvFallback()).resolves.toEqual({
      ok: false,
      error: {
        _tag: "fallback-failed",
        reason: "not-installed",
        retryable: false,
      },
    });
    response = "uncorrelated";
    await expect(session.startMpvFallback()).resolves.toEqual(invalidResponse());
  });

  it("requests mpv cleanup when a launch is cancelled or resolves late", async () => {
    const launch = deferred<unknown>();
    const ipc = new FakeNativeIpc((command, args) => {
      const sessionId = requirePlaybackSessionId(args);
      switch (command) {
        case NATIVE_COMMANDS.startPlayback:
          return Promise.resolve({
            _tag: "tauri-native-stream",
            sessionId,
            streamHandle: `stream1_${"9".repeat(16)}`,
            ...EMPTY_AUDIO,
          });
        case NATIVE_COMMANDS.stopPlayback:
          return Promise.resolve(null);
        case NATIVE_COMMANDS.startMpvFallback:
          return launch.promise;
        case NATIVE_COMMANDS.stopMpvFallback:
          return Promise.resolve({ _tag: "fallback-stopped", sessionId });
        default:
          return Promise.reject(new Error("unexpected fixture command"));
      }
    });
    const session = createNativeSparrowClient({ ipc }).createPlaybackSession({
      id: CHANNEL.id,
    });
    await session.start();
    await session.stop();
    const controller = new AbortController();
    const result = session.startMpvFallback({ signal: controller.signal });
    controller.abort();

    await expect(result).resolves.toEqual({
      ok: false,
      error: { _tag: "cancelled" },
    });
    const sessionId = requirePlaybackSessionId(requireFirst(ipc.invokes).args);
    launch.resolve({ _tag: "fallback-playing", sessionId });
    await launch.promise;
    await Promise.resolve();
    expect(
      ipc.invokes.filter(({ command }) => command === NATIVE_COMMANDS.stopMpvFallback),
    ).toHaveLength(1);
  });

  it("reopens the same pinned session after the initial open returns a safe failure", async () => {
    const ipc = new FakeNativeIpc((command, args) => {
      switch (command) {
        case NATIVE_COMMANDS.startPlayback:
          return Promise.reject({
            _tag: "playback-failed",
            reason: "unavailable",
            retryable: true,
          });
        case NATIVE_COMMANDS.reopenPlayback:
          return Promise.resolve({
            _tag: "tauri-native-stream",
            sessionId: requirePlaybackSessionId(args),
            streamHandle: `stream1_${"4".repeat(16)}`,
            ...EMPTY_AUDIO,
          });
        default:
          return Promise.reject(new Error("unexpected fixture command"));
      }
    });
    const session = createNativeSparrowClient({ ipc }).createPlaybackSession({
      id: CHANNEL.id,
    });

    await expect(session.start()).resolves.toEqual({
      ok: false,
      error: {
        _tag: "playback-failed",
        reason: "unavailable",
        retryable: true,
      },
    });
    await expect(session.reopen()).resolves.toMatchObject({
      ok: true,
      value: { _tag: "tauri-native-stream" },
    });
    const sessionIds = ipc.invokes.map(({ args }) =>
      requirePlaybackSessionId(args),
    );
    expect(new Set(sessionIds).size).toBe(1);
    expect(ipc.invokes.map(({ command }) => command)).toEqual([
      NATIVE_COMMANDS.startPlayback,
      NATIVE_COMMANDS.reopenPlayback,
    ]);
  });

  it("waits for a cancelled initial open, suspends its late handle, and never final-stops implicitly", async () => {
    const startFlight = deferred<unknown>();
    const ipc = new FakeNativeIpc((command, args) => {
      switch (command) {
        case NATIVE_COMMANDS.startPlayback:
          return startFlight.promise;
        case NATIVE_COMMANDS.reopenPlayback:
          return Promise.resolve({
            _tag: "tauri-native-stream",
            sessionId: requirePlaybackSessionId(args),
            streamHandle: `stream1_${"6".repeat(16)}`,
            ...EMPTY_AUDIO,
          });
        case NATIVE_COMMANDS.suspendPlayback:
        case NATIVE_COMMANDS.stopPlayback:
          return Promise.resolve(null);
        default:
          return Promise.reject(new Error("unexpected fixture command"));
      }
    });
    const session = createNativeSparrowClient({ ipc }).createPlaybackSession({
      id: CHANNEL.id,
    });
    const controller = new AbortController();
    const start = session.start({ signal: controller.signal });
    const sessionId = requirePlaybackSessionId(requireFirst(ipc.invokes).args);
    controller.abort();
    await expect(start).resolves.toEqual({
      ok: false,
      error: { _tag: "cancelled" },
    });

    const reopen = session.reopen();
    await Promise.resolve();
    expect(ipc.invokes.map(({ command }) => command)).toEqual([
      NATIVE_COMMANDS.startPlayback,
    ]);
    startFlight.resolve({
      _tag: "tauri-native-stream",
      sessionId,
      streamHandle: `stream1_${"5".repeat(16)}`,
      ...EMPTY_AUDIO,
    });
    await startFlight.promise;
    await reopen;
    expect(ipc.invokes.map(({ command }) => command)).toEqual([
      NATIVE_COMMANDS.startPlayback,
      NATIVE_COMMANDS.suspendPlayback,
      NATIVE_COMMANDS.reopenPlayback,
    ]);
    expect(
      ipc.invokes.filter(
        ({ command }) => command === NATIVE_COMMANDS.stopPlayback,
      ),
    ).toHaveLength(0);

    await session.stop();
    await session.stop();
    expect(
      ipc.invokes.filter(
        ({ command }) => command === NATIVE_COMMANDS.stopPlayback,
      ),
    ).toHaveLength(1);
  });

  it("delivers strict ordered Channel events and unsubscribes idempotently", async () => {
    const subscription = deferred<unknown>();
    const ipc = new FakeNativeIpc((command) =>
      command === NATIVE_COMMANDS.subscribe
        ? subscription.promise
        : Promise.resolve(null),
    );
    const client = createNativeSparrowClient({ ipc });
    const consoleSpies = [
      vi.spyOn(console, "log").mockImplementation(() => undefined),
      vi.spyOn(console, "warn").mockImplementation(() => undefined),
      vi.spyOn(console, "error").mockImplementation(() => undefined),
    ];
    const received: unknown[] = [];

    const release = client.subscribe((event) => received.push(event));
    const channel = requireFirst(ipc.channels);
    channel.onmessage({
      _tag: "catalog-published",
      occurredAt: "2026-08-30T10:01:00Z",
      generation: 8,
      sourceLocation: "https://provider.invalid/private.m3u",
    });
    const first = clientSchemas.sparrowEvent.parse({
      _tag: "catalog-published",
      occurredAt: "2026-08-30T10:01:01Z",
      generation: 8,
    });
    const second = clientSchemas.sparrowEvent.parse({
      _tag: "catalog-status-changed",
      occurredAt: "2026-08-30T10:01:02Z",
      status: FRESH_STATUS,
    });
    channel.onmessage(first);
    channel.onmessage(second);
    subscription.resolve("sub1_0000000000000001");
    await subscription.promise;
    await Promise.resolve();

    expect(received).toEqual([first, second]);
    expect(consoleSpies.every((spy) => spy.mock.calls.length === 0)).toBe(true);
    release();
    release();
    channel.onmessage(first);
    await Promise.resolve();
    expect(received).toEqual([first, second]);
    expect(ipc.invokes).toEqual([
      {
        command: NATIVE_COMMANDS.subscribe,
        args: { events: channel },
      },
      {
        command: NATIVE_COMMANDS.unsubscribe,
        args: { subscriptionId: "sub1_0000000000000001" },
      },
    ]);
  });

  it("releases a subscription that resolves after the caller unmounts", async () => {
    const subscription = deferred<unknown>();
    const ipc = new FakeNativeIpc((command) =>
      command === NATIVE_COMMANDS.subscribe
        ? subscription.promise
        : Promise.resolve(null),
    );
    const client = createNativeSparrowClient({ ipc });

    const release = client.subscribe(() => undefined);
    release();
    subscription.resolve("sub1_0000000000000002");
    await subscription.promise;
    await Promise.resolve();

    expect(ipc.invokes.map(({ command }) => command)).toEqual([
      NATIVE_COMMANDS.subscribe,
      NATIVE_COMMANDS.unsubscribe,
    ]);
  });

  it("rejects a subscription identifier outside the native contract", async () => {
    const ipc = new FakeNativeIpc((command) =>
      Promise.resolve(
        command === NATIVE_COMMANDS.subscribe ? "subscription-1" : null,
      ),
    );
    const client = createNativeSparrowClient({ ipc });
    const received: unknown[] = [];

    client.subscribe((event) => received.push(event));
    await Promise.resolve();
    await Promise.resolve();
    requireFirst(ipc.channels).onmessage(
      clientSchemas.sparrowEvent.parse({
        _tag: "catalog-published",
        occurredAt: "2026-08-30T10:01:01Z",
        generation: 8,
      }),
    );

    expect(received).toEqual([]);
    expect(ipc.invokes.map(({ command }) => command)).toEqual([
      NATIVE_COMMANDS.subscribe,
    ]);
  });
});

function invalidResponse(): ClientResult<never> {
  return {
    ok: false,
    error: {
      _tag: "transport",
      retryable: false,
      message: "The installed app returned an invalid response.",
    },
  };
}

function audioTracks(selected: typeof ENGLISH_AUDIO_ID | typeof SPANISH_AUDIO_ID) {
  return [
    {
      id: ENGLISH_AUDIO_ID,
      language: "eng",
      label: "Original",
      codec: "aac-adts" as const,
      selected: selected === ENGLISH_AUDIO_ID,
    },
    {
      id: SPANISH_AUDIO_ID,
      codec: "ac-3" as const,
      selected: selected === SPANISH_AUDIO_ID,
    },
  ];
}

function audioDescriptor(
  sessionId: string,
  streamHandle: string,
  selected: typeof ENGLISH_AUDIO_ID | typeof SPANISH_AUDIO_ID,
  selection: Readonly<Record<string, unknown>>,
  preferenceStatus?: "saved" | "not-saved" | "unchanged",
) {
  return {
    _tag: "tauri-native-stream" as const,
    sessionId,
    streamHandle,
    tracks: audioTracks(selected),
    selection,
    ...(preferenceStatus === undefined ? {} : { preferenceStatus }),
  };
}

function searchRequestId() {
  return expect.stringMatching(/^srch1_[0-9a-f]{32}_[0-9a-f]+$/u);
}

function requireSearchRequestId(invoke: RecordedInvoke): unknown {
  const input = invoke.args?.input;
  if (typeof input !== "object" || input === null || !("requestId" in input)) {
    throw new Error("expected a native search request identifier");
  }
  return input.requestId;
}

function requirePlaybackSessionId(
  args: Readonly<Record<string, unknown>> | undefined,
): string {
  const input = args?.input;
  if (
    typeof input !== "object" ||
    input === null ||
    !("sessionId" in input) ||
    typeof input.sessionId !== "string" ||
    !/^play1_[0-9a-f]{32}_[0-9a-f]+$/u.test(input.sessionId)
  ) {
    throw new Error("expected a native playback session identifier");
  }
  return input.sessionId;
}

interface Deferred<Value> {
  readonly promise: Promise<Value>;
  readonly resolve: (value: Value) => void;
}

function deferred<Value>(): Deferred<Value> {
  let resolver: ((value: Value) => void) | undefined;
  const promise = new Promise<Value>((resolve) => {
    resolver = resolve;
  });
  return {
    promise,
    resolve: (value) => {
      if (resolver === undefined) {
        throw new Error("deferred resolver was not initialized");
      }
      resolver(value);
    },
  };
}

function requireFirst<Value>(values: readonly Value[]): Value {
  const first = values[0];
  if (first === undefined) {
    throw new Error("expected one recorded value");
  }
  return first;
}

function parsedCursor(value: string) {
  const parsed = clientSchemas.groupsPage.safeParse({
    generation: 7,
    items: [],
    next: value,
  });
  if (!parsed.success || parsed.data.next === null) {
    throw new Error("expected a valid page cursor fixture");
  }
  return parsed.data.next;
}
