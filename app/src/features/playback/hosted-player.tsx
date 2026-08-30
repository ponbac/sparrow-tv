import { Maximize2, RotateCcw, Square, Volume2, VolumeX } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type {
  ChannelId,
  ClientError,
  SparrowClient,
} from "../../client/contracts";
import {
  mpegtsPlaybackEngine,
  type HostedPlaybackEngine,
  type HostedPlaybackFailure,
  type HostedPlaybackHandle,
} from "./mpegts-engine";
import "./hosted-player.css";

type PlayerState =
  | { readonly _tag: "starting" }
  | { readonly _tag: "playing" }
  | { readonly _tag: "autoplay-blocked" }
  | {
      readonly _tag: "failed";
      readonly failure: HostedPlaybackFailure;
      readonly retryable: boolean;
    };

export interface HostedPlayerProps {
  readonly channel: {
    readonly id: ChannelId;
    readonly name: string;
  };
  readonly client: SparrowClient;
  readonly onStop: () => void;
  readonly engine?: HostedPlaybackEngine;
}

/** Plays one hosted Channel while keeping the provider source outside React. */
export function HostedPlayer({
  channel,
  client,
  onStop,
  engine = mpegtsPlaybackEngine,
}: HostedPlayerProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [attempt, setAttempt] = useState(0);
  const [muted, setMuted] = useState(false);
  const [state, setState] = useState<PlayerState>({ _tag: "starting" });

  useEffect(() => {
    const video = videoRef.current;
    if (video === null) {
      return;
    }

    const controller = new AbortController();
    let active = true;
    let handle: HostedPlaybackHandle | null = null;
    setState({ _tag: "starting" });

    void client
      .startPlayback({ id: channel.id, signal: controller.signal })
      .then((result) => {
        if (!active) {
          return;
        }
        if (!result.ok) {
          setState({
            _tag: "failed",
            ...clientFailure(result.error),
          });
          return;
        }

        const started = engine.start({
          endpoint: result.value.endpoint,
          video,
          onAutoplayBlocked: () => {
            if (active) {
              setState({ _tag: "autoplay-blocked" });
            }
          },
          onFailure: (failure) => {
            if (active) {
              setState({
                _tag: "failed",
                failure,
                retryable: isRetryable(failure),
              });
            }
          },
        });
        if (typeof started === "string") {
          setState({
            _tag: "failed",
            failure: started,
            retryable: isRetryable(started),
          });
          return;
        }
        handle = started;
      });

    return () => {
      active = false;
      controller.abort();
      handle?.stop();
    };
  }, [attempt, channel.id, client, engine]);

  const beginBlockedPlayback = () => {
    const video = videoRef.current;
    if (video === null) {
      return;
    }
    void video.play().catch(() => {
      setState({
        _tag: "failed",
        failure: "media-unsupported",
        retryable: false,
      });
    });
  };
  const retry = () => setAttempt((current) => current + 1);
  const toggleMute = () => setMuted((current) => !current);
  const enterFullscreen = () => {
    const video = videoRef.current;
    if (video !== null && video.requestFullscreen !== undefined) {
      void video.requestFullscreen().catch(() => undefined);
    }
  };

  const presentation = playerPresentation(state);
  return (
    <section className="hosted-player" aria-labelledby="hosted-player-heading">
      <div className="hosted-player__heading">
        <div>
          <p className="eyebrow">Live monitor · same-origin relay</p>
          <h2 id="hosted-player-heading">{channel.name}</h2>
        </div>
        <div
          className="hosted-player__state"
          data-state={state._tag}
          role="status"
          aria-live="polite"
        >
          <span aria-hidden="true" />
          {presentation.status}
        </div>
      </div>

      <div className="hosted-player__screen" data-state={state._tag}>
        <video
          key={`${channel.id}:${attempt}`}
          ref={videoRef}
          aria-label={`${channel.name} live video`}
          autoPlay
          muted={muted}
          playsInline
          onPlaying={() => setState({ _tag: "playing" })}
        />
        {state._tag !== "playing" ? (
          <div className="hosted-player__overlay">
            <p>{presentation.title}</p>
            <span>{presentation.detail}</span>
            {state._tag === "autoplay-blocked" ? (
              <button type="button" onClick={beginBlockedPlayback}>
                Start audio &amp; video
              </button>
            ) : state._tag === "failed" && state.retryable ? (
              <button type="button" onClick={retry}>
                <RotateCcw aria-hidden="true" />
                {retryLabel(state.failure)}
              </button>
            ) : null}
          </div>
        ) : null}
      </div>

      <div
        className="hosted-player__controls"
        role="group"
        aria-label="Playback controls"
      >
        <button type="button" aria-pressed={muted} onClick={toggleMute}>
          {muted ? <VolumeX aria-hidden="true" /> : <Volume2 aria-hidden="true" />}
          {muted ? "Unmute" : "Mute"}
        </button>
        <button type="button" onClick={enterFullscreen}>
          <Maximize2 aria-hidden="true" />
          Full screen
        </button>
        <button type="button" onClick={onStop}>
          <Square aria-hidden="true" />
          Stop stream
        </button>
        <p>Provider details remain behind the Sparrow relay.</p>
      </div>
    </section>
  );
}

function clientFailure(error: ClientError): {
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

function playerPresentation(state: PlayerState): {
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

function isRetryable(failure: HostedPlaybackFailure): boolean {
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

function retryLabel(failure: HostedPlaybackFailure): string {
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
