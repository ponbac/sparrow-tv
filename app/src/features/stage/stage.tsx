import { Maximize2 } from "lucide-react";
import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";
import type { ChannelId } from "../../client/contracts";
import {
  focusGuideFallback,
  handleGuideDockArrowKey,
} from "../guide/guide-navigation";
import {
  focusIsInChannelBar,
  isPictureTap,
  visibleStageControls,
} from "./stage-dom";
import "./stage.css";

/** Inputs for the picture and the block of information beside it. */
export interface StageProps {
  /** On a TV the shell keeps focus on an actionable control. */
  readonly television?: boolean;
  /** The playing Channel, or null while nothing is playing. */
  readonly playingChannel: ChannelId | null;
  /** The player for the playing Channel; null shows the standby note. */
  readonly player: ReactNode;
  /** The info block; it carries the stage heading. */
  readonly info: ReactNode;
  /**
   * What changes Channel from the stage: Theater's row of nearby Channels or
   * pocket's channel bar. Null where there is none.
   */
  readonly rail: ReactNode;
  /** Shows which keys work over the full picture. */
  readonly keyHints: boolean;
  /**
   * Returns from the guide to the full picture. With it, a tap on the picture
   * and the button beside the docked picture both do so: pocket's way back.
   * On a TV, which has no watch mode, it takes the docked picture fullscreen.
   * Null in Theater, where the masthead and the keys do it.
   */
  readonly onShowPicture: (() => void) | null;
}

/** Presents live playback above its information so native Android video never obscures controls. */
export function Stage({
  television = false,
  playingChannel,
  player,
  info,
  rail,
  keyHints,
  onShowPicture,
}: StageProps) {
  const stageRef = useRef<HTMLElement>(null);
  const monitorRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (playingChannel === null || television) return;
    // Let search dialogs restore focus first, then dismiss the soft keyboard
    // without scrolling the fixed stage away from its native video viewport.
    const frame = requestAnimationFrame(() => {
      // A press in the channel bar changed the Channel: focus stays on the
      // button, ready for the next press.
      if (!focusIsInChannelBar()) {
        monitorRef.current?.focus({ preventScroll: true });
      }
    });
    return () => cancelAnimationFrame(frame);
  }, [playingChannel, television]);

  const handleStageKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (
      event.defaultPrevented ||
      event.altKey ||
      event.ctrlKey ||
      event.metaKey ||
      event.nativeEvent.isComposing ||
      document.fullscreenElement != null ||
      document.querySelector('[role="menu"][data-open]') !== null ||
      !(event.target instanceof HTMLElement)
    ) {
      return;
    }
    const { key, target } = event;
    // On a TV the docked picture's box has controls only when the player
    // could not load: its Reload and Close actions, reached by Center on the
    // Channel. Back and Down return to the Guide, not out of the app.
    const loadNoticeActions = television
      ? target.closest<HTMLElement>(".playback-load-notice__actions")
      : null;
    if (loadNoticeActions !== null && (key === "Escape" || key === "ArrowDown")) {
      event.preventDefault();
      event.stopPropagation();
      focusGuideFallback(document.querySelector(".programme-guide"), {
        requireLayoutBox: true,
      });
      return;
    }
    if (
      key !== "ArrowUp" &&
      key !== "ArrowDown" &&
      key !== "ArrowLeft" &&
      key !== "ArrowRight"
    ) {
      return;
    }
    const stage = stageRef.current;
    if (stage === null) return;
    if (target.classList.contains("stage__undock")) {
      handleGuideDockArrowKey(event, stage);
      return;
    }
    if (loadNoticeActions === null) return;
    if (key === "ArrowLeft" || key === "ArrowRight") {
      stepHorizontal(event, visibleStageControls(loadNoticeActions, "button"), target);
      return;
    }
    // Up: nothing lies above the box.
    event.preventDefault();
    event.stopPropagation();
  };

  return (
    <section
      className="stage"
      ref={stageRef}
      aria-labelledby="stage-heading"
      onKeyDown={handleStageKeyDown}
    >
      <div
        className="stage__monitor"
        ref={monitorRef}
        tabIndex={-1}
        // A tap on native Android video reaches the page under it.
        onClick={
          onShowPicture === null
            ? undefined
            : (event) => {
                if (isPictureTap(event.currentTarget, event.target)) {
                  onShowPicture();
                }
              }
        }
      >
        {player ?? (
          <div className="stage__standby" role="status">
            Nothing playing
          </div>
        )}
      </div>
      {info}
      {onShowPicture === null || playingChannel === null ? null : (
        <button
          className="stage__undock"
          type="button"
          aria-label={television ? "Watch full screen" : "Back to the picture"}
          onClick={() => {
            onShowPicture();
            // The button leaves the layout with the guide; focus goes on to
            // the picture it returned to.
            if (!television) monitorRef.current?.focus({ preventScroll: true });
          }}
        >
          <Maximize2 aria-hidden="true" />
        </button>
      )}
      {rail}
      {keyHints ? (
        <p className="stage__keys" aria-hidden="true">
          <span>
            <kbd>↑</kbd>
            <kbd>↓</kbd>
            Change channel
          </span>
          <span>
            <kbd>G</kbd>
            Guide
          </span>
          <span>
            <kbd>/</kbd>
            Search
          </span>
          <span>
            <kbd>F</kbd>
            Full screen
          </span>
        </p>
      ) : null}
    </section>
  );
}

function stepHorizontal(
  event: KeyboardEvent<HTMLElement>,
  items: readonly HTMLElement[],
  target: HTMLElement,
): void {
  const index = items.indexOf(target);
  if (index === -1) return;
  event.preventDefault();
  event.stopPropagation();
  items[event.key === "ArrowRight" ? index + 1 : index - 1]?.focus({
    preventScroll: true,
  });
}
