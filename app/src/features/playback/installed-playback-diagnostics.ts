import type {
  InstalledPlaybackFailure,
  InstalledPlaybackPhase,
  InstalledPlaybackState,
  MpvFallbackFailure,
} from "./installed-playback-state";

const MAX_TRANSITIONS = 20;
const MAX_DURATION_MS = 86_400_000;

/** Safe phase-only transition retained by the playback runner. */
export interface InstalledPlaybackTransition {
  readonly from: InstalledPlaybackPhase["_tag"];
  readonly to: InstalledPlaybackPhase["_tag"];
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
): string {
  return JSON.stringify({
    version: 1,
    engine: playbackEngine(state.phase),
    phase: state.phase._tag,
    intent: safeIntent(state.phase),
    transport: "tauri-native-stream",
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
): Promise<void> {
  return clipboard.writeText(
    installedPlaybackDiagnostics(state, transitions, now),
  );
}

function safeIntent(phase: InstalledPlaybackPhase): string {
  switch (phase._tag) {
    case "idle":
    case "playing":
    case "autoplay-blocked":
    case "failed":
    case "stopping":
      return phase._tag;
    case "primary-stopped":
      return "manual-failover-ready";
    case "fallback-starting":
      return "manual-failover-start";
    case "fallback-playing":
      return "manual-failover-playing";
    case "fallback-stop-failed":
      return "manual-failover-stop";
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
): InstalledPlaybackFailure | MpvFallbackFailure["reason"] | null {
  switch (phase._tag) {
    case "recovering":
    case "failed":
      return phase.failure;
    case "primary-stopped":
      return phase.fallbackFailure?.reason ?? null;
    case "fallback-stop-failed":
      return phase.failure.reason;
    case "idle":
    case "starting":
    case "playing":
    case "autoplay-blocked":
    case "replacing-audio":
    case "suspending":
    case "paused":
    case "stopping":
    case "fallback-starting":
    case "fallback-playing":
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

function playbackEngine(phase: InstalledPlaybackPhase): string {
  switch (phase._tag) {
    case "fallback-starting":
    case "fallback-playing":
    case "fallback-stop-failed":
      return "mpv-system";
    case "idle":
    case "starting":
    case "playing":
    case "autoplay-blocked":
    case "replacing-audio":
    case "suspending":
    case "paused":
    case "recovering":
    case "failed":
    case "primary-stopped":
    case "stopping":
      return "mpegts-native";
  }
}
function boundedInteger(value: number, maximum: number): number {
  if (!Number.isFinite(value)) {
    return 0;
  }
  return Math.max(0, Math.min(maximum, Math.round(value)));
}
