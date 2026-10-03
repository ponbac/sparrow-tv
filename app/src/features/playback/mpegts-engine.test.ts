import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => vi.useRealTimers());
import { clientSchemas } from "../../client/contracts";
import {
  createMpegtsPlaybackEngine,
  type MpegtsRuntime,
} from "./mpegts-engine";

describe("hosted mpegts.js adapter", () => {
  it("releases a live player whose picture stops progressing", () => {
    vi.useFakeTimers();
    const fixture = runtimeFixture();
    const failures: string[] = [];
    const video = document.createElement("video");
    const started = createMpegtsPlaybackEngine(fixture.runtime).start({
      endpoint: playbackEndpoint(), video,
      onFailure: (failure) => failures.push(failure),
      onAutoplayBlocked: () => undefined,
    });
    if (typeof started === "string") throw new Error("expected a handle");
    video.dispatchEvent(new Event("playing"));
    vi.advanceTimersByTime(14_999);
    expect(failures).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(failures).toEqual(["stream-interrupted"]);
    expect(fixture.calls.slice(-4)).toEqual(["pause", "unload", "detach", "destroy"]);
    started.stop();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("keeps a progressing picture alive despite transient waiting/stalled events", () => {
    vi.useFakeTimers();
    const fixture = runtimeFixture();
    const failures: string[] = [];
    const video = document.createElement("video");
    const started = createMpegtsPlaybackEngine(fixture.runtime).start({
      endpoint: playbackEndpoint(), video, onFailure: (failure) => failures.push(failure),
      onAutoplayBlocked: () => undefined,
    });
    if (typeof started === "string") throw new Error("expected a handle");
    for (let second = 1; second <= 45; second += 1) {
      video.currentTime = second;
      video.dispatchEvent(new Event(second % 2 === 0 ? "waiting" : "stalled"));
      vi.advanceTimersByTime(1_000);
    }
    expect(failures).toEqual([]);
    vi.advanceTimersByTime(14_999);
    expect(failures).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(failures).toEqual(["stream-interrupted"]);
    started.stop();
  });

  it("suspends the progress deadline on pause and restarts it on resume", () => {
    vi.useFakeTimers();
    const fixture = runtimeFixture();
    const video = document.createElement("video");
    const failures: string[] = [];
    let pauses = 0;
    const started = createMpegtsPlaybackEngine(fixture.runtime).start({
      endpoint: playbackEndpoint(), video, onFailure: (failure) => failures.push(failure),
      onAutoplayBlocked: () => undefined, onPause: () => { pauses += 1; },
    });
    if (typeof started === "string") throw new Error("expected a handle");
    vi.advanceTimersByTime(14_000);
    video.dispatchEvent(new Event("pause"));
    vi.advanceTimersByTime(120_000);
    expect(failures).toEqual([]);
    expect(pauses).toBe(1);
    video.dispatchEvent(new Event("play"));
    vi.advanceTimersByTime(15_000);
    expect(failures).toEqual(["stream-interrupted"]);
    // Adapter teardown calls pause, but it is not a viewer pause.
    expect(pauses).toBe(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("leaves autoplay rejection to the viewer without a startup timeout", async () => {
    vi.useFakeTimers();
    const blocked = new Error("gesture required");
    blocked.name = "NotAllowedError";
    const fixture = runtimeFixture(true, { play: () => Promise.reject(blocked) });
    const video = document.createElement("video");
    const failures: string[] = [];
    let blocks = 0;
    const started = createMpegtsPlaybackEngine(fixture.runtime).start({
      endpoint: playbackEndpoint(), video, onFailure: (failure) => failures.push(failure),
      onAutoplayBlocked: () => { blocks += 1; },
    });
    if (typeof started === "string") throw new Error("expected a handle");
    await vi.advanceTimersByTimeAsync(120_000);
    expect(blocks).toBe(1);
    expect(failures).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);
    video.dispatchEvent(new Event("playing"));
    vi.advanceTimersByTime(15_000);
    expect(failures).toEqual(["stream-interrupted"]);
  });

  it("bounds startup even if the play promise never settles and ignores its late rejection", async () => {
    vi.useFakeTimers();
    let reject: ((cause: unknown) => void) | undefined;
    const playing = new Promise<void>((_resolve, no) => { reject = no; });
    const fixture = runtimeFixture(true, { play: () => playing });
    const failures: string[] = [];
    let blocks = 0;
    createMpegtsPlaybackEngine(fixture.runtime).start({
      endpoint: playbackEndpoint(), video: document.createElement("video"),
      onFailure: (failure) => failures.push(failure), onAutoplayBlocked: () => { blocks += 1; },
    });
    await vi.advanceTimersByTimeAsync(15_000);
    expect(failures).toEqual(["stream-interrupted"]);
    const blocked = new Error("late rejection");
    blocked.name = "NotAllowedError";
    reject?.(blocked);
    await Promise.resolve();
    expect(blocks).toBe(0);
    expect(failures).toEqual(["stream-interrupted"]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("recovers native media end even when the browser emits pause first", () => {
    vi.useFakeTimers();
    const fixture = runtimeFixture();
    const video = document.createElement("video");
    Object.defineProperty(video, "ended", { value: true });
    const failures: string[] = [];
    let pauses = 0;
    createMpegtsPlaybackEngine(fixture.runtime).start({
      endpoint: playbackEndpoint(), video, onFailure: (failure) => failures.push(failure),
      onAutoplayBlocked: () => undefined, onPause: () => { pauses += 1; },
    });
    video.dispatchEvent(new Event("pause"));
    video.dispatchEvent(new Event("ended"));
    fixture.loadingCompleteListener?.();
    expect(pauses).toBe(0);
    expect(failures).toEqual(["stream-interrupted"]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not call play after a synchronous end during load", () => {
    const fixture = runtimeFixture(true, { completesOnLoad: true });
    const failures: string[] = [];
    createMpegtsPlaybackEngine(fixture.runtime).start({
      endpoint: playbackEndpoint(), video: document.createElement("video"),
      onFailure: (failure) => failures.push(failure), onAutoplayBlocked: () => undefined,
    });
    expect(failures).toEqual(["stream-interrupted"]);
    expect(fixture.calls).not.toContain("play");
  });

  it("opens only the branded Sparrow route with a jitter-tolerant live buffer and releases the player idempotently", () => {
    const fixture = runtimeFixture();
    const engine = createMpegtsPlaybackEngine(fixture.runtime);
    const failure = vi.fn();

    const started = engine.start({
      endpoint: playbackEndpoint(),
      video: document.createElement("video"),
      onFailure: failure,
      onAutoplayBlocked: vi.fn(),
    });

    if (typeof started === "string") {
      throw new Error(`expected a playback handle, received ${started}`);
    }
    expect(fixture.source).toEqual({
      type: "mpegts",
      url: "/api/v1/play/channel-one",
      isLive: true,
      cors: false,
      withCredentials: true,
    });
    expect(fixture.config).toEqual({
      isLive: true,
      enableStashBuffer: false,
      lazyLoad: false,
      liveBufferLatencyChasing: true,
      liveBufferLatencyMaxLatency: 4,
      liveBufferLatencyMinRemain: 2,
      autoCleanupSourceBuffer: true,
    });

    started.stop();
    started.stop();
    expect(fixture.calls).toEqual([
      "on:error",
      "on:loading-complete",
      "attach",
      "load",
      "play",
      "off:error",
      "off:loading-complete",
      "pause",
      "unload",
      "detach",
      "destroy",
    ]);
    expect(failure).not.toHaveBeenCalled();
  });

  it("treats a completed live response as an interrupted stream and releases media", () => {
    const fixture = runtimeFixture();
    const failure = vi.fn();
    const started = createMpegtsPlaybackEngine(fixture.runtime).start({
      endpoint: playbackEndpoint(),
      video: document.createElement("video"),
      onFailure: failure,
      onAutoplayBlocked: vi.fn(),
    });
    if (
      typeof started === "string" ||
      fixture.loadingCompleteListener === undefined
    ) {
      throw new Error("expected an active fixture player");
    }

    fixture.loadingCompleteListener();

    expect(failure).toHaveBeenCalledTimes(1);
    expect(failure).toHaveBeenCalledWith("stream-interrupted");
    expect(fixture.calls.slice(-7)).toEqual([
      "off:error",
      "off:loading-complete",
      "pause",
      "unload",
      "detach",
      "destroy",
      "failure",
    ]);
  });

  it("keeps rejected, invalid, unavailable, and timeout statuses distinct", () => {
    for (const [status, expected] of [
      [401, "authentication-required"],
      [404, "channel-not-found"],
      [400, "source-rejected"],
      [403, "source-rejected"],
      [408, "source-timeout"],
      [429, "source-unavailable"],
      [424, "source-rejected"],
      [502, "source-invalid"],
      [503, "source-unavailable"],
      [504, "source-timeout"],
    ] as const) {
      const fixture = runtimeFixture();
      const failure = vi.fn();
      const started = createMpegtsPlaybackEngine(fixture.runtime).start({
        endpoint: playbackEndpoint(),
        video: document.createElement("video"),
        onFailure: failure,
        onAutoplayBlocked: vi.fn(),
      });
      if (typeof started === "string" || fixture.errorListener === undefined) {
        throw new Error("expected an active fixture player");
      }

      fixture.errorListener(
        fixture.runtime.ErrorTypes.NETWORK_ERROR,
        fixture.runtime.ErrorDetails.NETWORK_STATUS_CODE_INVALID,
        { code: status },
      );

      expect(failure).toHaveBeenCalledWith(expected);
    }
  });

  it("reduces mpegts errors to safe actionable categories before cleanup", () => {
    const fixture = runtimeFixture();
    const failure = vi.fn();
    const started = createMpegtsPlaybackEngine(fixture.runtime).start({
      endpoint: playbackEndpoint(),
      video: document.createElement("video"),
      onFailure: failure,
      onAutoplayBlocked: vi.fn(),
    });
    if (typeof started === "string" || fixture.errorListener === undefined) {
      throw new Error("expected an active fixture player");
    }

    fixture.errorListener(
      fixture.runtime.ErrorTypes.NETWORK_ERROR,
      fixture.runtime.ErrorDetails.NETWORK_STATUS_CODE_INVALID,
      {
        code: 504,
        msg: "https://viewer:secret@provider.invalid/live?token=private",
      },
    );

    expect(failure).toHaveBeenCalledWith("source-timeout");
    expect(JSON.stringify(failure.mock.calls)).not.toContain("provider.invalid");
    expect(fixture.calls.slice(-5)).toEqual([
      "pause",
      "unload",
      "detach",
      "destroy",
      "failure",
    ]);
  });

  it("fails closed before player construction when live MSE is unavailable", () => {
    const fixture = runtimeFixture(false);

    expect(
      createMpegtsPlaybackEngine(fixture.runtime).start({
        endpoint: playbackEndpoint(),
        video: document.createElement("video"),
        onFailure: vi.fn(),
        onAutoplayBlocked: vi.fn(),
      }),
    ).toBe("browser-unsupported");
    expect(fixture.calls).toEqual([]);
  });
});

function runtimeFixture(mseLivePlayback = true, options: {
  readonly play?: () => Promise<void> | void;
  readonly completesOnLoad?: boolean;
} = {}): {
  readonly runtime: MpegtsRuntime;
  readonly calls: string[];
  readonly source:
    | Parameters<MpegtsRuntime["createPlayer"]>[0]
    | undefined;
  readonly config:
    | Parameters<MpegtsRuntime["createPlayer"]>[1]
    | undefined;
  readonly errorListener: ((...args: unknown[]) => void) | undefined;
  readonly loadingCompleteListener: (() => void) | undefined;
} {
  const calls: string[] = [];
  let source: Parameters<MpegtsRuntime["createPlayer"]>[0] | undefined;
  let config: Parameters<MpegtsRuntime["createPlayer"]>[1] | undefined;
  let errorListener: ((...args: unknown[]) => void) | undefined;
  let loadingCompleteListener: (() => void) | undefined;
  let video: HTMLMediaElement | undefined;
  const runtime: MpegtsRuntime = {
    getFeatureList: () => ({ mseLivePlayback }),
    Events: { ERROR: "error", LOADING_COMPLETE: "loading-complete" },
    ErrorTypes: { NETWORK_ERROR: "network", MEDIA_ERROR: "media" },
    ErrorDetails: {
      NETWORK_STATUS_CODE_INVALID: "status",
      NETWORK_TIMEOUT: "timeout",
    },
    createPlayer: (nextSource, nextConfig) => {
      source = nextSource;
      config = nextConfig;
      return {
        currentTime: 0,
        on: (event, listener) => {
          calls.push(`on:${event}`);
          const recordedListener = (...args: unknown[]) => {
            listener(...args);
            calls.push("failure");
          };
          if (event === runtime.Events.ERROR) {
            errorListener = recordedListener;
          } else if (event === runtime.Events.LOADING_COMPLETE) {
            loadingCompleteListener = recordedListener;
          }
        },
        off: (event) => calls.push(`off:${event}`),
        attachMediaElement: (element) => { video = element; calls.push("attach"); },
        detachMediaElement: () => calls.push("detach"),
        load: () => {
          calls.push("load");
          if (options.completesOnLoad === true) loadingCompleteListener?.();
        },
        unload: () => calls.push("unload"),
        play: () => {
          calls.push("play");
          return options.play?.();
        },
        pause: () => { calls.push("pause"); video?.dispatchEvent(new Event("pause")); },
        destroy: () => calls.push("destroy"),
      };
    },
  };
  return {
    runtime,
    calls,
    get source() {
      return source;
    },
    get config() {
      return config;
    },
    get errorListener() {
      return errorListener;
    },
    get loadingCompleteListener() {
      return loadingCompleteListener;
    },
  };
}

function playbackEndpoint() {
  return clientSchemas.hostedPlaybackDescriptor.parse({
    _tag: "same-origin-http",
    endpoint: "/api/v1/play/channel-one",
  }).endpoint;
}
