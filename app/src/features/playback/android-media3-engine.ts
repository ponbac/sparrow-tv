import type {
  AndroidPlaybackPresentation,
  AndroidPlaybackViewport,
  InstalledPlaybackSession,
  NativeStreamPlaybackTransport,
} from "../../client/contracts";
import type {
  HostedPlaybackFailure,
  HostedPlaybackHandle,
} from "./mpegts-engine";
import { clientPlaybackFailure } from "./playback-failure";

const STATUS_POLL_INTERVAL_MS = 250;
const MAX_STARTING_STATUS_POLLS = 60;
const MAX_STALLED_PLAYING_POLLS = 40;
const INITIAL_VIEWPORT_WAIT_MS = 15_000;
const MAX_VIEWPORT_VALUE = 32_768;
const HIDDEN_PLAYBACK_VIEWPORT: AndroidPlaybackViewport = Object.freeze({
  left: MAX_VIEWPORT_VALUE,
  top: MAX_VIEWPORT_VALUE,
  width: 1,
  height: 1,
  fullscreen: false,
});

/** Opaque native capability required by the Android Media3 adapter. */
export type AndroidMedia3PlaybackSession = Pick<
  InstalledPlaybackSession,
  "startAndroidPresentation"
>;

/** Inputs accepted by the Android native presentation adapter. */
export interface AndroidMedia3PlaybackRequest {
  readonly session: AndroidMedia3PlaybackSession;
  readonly descriptor: NativeStreamPlaybackTransport;
  readonly video: HTMLVideoElement;
  readonly onFailure: (
    failure: HostedPlaybackFailure,
    retryable: boolean,
  ) => void;
  readonly onAutoplayBlocked: () => void;
  readonly onPlaying: () => void;
}

/** Narrow Android engine seam used by the platform router and focused tests. */
export interface AndroidMedia3PlaybackEngine {
  readonly start: (
    request: AndroidMedia3PlaybackRequest,
  ) => HostedPlaybackHandle | HostedPlaybackFailure;
}

/** Browser facilities kept injectable so native ownership remains deterministic in tests. */
export interface AndroidMedia3Runtime {
  readonly measureViewport: (
    video: HTMLVideoElement,
  ) => AndroidPlaybackViewport | null;
  readonly observeViewport: (
    video: HTMLVideoElement,
    listener: (viewport: AndroidPlaybackViewport | null) => void,
  ) => () => void;
  readonly schedule: (delayMs: number, task: () => void) => () => void;
}

/**
 * Presents one Rust-owned MPEG-TS stream through Android Media3. The WebView
 * supplies only an opaque stream generation, controls, and physical geometry.
 */
export function createAndroidMedia3PlaybackEngine(
  runtime: AndroidMedia3Runtime = createBrowserAndroidMedia3Runtime(),
): AndroidMedia3PlaybackEngine {
  return {
    start(request) {
      const initialViewport = runtime.measureViewport(request.video);
      markPlaybackStatus(request.video, "starting", 0, 0, 0, false);
      let viewport: AndroidPlaybackViewport | null = initialViewport;

      let active = true;
      let startInFlight = false;
      let reportedPlaying = false;
      let startingStatusPolls = 0;
      let stalledPlayingPolls = 0;
      let lastDecodedFrames = 0;
      let presentation: AndroidPlaybackPresentation | null = null;
      let cancelStatusPoll: (() => void) | null = null;
      let cancelViewportWait: (() => void) | null = null;
      let releaseViewport: (() => void) | null = null;
      const startController = new AbortController();
      const stopPresentation = () => {
        const current = presentation;
        presentation = null;
        if (current !== null) {
          void current.stop().catch(() => undefined);
        }
      };
      const cleanup = () => {
        startController.abort();
        cancelStatusPoll?.();
        cancelStatusPoll = null;
        cancelViewportWait?.();
        cancelViewportWait = null;
        releaseViewport?.();
        releaseViewport = null;
        request.video.removeEventListener("volumechange", updateVolume);
        clearPlaybackStatus(request.video);
      };
      const stop = () => {
        if (active) {
          active = false;
          cleanup();
        }
        // Runner-owned teardown calls this only after the Rust transport has
        // suspended/stopped, so exact Media3 release cannot race a blocked read.
        stopPresentation();
      };
      const fail = (failure: HostedPlaybackFailure, retryable: boolean) => {
        if (!active) {
          return;
        }
        active = false;
        cleanup();
        request.onFailure(failure, retryable);
      };
      const failFromClient = (
        result: Extract<
          Awaited<ReturnType<AndroidPlaybackPresentation["status"]>>,
          { readonly ok: false }
        >,
      ) => {
        const classified = clientPlaybackFailure(result.error);
        fail(classified.failure, classified.retryable);
      };
      const observeStatus = () => {
        cancelStatusPoll = runtime.schedule(STATUS_POLL_INTERVAL_MS, () => {
          cancelStatusPoll = null;
          const current = presentation;
          if (!active || current === null) {
            return;
          }
          void current.status().then(
            (result) => {
              if (!active || presentation !== current) {
                return;
              }
              if (!result.ok) {
                failFromClient(result);
                return;
              }
              markPlaybackStatus(
                request.video,
                result.value.state,
                result.value.decodedFrames,
                result.value.droppedFrames,
                result.value.bufferedDurationMs,
                result.value.silent,
              );
              switch (result.value.state) {
                case "playing":
                  startingStatusPolls = 0;
                  if (result.value.decodedFrames > lastDecodedFrames) {
                    lastDecodedFrames = result.value.decodedFrames;
                    stalledPlayingPolls = 0;
                  } else {
                    stalledPlayingPolls += 1;
                  }
                  if (stalledPlayingPolls >= MAX_STALLED_PLAYING_POLLS) {
                    fail("stream-interrupted", true);
                    return;
                  }
                  if (!reportedPlaying && result.value.decodedFrames > 0) {
                    reportedPlaying = true;
                    request.onPlaying();
                  }
                  observeStatus();
                  return;
                case "starting":
                  startingStatusPolls += 1;
                  stalledPlayingPolls = 0;
                  if (startingStatusPolls >= MAX_STARTING_STATUS_POLLS) {
                    fail("stream-interrupted", true);
                    return;
                  }
                  observeStatus();
                  return;
                case "paused":
                  startingStatusPolls = 0;
                  stalledPlayingPolls = 0;
                  observeStatus();
                  return;
                case "failed":
                case "stopped":
                  fail("stream-interrupted", true);
                  return;
              }
            },
            () => fail("source-unavailable", true),
          );
        });
      };
      const applyPresentationViewport = (
        current: AndroidPlaybackPresentation,
        next: AndroidPlaybackViewport | null,
      ) => {
        const effective = next ?? HIDDEN_PLAYBACK_VIEWPORT;
        void current.setViewport(effective).then(
          (result) => {
            if (!result.ok && active && presentation === current) {
              failFromClient(result);
            }
          },
          () => fail("source-unavailable", true),
        );
      };
      const startPresentation = (next: AndroidPlaybackViewport) => {
        if (!active || presentation !== null || startInFlight) {
          return;
        }
        startInFlight = true;
        void request.session
          .startAndroidPresentation({
            streamHandle: request.descriptor.streamHandle,
            viewport: next,
            volume: request.video.volume,
            muted: request.video.muted,
            signal: startController.signal,
          })
          .then(
            (result) => {
              startInFlight = false;
              if (!active) {
                if (result.ok) {
                  void result.value.stop().catch(() => undefined);
                }
                return;
              }
              if (!result.ok) {
                failFromClient(result);
                return;
              }
              presentation = result.value;
              applyPresentationViewport(result.value, viewport);
              updateVolume();
              observeStatus();
            },
            () => {
              startInFlight = false;
              fail("source-unavailable", true);
            },
          );
      };
      const applyViewport = (next: AndroidPlaybackViewport | null) => {
        if (sameViewport(viewport, next)) {
          return;
        }
        viewport = next;
        if (next !== null) {
          cancelViewportWait?.();
          cancelViewportWait = null;
        }
        const current = presentation;
        if (!active) {
          return;
        }
        if (current !== null) {
          applyPresentationViewport(current, next);
        } else if (next !== null) {
          startPresentation(next);
        }
      };
      const updateVolume = () => {
        const current = presentation;
        if (!active || current === null) {
          return;
        }
        const volume = request.video.volume;
        const muted = request.video.muted;
        void current.setVolume(volume).then(
          (result) => {
            if (!result.ok && active && presentation === current) {
              failFromClient(result);
            }
          },
          () => fail("source-unavailable", true),
        );
        void current.setMuted(muted).then(
          (result) => {
            if (!result.ok && active && presentation === current) {
              failFromClient(result);
            }
          },
          () => fail("source-unavailable", true),
        );
      };
      if (initialViewport === null) {
        cancelViewportWait = runtime.schedule(
          INITIAL_VIEWPORT_WAIT_MS,
          () => {
            cancelViewportWait = null;
            if (!active || presentation !== null || startInFlight) {
              return;
            }
            const measured = runtime.measureViewport(request.video);
            if (measured !== null) {
              applyViewport(measured);
              return;
            }
            fail("media-unsupported", false);
          },
        );
      }
      releaseViewport = runtime.observeViewport(
        request.video,
        applyViewport,
      );
      request.video.addEventListener("volumechange", updateVolume);
      if (initialViewport !== null) {
        startPresentation(initialViewport);
      }

      return { stop };
    },
  };
}

/** Shared production instance selected only by Android's platform router. */
export const androidMedia3PlaybackEngine = createAndroidMedia3PlaybackEngine();

function markPlaybackStatus(
  video: HTMLVideoElement,
  state: "starting" | "playing" | "paused" | "failed" | "stopped",
  decodedFrames: number,
  droppedFrames: number,
  bufferedDurationMs: number,
  silent: boolean,
): void {
  video.dataset.playbackEngine = "android-media3";
  video.dataset.playbackState = state;
  video.dataset.decodedFrames = String(decodedFrames);
  video.dataset.droppedFrames = String(droppedFrames);
  video.dataset.bufferedDurationMs = String(bufferedDurationMs);
  video.dataset.processSilent = String(silent);
}

function clearPlaybackStatus(video: HTMLVideoElement): void {
  delete video.dataset.playbackEngine;
  delete video.dataset.playbackState;
  delete video.dataset.decodedFrames;
  delete video.dataset.droppedFrames;
  delete video.dataset.bufferedDurationMs;
  delete video.dataset.processSilent;
}

/** Creates the browser geometry adapter used by Android's native overlay. */
export function createBrowserAndroidMedia3Runtime(): AndroidMedia3Runtime {
  return {
    measureViewport,
    observeViewport(video, listener) {
      let animationFrame: number | null = null;
      const update = () => {
        if (animationFrame !== null) {
          return;
        }
        animationFrame = window.requestAnimationFrame(() => {
          animationFrame = null;
          listener(measureViewport(video));
        });
      };
      const observer =
        typeof ResizeObserver === "undefined"
          ? null
          : new ResizeObserver(update);
      observer?.observe(video);
      if (video.parentElement !== null) {
        observer?.observe(video.parentElement);
      }
      observer?.observe(document.body);
      window.addEventListener("resize", update);
      document.addEventListener("scroll", update, true);
      document.addEventListener("fullscreenchange", update);
      const visualViewport = window.visualViewport;
      visualViewport?.addEventListener("resize", update);
      visualViewport?.addEventListener("scroll", update);
      return () => {
        if (animationFrame !== null) {
          window.cancelAnimationFrame(animationFrame);
          animationFrame = null;
        }
        observer?.disconnect();
        window.removeEventListener("resize", update);
        document.removeEventListener("scroll", update, true);
        document.removeEventListener("fullscreenchange", update);
        visualViewport?.removeEventListener("resize", update);
        visualViewport?.removeEventListener("scroll", update);
      };
    },
    schedule(delayMs, task) {
      const timer = window.setTimeout(task, delayMs);
      return () => window.clearTimeout(timer);
    },
  };
}

function measureViewport(
  video: HTMLVideoElement,
): AndroidPlaybackViewport | null {
  const rectangle = video.getBoundingClientRect();
  const visualViewport = window.visualViewport;
  const viewportLeft = visualViewport?.offsetLeft ?? 0;
  const viewportTop = visualViewport?.offsetTop ?? 0;
  const viewportWidth = visualViewport?.width ?? window.innerWidth;
  const viewportHeight = visualViewport?.height ?? window.innerHeight;
  const viewportRight = viewportLeft + viewportWidth;
  const viewportBottom = viewportTop + viewportHeight;
  const right = Math.min(viewportRight, rectangle.right);
  const bottom = Math.min(viewportBottom, rectangle.bottom);
  const left = Math.max(viewportLeft, rectangle.left);
  const top = Math.max(viewportTop, rectangle.top);
  if (
    ![
      viewportLeft,
      viewportTop,
      viewportWidth,
      viewportHeight,
      left,
      top,
      right,
      bottom,
    ].every(Number.isFinite) ||
    right <= left ||
    bottom <= top
  ) {
    return null;
  }
  const rawScale =
    window.devicePixelRatio * (visualViewport?.scale ?? 1);
  const scale = Number.isFinite(rawScale)
    ? Math.min(8, Math.max(1, rawScale))
    : 1;
  const physical = {
    left: Math.round((left - viewportLeft) * scale),
    top: Math.round((top - viewportTop) * scale),
    width: Math.round((right - left) * scale),
    height: Math.round((bottom - top) * scale),
  };
  if (
    physical.left > MAX_VIEWPORT_VALUE ||
    physical.top > MAX_VIEWPORT_VALUE ||
    physical.width < 1 ||
    physical.width > MAX_VIEWPORT_VALUE ||
    physical.height < 1 ||
    physical.height > MAX_VIEWPORT_VALUE
  ) {
    return null;
  }
  return {
    ...physical,
    fullscreen: document.fullscreenElement === video,
  };
}

function sameViewport(
  left: AndroidPlaybackViewport | null,
  right: AndroidPlaybackViewport | null,
): boolean {
  return (
    left === right ||
    (left !== null &&
      right !== null &&
      left.left === right.left &&
      left.top === right.top &&
      left.width === right.width &&
      left.height === right.height &&
      left.fullscreen === right.fullscreen)
  );
}
