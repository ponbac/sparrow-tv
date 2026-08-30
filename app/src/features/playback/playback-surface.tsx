import { Maximize2, RotateCcw, Square, Volume2, VolumeX } from "lucide-react";
import { useState, type RefObject } from "react";
import type { ChannelId } from "../../client/contracts";
import {
  playerPresentation,
  retryLabel,
  type PlayerState,
} from "./playback-presentation";
import "./hosted-player.css";

export interface PlaybackSurfaceProps {
  readonly channel: { readonly id: ChannelId; readonly name: string };
  readonly state: PlayerState;
  readonly attempt: number;
  readonly videoRef: RefObject<HTMLVideoElement>;
  readonly transportLabel: string;
  readonly privacyCopy: string;
  readonly onPlaying: () => void;
  readonly onRetry: () => void;
  readonly onStop: () => void;
  readonly onAutoplayFailure: () => void;
}

/** Accessible playback chrome shared without sharing transport ownership. */
export function PlaybackSurface({
  channel,
  state,
  attempt,
  videoRef,
  transportLabel,
  privacyCopy,
  onPlaying,
  onRetry,
  onStop,
  onAutoplayFailure,
}: PlaybackSurfaceProps) {
  const [muted, setMuted] = useState(false);
  const beginBlockedPlayback = () => {
    const video = videoRef.current;
    if (video !== null) {
      void video.play().catch(onAutoplayFailure);
    }
  };
  const enterFullscreen = () => {
    const video = videoRef.current;
    if (video !== null && video.requestFullscreen !== undefined) {
      void video.requestFullscreen().catch(() => undefined);
    }
  };

  const presentation = playerPresentation(state);
  return (
    <section className="hosted-player" aria-labelledby="playback-player-heading">
      <div className="hosted-player__heading">
        <div>
          <p className="eyebrow">Live monitor · {transportLabel}</p>
          <h2 id="playback-player-heading">{channel.name}</h2>
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
          onPlaying={onPlaying}
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
              <button type="button" onClick={onRetry}>
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
        <button
          type="button"
          aria-pressed={muted}
          onClick={() => setMuted((current) => !current)}
        >
          {muted ? (
            <VolumeX aria-hidden="true" />
          ) : (
            <Volume2 aria-hidden="true" />
          )}
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
        <p>{privacyCopy}</p>
      </div>
    </section>
  );
}
