import type {
  InstalledPlaybackFailure,
  InstalledPlaybackPhase,
  InstalledPlaybackState,
} from "./installed-playback-state";
import type { NativeLiveMediaObservation } from "./native-live-buffer";

const MAX_TRANSITIONS = 20;
const MAX_DURATION_MS = 86_400_000;
const MAX_MEDIA_COUNT = 99_999;
const MAX_MEDIA_DIMENSION = 7_680;
const MAX_MEDIA_FRAMES = 10_000_000;
const MAX_READY_STATE = 4;

/** Safe phase-only transition retained by the playback runner. */
export interface InstalledPlaybackTransition {
  readonly from: InstalledPlaybackPhase["_tag"];
  readonly to: InstalledPlaybackPhase["_tag"];
}

/** Allowlisted numeric media counters copied with playback diagnostics. */
export interface InstalledPlaybackMediaReport {
  readonly readyState: number;
  readonly paused: boolean;
  readonly seeking: boolean;
  readonly currentTimeMs: number;
  readonly bufferAheadMs: number;
  readonly bufferedRangeCount: number;
  readonly waiting: number;
  readonly stalledEvents: number;
  readonly seekingEvents: number;
  readonly seeked: number;
  readonly standstills: number;
  readonly msSinceTimeAdvance: number;
  readonly width: number;
  readonly height: number;
  readonly totalVideoFrames: number;
  readonly droppedVideoFrames: number;
  readonly presentedFrames: number;
}

/** Minimal clipboard seam used by the installed diagnostics control. */
export interface PlaybackDiagnosticsClipboard {
  readonly writeText: (text: string) => Promise<void>;
}

/**
 * Creates bounded copyable JSON without Channel, session, handle, provider, or
 * arbitrary exception data.
 */
export function installedPlaybackDiagnostics(
  state: InstalledPlaybackState,
  transitions: readonly InstalledPlaybackTransition[],
  now: number,
  media: NativeLiveMediaObservation | null = null,
): string {
  return JSON.stringify({
    version: 2,
    engine: playbackEngine(state),
    phase: state.phase._tag,
    intent: safeIntent(state.phase),
    transport:
      state.presentation === "linux-mpv"
        ? "private-mpv-ipc"
        : "tauri-native-stream",
    failure: safeFailure(state.phase),
    recoveryCount: boundedInteger(state.recoveryCount, 99),
    playingDurationMs:
      state.phase._tag === "playing"
        ? boundedInteger(now - state.phase.stableSince, MAX_DURATION_MS)
        : 0,
    controls: {
      volumePercent: Math.round(state.controls.volume * 100),
      muted: state.controls.muted,
      fullscreen: state.controls.fullscreen,
    },
    audio: {
      trackCount: boundedInteger(state.audio.tracks.length, 32),
      selection: safeAudioSelection(state),
      preferenceStatus: state.audio.preferenceStatus ?? "none",
    },
    media: projectMedia(media),
    transitions: transitions.slice(-MAX_TRANSITIONS).map((transition) => ({
      from: transition.from,
      to: transition.to,
    })),
  });
}

/** Writes the already-redacted bounded diagnostics projection to a clipboard. */
export function copyInstalledPlaybackDiagnostics(
  clipboard: PlaybackDiagnosticsClipboard,
  state: InstalledPlaybackState,
  transitions: readonly InstalledPlaybackTransition[],
  now: number,
  media: NativeLiveMediaObservation | null = null,
): Promise<void> {
  return clipboard.writeText(
    installedPlaybackDiagnostics(state, transitions, now, media),
  );
}

/** Projects a live media observation into the copyable allowlist. */
export function projectInstalledPlaybackMedia(
  media: NativeLiveMediaObservation,
): InstalledPlaybackMediaReport {
  return {
    readyState: boundedInteger(media.readyState, MAX_READY_STATE),
    paused: media.paused,
    seeking: media.seeking,
    currentTimeMs: boundedInteger(media.currentTime * 1_000, MAX_DURATION_MS),
    bufferAheadMs: boundedInteger(media.bufferAhead * 1_000, MAX_DURATION_MS),
    bufferedRangeCount: boundedInteger(media.bufferedRangeCount, 32),
    waiting: boundedInteger(media.waiting, MAX_MEDIA_COUNT),
    stalledEvents: boundedInteger(media.stalledEvents, MAX_MEDIA_COUNT),
    seekingEvents: boundedInteger(media.seekingEvents, MAX_MEDIA_COUNT),
    seeked: boundedInteger(media.seeked, MAX_MEDIA_COUNT),
    standstills: boundedInteger(media.standstills, MAX_MEDIA_COUNT),
    msSinceTimeAdvance: boundedInteger(
      media.msSinceTimeAdvance,
      MAX_DURATION_MS,
    ),
    width: boundedInteger(media.width, MAX_MEDIA_DIMENSION),
    height: boundedInteger(media.height, MAX_MEDIA_DIMENSION),
    totalVideoFrames: boundedInteger(media.totalVideoFrames, MAX_MEDIA_FRAMES),
    droppedVideoFrames: boundedInteger(
      media.droppedVideoFrames,
      MAX_MEDIA_FRAMES,
    ),
    presentedFrames: boundedInteger(media.presentedFrames, MAX_MEDIA_FRAMES),
  };
}

function projectMedia(
  media: NativeLiveMediaObservation | null,
): InstalledPlaybackMediaReport | null {
  return media === null ? null : projectInstalledPlaybackMedia(media);
}

function safeIntent(phase: InstalledPlaybackPhase): string {
  switch (phase._tag) {
    case "idle":
    case "playing":
    case "autoplay-blocked":
    case "failed":
    case "stopping":
      return phase._tag;
    case "starting":
      return phase.reason;
    case "replacing-audio":
      return "audio-selection";
    case "suspending":
      return phase.next._tag;
    case "paused":
      return phase.cause;
    case "recovering":
      return "automatic-recovery";
  }
}

function safeFailure(
  phase: InstalledPlaybackPhase,
): InstalledPlaybackFailure | null {
  switch (phase._tag) {
    case "recovering":
    case "failed":
      return phase.failure;
    case "idle":
    case "starting":
    case "playing":
    case "autoplay-blocked":
    case "replacing-audio":
    case "suspending":
    case "paused":
    case "stopping":
      return null;
  }
}

function safeAudioSelection(state: InstalledPlaybackState): string {
  switch (state.audio.selection._tag) {
    case "none":
      return "none";
    case "selected":
      return state.audio.selection.reason;
    case "fallback":
      return state.audio.selection.missing + "-fallback";
  }
}

function playbackEngine(state: InstalledPlaybackState): string {
  switch (state.presentation) {
    case "android-media3":
      return "android-media3";
    case "linux-mpv":
      return "mpv-system";
    case "webview-mse":
    case null:
      return "mpegts-native";
  }
}
function boundedInteger(value: number, maximum: number): number {
  if (!Number.isFinite(value)) {
    return 0;
  }
  return Math.max(0, Math.min(maximum, Math.round(value)));
}
