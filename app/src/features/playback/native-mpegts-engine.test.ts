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
  it("binds an opaque stream to a main-thread custom loader and releases once", async () => {
    const fixture = runtimeFixture();
    const stopPlayback = vi.fn(async () => success(undefined));
    const started = createNativeMpegtsPlaybackEngine(fixture.runtime).start({
      client: {
        readPlayback: vi.fn(async () => success(new ArrayBuffer(0))),
        stopPlayback,
      },
      descriptor: DESCRIPTOR,
      video: document.createElement("video"),
      onFailure: vi.fn(),
      onAutoplayBlocked: vi.fn(),
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

    started.stop();
    started.stop();
    await Promise.resolve();
    expect(stopPlayback).toHaveBeenCalledTimes(1);
    expect(stopPlayback).toHaveBeenCalledWith({
      sessionId: DESCRIPTOR.sessionId,
    });
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

  it("releases a session before returning an unsupported-player failure", async () => {
    const fixture = runtimeFixture(false);
    const stopPlayback = vi.fn(async () => success(undefined));

    expect(
      createNativeMpegtsPlaybackEngine(fixture.runtime).start({
        client: {
          readPlayback: vi.fn(async () => success(new ArrayBuffer(0))),
          stopPlayback,
        },
        descriptor: DESCRIPTOR,
        video: document.createElement("video"),
        onFailure: vi.fn(),
        onAutoplayBlocked: vi.fn(),
      }),
    ).toBe("browser-unsupported");
    await Promise.resolve();
    expect(stopPlayback).toHaveBeenCalledTimes(1);
    expect(fixture.calls).toEqual([]);
  });

  it("maps native loader and media failures without reflecting their details", async () => {
    for (const [type, expected] of [
      ["network", "stream-interrupted"],
      ["media", "media-unsupported"],
    ] as const) {
      const fixture = runtimeFixture();
      const failure = vi.fn();
      const stopPlayback = vi.fn(async () => success(undefined));
      const started = createNativeMpegtsPlaybackEngine(fixture.runtime).start({
        client: {
          readPlayback: vi.fn(async () => success(new ArrayBuffer(0))),
          stopPlayback,
        },
        descriptor: DESCRIPTOR,
        video: document.createElement("video"),
        onFailure: failure,
        onAutoplayBlocked: vi.fn(),
      });
      if (typeof started === "string" || fixture.errorListener === undefined) {
        throw new Error("expected an active fixture player");
      }

      fixture.errorListener(type, "private-detail", {
        url: "https://user:secret@provider.invalid/live",
      });
      await Promise.resolve();

      expect(failure).toHaveBeenCalledWith(expected);
      expect(JSON.stringify(failure.mock.calls)).not.toContain("provider.invalid");
      expect(stopPlayback).toHaveBeenCalledTimes(1);
    }
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
