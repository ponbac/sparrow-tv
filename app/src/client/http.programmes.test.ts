// @vitest-environment node

import { describe, expect, it } from "vitest";
import {
  clientSchemas,
  type ChannelId,
  type PageCursor,
} from "./contracts";
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

const channel = { id: "channel-one", name: "World News", group: "News" };

const channelOnlySearch = {
  generation: 11,
  channels: {
    generation: 11,
    items: [channel],
    next: null,
  },
  programmes: {
    generation: 11,
    items: [],
    next: null,
  },
};

const programme = {
  channelId: "channel-one",
  title: "Evening Report",
  description: "Headlines and analysis.",
  startsAt: "2026-08-30T19:00:00Z",
  endsAt: "2026-08-30T20:00:00Z",
};

const programmeSearchHit = {
  channel,
  title: programme.title,
  titleTruncated: false,
  startsAt: programme.startsAt,
  endsAt: programme.endsAt,
};

const schedulePage = {
  generation: 11,
  items: [programme],
  next: "schedule-next",
};

const enrichedSearch = {
  generation: 11,
  channels: {
    generation: 11,
    items: [channel],
    next: "channel-next",
  },
  programmes: {
    generation: 11,
    items: [programmeSearchHit],
    next: "programme-next",
  },
};

describe("hosted HTTP Programme client", () => {
  it("parses channel-only and guide-enriched schedule/search payloads", async () => {
    const emptySchedule = { generation: 11, items: [], next: null };
    const http = createFakeHttp([
      { body: emptySchedule },
      { body: channelOnlySearch },
      { body: schedulePage },
      { body: enrichedSearch },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });
    const id = parsedChannelId("channel-one");

    await expect(client.schedule({ id, limit: 24 })).resolves.toEqual({
      ok: true,
      value: emptySchedule,
    });
    await expect(
      client.search({
        term: "news",
        channelLimit: 5,
        programmeLimit: 7,
      }),
    ).resolves.toEqual({ ok: true, value: channelOnlySearch });
    await expect(client.schedule({ id, limit: 1 })).resolves.toEqual({
      ok: true,
      value: schedulePage,
    });
    await expect(
      client.search({
        term: "report",
        channelLimit: 1,
        programmeLimit: 1,
      }),
    ).resolves.toEqual({ ok: true, value: enrichedSearch });

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

  it("encodes identifiers, search terms, and independent continuation cursors", async () => {
    const scheduleCursor = parsedCursor("schedule /?&=☃");
    const channelCursor = parsedCursor("channels /?&=☃");
    const programmeCursor = parsedCursor("programmes /?&=☃");
    const controller = new AbortController();
    const http = createFakeHttp([
      {
        body: {
          ...schedulePage,
          items: [{ ...programme, channelId: "channel /?&=☃" }],
          next: null,
        },
      },
      {
        body: {
          ...enrichedSearch,
          channels: { ...enrichedSearch.channels, next: null },
          programmes: { ...enrichedSearch.programmes, next: null },
        },
      },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await client.schedule({
      id: parsedChannelId("channel /?&=☃"),
      limit: 24,
      cursor: scheduleCursor,
      signal: controller.signal,
    });
    await client.search({
      term: "News & Å/World",
      channelLimit: 5,
      channelCursor,
      programmeLimit: 7,
      programmeCursor,
      signal: controller.signal,
    });

    expect(requestAt(http, 0).url).toBe(
      "/api/v1/channels/channel%20%2F%3F%26%3D%E2%98%83/schedule?limit=24&cursor=schedule+%2F%3F%26%3D%E2%98%83",
    );
    expect(requestAt(http, 1).url).toBe(
      "/api/v1/search?term=News+%26+%C3%85%2FWorld&channelLimit=5&channelCursor=channels+%2F%3F%26%3D%E2%98%83&programmeLimit=7&programmeCursor=programmes+%2F%3F%26%3D%E2%98%83",
    );
    expect(requestAt(http, 0).init?.signal).toBe(controller.signal);
    expect(requestAt(http, 1).init?.signal).toBe(controller.signal);
  });

  it("requires strict, chronological Programme payloads with one shared generation", async () => {
    const privateValue =
      "https://provider-user:provider-secret@private.example/guide.xml";
    const malformedPayloads: readonly unknown[] = [
      {
        ...schedulePage,
        next: null,
        items: [{ ...programme, playbackSource: privateValue }],
      },
      {
        ...schedulePage,
        next: null,
        items: [
          {
            ...programme,
            startsAt: "2026-08-30T20:00:00Z",
            endsAt: "2026-08-30T19:00:00Z",
          },
        ],
      },
      {
        ...schedulePage,
        next: null,
        items: [{ ...programme, startsAt: "2026-08-30T19:00:00" }],
      },
      {
        ...enrichedSearch,
        programmes: { ...enrichedSearch.programmes, generation: 12 },
      },
      {
        ...enrichedSearch,
        programmes: {
          ...enrichedSearch.programmes,
          items: [
            {
              ...programmeSearchHit,
              description: privateValue,
            },
          ],
        },
      },
      {
        ...enrichedSearch,
        providerDiagnostic: privateValue,
      },
    ];
    const http = createFakeHttp(
      malformedPayloads.map((body) => ({ body })),
    );
    const client = createHttpSparrowClient({ fetch: http.fetch });
    const id = parsedChannelId("channel-one");

    for (let index = 0; index < malformedPayloads.length; index += 1) {
      const result =
        index < 3
          ? await client.schedule({ id, limit: 24 })
          : await client.search({
              term: "news",
              channelLimit: 5,
              programmeLimit: 7,
            });
      expect(result).toEqual(invalidResponse(false));
      expect(JSON.stringify(result)).not.toContain(privateValue);
    }
  });

  it("rejects pages that exceed or impossibly continue past their requested limits", async () => {
    const laterProgramme = {
      ...programme,
      title: "Late Report",
      startsAt: "2026-08-30T20:00:00Z",
      endsAt: "2026-08-30T21:00:00Z",
    };
    const excessChannelSearch = {
      ...channelOnlySearch,
      channels: {
        ...channelOnlySearch.channels,
        items: [
          ...channelOnlySearch.channels.items,
          { id: "channel-two", name: "Local News", group: "News" },
        ],
      },
    };
    const partialProgrammeSearch = {
      ...enrichedSearch,
      channels: { ...enrichedSearch.channels, next: null },
    };
    const http = createFakeHttp([
      {
        body: {
          ...schedulePage,
          items: [programme, laterProgramme],
          next: null,
        },
      },
      { body: schedulePage },
      { body: excessChannelSearch },
      { body: partialProgrammeSearch },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });
    const id = parsedChannelId("channel-one");

    await expect(client.schedule({ id, limit: 1 })).resolves.toEqual(
      invalidResponse(false),
    );
    await expect(client.schedule({ id, limit: 2 })).resolves.toEqual(
      invalidResponse(false),
    );
    await expect(
      client.search({
        term: "news",
        channelLimit: 1,
        programmeLimit: 2,
      }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.search({
        term: "report",
        channelLimit: 2,
        programmeLimit: 2,
      }),
    ).resolves.toEqual(invalidResponse(false));
  });

  it("rejects schedule items from another Channel or out of start order", async () => {
    const laterProgramme = {
      ...programme,
      title: "Late Report",
      startsAt: "2026-08-30T20:00:00Z",
      endsAt: "2026-08-30T21:00:00Z",
    };
    const http = createFakeHttp([
      {
        body: {
          ...schedulePage,
          items: [{ ...programme, channelId: "channel-two" }],
          next: null,
        },
      },
      {
        body: {
          ...schedulePage,
          items: [laterProgramme, programme],
          next: null,
        },
      },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });
    const id = parsedChannelId("channel-one");

    await expect(client.schedule({ id, limit: 2 })).resolves.toEqual(
      invalidResponse(false),
    );
    await expect(client.schedule({ id, limit: 2 })).resolves.toEqual(
      invalidResponse(false),
    );
  });

  it("rejects echoed schedule cursors and cross-page chronological regressions", async () => {
    const echoedCursor = parsedCursor("submitted-schedule-cursor");
    const nextCursor = parsedCursor("next-schedule-cursor");
    const previousCursor = parsedCursor("previous-schedule-cursor");
    const earlierProgramme = {
      ...programme,
      title: "Earlier Report",
      startsAt: "2026-08-30T18:00:00Z",
      endsAt: "2026-08-30T19:00:00Z",
    };
    const http = createFakeHttp([
      { body: { ...schedulePage, next: echoedCursor } },
      { body: { ...schedulePage, items: [earlierProgramme], next: null } },
      { body: { ...schedulePage, next: previousCursor } },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });
    const id = parsedChannelId("channel-one");

    await expect(
      client.schedule({ id, limit: 1, cursor: echoedCursor }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.schedule({
        id,
        limit: 1,
        cursor: nextCursor,
        afterStartsAt: parsedProgrammeStart(),
      }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.schedule({
        id,
        limit: 1,
        cursor: nextCursor,
        previousCursors: [previousCursor],
      }),
    ).resolves.toEqual(invalidResponse(false));

    expect(requestAt(http, 1).url).toBe(
      "/api/v1/channels/channel-one/schedule?limit=1&cursor=next-schedule-cursor",
    );
    expect(requestAt(http, 2).url).toBe(
      "/api/v1/channels/channel-one/schedule?limit=1&cursor=next-schedule-cursor",
    );
  });

  it("rejects a combined-search cursor cycle in either result lane", async () => {
    const submittedChannel = parsedCursor("submitted-channel");
    const submittedProgramme = parsedCursor("submitted-programme");
    const earlierChannel = parsedCursor("earlier-channel");
    const earlierProgramme = parsedCursor("earlier-programme");
    const http = createFakeHttp([
      {
        body: {
          ...enrichedSearch,
          channels: { ...enrichedSearch.channels, next: earlierChannel },
          programmes: { ...enrichedSearch.programmes, next: null },
        },
      },
      {
        body: {
          ...enrichedSearch,
          channels: { ...enrichedSearch.channels, next: null },
          programmes: { ...enrichedSearch.programmes, next: earlierProgramme },
        },
      },
    ]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await expect(
      client.search({
        term: "news",
        channelLimit: 1,
        channelCursor: submittedChannel,
        channelPreviousCursors: [earlierChannel],
        programmeLimit: 1,
      }),
    ).resolves.toEqual(invalidResponse(false));
    await expect(
      client.search({
        term: "news",
        channelLimit: 1,
        programmeLimit: 1,
        programmeCursor: submittedProgramme,
        programmePreviousCursors: [earlierProgramme],
      }),
    ).resolves.toEqual(invalidResponse(false));

    expect(requestAt(http, 0).url).toContain("channelCursor=submitted-channel");
    expect(requestAt(http, 0).url).not.toContain("earlier-channel");
    expect(requestAt(http, 1).url).toContain(
      "programmeCursor=submitted-programme",
    );
    expect(requestAt(http, 1).url).not.toContain("earlier-programme");
  });

  it("bounds every server page to the core page-limit maximum", async () => {
    const oversizedSchedule = {
      generation: 11,
      items: Array.from({ length: 101 }, () => programme),
      next: null,
    };
    const http = createFakeHttp([{ body: oversizedSchedule }]);
    const client = createHttpSparrowClient({ fetch: http.fetch });

    await expect(
      client.schedule({ id: parsedChannelId("channel-one"), limit: 100 }),
    ).resolves.toEqual(invalidResponse(false));
  });

  it("returns typed server errors from schedule and search", async () => {
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
      client.schedule({ id: parsedChannelId("channel-one"), limit: 24 }),
    ).resolves.toEqual({
      ok: false,
      error: { _tag: "authentication-required" },
    });
    await expect(
      client.search({
        term: "news",
        channelLimit: 5,
        programmeLimit: 7,
      }),
    ).resolves.toEqual({
      ok: false,
      error: { _tag: "stale-cursor", current: 12 },
    });
  });

  it("classifies schedule and search cancellation before transport failure", async () => {
    const privateFailure =
      "GET https://private.example/guide.xml?token=secret failed";
    const controller = new AbortController();
    controller.abort();
    const fetchImplementation: typeof fetch = async () => {
      throw new DOMException(privateFailure, "AbortError");
    };
    const client = createHttpSparrowClient({ fetch: fetchImplementation });

    const schedule = await client.schedule({
      id: parsedChannelId("channel-one"),
      limit: 24,
      signal: controller.signal,
    });
    const search = await client.search({
      term: "news",
      channelLimit: 5,
      programmeLimit: 7,
    });

    expect(schedule).toEqual({
      ok: false,
      error: { _tag: "cancelled" },
    });
    expect(search).toEqual({
      ok: false,
      error: { _tag: "cancelled" },
    });
    expect(JSON.stringify([schedule, search])).not.toContain(privateFailure);
  });
});

function parsedChannelId(value: string): ChannelId {
  const parsed = clientSchemas.channel.safeParse({
    id: value,
    name: "Fixture Channel",
    group: "",
  });
  if (!parsed.success) {
    throw new Error("expected the fixture Channel Identifier to parse");
  }
  return parsed.data.id;
}

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

function parsedProgrammeStart() {
  const parsed = clientSchemas.schedulePage.safeParse({
    generation: 11,
    items: [programme],
    next: null,
  });
  const firstProgramme = parsed.success ? parsed.data.items[0] : undefined;
  if (firstProgramme === undefined) {
    throw new Error("expected the fixture Programme start to parse");
  }
  return firstProgramme.startsAt;
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
