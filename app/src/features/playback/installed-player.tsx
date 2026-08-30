import { useEffect, useRef, useState } from "react";
import type {
  ChannelId,
  InstalledSparrowClient,
} from "../../client/contracts";
import { clientPlaybackFailure } from "./playback-failure";
import type { HostedPlaybackHandle } from "./mpegts-engine";
import {
  nativeMpegtsPlaybackEngine,
  type NativePlaybackEngine,
} from "./native-mpegts-engine";
import {
  isRetryable,
  type PlayerState,
} from "./playback-presentation";
import { PlaybackSurface } from "./playback-surface";

export interface InstalledPlayerProps {
  readonly channel: { readonly id: ChannelId; readonly name: string };
  readonly client: Pick<
    InstalledSparrowClient,
    "startPlayback" | "readPlayback" | "stopPlayback"
  >;
  readonly onStop: () => void;
  readonly engine?: NativePlaybackEngine;
}

/** Plays one Channel through an opaque, cancellable Rust-owned stream. */
export function InstalledPlayer({
  channel,
  client,
  onStop,
  engine = nativeMpegtsPlaybackEngine,
}: InstalledPlayerProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [attempt, setAttempt] = useState(0);
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
            ...clientPlaybackFailure(result.error),
          });
          return;
        }
        if (result.value._tag !== "tauri-native-stream") {
          setState({
            _tag: "failed",
            failure: "source-invalid",
            retryable: false,
          });
          return;
        }

        const started = engine.start({
          client,
          descriptor: result.value,
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

  return (
    <PlaybackSurface
      channel={channel}
      state={state}
      attempt={attempt}
      videoRef={videoRef}
      transportLabel="native receiver"
      privacyCopy="Provider details remain inside the installed receiver."
      onPlaying={() => setState({ _tag: "playing" })}
      onRetry={() => setAttempt((current) => current + 1)}
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
