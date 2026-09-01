import { describe, expect, it, vi } from "vitest";
import {
  clientSchemas,
  type AndroidPlaybackPresentation,
  type AndroidPlaybackStatus,
  type AndroidPlaybackViewport,
  type InstalledPlaybackSession,
  type NativeStreamPlaybackTransport,
} from "../../client/contracts";
import {
  createBrowserAndroidMedia3Runtime,
  createAndroidMedia3PlaybackEngine,
  type AndroidMedia3Runtime,
} from "./android-media3-engine";

const DESCRIPTOR: NativeStreamPlaybackTransport = {
  _tag: "tauri-native-stream",
  streamHandle: clientSchemas.nativeStreamHandle.parse(
    `stream1_${"d".repeat(16)}`,
  ),
  presentation: "android-media3",
  tracks: [],
  selection: { _tag: "none" },
};

const INITIAL_VIEWPORT: AndroidPlaybackViewport = {
  left: 12,
  top: 20,
  width: 1_280,
  height: 720,
  fullscreen: false,
};

describe("Android Media3 playback adapter", () => {
  it("projects only an opaque handle, forwards controls/viewport, and reports aggregate status", async () => {
    const fixture = runtimeFixture();
    const status: { value: AndroidPlaybackStatus } = {
      value: {
        state: "starting",
        decodedFrames: 0,
        droppedFrames: 0,
        bufferedDurationMs: 0,
        silent: true,
      },
    };
    const presentation = presentationFixture(status);
    const startAndroidPresentation = vi.fn<
      InstalledPlaybackSession["startAndroidPresentation"]
    >(async () => success(presentation));
    const playing = vi.fn();
    const failure = vi.fn();
    const video = document.createElement("video");
    video.volume = 0.7;
    video.muted = true;
    const started = createAndroidMedia3PlaybackEngine(fixture.runtime).start({
      session: { startAndroidPresentation },
      descriptor: DESCRIPTOR,
      video,
      onFailure: failure,
      onAutoplayBlocked: vi.fn(),
      onPlaying: playing,
    });
    if (typeof started === "string") {
      throw new Error(`expected a Media3 handle, received ${started}`);
    }
    expect(video.dataset).toMatchObject({
      playbackEngine: "android-media3",
      playbackState: "starting",
      decodedFrames: "0",
      droppedFrames: "0",
      bufferedDurationMs: "0",
      processSilent: "false",
    });
    await flushPromises();

    expect(startAndroidPresentation).toHaveBeenCalledWith({
      streamHandle: DESCRIPTOR.streamHandle,
      viewport: INITIAL_VIEWPORT,
      volume: 0.7,
      muted: true,
      signal: expect.any(AbortSignal),
    });
    expect(JSON.stringify(startAndroidPresentation.mock.calls)).not.toContain(
      "provider",
    );

    status.value = {
      state: "playing",
      decodedFrames: 1_200,
      droppedFrames: 3,
      bufferedDurationMs: 1_500,
      silent: true,
    };
    fixture.runNextTask();
    await flushPromises();
    expect(playing).toHaveBeenCalledTimes(1);
    expect(failure).not.toHaveBeenCalled();
    expect(video.dataset).toMatchObject({
      playbackEngine: "android-media3",
      playbackState: "playing",
      decodedFrames: "1200",
      droppedFrames: "3",
      bufferedDurationMs: "1500",
      processSilent: "true",
    });

    fixture.runNextTask();
    await flushPromises();
    expect(playing).toHaveBeenCalledTimes(1);

    const fullscreenViewport = {
      left: 0,
      top: 0,
      width: 2_400,
      height: 1_080,
      fullscreen: true,
    } as const;
    fixture.pushViewport(fullscreenViewport);
    video.volume = 0.3;
    video.muted = false;
    video.dispatchEvent(new Event("volumechange"));
    await flushPromises();
    expect(presentation.setViewport).toHaveBeenLastCalledWith(
      fullscreenViewport,
    );
    expect(presentation.setVolume).toHaveBeenLastCalledWith(0.3);
    expect(presentation.setMuted).toHaveBeenLastCalledWith(false);

    status.value = {
      state: "failed",
      decodedFrames: 1_250,
      droppedFrames: 4,
      bufferedDurationMs: 0,
      silent: true,
    };
    fixture.runNextTask();
    await flushPromises();
    expect(failure).toHaveBeenCalledWith("stream-interrupted", true);
    expect(presentation.stop).not.toHaveBeenCalled();
    expect(video.dataset.playbackEngine).toBeUndefined();

    started.stop();
    started.stop();
    expect(presentation.stop).toHaveBeenCalledTimes(1);
    expect(fixture.released).toBe(true);
  });

  it("reduces a native failure to safe player vocabulary without reflecting details", async () => {
    const fixture = runtimeFixture();
    const failure = vi.fn();
    const startAndroidPresentation = vi.fn<
      InstalledPlaybackSession["startAndroidPresentation"]
    >(async () => ({
      ok: false,
      error: {
        _tag: "transport",
        retryable: false,
        message: "https://user:secret@provider.invalid/live.ts",
      },
    }));
    const started = createAndroidMedia3PlaybackEngine(fixture.runtime).start({
      session: { startAndroidPresentation },
      descriptor: DESCRIPTOR,
      video: document.createElement("video"),
      onFailure: failure,
      onAutoplayBlocked: vi.fn(),
      onPlaying: vi.fn(),
    });
    if (typeof started === "string") {
      throw new Error("expected an asynchronous Media3 start");
    }
    await flushPromises();

    expect(failure).toHaveBeenCalledWith("source-unavailable", false);
    expect(JSON.stringify(failure.mock.calls)).not.toContain(
      "provider.invalid",
    );
    expect(fixture.released).toBe(true);
  });

  it("defers native ownership until an offscreen video becomes visible and hides it when it leaves again", async () => {
    const fixture = runtimeFixture(null);
    const status = {
      value: {
        state: "starting",
        decodedFrames: 0,
        droppedFrames: 0,
        bufferedDurationMs: 0,
        silent: true,
      } satisfies AndroidPlaybackStatus,
    };
    const presentation = presentationFixture(status);
    const startAndroidPresentation = vi.fn<
      InstalledPlaybackSession["startAndroidPresentation"]
    >(async () => success(presentation));
    const failure = vi.fn();
    const started = createAndroidMedia3PlaybackEngine(fixture.runtime).start({
      session: { startAndroidPresentation },
      descriptor: DESCRIPTOR,
      video: document.createElement("video"),
      onFailure: failure,
      onAutoplayBlocked: vi.fn(),
      onPlaying: vi.fn(),
    });
    if (typeof started === "string") {
      throw new Error(`expected a deferred Media3 handle, received ${started}`);
    }

    expect(startAndroidPresentation).not.toHaveBeenCalled();
    expect(fixture.activeTaskCount).toBe(1);

    fixture.pushViewport(INITIAL_VIEWPORT);
    expect(fixture.activeTaskCount).toBe(0);
    await flushPromises();
    expect(startAndroidPresentation).toHaveBeenCalledWith({
      streamHandle: DESCRIPTOR.streamHandle,
      viewport: INITIAL_VIEWPORT,
      volume: 1,
      muted: false,
      signal: expect.any(AbortSignal),
    });

    fixture.pushViewport(null);
    await flushPromises();
    expect(presentation.setViewport).toHaveBeenLastCalledWith({
      left: 32_768,
      top: 32_768,
      width: 1,
      height: 1,
      fullscreen: false,
    });
    expect(failure).not.toHaveBeenCalled();

    started.stop();
    expect(presentation.stop).toHaveBeenCalledTimes(1);
    expect(fixture.released).toBe(true);
  });

  it("fails safely when an offscreen video never gets a bounded viewport", () => {
    const fixture = runtimeFixture(null);
    const startAndroidPresentation =
      vi.fn<InstalledPlaybackSession["startAndroidPresentation"]>();
    const failure = vi.fn();
    const started = createAndroidMedia3PlaybackEngine(fixture.runtime).start({
      session: { startAndroidPresentation },
      descriptor: DESCRIPTOR,
      video: document.createElement("video"),
      onFailure: failure,
      onAutoplayBlocked: vi.fn(),
      onPlaying: vi.fn(),
    });
    if (typeof started === "string") {
      throw new Error(`expected a deferred Media3 handle, received ${started}`);
    }

    fixture.runNextTask();

    expect(startAndroidPresentation).not.toHaveBeenCalled();
    expect(failure).toHaveBeenCalledWith("media-unsupported", false);
    expect(fixture.released).toBe(true);
    started.stop();
  });

  it("recovers when Media3 remains in starting without producing frames", async () => {
    const fixture = runtimeFixture();
    const status = {
      value: {
        state: "starting",
        decodedFrames: 0,
        droppedFrames: 0,
        bufferedDurationMs: 0,
        silent: true,
      } satisfies AndroidPlaybackStatus,
    };
    const presentation = presentationFixture(status);
    const failure = vi.fn();
    const started = createAndroidMedia3PlaybackEngine(fixture.runtime).start({
      session: {
        startAndroidPresentation: async () => success(presentation),
      },
      descriptor: DESCRIPTOR,
      video: document.createElement("video"),
      onFailure: failure,
      onAutoplayBlocked: vi.fn(),
      onPlaying: vi.fn(),
    });
    if (typeof started === "string") {
      throw new Error("expected an asynchronous Media3 start");
    }
    await flushPromises();

    for (let poll = 0; poll < 60; poll += 1) {
      fixture.runNextTask();
      await flushPromises();
    }

    expect(failure).toHaveBeenCalledWith("stream-interrupted", true);
    expect(presentation.stop).not.toHaveBeenCalled();
    started.stop();
    expect(presentation.stop).toHaveBeenCalledTimes(1);
  });

  it("recovers when a playing Media3 presentation stops making frame progress", async () => {
    const fixture = runtimeFixture();
    const status = {
      value: {
        state: "playing",
        decodedFrames: 25,
        droppedFrames: 0,
        bufferedDurationMs: 2_000,
        silent: true,
      } satisfies AndroidPlaybackStatus,
    };
    const presentation = presentationFixture(status);
    const failure = vi.fn();
    const playing = vi.fn();
    const started = createAndroidMedia3PlaybackEngine(fixture.runtime).start({
      session: {
        startAndroidPresentation: async () => success(presentation),
      },
      descriptor: DESCRIPTOR,
      video: document.createElement("video"),
      onFailure: failure,
      onAutoplayBlocked: vi.fn(),
      onPlaying: playing,
    });
    if (typeof started === "string") {
      throw new Error("expected an asynchronous Media3 start");
    }
    await flushPromises();

    for (let poll = 0; poll <= 40; poll += 1) {
      fixture.runNextTask();
      await flushPromises();
    }

    expect(playing).toHaveBeenCalledTimes(1);
    expect(failure).toHaveBeenCalledWith("stream-interrupted", true);
    expect(presentation.stop).not.toHaveBeenCalled();
    started.stop();
    expect(presentation.stop).toHaveBeenCalledTimes(1);
  });
});

describe("browser Android Media3 viewport runtime", () => {
  it("tracks scroll-only movement and releases scroll listeners and pending work", () => {
    const scrollContainer = document.createElement("div");
    const video = document.createElement("video");
    scrollContainer.append(video);
    document.body.append(scrollContainer);
    let rectangle = viewportRectangle(12, 20, 640, 360);
    vi.spyOn(video, "getBoundingClientRect").mockImplementation(
      () => rectangle,
    );
    const animationFrames = new Map<number, FrameRequestCallback>();
    let nextAnimationFrame = 1;
    const requestAnimationFrame = vi.fn(
      (callback: FrameRequestCallback): number => {
        const identifier = nextAnimationFrame;
        nextAnimationFrame += 1;
        animationFrames.set(identifier, callback);
        return identifier;
      },
    );
    const cancelAnimationFrame = vi.fn((identifier: number): void => {
      animationFrames.delete(identifier);
    });
    vi.stubGlobal("requestAnimationFrame", requestAnimationFrame);
    vi.stubGlobal("cancelAnimationFrame", cancelAnimationFrame);
    const listener = vi.fn();
    const release = createBrowserAndroidMedia3Runtime().observeViewport(
      video,
      listener,
    );

    try {
      rectangle = viewportRectangle(12, 80, 640, 360);
      scrollContainer.dispatchEvent(new Event("scroll"));
      expect(animationFrames.size).toBe(1);
      runNextAnimationFrame(animationFrames);
      expect(listener).toHaveBeenLastCalledWith({
        left: 12,
        top: 80,
        width: 640,
        height: 360,
        fullscreen: false,
      });

      rectangle = viewportRectangle(12, 120, 640, 360);
      scrollContainer.dispatchEvent(new Event("scroll"));
      expect(animationFrames.size).toBe(1);
      const requestCount = requestAnimationFrame.mock.calls.length;
      release();
      expect(cancelAnimationFrame).toHaveBeenCalledTimes(1);
      expect(animationFrames.size).toBe(0);

      scrollContainer.dispatchEvent(new Event("scroll"));
      expect(requestAnimationFrame).toHaveBeenCalledTimes(requestCount);
      expect(listener).toHaveBeenCalledTimes(1);
    } finally {
      release();
      scrollContainer.remove();
      vi.unstubAllGlobals();
    }
  });
});

function presentationFixture(status: {
  value: AndroidPlaybackStatus;
}): AndroidPlaybackPresentation & {
  readonly status: ReturnType<
    typeof vi.fn<AndroidPlaybackPresentation["status"]>
  >;
  readonly setVolume: ReturnType<
    typeof vi.fn<AndroidPlaybackPresentation["setVolume"]>
  >;
  readonly setMuted: ReturnType<
    typeof vi.fn<AndroidPlaybackPresentation["setMuted"]>
  >;
  readonly setViewport: ReturnType<
    typeof vi.fn<AndroidPlaybackPresentation["setViewport"]>
  >;
  readonly stop: ReturnType<typeof vi.fn<AndroidPlaybackPresentation["stop"]>>;
} {
  return {
    status: vi.fn<AndroidPlaybackPresentation["status"]>(async () =>
      success(status.value),
    ),
    pause: vi.fn<AndroidPlaybackPresentation["pause"]>(async () =>
      success(undefined),
    ),
    resume: vi.fn<AndroidPlaybackPresentation["resume"]>(async () =>
      success(undefined),
    ),
    setVolume: vi.fn<AndroidPlaybackPresentation["setVolume"]>(async () =>
      success(undefined),
    ),
    setMuted: vi.fn<AndroidPlaybackPresentation["setMuted"]>(async () =>
      success(undefined),
    ),
    setViewport: vi.fn<AndroidPlaybackPresentation["setViewport"]>(async () =>
      success(undefined),
    ),
    stop: vi.fn<AndroidPlaybackPresentation["stop"]>(async () =>
      success(undefined),
    ),
  };
}

function runtimeFixture(
  initialViewport: AndroidPlaybackViewport | null = INITIAL_VIEWPORT,
): {
  readonly runtime: AndroidMedia3Runtime;
  readonly released: boolean;
  readonly activeTaskCount: number;
  readonly pushViewport: (viewport: AndroidPlaybackViewport | null) => void;
  readonly runNextTask: () => void;
} {
  const tasks: Array<{ active: boolean; readonly task: () => void }> = [];
  let viewportListener:
    | ((viewport: AndroidPlaybackViewport | null) => void)
    | null = null;
  let released = false;
  return {
    runtime: {
      measureViewport: () => initialViewport,
      observeViewport: (_video, listener) => {
        viewportListener = listener;
        return () => {
          released = true;
          viewportListener = null;
        };
      },
      schedule: (_delayMs, task) => {
        const scheduled = { active: true, task };
        tasks.push(scheduled);
        return () => {
          scheduled.active = false;
        };
      },
    },
    get released() {
      return released;
    },
    get activeTaskCount() {
      return tasks.filter((task) => task.active).length;
    },
    pushViewport: (viewport) => viewportListener?.(viewport),
    runNextTask: () => {
      const next = tasks.shift();
      if (next === undefined || !next.active) {
        throw new Error("expected one active scheduled status poll");
      }
      next.task();
    },
  };
}

function viewportRectangle(
  left: number,
  top: number,
  width: number,
  height: number,
): DOMRect {
  return {
    left,
    top,
    width,
    height,
    right: left + width,
    bottom: top + height,
  } as DOMRect;
}

function runNextAnimationFrame(
  animationFrames: Map<number, FrameRequestCallback>,
): void {
  const next = animationFrames.entries().next().value as
    | [number, FrameRequestCallback]
    | undefined;
  if (next === undefined) {
    throw new Error("expected one scheduled animation frame");
  }
  animationFrames.delete(next[0]);
  next[1](0);
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
