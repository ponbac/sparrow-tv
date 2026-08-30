// @vitest-environment node

import { afterEach, describe, expect, it, vi } from "vitest";
import { clientSchemas } from "./contracts";
import {
  createHttpSparrowClient,
  type HttpEventSource,
} from "./http";

afterEach(() => vi.restoreAllMocks());

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
  it("posts an empty authenticated refresh marker and strictly parses its report", async () => {
    const report = {
      trigger: "manual",
      m3u: {
        _tag: "updated",
        validatedAt: "2026-08-30T00:02:00Z",
      },
      epg: {
        _tag: "failed",
        failure: {
          _tag: "invalid-epg-format",
          source: "epg",
          reason: "malformed-xml",
        },
        nextAttemptAt: "2026-08-30T00:03:00Z",
      },
      status: {
        ...freshStatus,
        generation: 8,
        epg: {
          _tag: "failed",
          validatedAt: "2026-08-30T00:00:01Z",
          failure: {
            _tag: "invalid-epg-format",
            source: "epg",
            reason: "malformed-xml",
          },
          nextAttemptAt: "2026-08-30T00:03:00Z",
        },
      },
    };
    const http = createFakeHttp([{ body: report }]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await expect(client.refresh()).resolves.toEqual({ ok: true, value: report });

    const request = requestAt(http, 0);
    const headers = new Headers(request.init?.headers);
    expect(request.url).toBe("/api/v1/refresh");
    expect(request.init?.method).toBe("POST");
    expect(request.init?.credentials).toBe("same-origin");
    expect(request.init?.body).toBeUndefined();
    expect(headers.get("X-Sparrow-Request")).toBe("refresh");
    expect(headers.has("authorization")).toBe(false);
  });

  it("accepts only closed same-origin events and releases the stream idempotently", () => {
    const source = new FakeEventSource();
    const endpoints: string[] = [];
    const client = createHttpSparrowClient({
      fetch: createFakeHttp([]).fetch,
      eventSource: (endpoint) => {
        endpoints.push(endpoint);
        return source;
      },
    });
    const consoleSpies = [
      vi.spyOn(console, "log").mockImplementation(() => undefined),
      vi.spyOn(console, "warn").mockImplementation(() => undefined),
      vi.spyOn(console, "error").mockImplementation(() => undefined),
    ];
    const events: unknown[] = [];
    const release = client.subscribe((event) => events.push(event));

    source.emit("not-json provider-secret.invalid");
    source.emit(
      JSON.stringify({
        _tag: "catalog-published",
        occurredAt: "2026-08-30T00:02:00Z",
        generation: 8,
        rawUrl: "https://user:secret@provider.invalid/list.m3u",
      }),
    );
    source.emit(
      JSON.stringify({
        _tag: "catalog-status-changed",
        occurredAt: "2026-08-30T00:02:01Z",
        status: freshStatus,
      }),
    );

    expect(endpoints).toEqual(["/api/v1/events"]);
    expect(events).toEqual([
      {
        _tag: "catalog-status-changed",
        occurredAt: "2026-08-30T00:02:01Z",
        status: freshStatus,
      },
    ]);
    expect(JSON.stringify(events)).not.toContain("provider-secret");
    expect(consoleSpies.every((spy) => spy.mock.calls.length === 0)).toBe(true);

    release();
    release();
    source.emit(
      JSON.stringify({
        _tag: "catalog-published",
        occurredAt: "2026-08-30T00:02:02Z",
        generation: 8,
      }),
    );
    expect(source.closeCalls).toBe(1);
    expect(source.listenerCount).toBe(0);
    expect(events).toHaveLength(1);
  });

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
        failure: {
          _tag: "source-access",
          source: "m3u",
          reason: "unavailable",
          retryAfterSeconds: null,
        },
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

  it("accepts the closed safe failure corpus and rejects unsafe context", () => {
    const failures: readonly {
      readonly source: "m3u" | "epg";
      readonly failure: unknown;
    }[] = [
      {
        source: "m3u",
        failure: {
          _tag: "source-access",
          source: "m3u",
          reason: "timed-out",
          retryAfterSeconds: 30,
        },
      },
      {
        source: "epg",
        failure: { _tag: "source-read", source: "epg", reason: "invalid-body" },
      },
      {
        source: "m3u",
        failure: {
          _tag: "snapshot",
          source: "m3u",
          operation: "prepare-activation",
          reason: "capacity",
        },
      },
      {
        source: "epg",
        failure: {
          _tag: "snapshot-recovery",
          source: "epg",
          reason: "checksum-mismatch",
        },
      },
      {
        source: "epg",
        failure: {
          _tag: "decoded-limit-exceeded",
          source: "epg",
          limitBytes: 1024,
        },
      },
      {
        source: "m3u",
        failure: { _tag: "invalid-encoding", source: "m3u" },
      },
      {
        source: "m3u",
        failure: {
          _tag: "invalid-format",
          source: "m3u",
          entry: 4,
          reason: "unsupported-playback-source",
        },
      },
      {
        source: "m3u",
        failure: { _tag: "no-playable-channels", source: "m3u" },
      },
      {
        source: "epg",
        failure: {
          _tag: "invalid-epg-format",
          source: "epg",
          reason: "malformed-xml",
        },
      },
      {
        source: "epg",
        failure: { _tag: "no-epg-channels", source: "epg" },
      },
    ];

    for (const { source, failure } of failures) {
      const failedState = {
        _tag: "failed",
        validatedAt: null,
        failure,
        nextAttemptAt: "2026-08-30T00:03:00Z",
      };
      expect(
        clientSchemas.status.safeParse({
          ...freshStatus,
          ...(source === "m3u" ? { m3u: failedState } : { epg: failedState }),
        }).success,
      ).toBe(true);
    }

    expect(
      clientSchemas.status.safeParse({
        ...freshStatus,
        m3u: {
          _tag: "failed",
          validatedAt: null,
          failure: {
            _tag: "source-access",
            source: "m3u",
            reason: "timed-out",
            retryAfterSeconds: Number.MAX_SAFE_INTEGER + 1,
            url: "https://user:secret@provider.invalid/list.m3u",
          },
          nextAttemptAt: "2026-08-30T00:03:00Z",
        },
      }).success,
    ).toBe(false);
  });

  it("rejects failures attributed to a different status, report, or event source", () => {
    const m3uFailure = {
      _tag: "source-access",
      source: "m3u",
      reason: "timed-out",
      retryAfterSeconds: 30,
    };
    const epgFailure = {
      _tag: "source-read",
      source: "epg",
      reason: "invalid-body",
    };
    const failedOutcome = (failure: unknown) => ({
      _tag: "failed",
      failure,
      nextAttemptAt: "2026-08-30T00:03:00Z",
    });
    const failedState = (failure: unknown) => ({
      ...failedOutcome(failure),
      validatedAt: null,
    });

    expect(
      clientSchemas.status.safeParse({
        ...freshStatus,
        m3u: failedState(epgFailure),
      }).success,
    ).toBe(false);
    expect(
      clientSchemas.status.safeParse({
        ...freshStatus,
        epg: failedState(m3uFailure),
      }).success,
    ).toBe(false);
    expect(
      clientSchemas.refreshReport.safeParse({
        trigger: "manual",
        m3u: failedOutcome(epgFailure),
        epg: failedOutcome(m3uFailure),
        status: freshStatus,
      }).success,
    ).toBe(false);
    expect(
      clientSchemas.sparrowEvent.safeParse({
        _tag: "refresh-completed",
        occurredAt: "2026-08-30T00:02:00Z",
        source: "m3u",
        outcome: failedOutcome(epgFailure),
      }).success,
    ).toBe(false);
    expect(
      clientSchemas.sparrowEvent.safeParse({
        _tag: "refresh-completed",
        occurredAt: "2026-08-30T00:02:00Z",
        source: "epg",
        outcome: failedOutcome(m3uFailure),
      }).success,
    ).toBe(false);
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
      { _tag: "invalid-input", field: "body", reason: "too-long" },
      { _tag: "invalid-input", field: "header", reason: "invalid-format" },
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

class FakeEventSource implements HttpEventSource {
  readonly #listeners = new Set<EventListener>();
  closeCalls = 0;

  get listenerCount(): number {
    return this.#listeners.size;
  }

  addEventListener(_type: "message", listener: EventListener): void {
    this.#listeners.add(listener);
  }

  removeEventListener(_type: "message", listener: EventListener): void {
    this.#listeners.delete(listener);
  }

  close(): void {
    this.closeCalls += 1;
  }

  emit(data: string): void {
    const event = new MessageEvent("message", { data });
    for (const listener of this.#listeners) {
      listener(event);
    }
  }
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
