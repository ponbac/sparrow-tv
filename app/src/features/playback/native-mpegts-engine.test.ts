import mpegts from "mpegts.js";
import { describe, expect, it, vi } from "vitest";
import { clientSchemas } from "../../client/contracts";
import {
  createNativeMpegtsPlaybackEngine,
  type NativeMpegtsRuntime,
} from "./native-mpegts-engine";
import { NATIVE_PLAYBACK_SENTINEL } from "./native-mpegts-loader";

const DESCRIPTOR = clientSchemas.nativePlaybackDescriptor.parse({
  _tag: "tauri-native-stream",
  sessionId: `play1_${"c".repeat(32)}_1`,
  streamHandle: `stream1_${"d".repeat(16)}`,
});

describe("installed mpegts.js adapter", () => {
  it("binds an opaque stream to a main-thread loader without final-stopping its session", async () => {
    const fixture = runtimeFixture();
    const read = vi.fn(async () => success(new ArrayBuffer(0)));
    const playing = vi.fn();
    const video = document.createElement("video");
    const started = createNativeMpegtsPlaybackEngine(fixture.runtime).start({
      session: { read },
      descriptor: DESCRIPTOR,
      video,
      onFailure: vi.fn(),
      onAutoplayBlocked: vi.fn(),
      onPlaying: playing,
    });

    if (typeof started === "string") {
      throw new Error(`expected a playback handle, received ${started}`);
    }
    expect(fixture.source).toEqual({
      type: "mpegts",
      url: NATIVE_PLAYBACK_SENTINEL,
      isLive: true,
      cors: false,
      withCredentials: false,
    });
    expect(fixture.config).toEqual({
      isLive: true,
      enableStashBuffer: false,
      lazyLoad: false,
      liveBufferLatencyChasing: true,
      autoCleanupSourceBuffer: true,
      enableWorker: false,
      customLoader: expect.any(Function),
    });
    expect(JSON.stringify(fixture.source)).not.toContain(DESCRIPTOR.sessionId);
    expect(JSON.stringify(fixture.source)).not.toContain(DESCRIPTOR.streamHandle);

    video.dispatchEvent(new Event("playing"));
    expect(playing).toHaveBeenCalledTimes(1);

    started.stop();
    started.stop();
    await Promise.resolve();
    video.dispatchEvent(new Event("playing"));
    expect(playing).toHaveBeenCalledTimes(1);
    expect(read).not.toHaveBeenCalled();
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
  });

  it("returns an unsupported-player failure without taking session ownership", async () => {
    const fixture = runtimeFixture(false);

    expect(
      createNativeMpegtsPlaybackEngine(fixture.runtime).start({
        session: {
          read: vi.fn(async () => success(new ArrayBuffer(0))),
        },
        descriptor: DESCRIPTOR,
        video: document.createElement("video"),
        onFailure: vi.fn(),
        onAutoplayBlocked: vi.fn(),
        onPlaying: vi.fn(),
      }),
    ).toBe("browser-unsupported");
    await Promise.resolve();
    expect(fixture.calls).toEqual([]);
  });

  it("maps native loader and media failures without reflecting their details", async () => {
    for (const [type, expected] of [
      ["network", "stream-interrupted"],
      ["media", "media-unsupported"],
    ] as const) {
      const fixture = runtimeFixture();
      const failure = vi.fn();
      const started = createNativeMpegtsPlaybackEngine(fixture.runtime).start({
        session: {
          read: vi.fn(async () => success(new ArrayBuffer(0))),
        },
        descriptor: DESCRIPTOR,
        video: document.createElement("video"),
        onFailure: failure,
        onAutoplayBlocked: vi.fn(),
        onPlaying: vi.fn(),
      });
      if (typeof started === "string" || fixture.errorListener === undefined) {
        throw new Error("expected an active fixture player");
      }

      fixture.errorListener(type, "private-detail", {
        url: "https://user:secret@provider.invalid/live",
      });
      await Promise.resolve();

      expect(failure).toHaveBeenCalledWith(
        expected,
        type === "network",
      );
      expect(JSON.stringify(failure.mock.calls)).not.toContain("provider.invalid");
    }
  });

  it("preserves a non-retryable safe read classification through the loader seam", async () => {
    const fixture = runtimeFixture();
    const failure = vi.fn();
    const started = createNativeMpegtsPlaybackEngine(fixture.runtime).start({
      session: {
        read: vi.fn(async () => ({
          ok: false as const,
          error: {
            _tag: "transport" as const,
            retryable: false,
            message: "https://user:secret@provider.invalid/live",
          },
        })),
      },
      descriptor: DESCRIPTOR,
      video: document.createElement("video"),
      onFailure: failure,
      onAutoplayBlocked: vi.fn(),
      onPlaying: vi.fn(),
    });
    const Loader = fixture.config?.customLoader;
    if (
      typeof started === "string" ||
      Loader === undefined ||
      fixture.errorListener === undefined
    ) {
      throw new Error("expected an active custom-loader fixture");
    }
    const loader = new Loader({}, {});
    loader.onError = vi.fn();
    loader.open(
      { url: NATIVE_PLAYBACK_SENTINEL, duration: 0 },
      { from: 0, to: -1 },
    );
    await until(() => vi.mocked(loader.onError).mock.calls.length === 1);

    fixture.errorListener("network", "private-detail");

    expect(failure).toHaveBeenCalledWith("source-unavailable", false);
    expect(JSON.stringify(failure.mock.calls)).not.toContain("provider.invalid");
  });
});

function runtimeFixture(mseLivePlayback = true): {
  readonly runtime: NativeMpegtsRuntime;
  readonly calls: string[];
  readonly source:
    | Parameters<NativeMpegtsRuntime["createPlayer"]>[0]
    | undefined;
  readonly config:
    | Parameters<NativeMpegtsRuntime["createPlayer"]>[1]
    | undefined;
  readonly errorListener: ((...args: unknown[]) => void) | undefined;
} {
  const calls: string[] = [];
  let source: Parameters<NativeMpegtsRuntime["createPlayer"]>[0] | undefined;
  let config: Parameters<NativeMpegtsRuntime["createPlayer"]>[1] | undefined;
  let errorListener: ((...args: unknown[]) => void) | undefined;
  const runtime: NativeMpegtsRuntime = {
    BaseLoader: mpegts.BaseLoader,
    LoaderStatus: mpegts.LoaderStatus,
    LoaderErrors: mpegts.LoaderErrors,
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
          if (event === runtime.Events.ERROR) {
            errorListener = listener;
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
  };
}

function success<Value>(value: Value): { readonly ok: true; readonly value: Value } {
  return { ok: true, value };
}

async function until(predicate: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (predicate()) {
      return;
    }
    await Promise.resolve();
  }
  throw new Error("asynchronous fixture did not settle");
}
