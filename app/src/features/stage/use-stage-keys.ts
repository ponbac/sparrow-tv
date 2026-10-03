import { useEffect, useLayoutEffect, useRef } from "react";
import {
  focusStageMonitor,
  playerFullscreenButton,
  stageSearchInput,
} from "./stage-dom";

// Where a key belongs to what has focus: typing, or moving through a list.
const OWNS_EVERY_KEY =
  'input:not([type="range"]), textarea, select, [contenteditable]:not([contenteditable="false"]), [role="menu"], [role="listbox"], [role="dialog"]';
// Where only the arrows do: they change a value or move the selection.
const OWNS_ARROWS = 'input[type="range"], [role="radiogroup"], [role="slider"]';

/** What the stage keys act on. */
export interface StageKeys {
  /** The keys work in the Theater layout only. */
  readonly active: boolean;
  /** The mode the shell is showing. */
  readonly mode: "watch" | "guide";
  /** The guide cannot be closed: there is no picture in the page to show. */
  readonly guideForced: boolean;
  readonly onToggleGuide: () => void;
  /** Return to the full picture. */
  readonly onWatch: () => void;
  /** Move to the previous (`-1`) or next (`1`) Channel. */
  readonly onZap: (direction: -1 | 1) => void;
}

/**
 * Handles the Theater layout's keys wherever focus is: `G` opens and closes
 * the guide, `/` goes to the search field, `F` toggles full screen, the up
 * and down arrows change Channel in watch mode, and `Esc` returns to the
 * picture. A key that another control has handled, or that is held with
 * Ctrl, Alt or Meta, is left alone.
 */
export function useStageKeys(keys: StageKeys): void {
  const latest = useRef(keys);
  useLayoutEffect(() => {
    latest.current = keys;
  });
  const { active } = keys;
  useEffect(() => {
    if (!active) {
      return;
    }
    // One listener for as long as the keys are active. A render can commit
    // between two listeners of the same key press, and a listener replaced
    // there would never hear that press.
    const onKeyDown = (event: KeyboardEvent) => {
      handleStageKey(event, latest.current);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [active]);
}

function handleStageKey(
  event: KeyboardEvent,
  { mode, guideForced, onToggleGuide, onWatch, onZap }: StageKeys,
): void {
  if (
    event.defaultPrevented ||
    event.ctrlKey ||
    event.metaKey ||
    event.altKey
  ) {
    return;
  }
  const key = event.key.toLowerCase();
  const target = event.target instanceof Element ? event.target : null;
  if (target?.closest(OWNS_EVERY_KEY) != null) {
    // The search field gives focus back once it has nothing left to close.
    if (
      key === "escape" &&
      target instanceof HTMLElement &&
      target === stageSearchInput() &&
      target.getAttribute("aria-expanded") !== "true"
    ) {
      target.blur();
      focusStageMonitor();
    }
    return;
  }
  switch (key) {
    case "g":
      if (!event.repeat && !guideForced) {
        onToggleGuide();
      }
      return;
    case "/":
      event.preventDefault();
      // The key press itself brings hidden chrome back.
      stageSearchInput()?.focus();
      return;
    case "f":
      if (!event.repeat) {
        toggleFullscreen();
      }
      return;
    case "arrowup":
    case "arrowdown":
      if (mode !== "watch" || target?.closest(OWNS_ARROWS) != null) {
        return;
      }
      event.preventDefault();
      onZap(key === "arrowup" ? -1 : 1);
      return;
    case "escape":
      if (mode === "guide" && !guideForced) {
        onWatch();
      }
      return;
  }
}

/**
 * Presses the player's own Full screen button, so the player decides what
 * goes fullscreen. With no player the document root does.
 */
function toggleFullscreen(): void {
  const button = playerFullscreenButton();
  if (button !== null) {
    button.click();
    return;
  }
  const root = document.documentElement;
  if (root.requestFullscreen === undefined) {
    return;
  }
  // A refused request leaves the window as it was; there is nothing to tell.
  void (
    document.fullscreenElement === root
      ? document.exitFullscreen()
      : root.requestFullscreen()
  ).catch(() => undefined);
}
