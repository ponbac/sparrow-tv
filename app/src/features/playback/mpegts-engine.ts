import mpegts from "mpegts.js";
import type { SameOriginPlaybackEndpoint } from "../../client/contracts";
import type { NativeLoaderConstructor } from "./native-mpegts-loader";

/** Safe terminal failures emitted by the hosted MPEG-TS adapter. */
export type HostedPlaybackFailure =
  | "authentication-required"
  | "channel-not-found"
  | "source-rejected"
  | "source-invalid"
  | "source-unavailable"
  | "source-timeout"
  | "stream-interrupted"
  | "media-unsupported"
  | "browser-unsupported";

/** One live player resource. Stop is idempotent and releases its HTTP request. */
export interface HostedPlaybackHandle {
  readonly stop: () => void;
}

/** Inputs accepted by the browser playback adapter. */
export interface HostedPlaybackRequest {
  readonly endpoint: SameOriginPlaybackEndpoint;
  readonly video: HTMLVideoElement;
  readonly onFailure: (failure: HostedPlaybackFailure) => void;
  readonly onAutoplayBlocked: () => void;
}

/** Narrow engine seam used by the hosted player and deterministic UI tests. */
export interface HostedPlaybackEngine {
  readonly start: (
    request: HostedPlaybackRequest,
  ) => HostedPlaybackHandle | HostedPlaybackFailure;
}

interface EnginePlayer {
  readonly on: (event: string, listener: (...args: unknown[]) => void) => void;
  readonly off: (event: string, listener: (...args: unknown[]) => void) => void;
  readonly attachMediaElement: (video: HTMLMediaElement) => void;
  readonly detachMediaElement: () => void;
  readonly load: () => void;
  readonly unload: () => void;
  readonly play: () => Promise<void> | void;
  readonly pause: () => void;
  readonly destroy: () => void;
}

/** Minimal mpegts.js surface consumed by the hosted adapter. */
export interface MpegtsRuntime {
  readonly createPlayer: (
    source: {
      readonly type: string;
      readonly url: string;
      readonly isLive: boolean;
      readonly cors: boolean;
      readonly withCredentials: boolean;
    },
    config: {
      readonly isLive: boolean;
      readonly enableStashBuffer: boolean;
      readonly lazyLoad: boolean;
      readonly liveBufferLatencyChasing: boolean;
      readonly autoCleanupSourceBuffer: boolean;
      readonly enableWorker?: false;
      readonly customLoader?: NativeLoaderConstructor;
    },
  ) => EnginePlayer;
  readonly getFeatureList: () => { readonly mseLivePlayback: boolean };
  readonly Events: {
    readonly ERROR: string;
    readonly LOADING_COMPLETE: string;
  };
  readonly ErrorTypes: {
    readonly NETWORK_ERROR: string;
    readonly MEDIA_ERROR: string;
  };
  readonly ErrorDetails: {
    readonly NETWORK_STATUS_CODE_INVALID: string;
    readonly NETWORK_TIMEOUT: string;
  };
}

/** Builds the production browser adapter around an injectable mpegts.js runtime. */
export function createMpegtsPlaybackEngine(
  runtime: MpegtsRuntime = mpegts,
): HostedPlaybackEngine {
  return {
    start(request) {
      if (!runtime.getFeatureList().mseLivePlayback) {
        return "browser-unsupported";
      }

      let active = true;
      let player: EnginePlayer | null = null;
      const onError = (type: unknown, detail: unknown, info: unknown) => {
        if (!active) {
          return;
        }
        const failure = classifyMpegtsFailure(runtime, type, detail, info);
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
      const stop = () => {
        if (!active) {
          return;
        }
        active = false;
        const current = player;
        player = null;
        if (current === null) {
          return;
        }
        safely(() => current.off(runtime.Events.ERROR, onError));
        safely(() =>
          current.off(runtime.Events.LOADING_COMPLETE, onLoadingComplete),
        );
        safely(() => current.pause());
        safely(() => current.unload());
        safely(() => current.detachMediaElement());
        safely(() => current.destroy());
      };

      try {
        player = runtime.createPlayer(
          {
            type: "mpegts",
            url: request.endpoint,
            isLive: true,
            cors: false,
            withCredentials: true,
          },
          {
            isLive: true,
            enableStashBuffer: false,
            lazyLoad: false,
            liveBufferLatencyChasing: true,
            autoCleanupSourceBuffer: true,
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

/** Shared production instance; it holds no session or provider state. */
export const mpegtsPlaybackEngine = createMpegtsPlaybackEngine();

function classifyMpegtsFailure(
  runtime: MpegtsRuntime,
  type: unknown,
  detail: unknown,
  info: unknown,
): HostedPlaybackFailure {
  if (type === runtime.ErrorTypes.MEDIA_ERROR) {
    return "media-unsupported";
  }
  if (type !== runtime.ErrorTypes.NETWORK_ERROR) {
    return "stream-interrupted";
  }

  if (detail === runtime.ErrorDetails.NETWORK_STATUS_CODE_INVALID) {
    switch (safeStatusCode(info)) {
      case 401:
        return "authentication-required";
      case 404:
        return "channel-not-found";
      case 424:
        return "source-rejected";
      case 502:
        return "source-invalid";
      case 503:
        return "source-unavailable";
      case 504:
        return "source-timeout";
      default:
        return "source-unavailable";
    }
  }
  if (detail === runtime.ErrorDetails.NETWORK_TIMEOUT) {
    return "source-timeout";
  }
  return "stream-interrupted";
}

function safeStatusCode(info: unknown): number | null {
  if (typeof info !== "object" || info === null || !("code" in info)) {
    return null;
  }
  const code = info.code;
  return typeof code === "number" && Number.isInteger(code) ? code : null;
}

function isAutoplayRejection(error: unknown): boolean {
  return error instanceof Error && error.name === "NotAllowedError";
}

function safely(operation: () => void): void {
  try {
    operation();
  } catch {
    // Cleanup is best-effort across partially initialized browser media APIs.
  }
}
