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
        status: "Tuning",
        title: "Opening the live signal",
        detail: "Sparrow is connecting this channel.",
      };
    case "playing":
      return {
        status: "On air",
        title: "Live signal",
        detail: "This channel is playing.",
      };
    case "autoplay-blocked":
      return {
        status: "Ready",
        title: "The signal is ready",
        detail: "Your browser needs one more gesture before playing sound.",
      };
    case "suspending":
      return {
        status: "Releasing",
        title: "Releasing the live signal",
        detail: "Sparrow is confirming that transport work has stopped.",
      };
    case "paused":
      return {
        status: "Paused",
        title: "Live playback is paused",
        detail: "The provider request is released. Resume to return at the live edge.",
      };
    case "recovering":
      return {
        status: "Reconnecting",
        title: `Recovery attempt ${state.attempt}`,
        detail: "The prior request is released before Sparrow reconnects.",
      };
    case "stopping":
      return {
        status: "Stopping",
        title: "Closing the player",
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
        status: "Access needed",
        title: "Playback needs authentication",
        detail: "Authenticate with this Sparrow deployment, then try the signal again.",
      };
    case "channel-not-found":
      return {
        status: "Channel gone",
        title: "That channel left the catalog",
        detail: "Choose a channel from the guide.",
      };
    case "source-rejected":
      return {
        status: "Source rejected",
        title: "The provider refused this signal",
        detail: "Choose another channel or refresh the sources.",
      };
    case "source-invalid":
      return {
        status: "Invalid signal",
        title: "The provider returned an invalid signal",
        detail: "Choose another channel. Trying this one again will not repair it.",
      };
    case "source-timeout":
      return {
        status: "Source timeout",
        title: "The signal took too long to answer",
        detail: "Try the channel again when the provider is responsive.",
      };
    case "source-unavailable":
      return {
        status: "Source offline",
        title: "The live signal is unavailable",
        detail: retryable
          ? "Try this channel again or choose another."
          : "Choose another channel or refresh the sources.",
      };
    case "stream-interrupted":
      return {
        status: "Signal lost",
        title: "The live stream was interrupted",
        detail: "Reconnect to resume at the live edge.",
      };
    case "media-unsupported":
      return {
        status: "Format missed",
        title: "This signal cannot play in the browser",
        detail: "The channel answered, but its media format is not supported here.",
      };
    case "browser-unsupported":
      return {
        status: "Player missing",
        title: "This browser cannot play MPEG-TS",
        detail: "Open Sparrow in a browser with Media Source live playback support.",
      };
    case "system-player-missing":
      return {
        status: "mpv missing",
        title: "System mpv is required for Linux playback",
        detail: "Install mpv, then restart playback.",
      };
    case "system-player-incompatible":
      return {
        status: "mpv update needed",
        title: "System mpv is not supported",
        detail: "Update mpv to a supported version, then restart playback.",
      };
    case "system-player-unavailable":
      return {
        status: "Player unavailable",
        title: "The system player stopped",
        detail: retryable
          ? "Retry playback to reopen the system player at the live edge."
          : "Restart playback after checking the system mpv installation.",
      };
    case "cleanup-unconfirmed":
      return {
        status: "Cleanup needed",
        title: "Playback cleanup was not confirmed",
        detail: "Sparrow will not open another request until the installed receiver confirms cleanup.",
      };
  }
}
