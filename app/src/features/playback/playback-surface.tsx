import { Menu } from "@base-ui/react/menu";
import {
  Ellipsis,
  ExternalLink,
  Maximize2,
  Minimize2,
  MonitorPlay,
  Pause,
  PictureInPicture2,
  RotateCcw,
  Square,
  Volume2,
  VolumeX,
  X,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import type { ChannelId } from "../../client/contracts";
import {
  controlPresentation,
  fullscreenToggleTarget,
  roomUnderPicture,
  useStageChrome,
  type WindowBox,
} from "../stage/stage-chrome";
import { inVisualOrder, visibleStageControls } from "../stage/stage-dom";
import {
  TelevisionHead,
  TelevisionProgress,
  TelevisionQuality,
} from "../stage/television-now";
import { useTelevisionNow } from "../stage/television-now-context";
import { playerPresentation, type PlayerState } from "./playback-presentation";
import "./hosted-player.css";

// How a menu held under the picture finds its place: on the other side of
// its button where that fits, else beside the button, moved along it as far
// as the room requires.
const UNDER_PICTURE_AVOIDANCE = {
  side: "flip",
  align: "shift",
  fallbackAxisSide: "end",
} as const;

/** A less-used action: a plain button in the bar, a "More" menu item when compact. */
export interface PlaybackSecondaryAction {
  readonly key: string;
  readonly label: string;
  readonly icon: ReactNode;
  readonly onSelect: () => void;
  readonly disabled?: boolean;
}

/** Shared chrome inputs with either session-owned or default gesture playback. */
export type PlaybackSurfaceProps = PlaybackSurfaceInputs & (
  | {
      /** Runs synchronously in the gesture; the session owns play and its outcomes. */
      readonly onBeginBlockedPlayback: () => void;
      readonly onAutoplayFailure?: never;
    }
  | {
      readonly onBeginBlockedPlayback?: never;
      /** Receives rejection from the surface's default video.play path. */
      readonly onAutoplayFailure: () => void;
    }
);

interface PlaybackSurfaceInputs {
  readonly channel: { readonly id: ChannelId; readonly name: string };
  /** Immediate heading shown during a pending zap before the session changes. */
  readonly displayName?: string;
  readonly state: PlayerState;
  readonly videoKey: string;
  readonly videoRef: RefObject<HTMLVideoElement>;
  readonly privacyCopy: string;
  readonly onPlaying: () => void;
  readonly recoveryAction?: {
    readonly label: string;
    readonly onAction: () => void;
  };
  readonly pause?: {
    readonly onPause: () => void;
    /** Keeps the Pause button mounted and focusable while suspending settles. */
    readonly pending?: boolean;
  };
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
  /** Shrinks the app into Android's picture-in-picture window. */
  readonly pictureInPicture?: { readonly onEnter: () => void };
  readonly volume: number;
  readonly muted: boolean;
  readonly fullscreen: boolean;
  readonly onVolumeChange: (volume: number) => void;
  readonly onToggleMuted: () => void;
  /**
   * Receives the element to make fullscreen: the chrome's target, or the
   * player section without one. While the player is fullscreen it receives
   * the element that is, to leave it.
   */
  readonly onRequestFullscreen: (surface: HTMLElement) => void;
  /** Native video occupies a separate Android surface above the WebView. */
  readonly nativeVideo?: boolean;
  /** The picture is in a separate window (mpv), not in the page. */
  readonly external?: boolean;
  /** The player knows the picture has no sound the viewer can hear. */
  readonly silent?: boolean;
  readonly showMediaControls?: boolean;
  readonly stopLabel?: string;
  readonly onStop: () => void;
}

/** Accessible playback chrome shared without sharing transport ownership. */
export function PlaybackSurface({
  channel,
  displayName,
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
  pictureInPicture,
  volume,
  muted,
  fullscreen,
  onVolumeChange,
  onToggleMuted,
  onRequestFullscreen,
  nativeVideo = false,
  external = false,
  silent = false,
  showMediaControls = true,
  stopLabel = "Stop stream",
  onStop,
  onBeginBlockedPlayback,
  onAutoplayFailure,
}: PlaybackSurfaceProps) {
  const chrome = useStageChrome();
  const { television, fullscreenTune } = chrome;
  // The TV shell's lines about the playing Channel: the fullscreen scrims
  // and the quality pill. Null outside a TV shell.
  const televisionNow = useTelevisionNow();
  const tvNow = television === true ? televisionNow : null;
  const surfaceRef = useRef<HTMLElement>(null);
  useLayoutEffect(() => {
    const section = surfaceRef.current;
    if (
      !television ||
      section === null ||
      fullscreenTune?.channel !== channel.id ||
      !fullscreenTune.take() ||
      document.fullscreenElement != null ||
      navigator.userActivation?.isActive !== true
    ) {
      return;
    }
    // The viewer's activation may expire while the player chunk loads. The
    // existing fullscreenchange observers reconcile the native viewport.
    void section.requestFullscreen?.().catch(() => undefined);
  }, [channel.id, fullscreenTune, television]);
  const hideControls = useRef<ReturnType<typeof setTimeout> | null>(null);
  const focusFrameRef = useRef<number | null>(null);
  const suppressRevealRef = useRef(false);
  const returnToFullscreenButtonRef = useRef(false);
  const wasSectionFullscreenRef = useRef(false);
  const [controlsVisible, setControlsVisible] = useState(true);
  // Where the More menu may open while it must keep clear of the picture;
  // measured each time it opens, because the picture's box moves with the
  // window and the layout.
  const [menuRoom, setMenuRoom] = useState<WindowBox | null>(null);
  const playing = state._tag === "playing";
  const canHideControls = fullscreen && playing;
  const cancelPendingFocus = useCallback(() => {
    if (focusFrameRef.current !== null) {
      cancelAnimationFrame(focusFrameRef.current);
      focusFrameRef.current = null;
    }
  }, []);
  const focusInFullscreenControls = useCallback((): boolean => {
    const section = surfaceRef.current;
    const controlsEl = section?.querySelector(".hosted-player__controls");
    return (
      television === true &&
      section !== null &&
      document.fullscreenElement === section &&
      controlsEl != null &&
      document.activeElement !== null &&
      controlsEl.contains(document.activeElement)
    );
  }, [television]);
  const hideControlsCleanly = useCallback(() => {
    cancelPendingFocus();
    if (hideControls.current !== null) {
      clearTimeout(hideControls.current);
      hideControls.current = null;
    }
    const section = surfaceRef.current;
    suppressRevealRef.current = true;
    if (television && section !== null && document.fullscreenElement === section) {
      section.focus({ preventScroll: true });
    }
    setControlsVisible(false);
    suppressRevealRef.current = false;
  }, [cancelPendingFocus, television]);
  const hideWhenIdle = useCallback(
    function hideWhenIdle() {
      const section = surfaceRef.current;
      const picker = section?.querySelector("select:focus");
      // A native picker receives no page events while open. Removing its select
      // from layout would dismiss it before the viewer can choose a track.
      const pickerOpen =
        picker != null &&
        (typeof CSS === "undefined" ||
          !CSS.supports("selector(:open)") ||
          picker.matches(":open"));
      const menuOpen =
        document.querySelector('[role="menu"][data-open]') !== null;
      const draggingVolume = section?.querySelector(
        'input[type="range"]:active',
      );
      if (pickerOpen || menuOpen || draggingVolume != null) {
        hideControls.current = setTimeout(hideWhenIdle, 2500);
        return;
      }
      hideControlsCleanly();
    },
    [hideControlsCleanly],
  );
  const scheduleHide = useCallback(
    (preferExtended = false) => {
      if (hideControls.current !== null) {
        clearTimeout(hideControls.current);
        hideControls.current = null;
      }
      if (canHideControls) {
        const delayMs =
          preferExtended || focusInFullscreenControls() ? 7500 : 2500;
        hideControls.current = setTimeout(hideWhenIdle, delayMs);
      }
    },
    [canHideControls, focusInFullscreenControls, hideWhenIdle],
  );
  const revealControls = useCallback(
    (preferExtended = false) => {
      if (suppressRevealRef.current) return;
      setControlsVisible(true);
      scheduleHide(preferExtended);
    },
    [scheduleHide],
  );
  const scheduleFullscreenBarFocus = useCallback(
    (pick: "first" | "last") => {
      cancelPendingFocus();
      focusFrameRef.current = requestAnimationFrame(() => {
        focusFrameRef.current = null;
        const section = surfaceRef.current;
        if (section === null || document.fullscreenElement !== section) return;
        const items = visibleControlBarItems(section);
        const destination =
          pick === "last" ? items[items.length - 1] : items[0];
        destination?.focus({ preventScroll: true });
      });
    },
    [cancelPendingFocus],
  );
  useEffect(() => {
    setControlsVisible(true);
    scheduleHide();
    return () => {
      cancelPendingFocus();
      if (hideControls.current !== null) {
        clearTimeout(hideControls.current);
        hideControls.current = null;
      }
    };
  }, [cancelPendingFocus, scheduleHide]);

  // Whether this section is the document's fullscreen element. The document
  // says so itself: the chrome's target can change while the section is
  // still fullscreen.
  const [sectionFullscreen, setSectionFullscreen] = useState(false);
  useEffect(() => {
    const update = () => {
      const section = surfaceRef.current;
      const active = section !== null && document.fullscreenElement === section;
      const wasActive = wasSectionFullscreenRef.current;
      wasSectionFullscreenRef.current = active;
      setSectionFullscreen(active);
      if (television && active) {
        returnToFullscreenButtonRef.current = false;
        section.focus({ preventScroll: true });
      } else if (television && wasActive && !active) {
        cancelPendingFocus();
      }
    };
    document.addEventListener("fullscreenchange", update);
    update();
    return () => {
      document.removeEventListener("fullscreenchange", update);
      returnToFullscreenButtonRef.current = false;
    };
  }, [cancelPendingFocus, television]);

  const presentation = playerPresentation(state);
  const { reportPicture } = chrome;
  useEffect(() => {
    reportPicture({
      state: state._tag,
      status: presentation.status,
      external,
      silent,
    });
  }, [reportPicture, state._tag, presentation.status, external, silent]);
  useEffect(() => () => reportPicture(null), [reportPicture]);

  const toggleFullscreen = () => {
    const target = fullscreenToggleTarget({
      fullscreen,
      fullscreenElement: document.fullscreenElement,
      target: chrome.fullscreenTarget,
      section: surfaceRef.current,
    });
    if (target !== null) {
      if (television && sectionFullscreen && target === surfaceRef.current) {
        returnToFullscreenButtonRef.current = true;
      }
      onRequestFullscreen(target);
    }
  };
  const beginBlockedPlayback = () => {
    if (onBeginBlockedPlayback !== undefined) {
      onBeginBlockedPlayback();
      return;
    }
    const video = videoRef.current;
    if (video !== null) {
      void video.play().catch(onAutoplayFailure);
    }
  };
  // While the player section is the fullscreen element the controls are its
  // own inline bar; the shell's slot lies outside what fullscreen draws.
  const controlsPlacement = controlPresentation(chrome, sectionFullscreen);
  useLayoutEffect(() => {
    if (
      !television ||
      sectionFullscreen ||
      !returnToFullscreenButtonRef.current
    ) {
      return;
    }
    const container = controlsPlacement.slot ?? surfaceRef.current;
    const button = container?.querySelector<HTMLElement>(
      '[data-stage-action="fullscreen"]',
    );
    if (button == null) return;
    returnToFullscreenButtonRef.current = false;
    button.focus({ preventScroll: true });
  }, [controlsPlacement.slot, sectionFullscreen, television]);
  const compact = controlsPlacement.variant === "compact";
  // Compact buttons show only an icon; the label stays in the DOM (the
  // accessible name and the text scripts match on) and doubles as the hint.
  const hint = (label: string) => (compact ? { title: label } : {});

  // One order for both variants; the pocket layout reorders by class in CSS.
  const controls = (
    <div
      className="hosted-player__controls"
      role="group"
      aria-label="Playback controls"
      data-variant={controlsPlacement.variant}
    >
      {state._tag === "autoplay-blocked" ? (
        <button
          key="primary-playback-action"
          className="hosted-player__primary-control hosted-player__recovery"
          type="button"
          onClick={beginBlockedPlayback}
        >
          Start audio &amp; video
        </button>
      ) : recoveryAction !== undefined ? (
        <button
          key="primary-playback-action"
          className="hosted-player__primary-control hosted-player__recovery"
          type="button"
          onClick={recoveryAction.onAction}
        >
          <RotateCcw aria-hidden="true" />
          {recoveryAction.label}
        </button>
      ) : pause !== undefined ? (
        <button
          key="primary-playback-action"
          className="hosted-player__primary-control"
          type="button"
          aria-disabled={pause.pending || undefined}
          aria-busy={pause.pending || undefined}
          onClick={pause.pending ? undefined : pause.onPause}
          {...hint("Pause")}
        >
          <Pause aria-hidden="true" />
          <span>Pause</span>
        </button>
      ) : null}
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
          <label
            className="hosted-player__volume"
            aria-hidden={television || undefined}
          >
            <span>Volume</span>
            <input
              type="range"
              min="0"
              max="100"
              step="1"
              value={Math.round(volume * 100)}
              aria-label="Volume"
              tabIndex={television ? -1 : undefined}
              onChange={(event) =>
                onVolumeChange(Number(event.currentTarget.value) / 100)
              }
            />
          </label>
        </>
      ) : null}
      {audio}
      {tvNow !== null && showMediaControls ? (
        <TelevisionQuality info={tvNow} />
      ) : null}
      {pictureInPicture === undefined ? null : (
        <button
          type="button"
          className="hosted-player__primary-control hosted-player__picture-in-picture"
          onClick={pictureInPicture.onEnter}
          {...hint("Picture in picture")}
        >
          <PictureInPicture2 aria-hidden="true" />
          <span>Picture in picture</span>
        </button>
      )}
      {showMediaControls ? (
        <button
          type="button"
          className="hosted-player__primary-control hosted-player__fullscreen"
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
          className="hosted-player__switch"
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
        <Menu.Root
          onOpenChange={(open) => {
            const video = videoRef.current;
            if (open && chrome.menuSide === "bottom" && video !== null) {
              setMenuRoom(
                roomUnderPicture(video.getBoundingClientRect().bottom, {
                  width: window.innerWidth,
                  height: window.innerHeight,
                }),
              );
            }
          }}
        >
          <Menu.Trigger
            className="hosted-player__more"
            aria-label="More"
            title="More"
          >
            <Ellipsis aria-hidden="true" />
          </Menu.Trigger>
          <Menu.Portal>
            <Menu.Positioner
              className="hosted-player__menu-positioner"
              side={chrome.menuSide}
              align="end"
              sideOffset={6}
              {...(chrome.menuSide === "bottom" && menuRoom !== null
                ? {
                    collisionBoundary: menuRoom,
                    collisionAvoidance: UNDER_PICTURE_AVOIDANCE,
                  }
                : {})}
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
        className="hosted-player__primary-control hosted-player__stop"
        type="button"
        onClick={onStop}
        {...hint(stopLabel)}
      >
        {tvNow === null ? (
          <Square aria-hidden="true" />
        ) : showMediaControls ? (
          // A filled square reads as Stop; the outline reads as a checkbox.
          <Square aria-hidden="true" fill="currentColor" strokeWidth={0} />
        ) : (
          <X aria-hidden="true" />
        )}
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
      tabIndex={television ? -1 : 0}
      data-controls-visible={controlsVisible || !canHideControls}
      data-native-video={nativeVideo}
      data-player-state={tvNow === null ? undefined : state._tag}
      onPointerMove={() => revealControls()}
      onPointerDown={() => revealControls()}
      onFocusCapture={() => revealControls()}
      onChangeCapture={() => revealControls()}
      onKeyDown={(event) => {
        if (
          television &&
          sectionFullscreen &&
          event.key === "Escape" &&
          !event.defaultPrevented &&
          !event.nativeEvent.isComposing
        ) {
          if (controlsVisible && canHideControls) {
            event.preventDefault();
            event.stopPropagation();
            hideControlsCleanly();
          }
          // When controls are already hidden or cannot hide, let Escape bubble
          // without revealing controls so TV Back/Escape exits fullscreen to Guide.
          return;
        }
        const target = event.target instanceof HTMLElement ? event.target : null;
        if (television && sectionFullscreen && target === event.currentTarget) {
          if (event.key === "Enter") {
            event.preventDefault();
            if (event.repeat) return;
            if (!controlsVisible && canHideControls) {
              revealControls(true);
              scheduleFullscreenBarFocus("first");
              return;
            }
            revealControls();
            (
              pause?.pending
                ? undefined
                : (pause?.onPause ??
                  recoveryAction?.onAction ??
                  (state._tag === "autoplay-blocked"
                    ? beginBlockedPlayback
                    : undefined))
            )?.();
            return;
          }
          if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
            event.preventDefault();
            revealControls(true);
            scheduleFullscreenBarFocus(
              event.key === "ArrowLeft" ? "last" : "first",
            );
            return;
          }
        }

        if (
          television &&
          sectionFullscreen &&
          target !== null &&
          !event.defaultPrevented &&
          !event.altKey &&
          !event.ctrlKey &&
          !event.metaKey &&
          target.closest(".hosted-player__controls") !== null &&
          target.closest('[role="menu"]') === null &&
          document.querySelector('[role="menu"][data-open]') === null &&
          (event.key === "ArrowLeft" ||
            event.key === "ArrowRight" ||
            event.key === "ArrowUp" ||
            event.key === "ArrowDown")
        ) {
          const bar = target.closest<HTMLElement>(".hosted-player__controls");
          const items = visibleControlBarItems(bar);
          const index = items.indexOf(target);
          event.preventDefault();
          event.stopPropagation();
          if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
            revealControls(true);
            if (index !== -1) {
              items[event.key === "ArrowRight" ? index + 1 : index - 1]?.focus({
                preventScroll: true,
              });
            }
            return;
          }
          if (event.key === "ArrowUp") {
            if (canHideControls) {
              hideControlsCleanly();
            } else {
              surfaceRef.current?.focus({ preventScroll: true });
            }
          } else {
            revealControls(true);
          }
          return;
        }

        if (event.key !== "Escape") {
          revealControls();
        }
        // Portalled controls and the More menu still bubble here through React.
        if (
          target !== null &&
          target.closest(
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
        <h2 id="playback-player-heading">{displayName ?? channel.name}</h2>
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
          // Android WebView draws a grey play glyph on a video with no frame
          // yet; a transparent poster keeps the starting and failed states clean.
          {...(television === true ? { poster: TRANSPARENT_POSTER } : {})}
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

      {tvNow !== null && sectionFullscreen ? (
        <>
          <TelevisionHead
            info={tvNow}
            state={state._tag === "playing" ? null : presentation.status}
          />
          {playing ? <TelevisionProgress info={tvNow} /> : null}
        </>
      ) : null}

      {controlsPlacement.slot === null
        ? controls
        : createPortal(controls, controlsPlacement.slot)}
    </section>
  );
}

const TRANSPARENT_POSTER =
  "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

function visibleControlBarItems(root: HTMLElement | null): HTMLElement[] {
  if (root === null) return [];
  const scope = root.classList.contains("hosted-player")
    ? root.querySelector<HTMLElement>(".hosted-player__controls")
    : root;
  // The TV bar orders its controls by class; the remote follows what is seen.
  return inVisualOrder(visibleStageControls(scope, "button, select"));
}
