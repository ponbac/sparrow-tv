import { useEffect, useRef, useState } from "react";
import type { ChannelId, SparrowClient } from "../../client/contracts";
import { clientPlaybackFailure } from "./playback-failure";
import {
  mpegtsPlaybackEngine,
  type HostedPlaybackEngine,
  type HostedPlaybackHandle,
} from "./mpegts-engine";
import {
  isRetryable,
  retryLabel,
  type PlayerState,
} from "./playback-presentation";
import { PlaybackSurface } from "./playback-surface";

export interface HostedPlayerProps {
  readonly channel: { readonly id: ChannelId; readonly name: string };
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
  const [state, setState] = useState<PlayerState>({ _tag: "starting" });
  const [muted, setMuted] = useState(false);
  const [volume, setVolume] = useState(1);
  const [fullscreen, setFullscreen] = useState(false);

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
            ...clientPlaybackFailure(result.error),
          });
          return;
        }
        if (result.value._tag !== "same-origin-http") {
          setState({
            _tag: "failed",
            failure: "source-invalid",
            retryable: false,
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

  useEffect(() => {
    const video = videoRef.current;
    if (video !== null) {
      video.volume = volume;
      video.muted = muted;
    }
  }, [attempt, muted, volume]);

  useEffect(() => {
    const updateFullscreen = () => {
      setFullscreen(document.fullscreenElement === videoRef.current);
    };
    document.addEventListener("fullscreenchange", updateFullscreen);
    return () => document.removeEventListener("fullscreenchange", updateFullscreen);
  }, []);

  const requestFullscreen = () => {
    const video = videoRef.current;
    if (video !== null && video.requestFullscreen !== undefined) {
      void video.requestFullscreen().then(
        () => setFullscreen(true),
        () => undefined,
      );
    }
  };
  const recoveryAction =
    state._tag === "failed" && state.retryable
      ? {
          label: retryLabel(state.failure),
          onAction: () => setAttempt((current) => current + 1),
        }
      : undefined;

  return (
    <PlaybackSurface
      channel={channel}
      state={state}
      videoKey={`${channel.id}:${attempt}`}
      videoRef={videoRef}
      transportLabel="same-origin relay"
      privacyCopy="Provider details remain behind the Sparrow relay."
      onPlaying={() => setState({ _tag: "playing" })}
      {...(recoveryAction === undefined ? {} : { recoveryAction })}
      volume={volume}
      muted={muted}
      fullscreen={fullscreen}
      onVolumeChange={setVolume}
      onToggleMuted={() => setMuted((current) => !current)}
      onRequestFullscreen={requestFullscreen}
      onStop={onStop}
      onAutoplayFailure={() =>
        setState({
          _tag: "failed",
          failure: "media-unsupported",
          retryable: false,
        })
      }
    />
  );
}
