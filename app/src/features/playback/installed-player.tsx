import { Pause, Play, RotateCcw, ScrollText } from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type {
  ChannelId,
  InstalledSparrowClient,
} from "../../client/contracts";
import { createInstalledPlaybackRunner } from "./installed-playback-runner";
import { installedPlayerState } from "./installed-playback-state";
import {
  nativeMpegtsPlaybackEngine,
  type NativePlaybackEngine,
} from "./native-mpegts-engine";
import { PlaybackSurface } from "./playback-surface";

export interface InstalledPlayerProps {
  readonly channel: { readonly id: ChannelId; readonly name: string };
  readonly client: Pick<InstalledSparrowClient, "createPlaybackSession">;
  readonly onStop: () => void;
  readonly engine?: NativePlaybackEngine;
}

/** Plays one Channel through an owned, recoverable, opaque native session. */
export function InstalledPlayer({
  channel,
  client,
  onStop,
  engine = nativeMpegtsPlaybackEngine,
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
    return () => document.removeEventListener("visibilitychange", updateVisibility);
  }, [runner]);

  useEffect(() => {
    const updateFullscreen = () => {
      runner.setFullscreen(document.fullscreenElement === videoRef.current);
    };
    document.addEventListener("fullscreenchange", updateFullscreen);
    return () => document.removeEventListener("fullscreenchange", updateFullscreen);
  }, [runner]);

  const phase = state.phase;
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
    !(phase._tag === "failed" && !phase.canRestart);

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
    void runner.stop().then((confirmed) => {
      if (confirmed) {
        onStop();
      }
    });
  };

  return (
    <PlaybackSurface
      channel={channel}
      state={installedPlayerState(state)}
      videoKey={channel.id}
      videoRef={videoRef}
      transportLabel="native receiver"
      privacyCopy="Provider details remain inside the installed receiver."
      onPlaying={() => undefined}
      {...(overlayAction === undefined ? {} : { overlayAction })}
      additionalControls={
        <>
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
      onStop={stop}
      onAutoplayFailure={() => void runner.reportAutoplayFailure()}
    />
  );
}
