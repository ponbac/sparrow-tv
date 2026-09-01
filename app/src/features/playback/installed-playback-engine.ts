import type {
  ClientError,
  InstalledPlaybackSession,
  InstalledPlaybackTransport,
  MpvPlaybackControl,
} from "../../client/contracts";
import {
  androidMedia3PlaybackEngine,
  type AndroidMedia3PlaybackEngine,
} from "./android-media3-engine";
import type { HostedPlaybackHandle } from "./mpegts-engine";
import {
  nativeMpegtsPlaybackEngine,
  type NativeMpegtsPlaybackEngine,
} from "./native-mpegts-engine";
import { installedClientPlaybackFailure } from "./playback-failure";
import type { InstalledPlaybackFailure } from "./installed-playback-state";

const MPV_HEALTH_CHECK_INTERVAL_MS = 1_000;

/** Inputs shared by every installed platform presentation. */
export interface InstalledPlaybackRequest {
  readonly session: InstalledPlaybackSession;
  readonly descriptor: InstalledPlaybackTransport;
  readonly video: HTMLVideoElement;
  readonly onFailure: (
    failure: InstalledPlaybackFailure,
    retryable: boolean,
  ) => void;
  readonly onAutoplayBlocked: () => void;
  readonly onPlaying: () => void;
}

/** Platform controls layered over deterministic local presentation cleanup. */
export interface InstalledPlaybackHandle extends HostedPlaybackHandle {
  readonly setControls?: (controls: {
    readonly volume: number;
    readonly muted: boolean;
  }) => void;
  readonly requestFullscreen?: (fullscreen: boolean) => Promise<boolean>;
}

/** Deep platform seam selected by an opaque installed descriptor. */
export interface InstalledPlaybackEngine {
  readonly start: (
    request: InstalledPlaybackRequest,
  ) => InstalledPlaybackHandle | InstalledPlaybackFailure;
}

export interface InstalledPlaybackEngineAdapters {
  readonly webviewMse: NativeMpegtsPlaybackEngine;
  readonly androidMedia3: AndroidMedia3PlaybackEngine;
}

/** Routes installed playback without exposing platform branches to the runner. */
export function createInstalledPlaybackEngine(
  adapters: InstalledPlaybackEngineAdapters = {
    webviewMse: nativeMpegtsPlaybackEngine,
    androidMedia3: androidMedia3PlaybackEngine,
  },
): InstalledPlaybackEngine {
  return {
    start(request) {
      const descriptor = request.descriptor;
      if (descriptor._tag === "linux-mpv") {
        return startLinuxMpvPresentation(request);
      }
      switch (descriptor.presentation) {
        case "android-media3":
          return adapters.androidMedia3.start({ ...request, descriptor });
        case "webview-mse":
          return adapters.webviewMse.start({ ...request, descriptor });
      }
    },
  };
}

/** Shared production router; platform descriptors are issued by Rust. */
export const installedPlaybackEngine = createInstalledPlaybackEngine();

function startLinuxMpvPresentation(
  request: InstalledPlaybackRequest,
): InstalledPlaybackHandle {
  let active = true;
  let queue: Promise<void> = Promise.resolve();
  let desiredVolume: number | null = null;
  let desiredMuted: boolean | null = null;
  let healthCheckTimer: ReturnType<typeof setTimeout> | null = null;

  const reportClientFailure = (error: ClientError) => {
    if (!active) {
      return;
    }
    active = false;
    const classified =
      error._tag === "mpv-failed"
        ? installedClientPlaybackFailure(error)
        : {
            failure: "system-player-unavailable" as const,
            retryable: error._tag === "transport" && error.retryable,
          };
    request.onFailure(classified.failure, classified.retryable);
  };
  const reportUnexpectedFailure = () => {
    if (!active) {
      return;
    }
    active = false;
    request.onFailure("system-player-unavailable", true);
  };
  const enqueue = <Result>(operation: () => Promise<Result>): Promise<Result> => {
    const flight = queue.then(operation);
    queue = flight.then(
      () => undefined,
      () => undefined,
    );
    return flight;
  };
  const control = async (command: MpvPlaybackControl): Promise<boolean> => {
    if (!active) {
      return false;
    }
    try {
      const result = await request.session.controlMpv(command);
      if (!active) {
        return false;
      }
      if (!result.ok) {
        reportClientFailure(result.error);
        return false;
      }
      return true;
    } catch {
      reportUnexpectedFailure();
      return false;
    }
  };
  const scheduleHealthCheck = () => {
    if (!active || healthCheckTimer !== null) {
      return;
    }
    healthCheckTimer = setTimeout(() => {
      healthCheckTimer = null;
      if (!active) {
        return;
      }
      void enqueue(async () => {
        if (await control({ _tag: "health-check" })) {
          scheduleHealthCheck();
        }
      });
    }, MPV_HEALTH_CHECK_INTERVAL_MS);
  };

  void Promise.resolve().then(() => {
    if (active) {
      request.onPlaying();
    }
  });
  scheduleHealthCheck();

  return {
    stop() {
      active = false;
      if (healthCheckTimer !== null) {
        clearTimeout(healthCheckTimer);
        healthCheckTimer = null;
      }
    },
    setControls(controls) {
      const volume = clampUnitVolume(controls.volume);
      const muted = controls.muted;
      const volumeChanged = volume !== desiredVolume;
      const mutedChanged = muted !== desiredMuted;
      desiredVolume = volume;
      desiredMuted = muted;
      if (!volumeChanged && !mutedChanged) {
        return;
      }
      void enqueue(async () => {
        if (volumeChanged) {
          const applied = await control({
            _tag: "set-volume",
            percent: Math.round(volume * 100),
          });
          if (!applied) {
            return;
          }
        }
        if (mutedChanged) {
          await control({ _tag: "set-muted", muted });
        }
      });
    },
    requestFullscreen(fullscreen) {
      return enqueue(() =>
        control({ _tag: "set-fullscreen", fullscreen }),
      );
    },
  };
}

function clampUnitVolume(value: number): number {
  return Number.isFinite(value) ? Math.min(1, Math.max(0, value)) : 0;
}
