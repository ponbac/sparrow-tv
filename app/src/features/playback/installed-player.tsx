import { AudioLines, RotateCcw, ScrollText } from "lucide-react";
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
  AudioTrack,
  ChannelId,
  InstalledSparrowClient,
} from "../../client/contracts";
import { bindAgentControlPlayback } from "../agent-control/agent-control-binding";
import {
  tauriInstalledLifecycleEvents,
  type InstalledLifecycleEvents,
} from "./installed-lifecycle";
import { createInstalledPlaybackRunner } from "./installed-playback-runner";
import {
  installedPlayerState,
  type InstalledPlaybackAudio,
} from "./installed-playback-state";
import {
  installedPlaybackEngine,
  type InstalledPlaybackEngine,
} from "./installed-playback-engine";
import {
  PlaybackSurface,
  type PlaybackSecondaryAction,
} from "./playback-surface";

export interface InstalledPlayerProps {
  readonly channel: { readonly id: ChannelId; readonly name: string };
  readonly client: Pick<
    InstalledSparrowClient,
    "createPlaybackSession" | "capabilities"
  >;
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
  const [canOpenMpv, setCanOpenMpv] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    void client.capabilities({ signal: controller.signal }).then(
      (result) => {
        if (!controller.signal.aborted)
          setCanOpenMpv(result.ok && result.value.mpvFailover);
      },
      () => {
        if (!controller.signal.aborted) setCanOpenMpv(false);
      },
    );
    return () => controller.abort();
  }, [client]);
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
    return bindAgentControlPlayback({
      channelName: channel.name,
      diagnostics: () => runner.diagnostics(),
      stop: async (signal) => {
        const confirmed = await runner.stop();
        if (confirmed && !signal?.aborted) {
          onStop();
        }
        return confirmed;
      },
    });
  }, [channel.name, onStop, runner]);

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
      runner.setFullscreen(
        videoRef.current !== null &&
          (document.fullscreenElement?.contains(videoRef.current) ?? false),
      );
    };
    document.addEventListener("fullscreenchange", updateFullscreen);
    // Fullscreen on the document root outlives the player that asked for it.
    updateFullscreen();
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
  // mpv reads the Playback Source itself: no Audio Tracks are listed for it,
  // and that says nothing about its sound.
  const silence = usesMpv ? null : audioSilence(state.audio);
  const audioStatus = installedAudioStatus(state.audio, silence);

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

  const secondaryActions: readonly PlaybackSecondaryAction[] = [
    ...(canRestart && recoveryAction === undefined
      ? [
          {
            key: "restart",
            label: "Restart",
            icon: <RotateCcw aria-hidden="true" />,
            onSelect: () => void runner.restart(),
          },
        ]
      : []),
    {
      key: "copy-diagnostics",
      label: "Copy diagnostics",
      icon: <ScrollText aria-hidden="true" />,
      onSelect: copyDiagnostics,
    },
  ];
  const switchPlayer = () => {
    const switchTo = async () => {
      if (document.fullscreenElement !== null) await document.exitFullscreen();
      await runner.switchPlayer(usesMpv ? "in-app" : "mpv");
    };
    void switchTo().catch(() => undefined);
  };

  return (
    <PlaybackSurface
      channel={channel}
      state={installedPlayerState(state)}
      videoKey={channel.id}
      videoRef={videoRef}
      privacyCopy={
        usesMpv
          ? "Provider details pass privately from the installed receiver to mpv over local IPC."
          : "Provider details remain inside the installed receiver."
      }
      onPlaying={() => undefined}
      {...(recoveryAction === undefined ? {} : { recoveryAction })}
      {...(canPause ? { pause: { onPause: () => void runner.pause() } } : {})}
      {...(state.audio.tracks.length === 0
        ? {}
        : {
            audio: (
              <label className="hosted-player__audio-track">
                <AudioLines aria-hidden="true" />
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
            ),
          })}
      {...(audioStatus === null && copyStatus === null
        ? {}
        : {
            status: (
              <>
                {audioStatus === null ? null : (
                  <span
                    className="hosted-player__audio-status"
                    data-warning={audioStatus.warning}
                    role="status"
                  >
                    {audioStatus.text}
                  </span>
                )}
                {copyStatus === null ? null : (
                  <span className="hosted-player__copy-status" role="status">
                    {copyStatus}
                  </span>
                )}
              </>
            ),
          })}
      {...(canOpenMpv
        ? {
            playerSwitch: {
              label: usesMpv ? "Play in app" : "Open in mpv",
              onSwitch: switchPlayer,
              disabled:
                phase._tag === "stopping" ||
                phase._tag === "starting" ||
                phase._tag === "suspending" ||
                phase._tag === "replacing-audio",
            },
          }
        : {})}
      secondaryActions={secondaryActions}
      volume={state.controls.volume}
      muted={state.controls.muted}
      fullscreen={state.controls.fullscreen}
      onVolumeChange={(volume) => runner.setVolume(volume)}
      onToggleMuted={() => runner.toggleMuted()}
      onRequestFullscreen={(surface) => void runner.requestFullscreen(surface)}
      nativeVideo={state.presentation === "android-media3"}
      external={usesMpv}
      silent={silence !== null}
      showMediaControls={!transportReleased}
      stopLabel={
        transportReleased
          ? "Close player"
          : usesMpv
            ? "Stop mpv"
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
  ].join(", ");
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
    case "e-ac-3":
      return "E-AC-3";
  }
}

/** One line about the sound; a warning when it is not what the viewer chose. */
function installedAudioStatus(
  audio: InstalledPlaybackAudio,
  silence: string | null,
): { readonly text: string; readonly warning: boolean } | null {
  // A picture playing in silence comes before anything about preferences.
  if (silence !== null) {
    return { text: silence, warning: true };
  }
  const { selection, preferenceStatus } = audio;
  if (selection._tag === "fallback") {
    return {
      text:
        selection.missing === "saved-preference"
          ? "Saved audio is unavailable. Using the first compatible track."
          : "Chosen audio is unavailable. Using the first compatible track.",
      warning: true,
    };
  }
  if (preferenceStatus === "not-saved") {
    return {
      text: "Audio changed, but the preference could not be saved.",
      warning: false,
    };
  }
  if (preferenceStatus === "saved") {
    return { text: "Audio preference saved for this channel.", warning: false };
  }
  if (preferenceStatus === "unchanged") {
    return { text: "Saved audio preference is unchanged.", warning: false };
  }
  if (
    selection._tag === "selected" &&
    selection.reason === "saved-preference"
  ) {
    return { text: "Saved audio preference applied.", warning: false };
  }
  return null;
}

/** Says why there is no sound, or null while there is or may yet be some. */
function audioSilence(audio: InstalledPlaybackAudio): string | null {
  if (audio.discovered && audio.tracks.length === 0) {
    return "No sound: no playable audio track found.";
  }
  switch (audio.output) {
    case "undecodable": {
      const codec = audio.tracks.find((track) => track.selected)?.codec;
      return codec === undefined
        ? "No sound: this device cannot play the audio."
        : `No sound: this device cannot play ${audioCodecLabel(codec)}.`;
    }
    case "absent":
      return "No sound: the player found no audio track.";
    case "pending":
    case "device-decoder":
    case "bundled-decoder":
    case null:
      return null;
  }
}
