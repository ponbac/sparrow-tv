import mpegts from "mpegts.js";
import {
  watchNativeLiveBuffer,
  type NativeLiveMediaObservation,
} from "./native-live-buffer";
import type { NativeStreamPlaybackTransport } from "../../client/contracts";
import type {
  HostedPlaybackFailure,
  HostedPlaybackHandle,
  MpegtsRuntime,
} from "./mpegts-engine";
import { clientPlaybackFailure } from "./playback-failure";
import {
  createNativeMpegtsLoader,
  NATIVE_PLAYBACK_SENTINEL,
  type NativeLoaderRuntime,
  type NativePlaybackClient,
} from "./native-mpegts-loader";
import { adoptLiveMpegtsIo } from "./native-mpegts-live-io";

/** Inputs accepted by the installed MPEG-TS adapter. */
export interface NativeMpegtsPlaybackRequest {
  readonly session: NativePlaybackClient;
  readonly descriptor: NativeStreamPlaybackTransport;
  readonly video: HTMLVideoElement;
  readonly onFailure: (
    failure: HostedPlaybackFailure,
    retryable: boolean,
  ) => void;
  readonly onAutoplayBlocked: () => void;
  readonly onPlaying: () => void;
}

export interface NativeMpegtsPlaybackHandle extends HostedPlaybackHandle {
  readonly mediaSnapshot?: () => NativeLiveMediaObservation | undefined;
}

/** Narrow engine seam used by the installed player and deterministic UI tests. */
export interface NativeMpegtsPlaybackEngine {
  readonly start: (
    request: NativeMpegtsPlaybackRequest,
  ) => NativeMpegtsPlaybackHandle | HostedPlaybackFailure;
}

export interface NativeMpegtsRuntime
  extends MpegtsRuntime,
    NativeLoaderRuntime {}

/** Builds an installed player around one opaque, Rust-owned stream lease. */
export function createNativeMpegtsPlaybackEngine(
  runtime: NativeMpegtsRuntime = mpegts,
): NativeMpegtsPlaybackEngine {
  return {
    start(request) {
      if (!runtime.getFeatureList().mseLivePlayback) {
        return "browser-unsupported";
      }

      let active = true;
      let mediaWatch: ReturnType<typeof watchNativeLiveBuffer> | null = null;
      let liveIoStop: () => void = () => undefined;
      let player: ReturnType<MpegtsRuntime["createPlayer"]> | null = null;
      let readFailure: {
        readonly failure: HostedPlaybackFailure;
        readonly retryable: boolean;
      } | null = null;
      const trackedSession: NativePlaybackClient = {
        read: async (input) => {
          const result = await request.session.read(input);
          if (!result.ok && result.error._tag !== "cancelled") {
            readFailure = clientPlaybackFailure(result.error);
          }
          return result;
        },
      };
      const stop = () => {
        if (!active) {
          return;
        }
        active = false;
        liveIoStop();
        liveIoStop = () => undefined;
        mediaWatch?.stop();
        mediaWatch = null;
        const current = player;
        player = null;
        if (current !== null) {
          safely(() => current.off(runtime.Events.ERROR, onError));
          safely(() =>
            current.off(runtime.Events.LOADING_COMPLETE, onLoadingComplete),
          );
          safely(() => current.pause());
          safely(() => current.unload());
          safely(() => current.detachMediaElement());
          safely(() => current.destroy());
        }
        request.video.removeEventListener("playing", request.onPlaying);
      };
      const onError = (type: unknown) => {
        if (!active) {
          return;
        }
        const classified =
          type === runtime.ErrorTypes.MEDIA_ERROR
            ? { failure: "media-unsupported" as const, retryable: false }
            : (readFailure ?? {
                failure: "stream-interrupted" as const,
                retryable: true,
              });
        stop();
        request.onFailure(classified.failure, classified.retryable);
      };
      const onLoadingComplete = () => {
        if (!active) {
          return;
        }
        stop();
        request.onFailure("stream-interrupted", true);
      };

      try {
        request.video.addEventListener("playing", request.onPlaying);
        player = runtime.createPlayer(
          {
            type: "mpegts",
            url: NATIVE_PLAYBACK_SENTINEL,
            isLive: true,
            cors: false,
            withCredentials: false,
          },
          {
            isLive: true,
            enableStashBuffer: false,
            lazyLoad: false,
            // Mid-stream MSE seeks stall WebKitGTK; do not chase live latency.
            liveBufferLatencyChasing: false,
            autoCleanupSourceBuffer: true,
            enableWorker: false,
            // Transmuxer must exist before we adapt native IO pause/resume.
            deferLoadAfterSourceOpen: false,
            // BUFFER_FULL pauses IO. The library default (30s) treats any
            // shorter live buffer as already recoverable, so it immediately
            // resumes and wedges WebKitGTK SourceBuffers.
            lazyLoadRecoverDuration: 3,
            customLoader: createNativeMpegtsLoader(
              trackedSession,
              request.descriptor,
              runtime,
            ),
          },
        );
        player.on(runtime.Events.ERROR, onError);
        player.on(runtime.Events.LOADING_COMPLETE, onLoadingComplete);
        player.attachMediaElement(request.video);
        player.load();
        // load() must create the transmuxer before this runs. mpegts.js otherwise
        // defers until MediaSource `sourceopen`, and BUFFER_FULL still HTTP-seeks.
        liveIoStop = adoptLiveMpegtsIo(player, request.video).stop;
        mediaWatch = watchNativeLiveBuffer(request.video);
        const play = player.play();
        if (play !== undefined) {
          void Promise.resolve(play).catch((error: unknown) => {
            if (!active) {
              return;
            }
            if (isAutoplayRejection(error)) {
              request.onAutoplayBlocked();
              return;
            }
            stop();
            request.onFailure("media-unsupported", false);
          });
        }
      } catch {
        stop();
        return "media-unsupported";
      }

      return {
        stop,
        mediaSnapshot: () => mediaWatch?.snapshot(),
      };
    },
  };
}

/** Shared production instance; the Playback Session runner owns final cleanup. */
export const nativeMpegtsPlaybackEngine = createNativeMpegtsPlaybackEngine();

function isAutoplayRejection(error: unknown): boolean {
  return error instanceof Error && error.name === "NotAllowedError";
}

function safely(operation: () => void): void {
  try {
    operation();
  } catch {
    // Media teardown is best-effort across partially initialized WebViews.
  }
}
