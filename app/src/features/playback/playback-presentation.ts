import type { HostedPlaybackFailure } from "./mpegts-engine";
import type { SystemPlayerFailure } from "./playback-failure";

/** Safe failure vocabulary renderable by the shared playback chrome. */
export type PlaybackFailure =
  | HostedPlaybackFailure
  | SystemPlayerFailure
  | "cleanup-unconfirmed";

/** User-visible state shared by hosted and installed live playback transports. */
export type PlayerState =
  | { readonly _tag: "starting" }
  | { readonly _tag: "playing" }
  | { readonly _tag: "autoplay-blocked" }
  | { readonly _tag: "suspending" }
  | { readonly _tag: "paused" }
  | {
      readonly _tag: "recovering";
      readonly attempt: number;
      readonly failure: PlaybackFailure;
    }
  | { readonly _tag: "stopping" }
  | {
      readonly _tag: "failed";
      readonly failure: PlaybackFailure;
      readonly retryable: boolean;
    };

/** Produces safe status copy for every closed player state. */
export function playerPresentation(state: PlayerState): {
  readonly status: string;
  readonly title: string;
  readonly detail: string;
} {
  switch (state._tag) {
    case "starting":
      return {
        status: "TUNING",
        title: "Opening the live signal",
        detail: "Sparrow is connecting this Channel to the monitor.",
      };
    case "playing":
      return {
        status: "ON AIR",
        title: "Live signal",
        detail: "The selected Channel is playing.",
      };
    case "autoplay-blocked":
      return {
        status: "READY",
        title: "The signal is ready",
        detail: "Your browser needs one more gesture before playing sound.",
      };
    case "suspending":
      return {
        status: "RELEASING",
        title: "Releasing the live signal",
        detail: "Sparrow is confirming that transport work has stopped.",
      };
    case "paused":
      return {
        status: "PAUSED",
        title: "Live playback is paused",
        detail: "The provider request is released. Resume to return at the live edge.",
      };
    case "recovering":
      return {
        status: "RECONNECTING",
        title: `Recovery attempt ${state.attempt}`,
        detail: "The prior request is released before Sparrow reconnects.",
      };
    case "stopping":
      return {
        status: "STOPPING",
        title: "Closing the Playback Session",
        detail: "Sparrow is confirming final resource cleanup.",
      };
    case "failed":
      return failurePresentation(state.failure, state.retryable);
  }
}

/** Returns whether a common failure may be retried by its owning policy. */
export function isRetryable(failure: PlaybackFailure): boolean {
  switch (failure) {
    case "authentication-required":
    case "source-unavailable":
    case "source-timeout":
    case "stream-interrupted":
    case "system-player-unavailable":
      return true;
    case "channel-not-found":
    case "source-rejected":
    case "source-invalid":
    case "media-unsupported":
    case "browser-unsupported":
    case "system-player-missing":
    case "system-player-incompatible":
    case "cleanup-unconfirmed":
      return false;
  }
}

/** Returns the hosted manual-retry label for a safe playback failure. */
export function retryLabel(failure: PlaybackFailure): string {
  switch (failure) {
    case "authentication-required":
      return "Try after authentication";
    case "stream-interrupted":
      return "Reconnect signal";
    case "source-unavailable":
    case "source-timeout":
      return "Try signal again";
    case "system-player-unavailable":
      return "Try system player again";
    case "channel-not-found":
    case "source-rejected":
    case "source-invalid":
    case "media-unsupported":
    case "browser-unsupported":
    case "system-player-missing":
    case "system-player-incompatible":
    case "cleanup-unconfirmed":
      return "";
  }
}

function failurePresentation(
  failure: PlaybackFailure,
  retryable: boolean,
): {
  readonly status: string;
  readonly title: string;
  readonly detail: string;
} {
  switch (failure) {
    case "authentication-required":
      return {
        status: "ACCESS NEEDED",
        title: "Playback needs authentication",
        detail: "Authenticate with this Sparrow deployment, then try the signal again.",
      };
    case "channel-not-found":
      return {
        status: "CHANNEL GONE",
        title: "That Channel left the catalog",
        detail: "Choose a Channel from the current catalog generation.",
      };
    case "source-rejected":
      return {
        status: "SOURCE REJECTED",
        title: "The provider refused this signal",
        detail: "Choose another Channel or refresh the source configuration.",
      };
    case "source-invalid":
      return {
        status: "INVALID SIGNAL",
        title: "The provider returned an invalid signal",
        detail: "Choose another Channel; retrying this response will not repair it.",
      };
    case "source-timeout":
      return {
        status: "SOURCE TIMEOUT",
        title: "The signal took too long to answer",
        detail: "Retry the Channel when the provider is responsive.",
      };
    case "source-unavailable":
      return {
        status: "SOURCE OFFLINE",
        title: "The live signal is unavailable",
        detail: retryable
          ? "Retry this Channel or choose another signal."
          : "Choose another Channel or refresh the catalog status.",
      };
    case "stream-interrupted":
      return {
        status: "SIGNAL LOST",
        title: "The live stream was interrupted",
        detail: "Reconnect to resume at the live edge.",
      };
    case "media-unsupported":
      return {
        status: "FORMAT MISSED",
        title: "This signal cannot play in the browser",
        detail: "The Channel answered, but its media format is not supported here.",
      };
    case "browser-unsupported":
      return {
        status: "PLAYER MISSING",
        title: "This browser cannot play MPEG-TS",
        detail: "Open Sparrow in a browser with Media Source live playback support.",
      };
    case "system-player-missing":
      return {
        status: "MPV MISSING",
        title: "System mpv is required for Linux playback",
        detail: "Install mpv, then restart playback.",
      };
    case "system-player-incompatible":
      return {
        status: "MPV UPDATE NEEDED",
        title: "System mpv is not supported",
        detail: "Update mpv to a supported version, then restart playback.",
      };
    case "system-player-unavailable":
      return {
        status: "PLAYER UNAVAILABLE",
        title: "The system player stopped",
        detail: retryable
          ? "Retry playback to reopen the system player at the live edge."
          : "Restart playback after checking the system mpv installation.",
      };
    case "cleanup-unconfirmed":
      return {
        status: "CLEANUP NEEDED",
        title: "Playback cleanup was not confirmed",
        detail: "Sparrow will not open another request until the installed receiver confirms cleanup.",
      };
  }
}
