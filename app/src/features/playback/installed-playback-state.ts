import type {
  AudioPreferenceStatus,
  AudioSelection,
  AudioTrack,
  AudioTrackId,
  ChannelId,
} from "../../client/contracts";
import type { HostedPlaybackFailure } from "./mpegts-engine";
import type { PlayerState } from "./playback-presentation";

/** Minimal Channel intent retained by the installed Playback Session state. */
export interface InstalledPlaybackChannel {
  readonly id: ChannelId;
  readonly name: string;
}

/** Browser-owned controls that survive every native transport recreation. */
export interface InstalledPlaybackControls {
  readonly volume: number;
  readonly muted: boolean;
  readonly fullscreen: boolean;
}

/** Native-discovered audio metadata retained across local engine recreation. */
export interface InstalledPlaybackAudio {
  readonly discovered: boolean;
  readonly tracks: readonly AudioTrack[];
  readonly selection: AudioSelection;
  readonly preferenceStatus: AudioPreferenceStatus | null;
}

/** Safe terminal failure unique to installed resource ownership. */
export type InstalledPlaybackFailure =
  | HostedPlaybackFailure
  | "cleanup-unconfirmed";

/** Why a fresh native transport is being opened inside the current intent. */
export type InstalledPlaybackStartReason =
  | "selection"
  | "resume"
  | "restart"
  | "recovery";

/** Why a transport-free installed Playback Session is paused. */
export type InstalledPlaybackPauseCause = "user" | "visibility";

/** Closed lifecycle phases for exactly one selected installed Channel. */
export type InstalledPlaybackPhase =
  | { readonly _tag: "idle" }
  | {
      readonly _tag: "starting";
      readonly reason: InstalledPlaybackStartReason;
    }
  | { readonly _tag: "playing"; readonly stableSince: number }
  | { readonly _tag: "autoplay-blocked" }
  | {
      readonly _tag: "replacing-audio";
      readonly requestedTrackId: AudioTrackId;
    }
  | {
      readonly _tag: "suspending";
      readonly next:
        | { readonly _tag: "paused"; readonly cause: InstalledPlaybackPauseCause }
        | { readonly _tag: "recovering" }
        | { readonly _tag: "restart" };
    }
  | {
      readonly _tag: "paused";
      readonly cause: InstalledPlaybackPauseCause;
      readonly resumeWhenVisible: boolean;
    }
  | {
      readonly _tag: "recovering";
      readonly attempt: number;
      readonly retryAt: number;
      readonly failure: HostedPlaybackFailure;
    }
  | {
      readonly _tag: "failed";
      readonly failure: InstalledPlaybackFailure;
      readonly attemptsUsed: number;
      readonly canRestart: boolean;
    }
  | {
      readonly _tag: "stopping";
      readonly nextChannel: InstalledPlaybackChannel | null;
    };

/** Complete immutable state consumed by React and copied diagnostics. */
export interface InstalledPlaybackState {
  readonly phase: InstalledPlaybackPhase;
  readonly channel: InstalledPlaybackChannel | null;
  readonly sessionEpoch: number;
  readonly transportEpoch: number;
  readonly recoveryCount: number;
  readonly visible: boolean;
  readonly controls: InstalledPlaybackControls;
  readonly audio: InstalledPlaybackAudio;
}

/** Closed event algebra accepted by the pure installed playback reducer. */
export type InstalledPlaybackEvent =
  | {
      readonly _tag: "select";
      readonly channel: InstalledPlaybackChannel;
      readonly sessionEpoch: number;
      readonly transportEpoch: number;
    }
  | {
      readonly _tag: "stopping";
      readonly nextChannel: InstalledPlaybackChannel | null;
      readonly sessionEpoch: number;
      readonly transportEpoch: number;
    }
  | {
      readonly _tag: "starting";
      readonly reason: InstalledPlaybackStartReason;
      readonly transportEpoch: number;
    }
  | {
      readonly _tag: "replacing-audio";
      readonly requestedTrackId: AudioTrackId;
      readonly transportEpoch: number;
    }
  | {
      readonly _tag: "transport-opened";
      readonly tracks: readonly AudioTrack[];
      readonly selection: AudioSelection;
      readonly preferenceStatus?: AudioPreferenceStatus;
    }
  | { readonly _tag: "playing"; readonly now: number }
  | { readonly _tag: "autoplay-blocked" }
  | {
      readonly _tag: "suspending";
      readonly next: Extract<InstalledPlaybackPhase, { readonly _tag: "suspending" }>["next"];
      readonly transportEpoch: number;
    }
  | {
      readonly _tag: "paused";
      readonly cause: InstalledPlaybackPauseCause;
      readonly resumeWhenVisible: boolean;
    }
  | {
      readonly _tag: "recovering";
      readonly attempt: number;
      readonly retryAt: number;
      readonly failure: HostedPlaybackFailure;
    }
  | {
      readonly _tag: "failed";
      readonly failure: InstalledPlaybackFailure;
      readonly attemptsUsed: number;
      readonly canRestart: boolean;
    }
  | { readonly _tag: "stopped" }
  | { readonly _tag: "stable" }
  | { readonly _tag: "visibility"; readonly visible: boolean }
  | { readonly _tag: "volume"; readonly volume: number }
  | { readonly _tag: "muted"; readonly muted: boolean }
  | { readonly _tag: "fullscreen"; readonly fullscreen: boolean };

/** Default audible browser controls for a newly-created player runner. */
export const INITIAL_INSTALLED_PLAYBACK_CONTROLS: InstalledPlaybackControls =
  Object.freeze({ volume: 1, muted: false, fullscreen: false });

/** Empty audio state before native programme metadata has been discovered. */
export const INITIAL_INSTALLED_PLAYBACK_AUDIO: InstalledPlaybackAudio =
  Object.freeze({
    discovered: false,
    tracks: Object.freeze([]),
    selection: Object.freeze({ _tag: "none" }),
    preferenceStatus: null,
  });

/** Creates the transport-free initial state without acquiring any resource. */
export function createInstalledPlaybackState(
  visible = true,
): InstalledPlaybackState {
  return {
    phase: { _tag: "idle" },
    channel: null,
    sessionEpoch: 0,
    transportEpoch: 0,
    recoveryCount: 0,
    visible,
    controls: INITIAL_INSTALLED_PLAYBACK_CONTROLS,
    audio: INITIAL_INSTALLED_PLAYBACK_AUDIO,
  };
}

/** Applies one exhaustive, side-effect-free Playback Session transition. */
export function reduceInstalledPlaybackState(
  state: InstalledPlaybackState,
  event: InstalledPlaybackEvent,
): InstalledPlaybackState {
  switch (event._tag) {
    case "select":
      return {
        ...state,
        phase: { _tag: "starting", reason: "selection" },
        channel: event.channel,
        sessionEpoch: event.sessionEpoch,
        transportEpoch: event.transportEpoch,
        recoveryCount: 0,
        audio: INITIAL_INSTALLED_PLAYBACK_AUDIO,
      };
    case "stopping":
      return {
        ...state,
        phase: { _tag: "stopping", nextChannel: event.nextChannel },
        sessionEpoch: event.sessionEpoch,
        transportEpoch: event.transportEpoch,
      };
    case "starting":
      requireSelectedChannel(state);
      return {
        ...state,
        phase: { _tag: "starting", reason: event.reason },
        transportEpoch: event.transportEpoch,
      };
    case "replacing-audio":
      requireSelectedChannel(state);
      return {
        ...state,
        phase: {
          _tag: "replacing-audio",
          requestedTrackId: event.requestedTrackId,
        },
        transportEpoch: event.transportEpoch,
      };
    case "transport-opened":
      requireSelectedChannel(state);
      return {
        ...state,
        audio: {
          discovered: true,
          tracks: event.tracks,
          selection: event.selection,
          preferenceStatus:
            event.preferenceStatus ?? state.audio.preferenceStatus,
        },
      };
    case "playing":
      requireSelectedChannel(state);
      return { ...state, phase: { _tag: "playing", stableSince: event.now } };
    case "autoplay-blocked":
      requireSelectedChannel(state);
      return { ...state, phase: { _tag: "autoplay-blocked" } };
    case "suspending":
      requireSelectedChannel(state);
      return {
        ...state,
        phase: { _tag: "suspending", next: event.next },
        transportEpoch: event.transportEpoch,
      };
    case "paused":
      requireSelectedChannel(state);
      return {
        ...state,
        phase: {
          _tag: "paused",
          cause: event.cause,
          resumeWhenVisible: event.resumeWhenVisible,
        },
      };
    case "recovering":
      requireSelectedChannel(state);
      return {
        ...state,
        phase: {
          _tag: "recovering",
          attempt: event.attempt,
          retryAt: event.retryAt,
          failure: event.failure,
        },
        recoveryCount: event.attempt,
      };
    case "failed":
      requireSelectedChannel(state);
      return {
        ...state,
        phase: {
          _tag: "failed",
          failure: event.failure,
          attemptsUsed: event.attemptsUsed,
          canRestart: event.canRestart,
        },
      };
    case "stopped":
      return {
        ...state,
        phase: { _tag: "idle" },
        channel: null,
        recoveryCount: 0,
        controls: { ...state.controls, fullscreen: false },
        audio: INITIAL_INSTALLED_PLAYBACK_AUDIO,
      };
    case "stable":
      return { ...state, recoveryCount: 0 };
    case "visibility":
      return { ...state, visible: event.visible };
    case "volume":
      return {
        ...state,
        controls: { ...state.controls, volume: clampVolume(event.volume) },
      };
    case "muted":
      return {
        ...state,
        controls: { ...state.controls, muted: event.muted },
      };
    case "fullscreen":
      return {
        ...state,
        controls: { ...state.controls, fullscreen: event.fullscreen },
      };
  }
}

/** Projects the richer installed phase into the shared playback chrome state. */
export function installedPlayerState(
  state: InstalledPlaybackState,
): PlayerState {
  switch (state.phase._tag) {
    case "idle":
      return { _tag: "starting" };
    case "starting":
      return { _tag: "starting" };
    case "playing":
      return { _tag: "playing" };
    case "autoplay-blocked":
      return { _tag: "autoplay-blocked" };
    case "replacing-audio":
      return { _tag: "starting" };
    case "suspending":
      return { _tag: "suspending" };
    case "paused":
      return { _tag: "paused" };
    case "recovering":
      return {
        _tag: "recovering",
        attempt: state.phase.attempt,
        failure: state.phase.failure,
      };
    case "failed":
      return {
        _tag: "failed",
        failure: state.phase.failure,
        retryable: state.phase.canRestart,
      };
    case "stopping":
      return { _tag: "stopping" };
  }
}

function requireSelectedChannel(
  state: InstalledPlaybackState,
): InstalledPlaybackChannel {
  if (state.channel === null) {
    throw new Error("A selected Channel is required for this playback transition.");
  }
  return state.channel;
}

function clampVolume(value: number): number {
  if (!Number.isFinite(value)) {
    return 0;
  }
  return Math.min(1, Math.max(0, value));
}
