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

/**
 * The player's own Full screen button, or null while there is no player.
 * Pressing it lets the player decide what goes fullscreen.
 */
export function playerFullscreenButton(): HTMLElement | null {
  return document.querySelector<HTMLElement>(
    '[data-stage-action="fullscreen"]',
  );
}

/** Reports whether focus is on one of the channel bar's buttons. */
export function focusIsInChannelBar(): boolean {
  return document.activeElement?.closest(".channel-bar") != null;
}

/**
 * Takes a TV's docked picture to the whole screen: the player's section in
 * the picture box goes element fullscreen. Call it inside the viewer's press,
 * which is what lets the document go fullscreen. A player that could not load
 * has no picture, only its Reload and Close actions in the box: focus goes to
 * them instead. "no-player" while the player's code still loads.
 */
export function showTelevisionPicture(): "fullscreen" | "load-notice" | "no-player" {
  const section = document.querySelector<HTMLElement>(
    ".stage__monitor .hosted-player",
  );
  if (section !== null) {
    // The players' fullscreenchange observers reconcile the native viewport.
    void section.requestFullscreen?.().catch(() => undefined);
    return "fullscreen";
  }
  const action = document.querySelector<HTMLElement>(
    ".stage__monitor .playback-load-notice__actions button",
  );
  if (action !== null) {
    action.focus({ preventScroll: true });
    return "load-notice";
  }
  return "no-player";
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

/** Options for checking whether an element is visible and focusable. */
export interface VisibleControlOptions {
  /** Allows roving-tabindex widgets (`role="radio"`, etc.) with `tabindex="-1"`. */
  readonly allowNegativeTabIndex?: boolean;
  /** Skips elements inside a closed `<details>` body outside its `<summary>`. */
  readonly checkDetails?: boolean;
  /** Always requires non-empty client rects even when the document has no layout. */
  readonly requireLayoutBox?: boolean;
}

/**
 * Reports whether an element is visible and focusable, walking ancestors so
 * controls inside a `display: none` or `visibility: hidden` wrapper are skipped
 * while keeping `aria-disabled="true"` controls reachable.
 */
export function isVisibleControl(
  element: HTMLElement,
  options: VisibleControlOptions = {},
): boolean {
  if (
    element.hidden ||
    element.matches(":disabled, [data-disabled]") ||
    element.closest('[inert], [hidden], [aria-hidden="true"]') !== null
  ) {
    return false;
  }
  if (!options.allowNegativeTabIndex && element.getAttribute("tabindex") === "-1") {
    return false;
  }
  if (options.checkDetails) {
    const details = element.closest("details");
    if (details !== null && !details.open && element.closest("summary") === null) {
      return false;
    }
  }
  for (
    let node: HTMLElement | null = element;
    node !== null && node !== document.documentElement;
    node = node.parentElement
  ) {
    const style = window.getComputedStyle(node);
    if (style.display === "none" || style.visibility === "hidden") {
      return false;
    }
  }
  const hasDocumentLayout =
    options.requireLayoutBox === true ||
    document.body.getBoundingClientRect().width > 0;
  if (hasDocumentLayout && element.getClientRects().length === 0) {
    return false;
  }
  return true;
}

/** Returns visible, focusable controls matching `selector` inside `root`. */
export function visibleStageControls(
  root: HTMLElement | null,
  selector = "button, select",
): HTMLElement[] {
  if (root === null) return [];
  return Array.from(root.querySelectorAll<HTMLElement>(selector)).filter((el) =>
    isVisibleControl(el),
  );
}

/**
 * Orders controls of one row as they are seen, left to right, where CSS
 * orders them differently from the markup. Controls without a layout box
 * (no layout at all, as in tests) keep their markup order.
 */
export function inVisualOrder(items: readonly HTMLElement[]): HTMLElement[] {
  const boxes = items.map((item) => item.getBoundingClientRect());
  if (boxes.some((box) => box.width === 0 && box.height === 0)) {
    return [...items];
  }
  return items
    .map((item, index) => ({ item, box: boxes[index], index }))
    .sort(
      (a, b) =>
        a.box.left - b.box.left || a.box.top - b.box.top || a.index - b.index,
    )
    .map(({ item }) => item);
}

const DEFAULT_SCROLL_MARGIN_PX = 12;

/**
 * Scrolls a container vertically to reveal `target` without invoking
 * `scrollIntoView`, which can scroll clipped ancestors holding native video.
 * Without a bottom margin of its own it keeps clear of the container's
 * `scroll-padding-bottom`, which a list with a fade over its last rows sets
 * to the fade's height.
 */
export function revealInScrollContainer(
  container: HTMLElement | null,
  target: HTMLElement,
  options: { readonly marginPx?: number; readonly bottomMarginPx?: number } = {},
): void {
  if (
    container === null ||
    container.clientHeight === 0 ||
    !container.contains(target)
  ) {
    return;
  }
  const topMargin = options.marginPx ?? DEFAULT_SCROLL_MARGIN_PX;
  const bottomMargin =
    options.bottomMarginPx ?? Math.max(topMargin, scrollPaddingBottom(container));
  const containerRect = container.getBoundingClientRect();
  const targetRect = target.getBoundingClientRect();
  const top = targetRect.top - containerRect.top + container.scrollTop;
  const bottom = top + targetRect.height;
  const viewTop = container.scrollTop;
  const viewBottom = viewTop + container.clientHeight;
  if (top < viewTop + topMargin) {
    container.scrollTop = Math.max(0, top - topMargin);
  } else if (bottom > viewBottom - bottomMargin) {
    container.scrollTop = bottom - container.clientHeight + bottomMargin;
  }
}

/** The bottom scroll padding a container's styles give it, in pixels. */
function scrollPaddingBottom(container: HTMLElement): number {
  const value = Number.parseFloat(
    window.getComputedStyle(container).scrollPaddingBottom,
  );
  return Number.isFinite(value) ? value : 0;
}

/**
 * Returns the scaled horizontal scroll padding (at least 8 px, ~2 rem) so
 * focused chips keep clear of edge fade masks on large TV canvases.
 */
export function horizontalRevealMarginPx(): number {
  const rootFontSize = Number.parseFloat(
    window.getComputedStyle(document.documentElement).fontSize || "16",
  );
  return Math.max(
    8,
    Math.round((Number.isFinite(rootFontSize) ? rootFontSize : 16) * 2),
  );
}

/** Scrolls a horizontal strip directly so `target` stays clear of edge fades. */
export function revealInHorizontalScroller(
  scroller: HTMLElement | null,
  target: HTMLElement,
): void {
  if (scroller === null) return;
  const scrollerRect = scroller.getBoundingClientRect();
  if (scrollerRect.width === 0) return;
  const targetRect = target.getBoundingClientRect();
  const margin = horizontalRevealMarginPx();
  if (targetRect.left < scrollerRect.left) {
    scroller.scrollLeft -= scrollerRect.left - targetRect.left + margin;
  } else if (targetRect.right > scrollerRect.right) {
    scroller.scrollLeft += targetRect.right - scrollerRect.right + margin;
  }
}
