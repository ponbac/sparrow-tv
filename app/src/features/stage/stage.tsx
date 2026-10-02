import { Maximize2 } from "lucide-react";
import { useEffect, useRef, type ReactNode } from "react";
import type { ChannelId } from "../../client/contracts";
import { focusIsInChannelBar, isPictureTap } from "./stage-dom";
import "./stage.css";

/** Inputs for the picture and the block of information beside it. */
export interface StageProps {
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
}

/** Presents live playback above its information so native Android video never obscures controls. */
export function Stage({
  playingChannel,
  player,
  info,
  rail,
  keyHints,
  onShowPicture,
}: StageProps) {
  const monitorRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (playingChannel === null) return;
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
  }, [playingChannel]);

  return (
    <section className="stage" aria-labelledby="stage-heading">
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
      {onShowPicture === null ? null : (
        <button
          className="stage__undock"
          type="button"
          aria-label="Back to the picture"
          onClick={() => {
            onShowPicture();
            // The button leaves the layout with the guide; focus goes on to
            // the picture it returned to.
            monitorRef.current?.focus({ preventScroll: true });
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
