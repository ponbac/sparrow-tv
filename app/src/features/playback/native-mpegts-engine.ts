import mpegts from "mpegts.js";
import type {
  NativePlaybackDescriptor,
} from "../../client/contracts";
import type {
  HostedPlaybackFailure,
  HostedPlaybackHandle,
  MpegtsRuntime,
} from "./mpegts-engine";
import {
  createNativeMpegtsLoader,
  NATIVE_PLAYBACK_SENTINEL,
  type NativeLoaderRuntime,
  type NativePlaybackClient,
} from "./native-mpegts-loader";

/** Inputs accepted by the installed MPEG-TS adapter. */
export interface NativePlaybackRequest {
  readonly client: NativePlaybackClient;
  readonly descriptor: NativePlaybackDescriptor;
  readonly video: HTMLVideoElement;
  readonly onFailure: (failure: HostedPlaybackFailure) => void;
  readonly onAutoplayBlocked: () => void;
}

/** Narrow engine seam used by the installed player and deterministic UI tests. */
export interface NativePlaybackEngine {
  readonly start: (
    request: NativePlaybackRequest,
  ) => HostedPlaybackHandle | HostedPlaybackFailure;
}

export interface NativeMpegtsRuntime
  extends MpegtsRuntime,
    NativeLoaderRuntime {}

/** Builds an installed player around one opaque, Rust-owned stream lease. */
export function createNativeMpegtsPlaybackEngine(
  runtime: NativeMpegtsRuntime = mpegts,
): NativePlaybackEngine {
  return {
    start(request) {
      const releaseSession = createSessionRelease(request);
      if (!runtime.getFeatureList().mseLivePlayback) {
        void releaseSession();
        return "browser-unsupported";
      }

      let active = true;
      let player: ReturnType<MpegtsRuntime["createPlayer"]> | null = null;
      const stop = () => {
        if (!active) {
          return;
        }
        active = false;
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
        void releaseSession();
      };
      const onError = (type: unknown) => {
        if (!active) {
          return;
        }
        const failure: HostedPlaybackFailure =
          type === runtime.ErrorTypes.MEDIA_ERROR
            ? "media-unsupported"
            : "stream-interrupted";
        stop();
        request.onFailure(failure);
      };
      const onLoadingComplete = () => {
        if (!active) {
          return;
        }
        stop();
        request.onFailure("stream-interrupted");
      };

      try {
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
            liveBufferLatencyChasing: true,
            autoCleanupSourceBuffer: true,
            enableWorker: false,
            customLoader: createNativeMpegtsLoader(
              request.client,
              request.descriptor,
              runtime,
              releaseSession,
            ),
          },
        );
        player.on(runtime.Events.ERROR, onError);
        player.on(runtime.Events.LOADING_COMPLETE, onLoadingComplete);
        player.attachMediaElement(request.video);
        player.load();
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
            request.onFailure("media-unsupported");
          });
        }
      } catch {
        stop();
        return "media-unsupported";
      }

      return { stop };
    },
  };
}

/** Shared production instance; session ownership stays inside each start call. */
export const nativeMpegtsPlaybackEngine = createNativeMpegtsPlaybackEngine();

function createSessionRelease({
  client,
  descriptor,
}: Pick<NativePlaybackRequest, "client" | "descriptor">): () => Promise<void> {
  let flight: Promise<void> | null = null;
  return () => {
    if (flight !== null) {
      return flight;
    }
    flight = client
      .stopPlayback({ sessionId: descriptor.sessionId })
      .then(() => undefined)
      .catch(() => undefined);
    return flight;
  };
}

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
