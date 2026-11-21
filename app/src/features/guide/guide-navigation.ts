import type { KeyboardEvent } from "react";
import type { StageLayout } from "../stage/use-stage-layout";

const IGNORE_NAVIGATION_SELECTOR =
  'input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="combobox"], [role="dialog"], [role="listbox"], [role="menu"]';

const ROW_TOP_TOLERANCE_PX = 4;
const BOARD_FOCUS_MARGIN_PX = 12;

export interface GuideArrowNavigation {
  readonly section: HTMLElement;
  readonly board: HTMLElement | null;
  readonly layout: StageLayout;
  readonly mode: "watch" | "guide";
  readonly hasMore: boolean;
  readonly onRequestMore: () => void;
}

/**
 * Provides deterministic 2D arrow / TV D-pad navigation across the guide's
 * horizontal bands (toolbar -> Channel Group lane -> optional times -> Channel
 * rows -> More button). Runs in capture phase so vertical arrows leave the
 * horizontal RadioGroup instead of stepping sideways through Channel Groups,
 * and channel movement steps directly from Channel to Channel without stopping
 * on inline quality chips.
 */
export function handleGuideArrowKey(
  event: KeyboardEvent<HTMLElement>,
  { section, board, layout, mode, hasMore, onRequestMore }: GuideArrowNavigation,
): void {
  if (
    event.defaultPrevented ||
    event.ctrlKey ||
    event.metaKey ||
    event.altKey ||
    mode !== "guide"
  ) {
    return;
  }
  const key = event.key;
  if (
    key !== "ArrowDown" &&
    key !== "ArrowUp" &&
    key !== "ArrowLeft" &&
    key !== "ArrowRight"
  ) {
    return;
  }
  if (!(event.target instanceof HTMLElement)) {
    return;
  }
  const target = event.target;
  const toolbar = section.querySelector<HTMLElement>(
    ".programme-guide__toolbar",
  );
  const lane = section.querySelector<HTMLElement>(".programme-guide__lane");
  const times = section.querySelector<HTMLElement>(".programme-guide__times");

  // Allow pressing Down to leave an empty, closed toolbar search box back to
  // the Channel Group lane instead of trapping TV D-pad focus in the input.
  if (
    key === "ArrowDown" &&
    target instanceof HTMLInputElement &&
    toolbar !== null &&
    toolbar.contains(target) &&
    target.value === "" &&
    target.getAttribute("aria-expanded") !== "true"
  ) {
    const next =
      preferredLaneTarget(lane, target) ??
      preferredTimesTarget(times) ??
      firstVisibleBoardTarget(board, target);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    }
    return;
  }

  if (target.closest(IGNORE_NAVIGATION_SELECTOR) !== null) {
    return;
  }

  if (toolbar !== null && toolbar.contains(target)) {
    handleToolbarArrow(event, target, toolbar, lane, times, board);
    return;
  }
  if (lane !== null && lane.contains(target)) {
    handleLaneArrow(event, target, toolbar, lane, times, board);
    return;
  }
  if (times !== null && times.contains(target)) {
    handleTimesArrow(event, target, lane, times, board);
    return;
  }
  if (board !== null && board.contains(target)) {
    handleBoardArrow(
      event,
      target,
      layout,
      lane,
      times,
      board,
      hasMore,
      onRequestMore,
    );
  }
}

function handleToolbarArrow(
  event: KeyboardEvent<HTMLElement>,
  target: HTMLElement,
  toolbar: HTMLElement,
  lane: HTMLElement | null,
  times: HTMLElement | null,
  board: HTMLElement | null,
): void {
  if (event.key === "ArrowDown") {
    const next =
      preferredLaneTarget(lane, target) ??
      preferredTimesTarget(times) ??
      firstVisibleBoardTarget(board, target);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    }
    return;
  }
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    const controls = visibleFocusables(
      toolbar,
      'button, input, a[href], [tabindex]:not([tabindex="-1"])',
    );
    const index = controls.indexOf(target);
    if (index === -1) {
      return;
    }
    const nextIndex = event.key === "ArrowRight" ? index + 1 : index - 1;
    const next = controls[nextIndex] ?? null;
    if (next !== null) {
      consumeAndFocus(event, next, board);
    }
  }
}

function handleLaneArrow(
  event: KeyboardEvent<HTMLElement>,
  target: HTMLElement,
  toolbar: HTMLElement | null,
  lane: HTMLElement,
  times: HTMLElement | null,
  board: HTMLElement | null,
): void {
  if (event.key === "ArrowDown") {
    const next =
      preferredTimesTarget(times) ?? firstVisibleBoardTarget(board, target);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    }
    return;
  }
  if (event.key === "ArrowUp") {
    const next = nearestToolbarTarget(toolbar, target);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    }
    return;
  }

  const whenButton = lane.querySelector<HTMLElement>(".programme-guide__when");
  const rosterButton = lane.querySelector<HTMLElement>(".group-roster-trigger");
  const groups = visibleFocusables(lane, ".programme-guide__group");
  const activeGroup =
    groups.find((group) => group.hasAttribute("data-checked")) ??
    groups[0] ??
    null;

  if (target === whenButton && event.key === "ArrowRight") {
    const next = activeGroup ?? rosterButton;
    if (next !== null) {
      consumeAndFocus(event, next, board);
    }
    return;
  }
  if (target === rosterButton && event.key === "ArrowLeft") {
    const next = activeGroup ?? whenButton;
    if (next !== null) {
      consumeAndFocus(event, next, board);
    }
    return;
  }

  const groupIndex = groups.indexOf(target);
  if (groupIndex === -1) {
    return;
  }
  // At the outer edges of the Channel Group RadioGroup, move cleanly to the
  // adjacent lane controls instead of wrapping across all groups.
  if (
    event.key === "ArrowLeft" &&
    groupIndex === 0 &&
    whenButton !== null &&
    isVisibleFocusable(whenButton)
  ) {
    consumeAndFocus(event, whenButton, board);
    return;
  }
  if (
    event.key === "ArrowRight" &&
    groupIndex === groups.length - 1 &&
    rosterButton !== null &&
    isVisibleFocusable(rosterButton)
  ) {
    consumeAndFocus(event, rosterButton, board);
  }
}

function handleTimesArrow(
  event: KeyboardEvent<HTMLElement>,
  target: HTMLElement,
  lane: HTMLElement | null,
  times: HTMLElement,
  board: HTMLElement | null,
): void {
  if (event.key === "ArrowDown") {
    const next = firstVisibleBoardTarget(board, target);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    }
    return;
  }
  if (event.key === "ArrowUp") {
    const next =
      lane?.querySelector<HTMLElement>(".programme-guide__when") ??
      preferredLaneTarget(lane, target);
    if (next !== null && isVisibleFocusable(next)) {
      consumeAndFocus(event, next, board);
    }
    return;
  }
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    const buttons = visibleFocusables(times, "button");
    const index = buttons.indexOf(target);
    if (index === -1) {
      return;
    }
    const next =
      buttons[event.key === "ArrowRight" ? index + 1 : index - 1] ?? null;
    if (next !== null) {
      consumeAndFocus(event, next, board);
    }
  }
}

function handleBoardArrow(
  event: KeyboardEvent<HTMLElement>,
  target: HTMLElement,
  layout: StageLayout,
  lane: HTMLElement | null,
  times: HTMLElement | null,
  board: HTMLElement,
  hasMore: boolean,
  onRequestMore: () => void,
): void {
  const moreButton = board.querySelector<HTMLElement>(".programme-guide__more");
  const channels = visibleFocusables(
    board,
    ".programme-guide__row .programme-guide__channel",
  );

  if (target === moreButton) {
    if (event.key === "ArrowUp") {
      const last = channels[channels.length - 1] ?? null;
      const next =
        last ??
        preferredTimesTarget(times) ??
        preferredLaneTarget(lane, target);
      if (next !== null) {
        consumeAndFocus(event, next, board);
      }
    }
    return;
  }

  // In Theater layout, moving sideways inside a row's Programme cells stays
  // along that row, while vertical movement jumps between rows cleanly.
  if (target.classList.contains("programme-guide__programme")) {
    const row = target.closest<HTMLElement>(".programme-guide__row");
    const rowChannel =
      row?.querySelector<HTMLElement>(".programme-guide__channel") ?? null;
    if (row !== null && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
      const programmes = visibleFocusables(row, ".programme-guide__programme");
      const progIndex = programmes.indexOf(target);
      if (event.key === "ArrowLeft") {
        const next = progIndex > 0 ? programmes[progIndex - 1] : rowChannel;
        if (next !== null) {
          consumeAndFocus(event, next, board);
        }
        return;
      }
      const next = programmes[progIndex + 1] ?? null;
      if (next !== null) {
        consumeAndFocus(event, next, board);
      }
      return;
    }
    if (rowChannel !== null) {
      handleChannelGridArrow(
        event,
        rowChannel,
        channels,
        layout,
        lane,
        times,
        board,
        moreButton,
        hasMore,
        onRequestMore,
      );
    }
    return;
  }

  if (target.classList.contains("programme-guide__channel")) {
    handleChannelGridArrow(
      event,
      target,
      channels,
      layout,
      lane,
      times,
      board,
      moreButton,
      hasMore,
      onRequestMore,
    );
    return;
  }

  // Empty/error notice retry button inside the board: ArrowUp returns to the lane.
  if (event.key === "ArrowUp") {
    const next =
      preferredTimesTarget(times) ?? preferredLaneTarget(lane, target);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    }
  }
}

function handleChannelGridArrow(
  event: KeyboardEvent<HTMLElement>,
  current: HTMLElement,
  channels: readonly HTMLElement[],
  layout: StageLayout,
  lane: HTMLElement | null,
  times: HTMLElement | null,
  board: HTMLElement,
  moreButton: HTMLElement | null,
  hasMore: boolean,
  onRequestMore: () => void,
): void {
  const index = channels.indexOf(current);
  if (index === -1) {
    return;
  }
  const columns = gridColumnCount(channels);

  if (event.key === "ArrowRight") {
    if (columns === 1 && layout === "theater") {
      const firstProgramme = current
        .closest<HTMLElement>(".programme-guide__row")
        ?.querySelector<HTMLElement>(".programme-guide__programme");
      if (firstProgramme !== null && firstProgramme !== undefined) {
        consumeAndFocus(event, firstProgramme, board);
      }
      return;
    }
    const col = index % columns;
    if (col + 1 < columns && index + 1 < channels.length) {
      consumeAndFocus(event, channels[index + 1], board);
    } else {
      // Prevent spatial navigation from escaping onto non-channel controls.
      event.preventDefault();
      event.stopPropagation();
    }
    return;
  }

  if (event.key === "ArrowLeft") {
    const col = index % columns;
    if (col > 0) {
      consumeAndFocus(event, channels[index - 1], board);
    } else {
      event.preventDefault();
      event.stopPropagation();
    }
    return;
  }

  if (event.key === "ArrowDown") {
    const nextIndex = index + columns;
    if (nextIndex < channels.length) {
      consumeAndFocus(event, channels[nextIndex], board);
      return;
    }
    const currentRow = Math.floor(index / columns);
    const lastRow = Math.floor((channels.length - 1) / columns);
    if (currentRow < lastRow) {
      consumeAndFocus(event, channels[channels.length - 1], board);
      return;
    }
    if (hasMore) {
      onRequestMore();
      if (moreButton !== null && isVisibleFocusable(moreButton)) {
        consumeAndFocus(event, moreButton, board);
        return;
      }
    }
    event.preventDefault();
    event.stopPropagation();
    return;
  }

  if (event.key === "ArrowUp") {
    const prevIndex = index - columns;
    if (prevIndex >= 0) {
      consumeAndFocus(event, channels[prevIndex], board);
      return;
    }
    const next =
      preferredTimesTarget(times) ??
      checkedGroupInLane(lane) ??
      preferredLaneTarget(lane, current);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    }
  }
}

function checkedGroupInLane(lane: HTMLElement | null): HTMLElement | null {
  if (lane === null) {
    return null;
  }
  const checked = lane.querySelector<HTMLElement>(
    ".programme-guide__group[data-checked]",
  );
  if (checked !== null && isVisibleFocusable(checked)) {
    return checked;
  }
  return visibleFocusables(lane, ".programme-guide__group")[0] ?? null;
}

function preferredLaneTarget(
  lane: HTMLElement | null,
  origin: HTMLElement,
): HTMLElement | null {
  if (lane === null) {
    return null;
  }
  const checkedGroup = checkedGroupInLane(lane);
  const whenButton = lane.querySelector<HTMLElement>(".programme-guide__when");
  const rosterButton = lane.querySelector<HTMLElement>(".group-roster-trigger");
  const candidates = [checkedGroup, whenButton, rosterButton].filter(
    (item): item is HTMLElement => item !== null && isVisibleFocusable(item),
  );
  return nearestByHorizontalCenter(candidates, origin);
}

function preferredTimesTarget(times: HTMLElement | null): HTMLElement | null {
  if (times === null) {
    return null;
  }
  const buttons = visibleFocusables(times, "button");
  return (
    buttons.find((button) => button.getAttribute("aria-pressed") === "true") ??
    buttons[0] ??
    null
  );
}

function nearestToolbarTarget(
  toolbar: HTMLElement | null,
  origin: HTMLElement,
): HTMLElement | null {
  if (toolbar === null) {
    return null;
  }
  // Prefer toolbar buttons over text inputs so pressing Up from the Channel
  // Group lane does not unexpectedly summon the Android TV on-screen keyboard.
  // Users can still reach the search input intentionally with ArrowLeft.
  const buttons = visibleFocusables(
    toolbar,
    'button, a[href], [tabindex]:not([tabindex="-1"]):not(input):not(textarea)',
  );
  if (buttons.length > 0) {
    return nearestByHorizontalCenter(buttons, origin);
  }
  const inputs = visibleFocusables(toolbar, "input, textarea");
  return nearestByHorizontalCenter(inputs, origin);
}

function firstVisibleBoardTarget(
  board: HTMLElement | null,
  origin: HTMLElement,
): HTMLElement | null {
  if (board === null) {
    return null;
  }
  const channels = visibleFocusables(
    board,
    ".programme-guide__row .programme-guide__channel",
  );
  if (channels.length === 0) {
    return visibleFocusables(board, "button")[0] ?? null;
  }
  const columns = gridColumnCount(channels);
  const boardRect = board.getBoundingClientRect();
  if (boardRect.height === 0) {
    return channels[0];
  }
  let topRowIndex = 0;
  for (let i = 0; i < channels.length; i += 1) {
    const rect = channels[i].getBoundingClientRect();
    if (rect.bottom > boardRect.top + ROW_TOP_TOLERANCE_PX) {
      topRowIndex = i;
      break;
    }
  }
  const firstTop = channels[topRowIndex].getBoundingClientRect().top;
  const topBand: HTMLElement[] = [];
  for (
    let i = topRowIndex;
    i < Math.min(channels.length, topRowIndex + columns);
    i += 1
  ) {
    const rect = channels[i].getBoundingClientRect();
    if (Math.abs(rect.top - firstTop) <= ROW_TOP_TOLERANCE_PX) {
      topBand.push(channels[i]);
    } else {
      break;
    }
  }
  return nearestByHorizontalCenter(topBand, origin) ?? channels[topRowIndex];
}

function gridColumnCount(channels: readonly HTMLElement[]): number {
  if (channels.length < 2) {
    return 1;
  }
  const first = channels[0].getBoundingClientRect();
  if (first.width === 0 && first.height === 0) {
    return 1;
  }
  let columns = 1;
  for (let i = 1; i < channels.length; i += 1) {
    const rect = channels[i].getBoundingClientRect();
    if (Math.abs(rect.top - first.top) <= ROW_TOP_TOLERANCE_PX) {
      columns += 1;
    } else {
      break;
    }
  }
  return columns;
}

function nearestByHorizontalCenter(
  candidates: readonly HTMLElement[],
  origin: HTMLElement,
): HTMLElement | null {
  if (candidates.length === 0) {
    return null;
  }
  if (candidates.length === 1) {
    return candidates[0];
  }
  const originRect = origin.getBoundingClientRect();
  if (originRect.width === 0 && originRect.height === 0) {
    return candidates[0];
  }
  const originCenter = originRect.left + originRect.width / 2;
  let best = candidates[0];
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const candidate of candidates) {
    const rect = candidate.getBoundingClientRect();
    const center = rect.left + rect.width / 2;
    const distance = Math.abs(center - originCenter);
    if (distance < bestDistance) {
      bestDistance = distance;
      best = candidate;
    }
  }
  return best;
}

function visibleFocusables(
  root: HTMLElement,
  selector: string,
): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(selector)).filter(
    isVisibleFocusable,
  );
}

function isVisibleFocusable(element: HTMLElement): boolean {
  if (element.hidden || element.hasAttribute("disabled")) {
    return false;
  }
  if (
    element.getAttribute("tabindex") === "-1" &&
    !element.classList.contains("programme-guide__group")
  ) {
    return false;
  }
  const style = window.getComputedStyle(element);
  return style.display !== "none" && style.visibility !== "hidden";
}

function consumeAndFocus(
  event: KeyboardEvent<HTMLElement>,
  next: HTMLElement,
  board: HTMLElement | null,
): void {
  event.preventDefault();
  event.stopPropagation();
  next.focus({ preventScroll: true });
  scrollHorizontalParentToReveal(next);
  if (board !== null && board.contains(next)) {
    scrollBoardToReveal(board, next);
  }
}

function scrollHorizontalParentToReveal(element: HTMLElement): void {
  const scroller = element.closest<HTMLElement>(
    ".programme-guide__groups-scroller, .programme-guide__times",
  );
  if (scroller === null) {
    return;
  }
  const scrollerRect = scroller.getBoundingClientRect();
  const childRect = element.getBoundingClientRect();
  if (scrollerRect.width === 0) {
    return;
  }
  if (childRect.left < scrollerRect.left) {
    scroller.scrollLeft -= scrollerRect.left - childRect.left + 8;
  } else if (childRect.right > scrollerRect.right) {
    scroller.scrollLeft += childRect.right - scrollerRect.right + 8;
  }
}

function scrollBoardToReveal(board: HTMLElement, target: HTMLElement): void {
  if (board.clientHeight === 0) {
    return;
  }
  const row = target.closest<HTMLElement>(".programme-guide__row") ?? target;
  const boardRect = board.getBoundingClientRect();
  const rowRect = row.getBoundingClientRect();
  const rowTop = rowRect.top - boardRect.top + board.scrollTop;
  const rowBottom = rowTop + rowRect.height;
  const viewTop = board.scrollTop;
  const viewBottom = viewTop + board.clientHeight;
  if (rowTop < viewTop + BOARD_FOCUS_MARGIN_PX) {
    board.scrollTop = Math.max(0, rowTop - BOARD_FOCUS_MARGIN_PX);
  } else if (rowBottom > viewBottom - BOARD_FOCUS_MARGIN_PX) {
    board.scrollTop = rowBottom - board.clientHeight + BOARD_FOCUS_MARGIN_PX;
  }
}
