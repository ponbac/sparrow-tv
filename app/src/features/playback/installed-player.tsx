import { MonitorPlay, Pause, Play, RotateCcw, ScrollText } from "lucide-react";
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
  nativeMpegtsPlaybackEngine,
  type NativePlaybackEngine,
} from "./native-mpegts-engine";
import { PlaybackSurface } from "./playback-surface";

export interface InstalledPlayerProps {
  readonly channel: { readonly id: ChannelId; readonly name: string };
  readonly client: Pick<
    InstalledSparrowClient,
    "capabilities" | "createPlaybackSession"
  >;
  readonly onStop: () => void;
  readonly engine?: NativePlaybackEngine;
  readonly lifecycleEvents?: InstalledLifecycleEvents;
}

/** Plays one Channel through an owned, recoverable, opaque native session. */
export function InstalledPlayer({
  channel,
  client,
  onStop,
  engine = nativeMpegtsPlaybackEngine,
  lifecycleEvents = tauriInstalledLifecycleEvents,
}: InstalledPlayerProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [copyStatus, setCopyStatus] = useState<string | null>(null);
  const [mpvFailoverAvailable, setMpvFailoverAvailable] = useState(false);
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
    const controller = new AbortController();
    void client.capabilities({ signal: controller.signal }).then(
      (result) => {
        if (!controller.signal.aborted) {
          setMpvFailoverAvailable(
            result.ok &&
              result.value.playbackTransport === "tauri-native-stream" &&
              result.value.mpvFailover,
          );
        }
      },
      () => {
        if (!controller.signal.aborted) {
          setMpvFailoverAvailable(false);
        }
      },
    );
    return () => controller.abort();
  }, [client]);

  useEffect(() => {
    const video = videoRef.current;
    if (video === null) {
      return;
    }
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
  const canOfferMpv =
    mpvFailoverAvailable &&
    ((phase._tag === "failed" && phase.canFailover) ||
      (phase._tag === "primary-stopped" && phase.canFailover));
  const fallbackPhase =
    phase._tag === "fallback-starting" ||
    phase._tag === "fallback-playing" ||
    phase._tag === "fallback-stop-failed";
  const primaryReleased = phase._tag === "failed" || phase._tag === "primary-stopped";
  const overlayAction =
    phase._tag === "paused"
      ? { label: "Resume live signal", onAction: () => void runner.resume() }
      : phase._tag === "failed" && phase.canRestart
        ? { label: "Restart signal", onAction: () => void runner.restart() }
        : undefined;
  const canPause =
    phase._tag === "starting" ||
    phase._tag === "playing" ||
    phase._tag === "autoplay-blocked" ||
    phase._tag === "recovering";
  const canRestart =
    phase._tag !== "idle" &&
    phase._tag !== "stopping" &&
    phase._tag !== "replacing-audio" &&
    !fallbackPhase &&
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
    if (mpvFailoverAvailable && !primaryReleased && !fallbackPhase) {
      void runner.stopPrimary();
      return;
    }
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
      transportLabel={fallbackPhase ? "system mpv" : "native receiver"}
      privacyCopy={
        fallbackPhase
          ? "Provider details pass directly from the installed receiver to mpv."
          : "Provider details remain inside the installed receiver."
      }
      onPlaying={() => undefined}
      {...(overlayAction === undefined ? {} : { overlayAction })}
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
          ) : phase._tag === "paused" ? (
            <button type="button" onClick={() => void runner.resume()}>
              <Play aria-hidden="true" />
              Resume
            </button>
          ) : null}
          {canRestart ? (
            <button type="button" onClick={() => void runner.restart()}>
              <RotateCcw aria-hidden="true" />
              Restart
            </button>
          ) : null}
          {canOfferMpv ? (
            <button type="button" onClick={() => void runner.startMpvFallback()}>
              <MonitorPlay aria-hidden="true" />
              Open in mpv
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
      showMediaControls={!primaryReleased && !fallbackPhase}
      stopLabel={
        fallbackPhase
          ? "Stop mpv"
          : primaryReleased
            ? "Close player"
            : mpvFailoverAvailable
              ? "Stop primary"
              : "Stop stream"
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
