import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ClientResult, PlaybackDescriptor, SparrowClient } from "../../client/contracts";
import { clientSchemas } from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import { createHttpSparrowClient } from "../../client/http";
import { startHostedPlaybackSession } from "./hosted-playback-session";
import type { HostedPlaybackEngine, HostedPlaybackFailure, HostedPlaybackRequest } from "./mpegts-engine";
import type { PlayerState } from "./playback-presentation";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("hosted Playback Session recovery", () => {
  it.each(["source-unavailable", "source-timeout", "stream-interrupted"] as const)(
    "bounds %s recovery at 1s/5s/15s without concurrent requests", async (failure) => {
      const fixture = sessionFixture();
      await flush();
      for (const delay of [1_000, 5_000, 15_000]) {
        const old = fixture.request();
        old.onFailure(failure);
        // Duplicate outcomes cannot schedule a second retry or alter its deadline.
        old.onFailure(failure);
        old.onAutoplayBlocked();
        expect(fixture.liveHandles()).toBe(0);
        const starts = fixture.starts();
        await vi.advanceTimersByTimeAsync(delay - 1);
        expect(fixture.starts()).toBe(starts);
        await vi.advanceTimersByTimeAsync(1);
        expect(fixture.starts()).toBe(starts + 1);
        expect(fixture.liveHandles()).toBe(1);
      }
      fixture.request().onFailure(failure);
      await vi.advanceTimersByTimeAsync(120_000);
      expect(fixture.starts()).toBe(4);
      expect(fixture.lastState()).toEqual({ _tag: "failed", failure, retryable: true });
      expect(fixture.maxLiveHandles()).toBe(1);
      fixture.session.stop();
      expect(vi.getTimerCount()).toBe(0);
    },
  );

  it("registers and releases a handle even when start fails synchronously", async () => {
    const fixture = sessionFixture({ synchronousFailure: "source-timeout" });
    await flush();
    expect(fixture.liveHandles()).toBe(0);
    await vi.advanceTimersByTimeAsync(21_000);
    expect(fixture.starts()).toBe(4);
    expect(fixture.maxLiveHandles()).toBe(1);
    expect(fixture.liveHandles()).toBe(0);
    expect(fixture.lastState()).toEqual({ _tag: "failed", failure: "source-timeout", retryable: true });
    fixture.session.stop();
  });

  it("restores the retry budget only after sixty seconds of uninterrupted playback", async () => {
    const fixture = sessionFixture();
    await flush();
    fixture.request().onFailure("stream-interrupted");
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.advanceTimersByTimeAsync(59_000);
    fixture.request().onFailure("stream-interrupted");
    expect(fixture.lastState()).toMatchObject({ _tag: "recovering", attempt: 2 });
    await vi.advanceTimersByTimeAsync(5_000);
    fixture.video.dispatchEvent(new Event("waiting"));
    await vi.advanceTimersByTimeAsync(60_000);
    fixture.request().onFailure("stream-interrupted");
    expect(fixture.lastState()).toMatchObject({ _tag: "recovering", attempt: 3 });
    await vi.advanceTimersByTimeAsync(15_000);
    await vi.advanceTimersByTimeAsync(60_000);
    fixture.request().onFailure("stream-interrupted");
    expect(fixture.lastState()).toMatchObject({ _tag: "recovering", attempt: 1 });
    fixture.session.stop();
  });

  it.each(["pause", "autoplay-blocked"] as const)("does not reconnect after %s", async (intent) => {
    const fixture = sessionFixture();
    await flush();
    if (intent === "pause") fixture.request().onPause?.();
    else fixture.request().onAutoplayBlocked();
    fixture.request().onFailure("stream-interrupted");
    await vi.advanceTimersByTimeAsync(120_000);
    expect(fixture.starts()).toBe(1);
    expect(fixture.lastState()).toEqual({ _tag: "failed", failure: "stream-interrupted", retryable: true });
    fixture.session.stop();
  });

  it("allows recovery again after the viewer resumes playback", async () => {
    const fixture = sessionFixture();
    await flush();
    fixture.request().onAutoplayBlocked();
    // Resume intent permits recovery even if the request fails before a frame.
    fixture.video.dispatchEvent(new Event("play"));
    fixture.request().onFailure("stream-interrupted");
    await vi.advanceTimersByTimeAsync(1_000);
    expect(fixture.starts()).toBe(2);
    fixture.session.stop();
  });

  it("permits recovery from gesture intent before a play event or promise settlement", async () => {
    const fixture = sessionFixture();
    await flush();
    let reject: ((cause: unknown) => void) | undefined;
    const pending = new Promise<void>((_resolve, no) => { reject = no; });
    let plays = 0;
    fixture.video.play = () => { plays += 1; return pending; };
    fixture.request().onAutoplayBlocked();
    fixture.session.beginBlockedPlayback();
    expect(plays).toBe(1);
    fixture.request().onFailure("stream-interrupted");
    expect(fixture.lastState()).toMatchObject({ _tag: "recovering", attempt: 1 });
    // No transport exists during backoff, so even a stale button cannot play it.
    fixture.session.beginBlockedPlayback();
    expect(plays).toBe(1);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(fixture.starts()).toBe(2);
    // A genuine gesture failure on the new transport must still release it.
    fixture.video.play = () => {
      plays += 1;
      return Promise.reject(new DOMException("unsupported playback", "NotSupportedError"));
    };
    fixture.session.beginBlockedPlayback();
    await flush();
    expect(fixture.lastState()).toEqual({
      _tag: "failed", failure: "media-unsupported", retryable: false,
    });
    expect(fixture.liveHandles()).toBe(0);
    fixture.session.stop();
    const stoppedState = fixture.lastState();
    reject?.(new DOMException("cancelled playback", "AbortError"));
    await flush();
    fixture.session.beginBlockedPlayback();
    expect(plays).toBe(2);
    expect(fixture.lastState()).toBe(stoppedState);
    expect(fixture.liveHandles()).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  it.each([
    "authentication-required", "channel-not-found", "source-rejected", "source-invalid",
    "media-unsupported", "browser-unsupported",
  ] as const)("leaves %s to the viewer rather than automatically reconnecting", async (failure) => {
    const fixture = sessionFixture();
    await flush();
    fixture.request().onFailure(failure);
    await vi.advanceTimersByTimeAsync(120_000);
    expect(fixture.starts()).toBe(1);
    expect(fixture.liveHandles()).toBe(0);
    expect(fixture.lastState()).toMatchObject({ _tag: "failed", failure });
    fixture.session.stop();
  });

  it("honors the client's non-retryable transport decision", async () => {
    const client: Pick<SparrowClient, "startPlayback"> = {
      startPlayback: async () => ({ ok: false, error: {
        _tag: "transport", retryable: false, message: "fixture boundary failure",
      } }),
    };
    const fixture = sessionFixture({ client });
    await flush();
    await vi.advanceTimersByTimeAsync(120_000);
    expect(fixture.starts()).toBe(0);
    expect(fixture.lastState()).toEqual({ _tag: "failed", failure: "source-unavailable", retryable: false });
    fixture.session.stop();
  });

  it("retries a retryable descriptor failure before constructing an engine", async () => {
    let resolutions = 0;
    const client: Pick<SparrowClient, "startPlayback"> = {
      startPlayback: async () => {
        resolutions += 1;
        return resolutions === 1
          ? { ok: false, error: { _tag: "service-unavailable" } }
          : { ok: true, value: descriptor() };
      },
    };
    const fixture = sessionFixture({ client });
    await flush();
    expect(fixture.starts()).toBe(0);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(resolutions).toBe(2);
    expect(fixture.starts()).toBe(1);
    fixture.session.stop();
  });

  it("cancels pending backoff and ignores stale engine and video events", async () => {
    const fixture = sessionFixture();
    await flush();
    const old = fixture.request();
    old.onFailure("stream-interrupted");
    fixture.session.stop();
    fixture.session.stop();
    const lastState = fixture.lastState();
    old.onFailure("source-timeout");
    old.onAutoplayBlocked();
    fixture.video.dispatchEvent(new Event("playing"));
    await vi.advanceTimersByTimeAsync(120_000);
    expect(fixture.lastState()).toBe(lastState);
    expect(fixture.starts()).toBe(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it.each(["resolve", "reject"] as const)("aborts pending resolution and ignores its late %s", async (outcome) => {
    let signal: AbortSignal | undefined;
    let resolve: ((result: ClientResult<PlaybackDescriptor>) => void) | undefined;
    let reject: ((cause: unknown) => void) | undefined;
    const pending = new Promise<ClientResult<PlaybackDescriptor>>((yes, no) => { resolve = yes; reject = no; });
    const fixture = sessionFixture({ client: {
      startPlayback: (input) => { signal = input.signal; return pending; },
    } });
    fixture.session.stop();
    expect(signal?.aborted).toBe(true);
    if (outcome === "resolve") resolve?.({ ok: true, value: descriptor() });
    else reject?.(new Error("private boundary diagnostic"));
    await flush();
    expect(fixture.starts()).toBe(0);
    expect(fixture.states).toEqual([{ _tag: "starting" }]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("handles rejected boundary work without exposing its diagnostic or retrying a defect", async () => {
    const fixture = sessionFixture({ client: { startPlayback: async () => {
      throw new Error("private boundary diagnostic");
    } } });
    await flush();
    expect(fixture.lastState()).toEqual({ _tag: "failed", failure: "source-unavailable", retryable: false });
    expect(vi.getTimerCount()).toBe(0);
    fixture.session.stop();
  });
});

function sessionFixture(options: {
  readonly synchronousFailure?: HostedPlaybackFailure;
  readonly client?: Pick<SparrowClient, "startPlayback">;
} = {}) {
  const video = document.createElement("video");
  const states: PlayerState[] = [];
  const requests: HostedPlaybackRequest[] = [];
  let liveHandles = 0;
  let maxLiveHandles = 0;
  const engine: HostedPlaybackEngine = {
    start: (request) => {
      requests.push(request);
      liveHandles += 1;
      maxLiveHandles = Math.max(maxLiveHandles, liveHandles);
      request.video.dispatchEvent(new Event("playing"));
      if (options.synchronousFailure !== undefined) request.onFailure(options.synchronousFailure);
      let stopped = false;
      return { stop: () => {
        if (stopped) return;
        stopped = true;
        liveHandles -= 1;
        request.video.dispatchEvent(new Event("pause"));
      } };
    },
  };
  const session = startHostedPlaybackSession({
    id: channelFixture({ id: "channel-one", name: "World News", group: "Fixtures" }).id,
    client: options.client ?? createHttpSparrowClient(), engine, video,
    onState: (state) => states.push(state),
  });
  return {
    session, states, video,
    request: () => {
      const request = requests.at(-1);
      if (request === undefined) throw new Error("expected a live request");
      return request;
    },
    starts: () => requests.length,
    liveHandles: () => liveHandles,
    maxLiveHandles: () => maxLiveHandles,
    lastState: () => states.at(-1),
  };
}

function descriptor() {
  return clientSchemas.hostedPlaybackDescriptor.parse({
    _tag: "same-origin-http", endpoint: "/api/v1/play/channel-one",
  });
}

async function flush() {
  await Promise.resolve();
  await Promise.resolve();
}
