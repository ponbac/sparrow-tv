import { useLayoutEffect, useRef, useState } from "react";
import { stageMonitorBox } from "./stage-dom";

/** The root properties `pocket.css` sizes a held picture by. */
const PINNED_WIDTH = "--pocket-pinned-w";
const PINNED_HEIGHT = "--pocket-pinned-h";
/**
 * Where the TV holds its picture (`television.css`): the TV places the box
 * and what lies around it in units of the window's width, so the origin is
 * held with the size.
 */
const PINNED_LEFT = "--pocket-pinned-x";
const PINNED_TOP = "--pocket-pinned-y";

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
 *
 * Android's picture-in-picture window shrinks the page, and Android puts the
 * picture back over its box only while it plays. A picture that stopped
 * following during or before picture-in-picture is therefore not pinned again
 * until it follows: leaving the window shows the page in its place, at the
 * size the expanded window gives it.
 */
export function usePicturePin(
  follows: boolean,
  pictureInPicture: boolean,
  television = false,
  mode?: "watch" | "guide",
): void {
  const [shrunk, setShrunk] = useState(false);
  if (pictureInPicture && !shrunk) {
    setShrunk(true);
  } else if (follows && !pictureInPicture && shrunk) {
    setShrunk(false);
  }
  const hold = !follows && !pictureInPicture && !shrunk;
  const wasFullscreenRef = useRef(false);
  const pendingExitRemeasureRef = useRef(false);
  const lastModeRef = useRef(mode);

  // Before paint, so the box is never drawn at a size the picture has left.
  useLayoutEffect(() => {
    if (!hold) {
      wasFullscreenRef.current = document.fullscreenElement != null;
      pendingExitRemeasureRef.current = false;
      return;
    }
    const root = document.documentElement;
    let clearExitFrame: number | null = null;
    function measure() {
      unpinPicture(root);
      // Android temporarily restores its default page scale in element fullscreen.
      // The hidden monitor has the right coordinates again on fullscreen exit.
      if (television && document.fullscreenElement != null) {
        wasFullscreenRef.current = true;
        pendingExitRemeasureRef.current = false;
        return;
      }
      pinPicture(root, television);
    }
    function onFullscreenChange() {
      if (clearExitFrame !== null) {
        cancelAnimationFrame(clearExitFrame);
        clearExitFrame = null;
      }
      if (document.fullscreenElement != null) {
        wasFullscreenRef.current = true;
        pendingExitRemeasureRef.current = false;
        unpinPicture(root);
        return;
      }
      if (wasFullscreenRef.current) {
        wasFullscreenRef.current = false;
        pendingExitRemeasureRef.current = true;
        // Expire the post-exit remeasure flag if fullscreen ended without a
        // subsequent mode commit (for example via the Exit fullscreen button).
        clearExitFrame = requestAnimationFrame(() => {
          clearExitFrame = null;
          pendingExitRemeasureRef.current = false;
        });
      }
      measure();
    }
    measure();
    if (television) document.addEventListener("fullscreenchange", onFullscreenChange);
    return () => {
      if (clearExitFrame !== null) cancelAnimationFrame(clearExitFrame);
      document.removeEventListener("fullscreenchange", onFullscreenChange);
      unpinPicture(root);
    };
  }, [hold, television]);

  // When TV Back exits fullscreen and then commits Guide mode, remeasure once
  // against the committed Guide layout without touching ordinary held pins.
  useLayoutEffect(() => {
    const modeChanged = lastModeRef.current !== mode;
    lastModeRef.current = mode;
    if (
      !television ||
      !hold ||
      !modeChanged ||
      !pendingExitRemeasureRef.current ||
      document.fullscreenElement != null
    ) {
      return;
    }
    pendingExitRemeasureRef.current = false;
    const root = document.documentElement;
    unpinPicture(root);
    pinPicture(root, television);
  }, [hold, mode, television]);
}

function unpinPicture(root: HTMLElement): void {
  root.style.removeProperty(PINNED_WIDTH);
  root.style.removeProperty(PINNED_HEIGHT);
  root.style.removeProperty(PINNED_LEFT);
  root.style.removeProperty(PINNED_TOP);
}

function pinPicture(root: HTMLElement, television: boolean): void {
  const box = stageMonitorBox();
  if (box === null) return;
  root.style.setProperty(PINNED_WIDTH, `${box.width}px`);
  root.style.setProperty(PINNED_HEIGHT, `${box.height}px`);
  if (!television) return;
  root.style.setProperty(PINNED_LEFT, `${box.left}px`);
  root.style.setProperty(PINNED_TOP, `${box.top}px`);
}
