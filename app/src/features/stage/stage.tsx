import { Maximize2 } from "lucide-react";
import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";
import type { ChannelId } from "../../client/contracts";
import { handleGuideDockArrowKey } from "../guide/guide-navigation";
import {
  focusIsInChannelBar,
  inVisualOrder,
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
   * Null in Theater, where the masthead and the keys do it.
   */
  readonly onShowPicture: (() => void) | null;
  /**
   * On a TV, Up and Down on the control row change Channel: the move a
   * remote's D-pad makes while watching. Without it they move focus.
   */
  readonly onZap?: (direction: -1 | 1) => void;
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
  onZap,
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
    if (!television) return;

    // The control row comes first: on a TV its quality pill is part of it.
    const playbackControls = target.closest<HTMLElement>(".now-playing__controls");
    if (playbackControls !== null) {
      if (key === "ArrowLeft" || key === "ArrowRight") {
        stepHorizontal(
          event,
          inVisualOrder(visibleStageControls(playbackControls, "button, select")),
          target,
        );
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      const notice = firstLoadNoticeControl(stage);
      if (key === "ArrowUp") {
        if (notice === undefined && onZap !== undefined) {
          onZap(-1);
          return;
        }
        (notice ?? preferredQualityChip(stage))?.focus({
          preventScroll: true,
        });
      } else if (onZap !== undefined) {
        onZap(1);
      } else {
        preferredChannelBarButton(stage)?.focus({ preventScroll: true });
      }
      return;
    }

    const qualitySwitch = target.closest<HTMLElement>(".now-playing .quality-switch");
    if (qualitySwitch !== null) {
      if (key === "ArrowLeft" || key === "ArrowRight") {
        stepHorizontal(event, visibleStageControls(qualitySwitch, "button"), target);
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      if (key === "ArrowDown") {
        (
          firstLoadNoticeControl(stage) ??
          firstPlaybackControl(stage) ??
          preferredChannelBarButton(stage)
        )?.focus({ preventScroll: true });
      }
      return;
    }

    const loadNoticeActions = target.closest<HTMLElement>(".playback-load-notice__actions");
    if (loadNoticeActions !== null) {
      if (key === "ArrowLeft" || key === "ArrowRight") {
        stepHorizontal(event, visibleStageControls(loadNoticeActions, "button"), target);
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      if (key === "ArrowUp") {
        preferredQualityChip(stage)?.focus({ preventScroll: true });
      } else {
        (firstPlaybackControl(stage) ?? preferredChannelBarButton(stage))?.focus({
          preventScroll: true,
        });
      }
      return;
    }

    const channelBar = target.closest<HTMLElement>(".channel-bar");
    if (channelBar !== null) {
      if (key === "ArrowLeft" || key === "ArrowRight") {
        stepHorizontal(event, visibleStageControls(channelBar, "button"), target);
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      if (key === "ArrowUp") {
        (
          firstLoadNoticeControl(stage) ??
          firstPlaybackControl(stage) ??
          preferredQualityChip(stage)
        )?.focus({ preventScroll: true });
      }
    }
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
          aria-label="Back to the picture"
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
      {television ? (
        // What the remote does while watching; the bar's own buttons say
        // which Channels lie on either side.
        <p className="stage__tv-keys" aria-hidden="true">
          <span>
            <kbd>OK</kbd>
            Select
          </span>
          <span>
            <kbd>▲▼</kbd>
            Change channel
          </span>
          <span>
            <kbd>BACK</kbd>
            Guide
          </span>
        </p>
      ) : null}
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

function firstLoadNoticeControl(stage: HTMLElement): HTMLElement | undefined {
  return visibleStageControls(
    stage.querySelector<HTMLElement>(".playback-load-notice__actions"),
    "button",
  )[0];
}

function firstPlaybackControl(stage: HTMLElement): HTMLElement | undefined {
  return visibleStageControls(
    stage.querySelector<HTMLElement>(".now-playing__controls"),
    "button, select",
  )[0];
}

function preferredChannelBarButton(stage: HTMLElement): HTMLElement | undefined {
  const bar = stage.querySelector<HTMLElement>(".channel-bar");
  if (bar === null) return undefined;
  const buttons = visibleStageControls(bar, "button");
  const guide = bar.querySelector<HTMLElement>(".channel-bar__guide");
  return guide !== null && buttons.includes(guide) ? guide : buttons[0];
}

function preferredQualityChip(stage: HTMLElement): HTMLElement | undefined {
  // The info block's own switch; a TV player's quality pill sits in the row.
  const switchGroup = stage.querySelector<HTMLElement>(
    ".now-playing__channel .quality-switch",
  );
  if (switchGroup === null) return undefined;
  const chips = visibleStageControls(switchGroup, "button");
  return chips.find((chip) => chip.getAttribute("data-current") === "true") ?? chips[0];
}
