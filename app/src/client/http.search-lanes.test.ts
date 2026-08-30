// @vitest-environment node

import { describe, expect, it } from "vitest";
import { clientSchemas, type PageCursor } from "./contracts";
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

const channel = {
  id: "channel-one",
  name: "World News",
  group: "News",
};

const programme = {
  channelId: "channel-one",
  title: "Evening Report",
  description: "Headlines and analysis.",
  startsAt: "2026-08-30T19:00:00Z",
  endsAt: "2026-08-30T20:00:00Z",
};

describe("hosted HTTP search-lane client", () => {
  it("encodes and runtime-parses independent Channel and Programme pages", async () => {
    const channelCursor = parsedCursor("channel page /?&=☃");
    const programmeCursor = parsedCursor("programme page /?&=☃");
    const channelPage = {
      generation: 11,
      items: [channel],
      next: "next-channel-page",
    };
    const programmePage = {
      generation: 11,
      items: [programme],
      next: "next-programme-page",
    };
    const controller = new AbortController();
    const http = createFakeHttp([
      { body: channelPage },
      { body: programmePage },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await expect(
      client.searchChannels({
        term: "News & Å/World",
        limit: 1,
        cursor: channelCursor,
        signal: controller.signal,
      }),
    ).resolves.toEqual({ ok: true, value: channelPage });
    await expect(
      client.searchProgrammes({
        term: "Report & Å/World",
        limit: 1,
        cursor: programmeCursor,
        signal: controller.signal,
      }),
    ).resolves.toEqual({ ok: true, value: programmePage });

    expect(requestAt(http, 0).url).toBe(
      "/api/v1/search/channels?term=News+%26+%C3%85%2FWorld&limit=1&cursor=channel+page+%2F%3F%26%3D%E2%98%83",
    );
    expect(requestAt(http, 1).url).toBe(
      "/api/v1/search/programmes?term=Report+%26+%C3%85%2FWorld&limit=1&cursor=programme+page+%2F%3F%26%3D%E2%98%83",
    );
    for (const request of http.requests) {
      expect(request.init?.signal).toBe(controller.signal);
      expect(request.init?.credentials).toBe("same-origin");
      expect(new Headers(request.init?.headers).has("authorization")).toBe(
        false,
      );
    }
  });

  it("returns typed server failures from either independent search lane", async () => {
    const http = createFakeHttp([
      {
        status: 401,
        body: { error: { _tag: "authentication-required" } },
      },
      {
        status: 409,
        body: { error: { _tag: "stale-cursor", current: 12 } },
      },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await expect(
      client.searchChannels({ term: "news", limit: 12 }),
    ).resolves.toEqual({
      ok: false,
      error: { _tag: "authentication-required" },
    });
    await expect(
      client.searchProgrammes({ term: "news", limit: 10 }),
    ).resolves.toEqual({
      ok: false,
      error: { _tag: "stale-cursor", current: 12 },
    });
  });

  it("rejects oversized and partial continuing lane pages", async () => {
    const secondChannel = {
      ...channel,
      id: "channel-two",
      name: "Local News",
    };
    const laterProgramme = {
      ...programme,
      title: "Late Report",
      startsAt: "2026-08-30T20:00:00Z",
      endsAt: "2026-08-30T21:00:00Z",
    };
    const http = createFakeHttp([
      {
        body: {
          generation: 11,
          items: [channel, secondChannel],
          next: null,
        },
      },
      {
        body: {
          generation: 11,
          items: [channel],
          next: "more-channels",
        },
      },
      {
        body: {
          generation: 11,
          items: [programme, laterProgramme],
          next: null,
        },
      },
      {
        body: {
          generation: 11,
          items: [programme],
          next: "more-programmes",
        },
      },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await expect(
      client.searchChannels({ term: "news", limit: 1 }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.searchChannels({ term: "news", limit: 2 }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.searchProgrammes({ term: "report", limit: 1 }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.searchProgrammes({ term: "report", limit: 2 }),
    ).resolves.toEqual(invalidResponse(false));
  });

  it("rejects payloads for the wrong search lane and invalid requested limits", async () => {
    const http = createFakeHttp([
      { body: { generation: 11, items: [programme], next: null } },
      { body: { generation: 11, items: [channel], next: null } },
      { body: { generation: 11, items: [], next: null } },
      { body: { generation: 11, items: [], next: null } },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await expect(
      client.searchChannels({ term: "news", limit: 1 }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.searchProgrammes({ term: "news", limit: 1 }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.searchChannels({ term: "news", limit: 101 }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.searchProgrammes({ term: "news", limit: 0 }),
    ).resolves.toEqual(invalidResponse(false));
  });

  it("rejects a continuation response that echoes its submitted cursor", async () => {
    const channelCursor = parsedCursor("submitted-channel-cursor");
    const programmeCursor = parsedCursor("submitted-programme-cursor");
    const earlierChannelCursor = parsedCursor("earlier-channel-cursor");
    const earlierProgrammeCursor = parsedCursor("earlier-programme-cursor");
    const http = createFakeHttp([
      {
        body: {
          generation: 11,
          items: [channel],
          next: channelCursor,
        },
      },
      {
        body: {
          generation: 11,
          items: [programme],
          next: programmeCursor,
        },
      },
      {
        body: {
          generation: 11,
          items: [channel],
          next: earlierChannelCursor,
        },
      },
      {
        body: {
          generation: 11,
          items: [programme],
          next: earlierProgrammeCursor,
        },
      },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await expect(
      client.searchChannels({
        term: "news",
        limit: 1,
        cursor: channelCursor,
      }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.searchProgrammes({
        term: "report",
        limit: 1,
        cursor: programmeCursor,
      }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.searchChannels({
        term: "news",
        limit: 1,
        cursor: channelCursor,
        previousCursors: [earlierChannelCursor],
      }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.searchProgrammes({
        term: "report",
        limit: 1,
        cursor: programmeCursor,
        previousCursors: [earlierProgrammeCursor],
      }),
    ).resolves.toEqual(invalidResponse(false));
  });
});

function parsedCursor(value: string): PageCursor {
  const parsed = clientSchemas.groupsPage.safeParse({
    generation: 11,
    items: [],
    next: value,
  });
  if (!parsed.success || parsed.data.next === null) {
    throw new Error("expected the fixture page cursor to parse");
  }
  return parsed.data.next;
}

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
