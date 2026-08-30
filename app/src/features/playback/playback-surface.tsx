import { Maximize2, RotateCcw, Square, Volume2, VolumeX } from "lucide-react";
import type { ReactNode, RefObject } from "react";
import type { ChannelId } from "../../client/contracts";
import {
  playerPresentation,
  type PlayerState,
} from "./playback-presentation";
import "./hosted-player.css";

export interface PlaybackSurfaceProps {
  readonly channel: { readonly id: ChannelId; readonly name: string };
  readonly state: PlayerState;
  readonly videoKey: string;
  readonly videoRef: RefObject<HTMLVideoElement>;
  readonly transportLabel: string;
  readonly privacyCopy: string;
  readonly onPlaying: () => void;
  readonly overlayAction?: {
    readonly label: string;
    readonly onAction: () => void;
  };
  readonly additionalControls?: ReactNode;
  readonly volume: number;
  readonly muted: boolean;
  readonly fullscreen: boolean;
  readonly onVolumeChange: (volume: number) => void;
  readonly onToggleMuted: () => void;
  readonly onRequestFullscreen: () => void;
  readonly showMediaControls?: boolean;
  readonly stopLabel?: string;
  readonly onStop: () => void;
  readonly onAutoplayFailure: () => void;
}

/** Accessible playback chrome shared without sharing transport ownership. */
export function PlaybackSurface({
  channel,
  state,
  videoKey,
  videoRef,
  transportLabel,
  privacyCopy,
  onPlaying,
  overlayAction,
  additionalControls,
  volume,
  muted,
  fullscreen,
  onVolumeChange,
  onToggleMuted,
  onRequestFullscreen,
  showMediaControls = true,
  stopLabel = "Stop stream",
  onStop,
  onAutoplayFailure,
}: PlaybackSurfaceProps) {
  const beginBlockedPlayback = () => {
    const video = videoRef.current;
    if (video !== null) {
      void video.play().catch(onAutoplayFailure);
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
          key={videoKey}
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
            ) : overlayAction !== undefined ? (
              <button type="button" onClick={overlayAction.onAction}>
                <RotateCcw aria-hidden="true" />
                {overlayAction.label}
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
        {additionalControls}
        {showMediaControls ? (
          <>
            <button
              type="button"
              aria-pressed={muted}
              onClick={onToggleMuted}
            >
              {muted ? (
                <VolumeX aria-hidden="true" />
              ) : (
                <Volume2 aria-hidden="true" />
              )}
              {muted ? "Unmute" : "Mute"}
            </button>
            <label className="hosted-player__volume">
              <span>Volume</span>
              <input
                type="range"
                min="0"
                max="100"
                step="1"
                value={Math.round(volume * 100)}
                aria-label="Volume"
                onChange={(event) =>
                  onVolumeChange(Number(event.currentTarget.value) / 100)
                }
              />
            </label>
            <button
              type="button"
              aria-pressed={fullscreen}
              onClick={onRequestFullscreen}
            >
              <Maximize2 aria-hidden="true" />
              Full screen
            </button>
          </>
        ) : null}
        <button type="button" onClick={onStop}>
          <Square aria-hidden="true" />
          {stopLabel}
        </button>
        <p>{privacyCopy}</p>
      </div>
    </section>
  );
}
