import { useEffect, useRef, type ReactNode } from "react";
import type { ChannelId } from "../../client/contracts";
import "./stage.css";

/** Inputs for the picture and the block of information beside it. */
export interface StageProps {
  /** The playing Channel, or null while nothing is playing. */
  readonly playingChannel: ChannelId | null;
  /** The player for the playing Channel; null shows the standby note. */
  readonly player: ReactNode;
  /** The info block; it carries the stage heading. */
  readonly info: ReactNode;
  /** The row of nearby Channels; null wherever nothing lies over the picture. */
  readonly rail: ReactNode;
  /** Shows which keys work over the full picture. */
  readonly keyHints: boolean;
}

/** Presents live playback above its information so native Android video never obscures controls. */
export function Stage({
  playingChannel,
  player,
  info,
  rail,
  keyHints,
}: StageProps) {
  const monitorRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (playingChannel === null) return;
    // Let search dialogs restore focus first, then dismiss the soft keyboard
    // without scrolling the fixed stage away from its native video viewport.
    const frame = requestAnimationFrame(() => {
      monitorRef.current?.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, [playingChannel]);

  return (
    <section className="stage" aria-labelledby="stage-heading">
      <div className="stage__monitor" ref={monitorRef} tabIndex={-1}>
        {player ?? (
          <div className="stage__standby" role="status">
            Nothing playing
          </div>
        )}
      </div>
      {info}
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
