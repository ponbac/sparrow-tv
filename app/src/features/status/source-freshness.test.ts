import { describe, expect, it } from "vitest";
import { clientSchemas } from "../../client/contracts";
import { sourceFreshness } from "./source-freshness";

const NOW = new Date("2026-08-30T10:04:30Z");

const TIMED_OUT = {
  _tag: "source-access",
  source: "m3u",
  reason: "timed-out",
  retryAfterSeconds: null,
};

function status(m3u: unknown, epg: unknown = null, configured = true) {
  return clientSchemas.status.parse({
    generation: null,
    configuration: { configured, epgConfigured: epg !== null },
    m3u,
    epg,
  });
}

describe("source freshness", () => {
  it.each([
    {
      name: "minutes since a fresh check",
      m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
      state: "fresh",
      sentence: "Channels updated 4 min ago",
    },
    {
      name: "a check within the last minute",
      m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:04:00Z" },
      state: "fresh",
      sentence: "Channels updated just now",
    },
    {
      name: "a check stamped ahead of this clock",
      m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:09:00Z" },
      state: "fresh",
      sentence: "Channels updated just now",
    },
    {
      name: "hours since a stale check",
      m3u: {
        _tag: "stale",
        validatedAt: "2026-08-30T07:00:00Z",
        nextAttemptAt: null,
      },
      state: "stale",
      sentence: "Channels updated 3 h ago",
    },
    {
      name: "days once two have passed",
      m3u: {
        _tag: "stale",
        validatedAt: "2026-08-27T09:00:00Z",
        nextAttemptAt: null,
      },
      state: "stale",
      sentence: "Channels updated 3 days ago",
    },
    {
      name: "a refresh in progress",
      m3u: {
        _tag: "refreshing",
        validatedAt: "2026-08-30T10:00:00Z",
        startedAt: "2026-08-30T10:04:00Z",
      },
      state: "refreshing",
      sentence: "Channels updating…",
    },
    {
      name: "a deferred first load",
      m3u: {
        _tag: "deferred",
        validatedAt: null,
        deferredAt: "2026-08-30T10:04:00Z",
      },
      state: "deferred",
      sentence: "Channels waiting to update",
    },
    {
      name: "a deferred refresh over a saved copy",
      m3u: {
        _tag: "deferred",
        validatedAt: "2026-08-30T09:04:00Z",
        deferredAt: "2026-08-30T10:04:00Z",
      },
      state: "deferred",
      sentence: "Channels updated 1 h ago",
    },
    {
      name: "a failed refresh",
      m3u: {
        _tag: "failed",
        validatedAt: "2026-08-30T10:00:00Z",
        failure: TIMED_OUT,
        nextAttemptAt: "2026-08-30T10:10:00Z",
      },
      state: "failed",
      sentence: "Channels update failed",
    },
    {
      name: "a configured source with nothing loaded",
      m3u: { _tag: "unavailable", failure: TIMED_OUT },
      state: "unavailable",
      sentence: "Channels unavailable",
    },
  ])("describes $name", ({ m3u, state, sentence }) => {
    expect(sourceFreshness("m3u", status(m3u), NOW)).toEqual({
      state,
      sentence,
    });
  });

  it("separates a source that is not set up from one that is unavailable", () => {
    const unconfigured = status(
      { _tag: "unavailable", failure: null },
      null,
      false,
    );

    expect(sourceFreshness("m3u", unconfigured, NOW)).toEqual({
      state: "unavailable",
      sentence: "Channels not set up",
    });
    expect(sourceFreshness("epg", unconfigured, NOW)).toEqual({
      state: "unavailable",
      sentence: "Guide not set up",
    });
    expect(sourceFreshness("epg", null, NOW)).toEqual({
      state: "unavailable",
      sentence: "Guide unavailable",
    });
  });

  it("describes the guide source by its own state", () => {
    const both = status(
      { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
      { _tag: "fresh", validatedAt: "2026-08-30T10:04:10Z" },
    );

    expect(sourceFreshness("epg", both, NOW)).toEqual({
      state: "fresh",
      sentence: "Guide updated just now",
    });
  });
});
