import type { HostedPlaybackFailure } from "./mpegts-engine";

/** User-visible state shared by hosted and installed live playback transports. */
export type PlayerState =
  | { readonly _tag: "starting" }
  | { readonly _tag: "playing" }
  | { readonly _tag: "autoplay-blocked" }
  | {
      readonly _tag: "failed";
      readonly failure: HostedPlaybackFailure;
      readonly retryable: boolean;
    };

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
    case "failed":
      return failurePresentation(state.failure, state.retryable);
  }
}

export function isRetryable(failure: HostedPlaybackFailure): boolean {
  switch (failure) {
    case "authentication-required":
    case "source-unavailable":
    case "source-timeout":
    case "stream-interrupted":
      return true;
    case "channel-not-found":
    case "source-rejected":
    case "source-invalid":
    case "media-unsupported":
    case "browser-unsupported":
      return false;
  }
}

export function retryLabel(failure: HostedPlaybackFailure): string {
  switch (failure) {
    case "authentication-required":
      return "Try after authentication";
    case "stream-interrupted":
      return "Reconnect signal";
    case "source-unavailable":
    case "source-timeout":
      return "Try signal again";
    case "channel-not-found":
    case "source-rejected":
    case "source-invalid":
    case "media-unsupported":
    case "browser-unsupported":
      return "";
  }
}

function failurePresentation(
  failure: HostedPlaybackFailure,
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
  }
}
