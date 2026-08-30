import { describe, expect, it, vi } from "vitest";
import { clientSchemas } from "../../client/contracts";
import {
  createMpegtsPlaybackEngine,
  type MpegtsRuntime,
} from "./mpegts-engine";

describe("hosted mpegts.js adapter", () => {
  it("opens only the branded Sparrow route and releases the player idempotently", () => {
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

function runtimeFixture(mseLivePlayback = true): {
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
        attachMediaElement: () => calls.push("attach"),
        detachMediaElement: () => calls.push("detach"),
        load: () => calls.push("load"),
        unload: () => calls.push("unload"),
        play: () => {
          calls.push("play");
        },
        pause: () => calls.push("pause"),
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
