// @vitest-environment node

import { describe, expect, it } from "vitest";
import { clientSchemas, type PageCursor } from "./contracts";
import { createHttpSparrowClient } from "./http";

const startsAt = clientSchemas.isoInstant.parse("2026-08-30T19:00:00Z");
const endsAt = clientSchemas.isoInstant.parse("2026-08-30T22:00:00Z");
const programme = {
  title: "Evening Report",
  titleTruncated: false,
  startsAt: "2026-08-30T18:30:00Z",
  endsAt: "2026-08-30T19:30:00Z",
};
const guideWindow = {
  generation: 11,
  items: [
    {
      channel: { id: "channel-one", name: "World News", group: "News" },
      programmes: [programme],
      programmesTruncated: false,
    },
  ],
  next: null,
};

describe("hosted HTTP guide-window client", () => {
  it("exposes the instant parser and reads one encoded deep guide query", async () => {
    expect(clientSchemas.isoInstant.safeParse("2026-08-30T19:00:00").success).toBe(
      false,
    );
    expect(clientSchemas.isoInstant.safeParse("x".repeat(65)).success).toBe(false);
    const cursor = parsedCursor("guide /?&=☃");
    const encodedGuideWindow = {
      ...guideWindow,
      items: [
        {
          ...guideWindow.items[0],
          channel: {
            ...guideWindow.items[0].channel,
            group: "News & Current",
          },
        },
      ],
    };
    const requests: string[] = [];
    const client = createHttpSparrowClient({
      fetch: queuedFetch([encodedGuideWindow], requests),
    });
    const parsedGuideWindow = clientSchemas.guideWindow.parse(encodedGuideWindow);

    await expect(
      client.guideWindow({
        startsAt,
        endsAt,
        group: "News & Current",
        channelLimit: 1,
        cursor,
      }),
    ).resolves.toEqual({ ok: true, value: parsedGuideWindow });

    expect(requests).toEqual([
      "/api/v1/guide?startsAt=2026-08-30T19%3A00%3A00Z&endsAt=2026-08-30T22%3A00%3A00Z&channelLimit=1&group=News+%26+Current&cursor=guide+%2F%3F%26%3D%E2%98%83",
    ]);
  });

  it("rejects rows that violate the requested group, interval, projection, or cap facts", async () => {
    const privateValue =
      "https://provider-user:provider-secret@private.invalid/guide.xml";
    const malformed = [
      {
        ...guideWindow,
        items: [
          {
            ...guideWindow.items[0],
            channel: { ...guideWindow.items[0].channel, group: "Cinema" },
          },
        ],
      },
      {
        ...guideWindow,
        items: [
          {
            ...guideWindow.items[0],
            programmes: [
              {
                ...programme,
                startsAt: "2026-08-30T22:00:00Z",
                endsAt: "2026-08-30T23:00:00Z",
              },
            ],
          },
        ],
      },
      {
        ...guideWindow,
        items: [
          {
            ...guideWindow.items[0],
            programmes: [
              {
                ...programme,
                description: "The guide projection must omit descriptions.",
              },
            ],
          },
        ],
      },
      {
        ...guideWindow,
        items: [
          {
            ...guideWindow.items[0],
            programmesTruncated: true,
          },
        ],
      },
      {
        ...guideWindow,
        items: [guideWindow.items[0], guideWindow.items[0]],
      },
      {
        ...guideWindow,
        items: [
          {
            ...guideWindow.items[0],
            programmes: [{ ...programme, titleTruncated: true }],
          },
        ],
      },
      {
        ...guideWindow,
        items: [
          {
            ...guideWindow.items[0],
            programmes: [
              { ...programme, title: "é".repeat(129) },
            ],
          },
        ],
      },
      { ...guideWindow, providerUrl: privateValue },
    ];
    const client = createHttpSparrowClient({ fetch: queuedFetch(malformed) });

    for (let index = 0; index < malformed.length; index += 1) {
      const result = await client.guideWindow({
        startsAt,
        endsAt,
        group: "News",
        channelLimit: 1,
      });
      expect(result).toEqual(invalidResponse());
      expect(JSON.stringify(result)).not.toContain(privateValue);
    }
  });

  it("rejects guide continuation cycles and accepts typed guide input errors", async () => {
    const cursor = parsedCursor("current-guide");
    const earlier = parsedCursor("earlier-guide");
    const client = createHttpSparrowClient({
      fetch: queuedFetch(
        [
          { ...guideWindow, next: earlier },
          {
            error: {
              _tag: "invalid-input",
              field: "guide-ends-at",
              reason: "out-of-range",
            },
          },
        ],
        undefined,
        [200, 400],
      ),
    });

    await expect(
      client.guideWindow({
        startsAt,
        endsAt,
        channelLimit: 1,
        cursor,
        previousCursors: [earlier],
      }),
    ).resolves.toEqual(invalidResponse());
    await expect(
      client.guideWindow({ startsAt, endsAt, channelLimit: 1 }),
    ).resolves.toEqual({
      ok: false,
      error: {
        _tag: "invalid-input",
        field: "guide-ends-at",
        reason: "out-of-range",
      },
    });
  });

  it("matches Rust precision for nanosecond and longer RFC 3339 fractions", async () => {
    const windows = [
      {
        startsAt: clientSchemas.isoInstant.parse(
          "2026-08-30T19:00:00.000000001Z",
        ),
        endsAt: clientSchemas.isoInstant.parse(
          "2026-08-30T19:00:00.000000002Z",
        ),
      },
      {
        startsAt: clientSchemas.isoInstant.parse(
          "2026-08-30T19:00:00.0000000001Z",
        ),
        endsAt: clientSchemas.isoInstant.parse(
          "2026-08-30T19:00:00.0000000019Z",
        ),
      },
    ] as const;
    const emptyWindow = { generation: 11, items: [], next: null };
    const client = createHttpSparrowClient({
      fetch: queuedFetch([emptyWindow, emptyWindow]),
    });

    for (const window of windows) {
      await expect(
        client.guideWindow({ ...window, channelLimit: 1 }),
      ).resolves.toEqual({ ok: true, value: emptyWindow });
    }
  });
});

function parsedCursor(value: string): PageCursor {
  const next = clientSchemas.channelsPage.parse({
    generation: 11,
    items: [],
    next: value,
  }).next;
  if (next === null) {
    throw new Error("expected the guide cursor fixture to parse");
  }
  return next;
}

function queuedFetch(
  payloads: readonly unknown[],
  requests: string[] = [],
  statuses: readonly number[] = payloads.map(() => 200),
): typeof fetch {
  const remainingPayloads = [...payloads];
  const remainingStatuses = [...statuses];
  return async (input) => {
    const payload = remainingPayloads.shift();
    const status = remainingStatuses.shift();
    if (payload === undefined || status === undefined) {
      throw new Error("unexpected guide HTTP request");
    }
    requests.push(typeof input === "string" ? input : input.toString());
    return new Response(JSON.stringify(payload), {
      status,
      headers: { "content-type": "application/json" },
    });
  };
}

function invalidResponse() {
  return {
    ok: false,
    error: {
      _tag: "transport",
      retryable: false,
      message: "The Sparrow server returned an invalid response.",
    },
  };
}
