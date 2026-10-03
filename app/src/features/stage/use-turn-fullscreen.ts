import { useEffect, useRef } from "react";
import { playerFullscreenButton } from "./stage-dom";

/** What the shell knows of the picture when a phone is turned. */
export interface TurnFullscreenPicture {
  /**
   * A turn onto the side should take the picture fullscreen: there is a live
   * picture in the page and it is what the viewer is looking at. A turn made
   * while browsing the guide, or with nothing playing, only turns the layout.
   */
  readonly watching: boolean;
  /**
   * The picture moves with its box. A native picture that is paused or has
   * failed stays where it was, so leaving fullscreen under it must wait.
   */
  readonly follows: boolean;
}

/**
 * Takes the picture fullscreen when a phone is turned on its side, and back
 * out when it is turned upright again.
 *
 * A document may go fullscreen without a press only from inside the event
 * that reports the turn, so the player's own Full screen button is pressed
 * there, in the same task. Fullscreen the viewer asked for is theirs to
 * leave: turning back ends only fullscreen that a turn began. It ends it once
 * the picture can follow its box, which may be later than the turn.
 */
export function useTurnFullscreen({
  watching,
  follows,
}: TurnFullscreenPicture): void {
  const picture = useRef({ watching, follows });
  const turns = useRef({
    /** The phone is on its side. */
    onSide: false,
    /** A turn pressed Full screen and the document has not answered yet. */
    asked: false,
    /** The document is fullscreen because of a turn. */
    turned: false,
  });

  useEffect(() => {
    picture.current = { watching, follows };
    // Turned upright while the picture could not follow, the turn's
    // fullscreen stayed. It ends now that the picture can.
    if (follows) {
      leaveTurnFullscreen(turns.current);
    }
  }, [watching, follows]);

  useEffect(() => {
    const orientation =
      typeof screen === "undefined" ? undefined : screen.orientation;
    if (orientation === undefined) {
      return;
    }
    const state = turns.current;
    state.onSide = isOnSide(orientation);
    const turn = () => {
      const next = isOnSide(orientation);
      // A half turn from one side to the other changes nothing here.
      if (next === state.onSide) {
        return;
      }
      state.onSide = next;
      if (!next) {
        if (picture.current.follows) {
          leaveTurnFullscreen(state);
        }
        return;
      }
      if (!isHandheld() || !picture.current.watching || documentIsFullscreen()) {
        return;
      }
      const button = playerFullscreenButton();
      if (button !== null) {
        state.asked = true;
        button.click();
      }
    };
    const settle = () => {
      state.turned = state.asked && documentIsFullscreen();
      state.asked = false;
    };
    const refuse = () => {
      state.asked = false;
    };
    orientation.addEventListener("change", turn);
    document.addEventListener("fullscreenchange", settle);
    document.addEventListener("fullscreenerror", refuse);
    return () => {
      orientation.removeEventListener("change", turn);
      document.removeEventListener("fullscreenchange", settle);
      document.removeEventListener("fullscreenerror", refuse);
    };
  }, []);
}

/** Ends fullscreen a turn began, once the phone is upright again. */
function leaveTurnFullscreen(state: {
  readonly onSide: boolean;
  readonly turned: boolean;
}): void {
  if (state.turned && !state.onSide && documentIsFullscreen()) {
    // A refusal leaves the window as it was; there is nothing to tell.
    void document.exitFullscreen().catch(() => undefined);
  }
}

/** Whether anything is fullscreen; a document without the API has nothing. */
function documentIsFullscreen(): boolean {
  return document.fullscreenElement != null;
}

function isOnSide(orientation: ScreenOrientation): boolean {
  return orientation.type.startsWith("landscape");
}

/**
 * Whether this is a device held in the hand. A desktop screen changes
 * orientation too, when a window moves to a monitor that stands upright, and
 * that is no reason to take the window fullscreen.
 */
function isHandheld(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia("(pointer: coarse)").matches
  );
}
