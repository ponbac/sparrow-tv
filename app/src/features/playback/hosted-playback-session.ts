import type { ChannelId, SparrowClient } from "../../client/contracts";
import { clientPlaybackFailure } from "./playback-failure";
import type {
  HostedPlaybackEngine,
  HostedPlaybackFailure,
  HostedPlaybackHandle,
} from "./mpegts-engine";
import { isRetryable, type PlayerState } from "./playback-presentation";

const RECOVERY_DELAYS_MS = [1_000, 5_000, 15_000] as const;
const STABLE_RESET_MS = 60_000;

/** Inputs for one hosted Playback Session; no provider details enter its state. */
export interface HostedPlaybackSessionOptions {
  readonly id: ChannelId;
  readonly client: Pick<SparrowClient, "startPlayback">;
  readonly engine: HostedPlaybackEngine;
  readonly video: HTMLVideoElement;
  readonly onState: (state: PlayerState) => void;
}

/** A hosted session owns gesture playback as well as transport teardown. */
export interface HostedPlaybackSessionHandle extends HostedPlaybackHandle {
  /**
   * Calls play synchronously for a live transport. Retired/aborted outcomes are
   * ignored; policy rejection restores blocked state, other failures release
   * the transport as unsupported media. No rejection escapes to the caller.
   */
  readonly beginBlockedPlayback: () => void;
}

/**
 * Starts and owns a hosted Playback Session, including detached descriptor work,
 * bounded recovery and media listeners. The returned stop cancels every timer,
 * aborts pending resolution and releases the handle. Late outcomes are ignored.
 * Manual retry creates a fresh scope (and therefore a fresh recovery budget).
 */
export function startHostedPlaybackSession({
  id,
  client,
  engine,
  video,
  onState,
}: HostedPlaybackSessionOptions): HostedPlaybackSessionHandle {
  let active = true;
  let epoch = 0;
  let handle: HostedPlaybackHandle | null = null;
  let controller: AbortController | null = null;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let stableTimer: ReturnType<typeof setTimeout> | undefined;
  let recoveries = 0;
  let transportLive = false;
  let automaticRecoveryAllowed = true;

  const clearStable = () => {
    clearTimeout(stableTimer);
    stableTimer = undefined;
  };
  const releaseTransport = () => {
    transportLive = false;
    clearStable();
    controller?.abort();
    controller = null;
    const old = handle;
    handle = null;
    old?.stop();
  };
  const matches = (expected: number) => active && epoch === expected;
  const fail = (
    expected: number,
    failure: HostedPlaybackFailure,
    retryable: boolean,
  ) => {
    if (!matches(expected)) return;
    // Invalidate callbacks before stop: teardown may itself dispatch media events.
    epoch += 1;
    releaseTransport();
    const delay = RECOVERY_DELAYS_MS[recoveries];
    const canRecover =
      automaticRecoveryAllowed &&
      retryable &&
      isAutomaticallyRecoverable(failure) &&
      delay !== undefined;
    if (!canRecover) {
      onState({ _tag: "failed", failure, retryable });
      return;
    }
    recoveries += 1;
    onState({ _tag: "recovering", attempt: recoveries, failure });
    retryTimer = setTimeout(() => {
      retryTimer = undefined;
      launch();
    }, delay);
  };
  const markAutoplayBlocked = (expected: number) => {
    if (!matches(expected)) return;
    automaticRecoveryAllowed = false;
    clearStable();
    onState({ _tag: "autoplay-blocked" });
  };
  const beginBlockedPlayback = () => {
    if (!active || !transportLive) return;
    const expected = epoch;
    // The gesture is resume intent even before the browser emits a play event.
    automaticRecoveryAllowed = true;
    const onRejected = (cause: unknown) => {
      // Teardown invalidates the epoch before it can reject a pending play.
      if (!matches(expected)) return;
      const name =
        cause instanceof DOMException || cause instanceof Error
          ? cause.name
          : undefined;
      if (name === "AbortError") return;
      if (name === "NotAllowedError") {
        markAutoplayBlocked(expected);
        return;
      }
      fail(expected, "media-unsupported", false);
    };
    try {
      // Do not defer this call: it must retain the browser's user activation.
      void video.play().catch(onRejected);
    } catch (cause: unknown) {
      onRejected(cause);
    }
  };
  const markPlaying = () => {
    if (!active || !transportLive) return;
    automaticRecoveryAllowed = true;
    onState({ _tag: "playing" });
    // Repeated playing events do not postpone the stability deadline.
    if (stableTimer === undefined) {
      stableTimer = setTimeout(() => {
        stableTimer = undefined;
        if (active && transportLive && automaticRecoveryAllowed) recoveries = 0;
      }, STABLE_RESET_MS);
    }
  };
  const markResuming = () => {
    if (active && transportLive) automaticRecoveryAllowed = true;
  };
  const markBuffering = () => clearStable();
  video.addEventListener("play", markResuming);
  video.addEventListener("playing", markPlaying);
  video.addEventListener("waiting", markBuffering);
  video.addEventListener("stalled", markBuffering);

  const open = async (expected: number, signal: AbortSignal) => {
    const result = await client.startPlayback({ id, signal });
    if (!matches(expected)) return;
    if (!result.ok) {
      const mapped = clientPlaybackFailure(result.error);
      fail(expected, mapped.failure, mapped.retryable);
      return;
    }
    if (result.value._tag !== "same-origin-http") {
      fail(expected, "source-invalid", false);
      return;
    }

    // An adapter may report failure synchronously inside start. Register its
    // handle first so recovery always releases it before another request opens.
    let registered = false;
    let synchronousFailure: HostedPlaybackFailure | undefined;
    transportLive = true;
    const started = engine.start({
      endpoint: result.value.endpoint,
      video,
      onAutoplayBlocked: () => markAutoplayBlocked(expected),
      onPause: () => {
        if (!matches(expected)) return;
        automaticRecoveryAllowed = false;
        clearStable();
      },
      onFailure: (failure) => {
        if (!matches(expected)) return;
        if (!registered) {
          synchronousFailure = failure;
        } else {
          fail(expected, failure, isRetryable(failure));
        }
      },
    });
    registered = true;
    if (typeof started === "string") {
      fail(expected, started, isRetryable(started));
      return;
    }
    if (!matches(expected)) {
      started.stop();
      return;
    }
    handle = started;
    if (synchronousFailure !== undefined) {
      fail(expected, synchronousFailure, isRetryable(synchronousFailure));
    }
  };
  const launch = () => {
    if (!active) return;
    const expected = ++epoch;
    automaticRecoveryAllowed = true;
    controller = new AbortController();
    onState({ _tag: "starting" });
    // Detached work belongs to this scope: abort + epoch gate cancel it, and
    // rejected boundary work is reduced to safe copy, never an unhandled promise.
    void open(expected, controller.signal).catch(() => {
      if (matches(expected)) fail(expected, "source-unavailable", false);
    });
  };
  launch();

  return {
    beginBlockedPlayback,
    stop: () => {
      if (!active) return;
      active = false;
      epoch += 1;
      clearTimeout(retryTimer);
      retryTimer = undefined;
      video.removeEventListener("play", markResuming);
      video.removeEventListener("playing", markPlaying);
      video.removeEventListener("waiting", markBuffering);
      video.removeEventListener("stalled", markBuffering);
      releaseTransport();
    },
  };
}

function isAutomaticallyRecoverable(failure: HostedPlaybackFailure): boolean {
  switch (failure) {
    case "source-unavailable":
    case "source-timeout":
    case "stream-interrupted":
      return true;
    case "authentication-required":
    case "channel-not-found":
    case "source-rejected":
    case "source-invalid":
    case "media-unsupported":
    case "browser-unsupported":
      return false;
  }
}
