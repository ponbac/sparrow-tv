// @vitest-environment node

import { describe, expect, it } from "vitest";
import { clientSchemas } from "./contracts";
import { createHttpSparrowClient } from "./http";

interface JsonFixture {
  readonly body: unknown;
  readonly status?: number;
}

interface RecordedRequest {
  readonly url: string;
  readonly init: RequestInit | undefined;
}

interface FakeHttp {
  readonly fetch: typeof fetch;
  readonly requests: readonly RecordedRequest[];
}

const capabilities = {
  sourceConfiguration: "deployment-readonly",
  playbackTransport: "same-origin-http",
  audioTrackSelection: false,
  mpvFailover: false,
};

const freshStatus = {
  generation: 7,
  configuration: { configured: true, epgConfigured: true },
  m3u: { _tag: "fresh", validatedAt: "2026-08-30T00:00:00Z" },
  epg: { _tag: "fresh", validatedAt: "2026-08-30T00:00:01Z" },
};

const groupsPage = {
  generation: 7,
  items: [{ name: "News", channelCount: 2 }],
  next: "groups-next",
};

const channelsPage = {
  generation: 7,
  items: [{ id: "channel-one", name: "World News", group: "News" }],
  next: "channels-next",
};

const channelDetails = {
  id: "channel-one",
  name: "World News",
  group: "News",
};

describe("hosted HTTP Sparrow client", () => {
  it("runtime-parses every success response through the narrow client", async () => {
    const http = createFakeHttp([
      { body: capabilities },
      { body: freshStatus },
      { body: groupsPage },
      { body: channelsPage },
      { body: channelDetails },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await expect(client.capabilities()).resolves.toEqual({
      ok: true,
      value: capabilities,
    });
    await expect(client.status()).resolves.toEqual({
      ok: true,
      value: freshStatus,
    });
    await expect(client.listGroups({ limit: 50 })).resolves.toEqual({
      ok: true,
      value: groupsPage,
    });

    const channels = await client.listChannels({ limit: 24, group: "News" });
    expect(channels).toEqual({ ok: true, value: channelsPage });
    if (!channels.ok) {
      throw new Error("expected the channel page fixture to parse");
    }
    const firstChannel = channels.value.items[0];
    if (firstChannel === undefined) {
      throw new Error("expected the channel page fixture to contain one item");
    }

    await expect(client.channel({ id: firstChannel.id })).resolves.toEqual({
      ok: true,
      value: channelDetails,
    });

    expect(http.requests).toHaveLength(5);
    for (const request of http.requests) {
      expect(request.url.startsWith("/api/v1/")).toBe(true);
      expect(request.url).not.toContain("http://");
      expect(request.url).not.toContain("https://");
      expect(request.init?.credentials).toBe("same-origin");
      expect(new Headers(request.init?.headers).has("authorization")).toBe(
        false,
      );
    }
  });

  it("accepts every source lifecycle variant in status responses", async () => {
    const states: readonly unknown[] = [
      { _tag: "fresh", validatedAt: "2026-08-30T00:00:00Z" },
      {
        _tag: "stale",
        validatedAt: "2026-08-30T00:00:00Z",
        nextAttemptAt: null,
      },
      { _tag: "unavailable", failure: null },
      {
        _tag: "refreshing",
        validatedAt: null,
        startedAt: "2026-08-30T00:00:02Z",
      },
      {
        _tag: "deferred",
        validatedAt: "2026-08-30T00:00:00Z",
        deferredAt: "2026-08-30T00:00:02Z",
      },
      {
        _tag: "failed",
        validatedAt: null,
        failure: { _tag: "source-access" },
        nextAttemptAt: "2026-08-30T00:01:00Z",
      },
    ];
    const http = createFakeHttp(
      states.map((m3u) => ({
        body: {
          generation: null,
          configuration: { configured: false, epgConfigured: false },
          m3u,
          epg: null,
        },
      })),
    );
    const client = createHttpSparrowClient({ fetch: http.fetch });

    const results = await Promise.all(states.map(() => client.status()));
    for (const result of results) {
      expect(result.ok).toBe(true);
    }
  });

  it("returns a safe transport failure for malformed success payloads", async () => {
    const http = createFakeHttp([
      {
        body: {
          ...groupsPage,
          generation: Number.MAX_SAFE_INTEGER + 1,
        },
      },
      {
        body: {
          ...channelDetails,
          sourceUrl: "https://provider.example/private.m3u",
        },
      },
      {
        body: {
          ...freshStatus,
          m3u: {
            _tag: "failed",
            validatedAt: null,
            failure: {
              _tag: "http",
              url: "https://provider.example/private.m3u",
            },
            nextAttemptAt: "2026-08-30T00:01:00Z",
          },
        },
      },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await expect(client.listGroups({ limit: 50 })).resolves.toEqual(
      invalidResponse(false),
    );

    const parsedChannel = clientSchemas.channelsPage.safeParse(channelsPage);
    if (!parsedChannel.success) {
      throw new Error("expected the channel fixture to parse");
    }
    const channel = parsedChannel.data.items[0];
    if (channel === undefined) {
      throw new Error("expected the channel fixture to contain one item");
    }
    await expect(client.channel({ id: channel.id })).resolves.toEqual(
      invalidResponse(false),
    );
    await expect(client.status()).resolves.toEqual(invalidResponse(false));
  });

  it("requires an error envelope on non-success statuses", async () => {
    const http = createFakeHttp([
      { status: 401, body: { _tag: "authentication-required" } },
      { status: 503, body: groupsPage },
      { status: 200, body: { error: { _tag: "not-configured" } } },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await expect(client.capabilities()).resolves.toEqual(
      invalidResponse(false),
    );
    await expect(client.listGroups({ limit: 50 })).resolves.toEqual(
      invalidResponse(true),
    );
    await expect(client.status()).resolves.toEqual(invalidResponse(false));
  });

  it("returns typed authentication and catalog failures", async () => {
    const http = createFakeHttp([
      {
        status: 401,
        body: { error: { _tag: "authentication-required" } },
      },
      {
        status: 503,
        body: {
          error: { _tag: "catalog-unavailable", status: freshStatus },
        },
      },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await expect(client.capabilities()).resolves.toEqual({
      ok: false,
      error: { _tag: "authentication-required" },
    });
    await expect(client.listChannels({ limit: 24 })).resolves.toEqual({
      ok: false,
      error: { _tag: "catalog-unavailable", status: freshStatus },
    });
  });

  it("runtime-parses every remaining typed server error", async () => {
    const errors: readonly unknown[] = [
      { _tag: "service-unavailable" },
      { _tag: "invalid-input", field: "page-limit", reason: "out-of-range" },
      { _tag: "invalid-input", field: "route", reason: "invalid-format" },
      { _tag: "not-configured" },
      { _tag: "not-found", resource: "channel" },
      { _tag: "stale-cursor", current: 8 },
      {
        _tag: "playback-failed",
        reason: "timed-out",
        retryable: true,
      },
    ];
    const http = createFakeHttp(
      errors.map((error) => ({ status: 400, body: { error } })),
    );
    const client = createHttpSparrowClient({ fetch: http.fetch });

    for (const error of errors) {
      await expect(client.status()).resolves.toEqual({ ok: false, error });
    }
  });

  it("rejects adapter-local failures when a server tries to send them", async () => {
    const privateMessage = "private provider diagnostic";
    const http = createFakeHttp([
      {
        status: 502,
        body: {
          error: {
            _tag: "transport",
            retryable: true,
            message: privateMessage,
          },
        },
      },
      { status: 400, body: { error: { _tag: "cancelled" } } },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    const transport = await client.status();
    expect(transport).toEqual(invalidResponse(true));
    expect(JSON.stringify(transport)).not.toContain(privateMessage);
    await expect(client.status()).resolves.toEqual(invalidResponse(false));
  });

  it("classifies aborts before transport failures", async () => {
    const controller = new AbortController();
    controller.abort();
    const fetchImplementation: typeof fetch = async () => {
      throw new Error("provider URL must never escape");
    };
    const client = createHttpSparrowClient({ fetch: fetchImplementation });

    await expect(client.status({ signal: controller.signal })).resolves.toEqual({
      ok: false,
      error: { _tag: "cancelled" },
    });
  });

  it("recognizes an AbortError even when no signal was supplied", async () => {
    const fetchImplementation: typeof fetch = async () => {
      throw new DOMException("operation aborted", "AbortError");
    };
    const client = createHttpSparrowClient({ fetch: fetchImplementation });

    await expect(client.status()).resolves.toEqual({
      ok: false,
      error: { _tag: "cancelled" },
    });
  });

  it("minimizes network and non-JSON failures", async () => {
    const privateFailure =
      "GET https://user:secret@provider.example/private.m3u failed";
    const failingFetch: typeof fetch = async () => {
      throw new Error(privateFailure);
    };
    const networkClient = createHttpSparrowClient({ fetch: failingFetch });

    const networkResult = await networkClient.status();
    expect(networkResult).toEqual({
      ok: false,
      error: {
        _tag: "transport",
        retryable: true,
        message: "The Sparrow server could not be reached.",
      },
    });
    expect(JSON.stringify(networkResult)).not.toContain(privateFailure);

    const nonJsonFetch: typeof fetch = async () =>
      new Response("upstream exploded", {
        status: 502,
        headers: { "content-type": "text/plain" },
      });
    const nonJsonClient = createHttpSparrowClient({ fetch: nonJsonFetch });
    await expect(nonJsonClient.status()).resolves.toEqual(
      invalidResponse(true),
    );
  });

  it("preserves absent, empty, and encoded group filters", async () => {
    const http = createFakeHttp([
      { body: { ...channelsPage, next: null } },
      { body: { ...channelsPage, next: null } },
      { body: { ...channelsPage, next: null } },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await client.listChannels({ limit: 24 });
    await client.listChannels({ limit: 24, group: "" });
    await client.listChannels({ limit: 24, group: "News & Å/World" });

    expect(requestAt(http, 0).url).toBe("/api/v1/channels?limit=24");
    expect(requestAt(http, 1).url).toBe("/api/v1/channels?limit=24&group=");
    expect(requestAt(http, 2).url).toBe(
      "/api/v1/channels?limit=24&group=News+%26+%C3%85%2FWorld",
    );
  });

  it("encodes opaque cursors and channel identifiers", async () => {
    const opaqueGroupsPage = {
      generation: 7,
      items: [],
      next: "next /?&=☃",
    };
    const opaqueChannelsPage = {
      generation: 7,
      items: [{ id: "channel /?&=☃", name: "Encoded", group: "" }],
      next: null,
    };
    const parsedGroups = clientSchemas.groupsPage.safeParse(opaqueGroupsPage);
    const parsedChannels =
      clientSchemas.channelsPage.safeParse(opaqueChannelsPage);
    if (!parsedGroups.success || parsedGroups.data.next === null) {
      throw new Error("expected the opaque group cursor fixture to parse");
    }
    if (!parsedChannels.success) {
      throw new Error("expected the opaque channel fixture to parse");
    }
    const channel = parsedChannels.data.items[0];
    if (channel === undefined) {
      throw new Error("expected the opaque channel fixture to contain one item");
    }

    const http = createFakeHttp([
      { body: { ...groupsPage, next: null } },
      { body: channelDetails },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });
    await client.listGroups({ limit: 50, cursor: parsedGroups.data.next });
    await client.channel({ id: channel.id });

    expect(requestAt(http, 0).url).toBe(
      "/api/v1/groups?limit=50&cursor=next+%2F%3F%26%3D%E2%98%83",
    );
    expect(requestAt(http, 1).url).toBe(
      "/api/v1/channels/channel%20%2F%3F%26%3D%E2%98%83",
    );
  });

  it("accepts only group names and cursors that the HTTP query boundary can round-trip", () => {
    const validBoundary = clientSchemas.groupsPage.safeParse({
      generation: 7,
      items: [{ name: "x".repeat(1024), channelCount: 1 }],
      next: "c".repeat(1024),
    });
    const controlGroup = clientSchemas.groupsPage.safeParse({
      generation: 7,
      items: [{ name: "invalid\u0007group", channelCount: 1 }],
      next: null,
    });
    const oversizedGroup = clientSchemas.groupsPage.safeParse({
      generation: 7,
      items: [{ name: "x".repeat(1025), channelCount: 1 }],
      next: null,
    });
    const oversizedCursor = clientSchemas.groupsPage.safeParse({
      generation: 7,
      items: [],
      next: "c".repeat(1025),
    });
    const oversizedUtf8Values = clientSchemas.groupsPage.safeParse({
      generation: 7,
      items: [{ name: "å".repeat(600), channelCount: 1 }],
      next: "å".repeat(600),
    });

    expect(validBoundary.success).toBe(true);
    expect(controlGroup.success).toBe(false);
    expect(oversizedGroup.success).toBe(false);
    expect(oversizedCursor.success).toBe(false);
    expect(oversizedUtf8Values.success).toBe(false);
  });
});

function createFakeHttp(fixtures: readonly JsonFixture[]): FakeHttp {
  const remaining = [...fixtures];
  const requests: RecordedRequest[] = [];
  const fetchImplementation: typeof fetch = async (input, init) => {
    const fixture = remaining.shift();
    if (fixture === undefined) {
      throw new Error("unexpected HTTP request in test");
    }

    requests.push({ url: requestUrl(input), init });
    return new Response(JSON.stringify(fixture.body), {
      status: fixture.status ?? 200,
      headers: { "content-type": "application/json" },
    });
  };

  return { fetch: fetchImplementation, requests };
}

function requestUrl(input: RequestInfo | URL): string {
  if (typeof input === "string") {
    return input;
  }
  if (input instanceof URL) {
    return input.toString();
  }
  return input.url;
}

function requestAt(http: FakeHttp, index: number): RecordedRequest {
  const request = http.requests[index];
  if (request === undefined) {
    throw new Error(`expected request ${index} to have been recorded`);
  }
  return request;
}

function invalidResponse(retryable: boolean) {
  return {
    ok: false,
    error: {
      _tag: "transport",
      retryable,
      message: "The Sparrow server returned an invalid response.",
    },
  };
}
