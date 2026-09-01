import type { ClientError } from "../../client/contracts";
import type { HostedPlaybackFailure } from "./mpegts-engine";

/** Safe Linux system-player failures that never carry process or source data. */
export type SystemPlayerFailure =
  | "system-player-missing"
  | "system-player-incompatible"
  | "system-player-unavailable";

/** Reduces a transport-safe client failure to the common player vocabulary. */
export function clientPlaybackFailure(error: ClientError): {
  readonly failure: HostedPlaybackFailure;
  readonly retryable: boolean;
} {
  switch (error._tag) {
    case "authentication-required":
      return { failure: "authentication-required", retryable: true };
    case "not-found":
      return { failure: "channel-not-found", retryable: false };
    case "playback-failed":
      return {
        failure: serverPlaybackFailure(error.reason),
        retryable: error.retryable,
      };
    case "mpv-failed":
      return { failure: "source-unavailable", retryable: error.retryable };
    case "transport":
      return { failure: "source-unavailable", retryable: error.retryable };
    case "service-unavailable":
      return { failure: "source-unavailable", retryable: true };
    case "catalog-unavailable":
    case "not-configured":
    case "stale-cursor":
    case "invalid-input":
    case "cancelled":
      return { failure: "source-unavailable", retryable: false };
  }
}

/** Preserves actionable system-mpv failures at the installed-player boundary. */
export function installedClientPlaybackFailure(error: ClientError): {
  readonly failure: HostedPlaybackFailure | SystemPlayerFailure;
  readonly retryable: boolean;
} {
  if (error._tag !== "mpv-failed") {
    return clientPlaybackFailure(error);
  }
  switch (error.reason) {
    case "not-installed":
      return { failure: "system-player-missing", retryable: false };
    case "incompatible":
      return { failure: "system-player-incompatible", retryable: false };
    case "unsupported":
    case "primary-active":
    case "stale-session":
    case "launch-failed":
    case "control-unavailable":
    case "terminated":
      return {
        failure: "system-player-unavailable",
        retryable: error.retryable,
      };
  }
}

function serverPlaybackFailure(
  reason: Extract<ClientError, { readonly _tag: "playback-failed" }>["reason"],
): HostedPlaybackFailure {
  switch (reason) {
    case "rejected":
      return "source-rejected";
    case "invalid-response":
      return "source-invalid";
    case "timed-out":
      return "source-timeout";
    case "unavailable":
      return "source-unavailable";
  }
}
