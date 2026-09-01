import { describe, expect, it, vi } from "vitest";
import {
  clientSchemas,
  type AndroidPlaybackPresentation,
  type InstalledPlaybackSession,
  type InstalledPlaybackTransport,
  type NativeStreamPlaybackTransport,
} from "../../client/contracts";
import type { AndroidMedia3PlaybackEngine } from "./android-media3-engine";
import {
  createInstalledPlaybackEngine,
  type InstalledPlaybackEngineAdapters,
  type InstalledPlaybackRequest,
} from "./installed-playback-engine";
import type { NativeMpegtsPlaybackEngine } from "./native-mpegts-engine";

const STREAM_HANDLE = clientSchemas.nativeStreamHandle.parse(
  `stream1_${"8".repeat(16)}`,
);
const LINUX_TRANSPORT = { _tag: "linux-mpv" } as const;
const ANDROID_TRANSPORT: NativeStreamPlaybackTransport = {
  _tag: "tauri-native-stream",
  streamHandle: STREAM_HANDLE,
  presentation: "android-media3",
  tracks: [],
  selection: { _tag: "none" },
};
const WEBVIEW_TRANSPORT: NativeStreamPlaybackTransport = {
  ...ANDROID_TRANSPORT,
  presentation: "webview-mse",
};

describe("installed playback platform router", () => {
  it("applies initial Linux controls and fullscreen through the correlated private session", async () => {
    const controlMpv = vi.fn<InstalledPlaybackSession["controlMpv"]>(async () =>
      success(undefined),
    );
    const playing = vi.fn();
    const failure = vi.fn();
    const engine = createInstalledPlaybackEngine(unusedAdapters());
    const started = engine.start(
      requestFixture(LINUX_TRANSPORT, sessionFixture(controlMpv), {
        onPlaying: playing,
        onFailure: failure,
      }),
    );
    const handle = requireHandle(started);
    if (
      handle.setControls === undefined ||
      handle.requestFullscreen === undefined
    ) {
      throw new Error("expected Linux presentation controls");
    }

    handle.setControls({ volume: 1, muted: true });
    await expect(handle.requestFullscreen(true)).resolves.toBe(true);
    await expect(handle.requestFullscreen(false)).resolves.toBe(true);
    await flushPromises();

    expect(playing).toHaveBeenCalledTimes(1);
    expect(failure).not.toHaveBeenCalled();
    expect(controlMpv.mock.calls).toEqual([
      [{ _tag: "set-volume", percent: 100 }],
      [{ _tag: "set-muted", muted: true }],
      [{ _tag: "set-fullscreen", fullscreen: true }],
      [{ _tag: "set-fullscreen", fullscreen: false }],
    ]);
    expect(JSON.stringify(controlMpv.mock.calls)).not.toContain("provider");

    handle.setControls({ volume: 1, muted: true });
    await flushPromises();
    expect(controlMpv).toHaveBeenCalledTimes(4);
    handle.stop();
    await expect(handle.requestFullscreen(true)).resolves.toBe(false);
    expect(controlMpv).toHaveBeenCalledTimes(4);
  });

  it("maps a typed Linux control failure once without reflecting private details", async () => {
    const controlMpv = vi.fn<InstalledPlaybackSession["controlMpv"]>(async () => ({
      ok: false,
      error: {
        _tag: "transport",
        retryable: false,
        message: "https://user:secret@provider.invalid/private.ts",
      },
    }));
    const failure = vi.fn();
    const started = createInstalledPlaybackEngine(unusedAdapters()).start(
      requestFixture(LINUX_TRANSPORT, sessionFixture(controlMpv), {
        onFailure: failure,
      }),
    );
    const handle = requireHandle(started);
    if (handle.setControls === undefined) {
      throw new Error("expected Linux presentation controls");
    }

    handle.setControls({ volume: 0.42, muted: true });
    await until(() => failure.mock.calls.length === 1);

    expect(failure).toHaveBeenCalledWith(
      "system-player-unavailable",
      false,
    );
    expect(JSON.stringify(failure.mock.calls)).not.toContain("provider.invalid");
    expect(controlMpv).toHaveBeenCalledTimes(1);
    expect(controlMpv).toHaveBeenCalledWith({
      _tag: "set-volume",
      percent: 42,
    });
  });

  it("reduces a rejected Linux control to a retryable safe failure", async () => {
    const controlMpv = vi.fn<InstalledPlaybackSession["controlMpv"]>(() =>
      Promise.reject(new Error("private process detail")),
    );
    const failure = vi.fn();
    const started = createInstalledPlaybackEngine(unusedAdapters()).start(
      requestFixture(LINUX_TRANSPORT, sessionFixture(controlMpv), {
        onFailure: failure,
      }),
    );
    const handle = requireHandle(started);
    if (handle.requestFullscreen === undefined) {
      throw new Error("expected Linux fullscreen control");
    }

    await expect(handle.requestFullscreen(true)).resolves.toBe(false);
    expect(failure).toHaveBeenCalledWith(
      "system-player-unavailable",
      true,
    );
    expect(JSON.stringify(failure.mock.calls)).not.toContain(
      "private process detail",
    );
  });

  it("preserves actionable typed system-mpv setup failures", async () => {
    const failure = vi.fn();
    const controlMpv = vi.fn<InstalledPlaybackSession["controlMpv"]>(async () => ({
      ok: false,
      error: {
        _tag: "mpv-failed",
        reason: "not-installed",
        retryable: false,
      },
    }));
    const started = createInstalledPlaybackEngine(unusedAdapters()).start(
      requestFixture(LINUX_TRANSPORT, sessionFixture(controlMpv), {
        onFailure: failure,
      }),
    );
    const handle = requireHandle(started);
    if (handle.requestFullscreen === undefined) {
      throw new Error("expected Linux fullscreen control");
    }

    await expect(handle.requestFullscreen(true)).resolves.toBe(false);
    expect(failure).toHaveBeenCalledWith("system-player-missing", false);
  });

  it("detects an unexpected mpv exit through a bounded private health check", async () => {
    vi.useFakeTimers();
    try {
      const failure = vi.fn();
      const controlMpv = vi.fn<InstalledPlaybackSession["controlMpv"]>(
        async (control) =>
          control._tag === "health-check"
            ? {
                ok: false,
                error: {
                  _tag: "mpv-failed",
                  reason: "terminated",
                  retryable: true,
                },
              }
            : success(undefined),
      );
      const started = createInstalledPlaybackEngine(unusedAdapters()).start(
        requestFixture(LINUX_TRANSPORT, sessionFixture(controlMpv), {
          onFailure: failure,
        }),
      );
      requireHandle(started);

      await vi.advanceTimersByTimeAsync(1_000);

      expect(controlMpv).toHaveBeenCalledWith({ _tag: "health-check" });
      expect(failure).toHaveBeenCalledWith(
        "system-player-unavailable",
        true,
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("selects Android Media3 and WebView MSE only from the parsed presentation tag", () => {
    const androidHandle = { stop: vi.fn() };
    const webviewHandle = { stop: vi.fn() };
    const androidStart = vi.fn<AndroidMedia3PlaybackEngine["start"]>(
      () => androidHandle,
    );
    const webviewStart = vi.fn<NativeMpegtsPlaybackEngine["start"]>(
      () => webviewHandle,
    );
    const adapters: InstalledPlaybackEngineAdapters = {
      androidMedia3: { start: androidStart },
      webviewMse: { start: webviewStart },
    };
    const engine = createInstalledPlaybackEngine(adapters);
    const session = sessionFixture();
    const androidRequest = requestFixture(ANDROID_TRANSPORT, session);
    const webviewRequest = requestFixture(WEBVIEW_TRANSPORT, session);

    expect(engine.start(androidRequest)).toBe(androidHandle);
    expect(androidStart).toHaveBeenCalledWith({
      ...androidRequest,
      descriptor: ANDROID_TRANSPORT,
    });
    expect(webviewStart).not.toHaveBeenCalled();

    expect(engine.start(webviewRequest)).toBe(webviewHandle);
    expect(webviewStart).toHaveBeenCalledWith({
      ...webviewRequest,
      descriptor: WEBVIEW_TRANSPORT,
    });
    expect(androidStart).toHaveBeenCalledTimes(1);
  });
});

function requestFixture(
  descriptor: InstalledPlaybackTransport,
  session: InstalledPlaybackSession,
  callbacks: Partial<
    Pick<InstalledPlaybackRequest, "onFailure" | "onPlaying">
  > = {},
): InstalledPlaybackRequest {
  return {
    session,
    descriptor,
    video: document.createElement("video"),
    onFailure: callbacks.onFailure ?? vi.fn(),
    onAutoplayBlocked: vi.fn(),
    onPlaying: callbacks.onPlaying ?? vi.fn(),
  };
}

function sessionFixture(
  controlMpv: InstalledPlaybackSession["controlMpv"] = async () =>
    success(undefined),
): InstalledPlaybackSession {
  return {
    start: async () => success(LINUX_TRANSPORT),
    reopen: async () => success(LINUX_TRANSPORT),
    restart: async () => success(WEBVIEW_TRANSPORT),
    read: async () => success(new ArrayBuffer(0)),
    startAndroidPresentation: async () =>
      success(androidPresentationFixture()),
    controlMpv,
    suspend: async () => success(undefined),
    setActivity: async () => success(undefined),
    stop: async () => success(undefined),
  };
}

function androidPresentationFixture(): AndroidPlaybackPresentation {
  return {
    status: async () =>
      success({
        state: "stopped",
        decodedFrames: 0,
        droppedFrames: 0,
        bufferedDurationMs: 0,
        silent: true,
      }),
    pause: async () => success(undefined),
    resume: async () => success(undefined),
    setVolume: async () => success(undefined),
    setMuted: async () => success(undefined),
    setViewport: async () => success(undefined),
    stop: async () => success(undefined),
  };
}

function unusedAdapters(): InstalledPlaybackEngineAdapters {
  return {
    androidMedia3: {
      start: () => {
        throw new Error("unexpected Android adapter route");
      },
    },
    webviewMse: {
      start: () => {
        throw new Error("unexpected WebView adapter route");
      },
    },
  };
}

function requireHandle(
  result: ReturnType<ReturnType<typeof createInstalledPlaybackEngine>["start"]>,
) {
  if (typeof result === "string") {
    throw new Error(`expected an installed playback handle, received ${result}`);
  }
  return result;
}

function success<Value>(value: Value): {
  readonly ok: true;
  readonly value: Value;
} {
  return { ok: true, value };
}

async function flushPromises(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

async function until(predicate: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 30; attempt += 1) {
    if (predicate()) {
      return;
    }
    await Promise.resolve();
  }
  throw new Error("asynchronous fixture did not settle");
}
