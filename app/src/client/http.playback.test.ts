// @vitest-environment node

import { describe, expect, it } from "vitest";
import { clientSchemas, type ChannelId } from "./contracts";
import { createHttpSparrowClient } from "./http";

describe("hosted HTTP playback client", () => {
  it("derives one fixed same-origin route from only the opaque Channel Identifier", async () => {
    let fetchCalls = 0;
    const client = createHttpSparrowClient({
      fetch: async () => {
        fetchCalls += 1;
        throw new Error("playback descriptor resolution must not open a provider");
      },
    });

    const result = await client.startPlayback({
      id: parsedChannelId("channel /?&=☃"),
    });

    expect(result).toEqual({
      ok: true,
      value: {
        _tag: "same-origin-http",
        endpoint: "/api/v1/play/channel%20%2F%3F%26%3D%E2%98%83",
      },
    });
    expect(fetchCalls).toBe(0);
    expect(JSON.stringify(result)).not.toContain("http://");
    expect(JSON.stringify(result)).not.toContain("https://");
  });

  it("honors cancellation before returning the local descriptor", async () => {
    const controller = new AbortController();
    controller.abort();
    const client = createHttpSparrowClient();

    await expect(
      client.startPlayback({
        id: parsedChannelId("channel-one"),
        signal: controller.signal,
      }),
    ).resolves.toEqual({
      ok: false,
      error: { _tag: "cancelled" },
    });
  });

  it("fails safely when an opaque value cannot be encoded as a path segment", async () => {
    const client = createHttpSparrowClient();

    await expect(
      client.startPlayback({ id: parsedChannelId("unpaired-\ud800") }),
    ).resolves.toEqual({
      ok: false,
      error: {
        _tag: "transport",
        retryable: false,
        message: "The Sparrow server returned an invalid response.",
      },
    });
  });

  it("accepts only closed playback failures and Sparrow-owned relative routes", () => {
    for (const reason of [
      "rejected",
      "timed-out",
      "unavailable",
      "invalid-response",
    ] as const) {
      expect(
        clientSchemas.errorEnvelope.safeParse({
          error: {
            _tag: "playback-failed",
            reason,
            retryable: reason === "timed-out" || reason === "unavailable",
          },
        }).success,
      ).toBe(true);
    }

    expect(
      clientSchemas.errorEnvelope.safeParse({
        error: {
          _tag: "playback-failed",
          reason: "unavailable",
          retryable: true,
          sourceUrl: "https://provider.invalid/private?token=secret",
        },
      }).success,
    ).toBe(false);
    expect(
      clientSchemas.playbackDescriptor.safeParse({
        _tag: "same-origin-http",
        endpoint: "https://provider.invalid/private?token=secret",
      }).success,
    ).toBe(false);
  });
});

function parsedChannelId(value: string): ChannelId {
  return clientSchemas.channel.parse({
    id: value,
    name: "Fixture Channel",
    group: "Fixtures",
  }).id;
}
