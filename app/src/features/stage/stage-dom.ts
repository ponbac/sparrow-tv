/*
 * Where the Theater layout's document-level listeners find the shell's parts.
 * They work on native events, outside React, so they look the elements up by
 * the classes the shell and the stage render.
 */

/** Everything laid over the picture in watch mode; it hides when the viewer is idle. */
export const STAGE_CHROME =
  ".shell__masthead, .shell__retained, .now-playing, .zap-rail, .stage__keys";

/** The search field in the Theater masthead, or null in the stacked layout. */
export function stageSearchInput(): HTMLInputElement | null {
  return document.querySelector<HTMLInputElement>(".shell__search input");
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
