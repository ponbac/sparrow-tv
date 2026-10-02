/*
 * Where the shell's handlers find its parts in the document. Theater's
 * document-level listeners work on native events, outside React, so they look
 * the elements up by the classes the shell and the stage render.
 */

/** Everything laid over the picture in watch mode; it hides when the viewer is idle. */
export const STAGE_CHROME =
  ".shell__masthead, .shell__retained, .now-playing, .zap-rail, .stage__keys";

/** The search field in the Theater masthead, or null in the pocket layout. */
export function stageSearchInput(): HTMLInputElement | null {
  return document.querySelector<HTMLInputElement>(".shell__search input");
}

/** The guide's search field wherever the layout puts it, or null before the guide renders. */
export function guideSearchInput(): HTMLInputElement | null {
  return document.querySelector<HTMLInputElement>(
    "[data-acceptance-search] input",
  );
}

/**
 * Reports whether a click landed on the picture itself: inside the picture
 * box in the document, and not on a control drawn in it. Clicks on controls
 * the player portals elsewhere still bubble to the box through React.
 */
export function isPictureTap(
  monitor: HTMLElement,
  target: EventTarget | null,
): boolean {
  return (
    target instanceof Element &&
    monitor.contains(target) &&
    target.closest("button, a, input, select, label") === null
  );
}

/** Reports whether focus is on one of the channel bar's buttons. */
export function focusIsInChannelBar(): boolean {
  return document.activeElement?.closest(".channel-bar") != null;
}

/**
 * Moves focus to the band's button back to the picture: where focus goes
 * when the guide opens over a playing Channel. Where that button is not
 * shown it cannot take focus, and focus rests on the picture box instead.
 */
export function focusPictureReturn(): void {
  const button = document.querySelector<HTMLElement>(".stage__undock");
  button?.focus({ preventScroll: true });
  if (button === null || document.activeElement !== button) {
    focusStageMonitor();
  }
}

/**
 * Measures the picture box: its width and height in CSS pixels. Null where
 * there is no box, and for one that is not laid out and so has no size.
 */
export function stageMonitorSize(): {
  readonly width: number;
  readonly height: number;
} | null {
  const box = document
    .querySelector<HTMLElement>(".stage__monitor")
    ?.getBoundingClientRect();
  return box === undefined || box.width <= 0 || box.height <= 0
    ? null
    : { width: box.width, height: box.height };
}

/**
 * Moves focus to the picture box without scrolling: the place focus rests
 * while nothing in the chrome needs it, so the stage keys apply.
 */
export function focusStageMonitor(): void {
  document
    .querySelector<HTMLElement>(".stage__monitor")
    ?.focus({ preventScroll: true });
}
