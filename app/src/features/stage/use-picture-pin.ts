import { useLayoutEffect } from "react";
import { stageMonitorSize } from "./stage-dom";

/** The root properties `pocket.css` sizes a held picture by. */
const PINNED_WIDTH = "--pocket-pinned-w";
const PINNED_HEIGHT = "--pocket-pinned-h";

/**
 * Holds the picture's box at the size it has for as long as the picture
 * cannot follow it; see `pictureFollows`. The dock latch keeps the box from
 * changing with the mode, but its size also depends on the window, and the
 * soft keyboard makes the window smaller. Native Android video would then
 * stay at its old rectangle, over whatever the page had moved under it.
 *
 * The box is measured once, when following stops, and its size is written to
 * the document root in pixels. The root, because the sheets are portalled
 * outside the shell and open under the picture's bottom edge. The properties
 * are removed when the picture follows again or the player goes away.
 */
export function usePicturePin(follows: boolean): void {
  // Before paint, so the box is never drawn at a size the picture has left.
  useLayoutEffect(() => {
    if (follows) {
      return;
    }
    const size = stageMonitorSize();
    if (size === null) {
      return;
    }
    const root = document.documentElement;
    root.style.setProperty(PINNED_WIDTH, `${size.width}px`);
    root.style.setProperty(PINNED_HEIGHT, `${size.height}px`);
    return () => {
      root.style.removeProperty(PINNED_WIDTH);
      root.style.removeProperty(PINNED_HEIGHT);
    };
  }, [follows]);
}
