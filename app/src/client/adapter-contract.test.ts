// @vitest-environment node

import { describe, expect, it } from "vitest";
import {
  clientSchemas,
  type ChannelId,
  type ClientError,
  type ClientResult,
} from "./contracts";
import { createHttpSparrowClient } from "./http";
import {
  createNativeSparrowClient,
  NATIVE_COMMANDS,
  type NativeChannel,
  type NativeIpc,
} from "./native";

const STATUS = clientSchemas.status.parse({
  generation: 7,
  configuration: { configured: true, epgConfigured: true },
  m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
  epg: { _tag: "fresh", validatedAt: "2026-08-30T10:00:01Z" },
});

const CHANNEL_PAGE = clientSchemas.channelsPage.parse({
  generation: 7,
  items: [{ id: "world-news", name: "World News", group: "News" }],
  next: null,
});

const PROGRAMME_PAGE = clientSchemas.schedulePage.parse({
  generation: 7,
  items: [
    {
      channelId: "world-news",
      title: "Evening Report",
      description: "Headlines and analysis.",
      startsAt: "2026-08-30T19:00:00Z",
      endsAt: "2026-08-30T20:00:00Z",
    },
  ],
  next: null,
});

const SEARCH_RESULTS = clientSchemas.searchResults.parse({
  generation: 7,
  channels: CHANNEL_PAGE,
  programmes: PROGRAMME_PAGE,
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
  status: STATUS,
});

type ContractResult = ClientResult<unknown>;

describe("hosted and installed query-adapter contract", () => {
  it("returns the same safe successes for refresh, schedule, search, and both continuation lanes", async () => {
    const payloads = [
      REFRESH_REPORT,
      PROGRAMME_PAGE,
      SEARCH_RESULTS,
      CHANNEL_PAGE,
      PROGRAMME_PAGE,
    ] as const;
    const http = createHttpSparrowClient({ fetch: queuedFetch(payloads) });
    const native = createNativeSparrowClient({
      ipc: new CommandNativeIpc(
        new Map<string, unknown>([
          [NATIVE_COMMANDS.refresh, REFRESH_REPORT],
          [NATIVE_COMMANDS.schedule, PROGRAMME_PAGE],
          [NATIVE_COMMANDS.search, SEARCH_RESULTS],
          [NATIVE_COMMANDS.searchChannels, CHANNEL_PAGE],
          [NATIVE_COMMANDS.searchProgrammes, PROGRAMME_PAGE],
        ]),
      ),
    });
    const id = parsedChannelId("world-news");

    const hostedResults: readonly ContractResult[] = [
      await http.refresh(),
      await http.schedule({ id, limit: 1 }),
      await http.search({
        term: "news",
        channelLimit: 1,
        programmeLimit: 1,
      }),
      await http.searchChannels({ term: "news", limit: 1 }),
      await http.searchProgrammes({ term: "report", limit: 1 }),
    ];
    const installedResults: readonly ContractResult[] = [
      await native.refresh(),
      await native.schedule({ id, limit: 1 }),
      await native.search({
        term: "news",
        channelLimit: 1,
        programmeLimit: 1,
      }),
      await native.searchChannels({ term: "news", limit: 1 }),
      await native.searchProgrammes({ term: "report", limit: 1 }),
    ];

    expect(installedResults).toEqual(hostedResults);
  });

  it("returns the same typed failures for every shared query operation", async () => {
    const failures = [
      { _tag: "service-unavailable" },
      { _tag: "not-found", resource: "channel" },
      {
        _tag: "invalid-input",
        field: "search-term",
        reason: "required",
      },
      { _tag: "stale-cursor", current: CHANNEL_PAGE.generation },
      { _tag: "catalog-unavailable", status: STATUS },
    ] as const satisfies readonly ClientError[];
    const http = createHttpSparrowClient({
      fetch: queuedFetch(
        failures.map((error) => ({ error })),
        failures.map(() => 409),
      ),
    });
    const native = createNativeSparrowClient({
      ipc: new RejectingNativeIpc(
        new Map<string, ClientError>([
          [NATIVE_COMMANDS.refresh, failures[0]],
          [NATIVE_COMMANDS.schedule, failures[1]],
          [NATIVE_COMMANDS.search, failures[2]],
          [NATIVE_COMMANDS.searchChannels, failures[3]],
          [NATIVE_COMMANDS.searchProgrammes, failures[4]],
        ]),
      ),
    });
    const id = parsedChannelId("world-news");

    const hostedResults: readonly ContractResult[] = [
      await http.refresh(),
      await http.schedule({ id, limit: 1 }),
      await http.search({
        term: "news",
        channelLimit: 1,
        programmeLimit: 1,
      }),
      await http.searchChannels({ term: "news", limit: 1 }),
      await http.searchProgrammes({ term: "report", limit: 1 }),
    ];
    const installedResults: readonly ContractResult[] = [
      await native.refresh(),
      await native.schedule({ id, limit: 1 }),
      await native.search({
        term: "news",
        channelLimit: 1,
        programmeLimit: 1,
      }),
      await native.searchChannels({ term: "news", limit: 1 }),
      await native.searchProgrammes({ term: "report", limit: 1 }),
    ];

    expect(installedResults).toEqual(hostedResults);
    expect(installedResults).toEqual(
      failures.map((error) => ({ ok: false, error })),
    );
  });
});

class CommandNativeIpc implements NativeIpc {
  constructor(private readonly responses: ReadonlyMap<string, unknown>) {}

  invoke(command: string): Promise<unknown> {
    if (!this.responses.has(command)) {
      return Promise.reject(new Error("unexpected native command"));
    }
    return Promise.resolve(this.responses.get(command));
  }

  createChannel(onmessage: (message: unknown) => void): NativeChannel {
    return { onmessage };
  }
}

class RejectingNativeIpc implements NativeIpc {
  constructor(private readonly failures: ReadonlyMap<string, ClientError>) {}

  invoke(command: string): Promise<unknown> {
    const failure = this.failures.get(command);
    return failure === undefined
      ? Promise.reject(new Error("unexpected native command"))
      : Promise.reject(failure);
  }

  createChannel(onmessage: (message: unknown) => void): NativeChannel {
    return { onmessage };
  }
}

function queuedFetch(
  payloads: readonly unknown[],
  statuses: readonly number[] = payloads.map(() => 200),
): typeof fetch {
  const remainingPayloads = [...payloads];
  const remainingStatuses = [...statuses];
  return async () => {
    const payload = remainingPayloads.shift();
    const status = remainingStatuses.shift();
    if (payload === undefined || status === undefined) {
      throw new Error("unexpected hosted request");
    }
    return new Response(JSON.stringify(payload), {
      status,
      headers: { "content-type": "application/json" },
    });
  };
}

function parsedChannelId(value: string): ChannelId {
  const parsed = clientSchemas.channel.safeParse({
    id: value,
    name: "Fixture Channel",
    group: "Fixture",
  });
  if (!parsed.success) {
    throw new Error("expected a valid Channel Identifier fixture");
  }
  return parsed.data.id;
}
