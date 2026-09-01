import { Pause, RotateCcw, ScrollText } from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { clientSchemas } from "../../client/contracts";
import type {
  AudioCodec,
  AudioPreferenceStatus,
  AudioSelection,
  AudioTrack,
  ChannelId,
  InstalledSparrowClient,
} from "../../client/contracts";
import {
  tauriInstalledLifecycleEvents,
  type InstalledLifecycleEvents,
} from "./installed-lifecycle";
import { createInstalledPlaybackRunner } from "./installed-playback-runner";
import { installedPlayerState } from "./installed-playback-state";
import {
  installedPlaybackEngine,
  type InstalledPlaybackEngine,
} from "./installed-playback-engine";
import { PlaybackSurface } from "./playback-surface";

export interface InstalledPlayerProps {
  readonly channel: { readonly id: ChannelId; readonly name: string };
  readonly client: Pick<InstalledSparrowClient, "createPlaybackSession">;
  readonly onStop: () => void;
  readonly engine?: InstalledPlaybackEngine;
  readonly lifecycleEvents?: InstalledLifecycleEvents;
}

/** Plays one Channel through an owned, recoverable, opaque native session. */
export function InstalledPlayer({
  channel,
  client,
  onStop,
  engine = installedPlaybackEngine,
  lifecycleEvents = tauriInstalledLifecycleEvents,
}: InstalledPlayerProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [copyStatus, setCopyStatus] = useState<string | null>(null);
  const runner = useMemo(
    () =>
      createInstalledPlaybackRunner({
        client,
        engine,
        initiallyVisible: document.visibilityState !== "hidden",
      }),
    [client, engine],
  );
  const state = useSyncExternalStore(
    runner.subscribe,
    runner.getSnapshot,
    runner.getSnapshot,
  );

  useEffect(() => {
    const video = videoRef.current;
    if (video === null) {
      return;
    }
    revealPlayerOnMobile(video);
    void runner.select({ id: channel.id, name: channel.name }, video);
    return () => {
      void runner.stop();
    };
  }, [channel.id, channel.name, runner]);

  useEffect(() => {
    const updateVisibility = () => {
      void runner.setVisible(document.visibilityState !== "hidden");
    };
    document.addEventListener("visibilitychange", updateVisibility);
    updateVisibility();
    return () =>
      document.removeEventListener("visibilitychange", updateVisibility);
  }, [runner]);

  useEffect(() => {
    let disposed = false;
    let release: (() => void) | null = null;
    void lifecycleEvents
      .subscribe((signal) => {
        if (!disposed) {
          void runner.setForeground(signal === "resumed");
        }
      })
      .then(
        (subscriptionRelease) => {
          if (disposed) {
            subscriptionRelease();
          } else {
            release = subscriptionRelease;
          }
        },
        () => undefined,
      );
    return () => {
      disposed = true;
      release?.();
    };
  }, [lifecycleEvents, runner]);

  useEffect(() => {
    const updateFullscreen = () => {
      runner.setFullscreen(document.fullscreenElement === videoRef.current);
    };
    document.addEventListener("fullscreenchange", updateFullscreen);
    return () =>
      document.removeEventListener("fullscreenchange", updateFullscreen);
  }, [runner]);

  const phase = state.phase;
  const transportReleased = phase._tag === "failed";
  const usesMpv = state.presentation === "linux-mpv";
  const recoveryAction =
    phase._tag === "paused"
      ? { label: "Resume", onAction: () => void runner.resume() }
      : phase._tag === "failed" && phase.canRestart
        ? { label: "Restart", onAction: () => void runner.restart() }
        : undefined;
  const canPause =
    phase._tag === "starting" ||
    phase._tag === "playing" ||
    phase._tag === "recovering";
  const canRestart =
    phase._tag !== "idle" &&
    phase._tag !== "stopping" &&
    phase._tag !== "replacing-audio" &&
    !(phase._tag === "failed" && !phase.canRestart);
  const selectedAudioTrack = state.audio.tracks.find((track) => track.selected);
  const canSelectAudio =
    state.audio.tracks.length > 1 &&
    (phase._tag === "playing" || phase._tag === "autoplay-blocked");
  const audioStatus = installedAudioStatus(
    state.audio.discovered,
    state.audio.tracks,
    state.audio.selection,
    state.audio.preferenceStatus,
  );

  const copyDiagnostics = () => {
    const clipboard = navigator.clipboard;
    if (clipboard === undefined) {
      setCopyStatus("Diagnostics copy unavailable");
      return;
    }
    void runner
      .copyDiagnostics({
        writeText: (text) => clipboard.writeText(text),
      })
      .then(
        () => setCopyStatus("Diagnostics copied"),
        () => setCopyStatus("Diagnostics copy unavailable"),
      );
  };
  const stop = () => {
    void runner
      .stop()
      .then((confirmed) => {
        if (confirmed) {
          onStop();
        }
      })
      .catch(() => undefined);
  };

  return (
    <PlaybackSurface
      channel={channel}
      state={installedPlayerState(state)}
      videoKey={channel.id}
      videoRef={videoRef}
      transportLabel={
        usesMpv
          ? "system mpv"
          : state.presentation === "android-media3"
            ? "Android Media3"
            : "native receiver"
      }
      privacyCopy={
        usesMpv
          ? "Provider details pass privately from the installed receiver to mpv over local IPC."
          : "Provider details remain inside the installed receiver."
      }
      onPlaying={() => undefined}
      {...(recoveryAction === undefined ? {} : { recoveryAction })}
      additionalControls={
        <>
          {state.audio.tracks.length === 0 ? null : (
            <label className="hosted-player__audio-track">
              <span>Audio</span>
              <select
                aria-label="Audio track"
                value={selectedAudioTrack?.id ?? ""}
                disabled={!canSelectAudio}
                onChange={(event) => {
                  const parsed = clientSchemas.audioTrackId.safeParse(
                    event.currentTarget.value,
                  );
                  if (parsed.success) {
                    void runner.selectAudio(parsed.data);
                  }
                }}
              >
                {state.audio.tracks.map((track, index) => (
                  <option key={track.id} value={track.id}>
                    {audioTrackLabel(track, index)}
                  </option>
                ))}
              </select>
            </label>
          )}
          {audioStatus === null ? null : (
            <span
              className="hosted-player__audio-status"
              data-fallback={state.audio.selection._tag === "fallback"}
              role="status"
            >
              {audioStatus}
            </span>
          )}
          {canPause ? (
            <button type="button" onClick={() => void runner.pause()}>
              <Pause aria-hidden="true" />
              Pause
            </button>
          ) : null}
          {canRestart && recoveryAction === undefined ? (
            <button type="button" onClick={() => void runner.restart()}>
              <RotateCcw aria-hidden="true" />
              Restart
            </button>
          ) : null}
          <button type="button" onClick={copyDiagnostics}>
            <ScrollText aria-hidden="true" />
            Copy diagnostics
          </button>
          {copyStatus === null ? null : (
            <span className="hosted-player__copy-status" role="status">
              {copyStatus}
            </span>
          )}
        </>
      }
      volume={state.controls.volume}
      muted={state.controls.muted}
      fullscreen={state.controls.fullscreen}
      onVolumeChange={(volume) => runner.setVolume(volume)}
      onToggleMuted={() => runner.toggleMuted()}
      onRequestFullscreen={() => void runner.requestFullscreen()}
      showMediaControls={!transportReleased}
      stopLabel={
        transportReleased ? "Close player" : usesMpv ? "Stop mpv" : "Stop stream"
      }
      onStop={stop}
      onAutoplayFailure={() => void runner.reportAutoplayFailure()}
    />
  );
}

function audioTrackLabel(track: AudioTrack, index: number): string {
  const metadata = [track.label, track.language?.toUpperCase()].filter(
    (value, position, values): value is string =>
      value !== undefined && values.indexOf(value) === position,
  );
  return [
    ...(metadata.length === 0 ? [`Audio ${index + 1}`] : metadata),
    audioCodecLabel(track.codec),
  ].join(" · ");
}

function audioCodecLabel(codec: AudioCodec): string {
  switch (codec) {
    case "mpeg-1-audio":
      return "MPEG-1";
    case "mpeg-2-audio":
      return "MPEG-2";
    case "aac-adts":
      return "AAC";
    case "aac-latm":
      return "AAC LATM";
    case "ac-3":
      return "AC-3";
  }
}

function revealPlayerOnMobile(video: HTMLVideoElement): void {
  if (
    typeof window.matchMedia !== "function" ||
    !window.matchMedia("(max-width: 760px)").matches
  ) {
    return;
  }
  video.scrollIntoView?.({
    behavior: "auto",
    block: "center",
    inline: "nearest",
  });
}

function installedAudioStatus(
  discovered: boolean,
  tracks: readonly AudioTrack[],
  selection: AudioSelection,
  preferenceStatus: AudioPreferenceStatus | null,
): string | null {
  if (selection._tag === "fallback") {
    return selection.missing === "saved-preference"
      ? "Saved audio is unavailable. Using the first compatible track."
      : "Chosen audio is unavailable. Using the first compatible track.";
  }
  if (preferenceStatus === "not-saved") {
    return "Audio changed, but the preference could not be saved.";
  }
  if (preferenceStatus === "saved") {
    return "Audio preference saved for this channel.";
  }
  if (preferenceStatus === "unchanged") {
    return "Saved audio preference is unchanged.";
  }
  if (selection._tag === "selected" && selection.reason === "saved-preference") {
    return "Saved audio preference applied.";
  }
  return discovered && tracks.length === 0
    ? "No compatible audio track was found."
    : null;
}
