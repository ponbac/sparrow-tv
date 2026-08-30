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
  playbackTransport: "unavailable",
  audioTrackSelection: false,
  mpvFailover: false,
} as const;

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
            : command === NATIVE_COMMANDS.groups
              ? GROUPS_PAGE
              : command === NATIVE_COMMANDS.channels
                ? CHANNELS_PAGE
                : command === NATIVE_COMMANDS.channel
                  ? CHANNEL
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

    expect(ipc.invokes).toEqual([
      { command: NATIVE_COMMANDS.capabilities, args: undefined },
      { command: NATIVE_COMMANDS.status, args: undefined },
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
    ]);
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
