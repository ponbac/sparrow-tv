import { Menu } from "@base-ui/react/menu";
import {
  Ellipsis,
  ExternalLink,
  Maximize2,
  Minimize2,
  MonitorPlay,
  Pause,
  RotateCcw,
  Square,
  Volume2,
  VolumeX,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import type { ChannelId } from "../../client/contracts";
import { useStageChrome } from "../stage/stage-chrome";
import { playerPresentation, type PlayerState } from "./playback-presentation";
import "./hosted-player.css";

/** A less-used action: a plain button in the bar, a "More" menu item when compact. */
export interface PlaybackSecondaryAction {
  readonly key: string;
  readonly label: string;
  readonly icon: ReactNode;
  readonly onSelect: () => void;
  readonly disabled?: boolean;
}

export interface PlaybackSurfaceProps {
  readonly channel: { readonly id: ChannelId; readonly name: string };
  readonly state: PlayerState;
  readonly videoKey: string;
  readonly videoRef: RefObject<HTMLVideoElement>;
  readonly privacyCopy: string;
  readonly onPlaying: () => void;
  readonly recoveryAction?: {
    readonly label: string;
    readonly onAction: () => void;
  };
  readonly pause?: { readonly onPause: () => void };
  /** A label wrapping the native Audio Track select. */
  readonly audio?: ReactNode;
  /** Audio and copy status messages. */
  readonly status?: ReactNode;
  readonly playerSwitch?: {
    readonly label: "Open in mpv" | "Play in app";
    readonly onSwitch: () => void;
    readonly disabled?: boolean;
  };
  readonly secondaryActions?: readonly PlaybackSecondaryAction[];
  readonly volume: number;
  readonly muted: boolean;
  readonly fullscreen: boolean;
  readonly onVolumeChange: (volume: number) => void;
  readonly onToggleMuted: () => void;
  /** Receives the chrome's fullscreen target, or the player section without one. */
  readonly onRequestFullscreen: (surface: HTMLElement) => void;
  /** Native video occupies a separate Android surface above the WebView. */
  readonly nativeVideo?: boolean;
  /** The picture is in a separate window (mpv), not in the page. */
  readonly external?: boolean;
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
  privacyCopy,
  onPlaying,
  recoveryAction,
  pause,
  audio,
  status,
  playerSwitch,
  secondaryActions = [],
  volume,
  muted,
  fullscreen,
  onVolumeChange,
  onToggleMuted,
  onRequestFullscreen,
  nativeVideo = false,
  external = false,
  showMediaControls = true,
  stopLabel = "Stop stream",
  onStop,
  onAutoplayFailure,
}: PlaybackSurfaceProps) {
  const chrome = useStageChrome();
  const surfaceRef = useRef<HTMLElement>(null);
  const hideControls = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [controlsVisible, setControlsVisible] = useState(true);
  const playing = state._tag === "playing";
  const canHideControls = fullscreen && playing;
  const hideWhenIdle = useCallback(function hideWhenIdle() {
    const picker = surfaceRef.current?.querySelector("select:focus");
    // A native picker receives no page events while open. Removing its select
    // from layout would dismiss it before the viewer can choose a track.
    const pickerOpen = picker != null && (
      typeof CSS === "undefined" ||
      !CSS.supports("selector(:open)") ||
      picker.matches(":open")
    );
    const draggingVolume = surfaceRef.current?.querySelector('input[type="range"]:active');
    if (pickerOpen || draggingVolume != null) {
      hideControls.current = setTimeout(hideWhenIdle, 2500);
      return;
    }
    hideControls.current = null;
    setControlsVisible(false);
  }, []);
  const revealControls = () => {
    setControlsVisible(true);
    if (hideControls.current !== null) clearTimeout(hideControls.current);
    if (canHideControls)
      hideControls.current = setTimeout(hideWhenIdle, 2500);
  };
  useEffect(() => {
    setControlsVisible(true);
    if (canHideControls)
      hideControls.current = setTimeout(hideWhenIdle, 2500);
    return () => {
      if (hideControls.current !== null) clearTimeout(hideControls.current);
    };
  }, [canHideControls, hideWhenIdle]);

  const { reportPicture } = chrome;
  useEffect(() => {
    reportPicture({ playing, external });
  }, [reportPicture, playing, external]);
  useEffect(() => () => reportPicture(null), [reportPicture]);

  const toggleFullscreen = () => {
    const target = chrome.fullscreenTarget ?? surfaceRef.current;
    if (target !== null) onRequestFullscreen(target);
  };
  const beginBlockedPlayback = () => {
    const video = videoRef.current;
    if (video !== null) {
      void video.play().catch(onAutoplayFailure);
    }
  };
  const presentation = playerPresentation(state);
  const compact = chrome.controls === "compact";
  // Compact buttons show only an icon; the label stays in the DOM (the
  // accessible name and the text scripts match on) and doubles as the hint.
  const hint = (label: string) => (compact ? { title: label } : {});

  // One order for both variants; the phone layout reorders by class in CSS.
  const controls = (
    <div
      className="hosted-player__controls"
      role="group"
      aria-label="Playback controls"
      data-variant={chrome.controls}
    >
      {state._tag === "autoplay-blocked" ? (
        <button
          className="hosted-player__primary-control hosted-player__recovery"
          type="button"
          onClick={beginBlockedPlayback}
        >
          Start audio &amp; video
        </button>
      ) : recoveryAction !== undefined ? (
        <button
          className="hosted-player__primary-control hosted-player__recovery"
          type="button"
          onClick={recoveryAction.onAction}
        >
          <RotateCcw aria-hidden="true" />
          {recoveryAction.label}
        </button>
      ) : null}
      {pause === undefined ? null : (
        <button
          className="hosted-player__primary-control"
          type="button"
          onClick={pause.onPause}
          {...hint("Pause")}
        >
          <Pause aria-hidden="true" />
          <span>Pause</span>
        </button>
      )}
      {showMediaControls ? (
        <>
          <button
            className="hosted-player__primary-control"
            type="button"
            aria-pressed={muted}
            onClick={onToggleMuted}
            {...hint(muted ? "Unmute" : "Mute")}
          >
            {muted ? (
              <VolumeX aria-hidden="true" />
            ) : (
              <Volume2 aria-hidden="true" />
            )}
            <span>{muted ? "Unmute" : "Mute"}</span>
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
        </>
      ) : null}
      {audio}
      {showMediaControls ? (
        <button
          type="button"
          className="hosted-player__primary-control"
          aria-pressed={fullscreen}
          data-stage-action="fullscreen"
          onClick={toggleFullscreen}
          title={
            fullscreen
              ? "Exit fullscreen (Esc)"
              : "Fullscreen (F or double-click)"
          }
        >
          {fullscreen ? (
            <Minimize2 aria-hidden="true" />
          ) : (
            <Maximize2 aria-hidden="true" />
          )}
          <span>{fullscreen ? "Exit fullscreen" : "Full screen"}</span>
        </button>
      ) : null}
      {playerSwitch === undefined ? null : (
        <button
          type="button"
          disabled={playerSwitch.disabled ?? false}
          onClick={playerSwitch.onSwitch}
          {...hint(playerSwitch.label)}
        >
          {playerSwitch.label === "Play in app" ? (
            <MonitorPlay aria-hidden="true" />
          ) : (
            <ExternalLink aria-hidden="true" />
          )}
          <span>{playerSwitch.label}</span>
        </button>
      )}
      {secondaryActions.length === 0 ? null : compact ? (
        <Menu.Root>
          <Menu.Trigger aria-label="More" title="More">
            <Ellipsis aria-hidden="true" />
          </Menu.Trigger>
          <Menu.Portal>
            <Menu.Positioner
              className="hosted-player__menu-positioner"
              side="top"
              align="end"
              sideOffset={6}
            >
              <Menu.Popup className="hosted-player__menu">
                {secondaryActions.map((action) => (
                  <Menu.Item
                    key={action.key}
                    disabled={action.disabled ?? false}
                    onClick={action.onSelect}
                  >
                    {action.icon}
                    {action.label}
                  </Menu.Item>
                ))}
              </Menu.Popup>
            </Menu.Positioner>
          </Menu.Portal>
        </Menu.Root>
      ) : (
        secondaryActions.map((action) => (
          <button
            key={action.key}
            type="button"
            disabled={action.disabled ?? false}
            onClick={action.onSelect}
          >
            {action.icon}
            <span>{action.label}</span>
          </button>
        ))
      )}
      <button
        className="hosted-player__primary-control"
        type="button"
        onClick={onStop}
        {...hint(stopLabel)}
      >
        <Square aria-hidden="true" />
        <span>{stopLabel}</span>
      </button>
      {status === undefined ? null : (
        <div className="hosted-player__status">{status}</div>
      )}
      <p>{privacyCopy}</p>
    </div>
  );

  return (
    <section
      className="hosted-player"
      aria-labelledby="playback-player-heading"
      ref={surfaceRef}
      tabIndex={0}
      data-controls-visible={controlsVisible || !canHideControls}
      data-native-video={nativeVideo}
      onPointerMove={revealControls}
      onPointerDown={revealControls}
      onFocusCapture={revealControls}
      onChangeCapture={revealControls}
      onKeyDown={(event) => {
        revealControls();
        // Portalled controls and the More menu still bubble here through React.
        if (
          event.target instanceof HTMLElement &&
          event.target.closest(
            'input, select, textarea, [contenteditable="true"], [role="menu"]',
          )
        )
          return;
        if (
          event.key.toLowerCase() === "f" &&
          !event.repeat &&
          !event.ctrlKey &&
          !event.metaKey &&
          !event.altKey
        ) {
          event.preventDefault();
          toggleFullscreen();
        }
      }}
    >
      <div className="hosted-player__heading">
        <h2 id="playback-player-heading">{channel.name}</h2>
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

      <div
        className="hosted-player__screen"
        data-state={state._tag}
        onDoubleClick={toggleFullscreen}
      >
        <video
          key={videoKey}
          ref={videoRef}
          aria-label={`${channel.name} live video`}
          autoPlay
          muted={muted}
          playsInline
          onPlaying={onPlaying}
        />
        {!playing ? (
          <div className="hosted-player__overlay">
            <p>{presentation.title}</p>
            <span>{presentation.detail}</span>
          </div>
        ) : external ? (
          <div className="hosted-player__overlay">
            <p>Playing in mpv</p>
            <span>The picture is in the mpv window.</span>
          </div>
        ) : null}
      </div>

      {chrome.controlsSlot === null
        ? controls
        : createPortal(controls, chrome.controlsSlot)}
    </section>
  );
}
