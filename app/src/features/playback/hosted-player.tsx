import { useEffect, useRef, useState } from "react";
import type { ChannelId, SparrowClient } from "../../client/contracts";
import {
  startHostedPlaybackSession,
  type HostedPlaybackSessionHandle,
} from "./hosted-playback-session";
import {
  mpegtsPlaybackEngine,
  type HostedPlaybackEngine,
} from "./mpegts-engine";
import { retryLabel, type PlayerState } from "./playback-presentation";
import { PlaybackSurface } from "./playback-surface";

/** Hosted playback inputs; the client only resolves ephemeral playback descriptors. */
export interface HostedPlayerProps {
  readonly channel: { readonly id: ChannelId; readonly name: string };
  readonly client: Pick<SparrowClient, "startPlayback">;
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
  const sessionRef = useRef<{
    readonly id: ChannelId;
    readonly attempt: number;
    readonly handle: HostedPlaybackSessionHandle;
  } | null>(null);
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

    const session = startHostedPlaybackSession({
      id: channel.id,
      client,
      engine,
      video,
      onState: setState,
    });
    sessionRef.current = { id: channel.id, attempt, handle: session };
    return () => {
      sessionRef.current = null;
      session.stop();
    };
  }, [attempt, channel.id, client, engine]);

  useEffect(() => {
    // The video element is rekeyed per Channel and attempt; each new one
    // starts at full volume until the viewer's level is applied again.
    const video = videoRef.current;
    if (video !== null) {
      video.volume = volume;
      video.muted = muted;
    }
  }, [attempt, channel.id, muted, volume]);

  useEffect(() => {
    const updateFullscreen = () => {
      setFullscreen(
        videoRef.current !== null &&
          (document.fullscreenElement?.contains(videoRef.current) ?? false),
      );
    };
    document.addEventListener("fullscreenchange", updateFullscreen);
    // Fullscreen on the document root outlives the player that asked for it.
    updateFullscreen();
    return () =>
      document.removeEventListener("fullscreenchange", updateFullscreen);
  }, []);

  const requestFullscreen = (surface: HTMLElement) => {
    if (surface.requestFullscreen !== undefined) {
      const action =
        document.fullscreenElement === surface
          ? document.exitFullscreen()
          : surface.requestFullscreen();
      void action.then(
        () => setFullscreen(document.fullscreenElement === surface),
        () => undefined,
      );
    }
  };
  const recoveryAction =
    (state._tag === "failed" && state.retryable) || state._tag === "recovering"
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
      privacyCopy="Provider details remain behind the Sparrow relay."
      // The session owns playing events, including guards against stale media.
      onPlaying={() => undefined}
      {...(recoveryAction === undefined ? {} : { recoveryAction })}
      volume={volume}
      muted={muted}
      fullscreen={fullscreen}
      onVolumeChange={setVolume}
      onToggleMuted={() => setMuted((current) => !current)}
      onRequestFullscreen={requestFullscreen}
      onStop={() => {
        sessionRef.current?.handle.stop();
        sessionRef.current = null;
        setState({ _tag: "stopping" });
        onStop();
      }}
      onBeginBlockedPlayback={() => {
        const current = sessionRef.current;
        // Only the matching session may start a gesture; it owns late outcomes.
        if (
          current === null ||
          current.id !== channel.id ||
          current.attempt !== attempt
        ) return;
        current.handle.beginBlockedPlayback();
      }}
    />
  );
}
