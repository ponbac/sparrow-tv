import type { StageLayout } from "../stage/use-stage-layout";
import {
  isVisibleControl,
  revealInHorizontalScroller,
  revealInScrollContainer,
} from "../stage/stage-dom";

const IGNORE_NAVIGATION_SELECTOR =
  'input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="combobox"], [role="dialog"], [role="listbox"], [role="menu"]';

const ROW_TOP_TOLERANCE_PX = 4;
/** The chip at the head of the lane: pocket's time chip or the TV layout switch. */
const LEAD_SELECTOR = ".programme-guide__when, .programme-guide__view";
/** The minutes of the time axis the TV timeline shows at once. */
const TIMELINE_VIEW_MINUTES = 120;
/** The cell focus last rested on in each timeline, where Right from the rail returns. */
const lastTimelineCell = new WeakMap<HTMLElement, HTMLElement>();
const BOARD_FOCUS_MARGIN_PX = 12;

type ArrowKeyEvent = {
  readonly key: string;
  readonly defaultPrevented: boolean;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly altKey: boolean;
  readonly isComposing?: boolean;
  readonly nativeEvent?: { readonly isComposing?: boolean };
  readonly target: EventTarget | null;
  readonly preventDefault: () => void;
  readonly stopPropagation: () => void;
};

function isComposingEvent(event: ArrowKeyEvent): boolean {
  return event.isComposing === true || event.nativeEvent?.isComposing === true;
}

/** Context required to route 2D arrow and TV D-pad movement inside the Guide. */
export interface GuideArrowNavigation {
  readonly section: HTMLElement;
  readonly workspace?: HTMLElement | null;
  readonly board: HTMLElement | null;
  readonly layout: StageLayout;
  readonly mode: "watch" | "guide";
  readonly television?: boolean;
  readonly hasMore: boolean;
  readonly onRequestMore: () => void;
}

/**
 * Provides deterministic 2D arrow / TV D-pad navigation across the guide's
 * horizontal bands (docked preview <-> toolbar -> Channel Group lane ->
 * optional times -> Channel rows -> retained error recovery -> More button).
 * Runs in capture phase so vertical arrows leave the horizontal RadioGroup
 * instead of stepping sideways through Channel Groups, TV D-pad arrows browse
 * groups without committing until Enter/Space, and channel movement steps
 * directly from Channel to Channel without stopping on inline quality chips.
 */
export function handleGuideArrowKey(
  event: ArrowKeyEvent,
  {
    section,
    workspace = null,
    board,
    layout,
    mode,
    television = false,
    hasMore,
    onRequestMore,
  }: GuideArrowNavigation,
): void {
  if (
    event.defaultPrevented ||
    event.ctrlKey ||
    event.metaKey ||
    event.altKey ||
    isComposingEvent(event) ||
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
  const dockControl = visibleDockControl(workspace);
  const sideBySideDock =
    dockControl !== null && isSideBySideDock(dockControl, workspace);

  if (dockControl !== null && target === dockControl) {
    handleDockArrow(
      event,
      dockControl,
      toolbar,
      lane,
      times,
      board,
      sideBySideDock,
    );
    return;
  }

  // Handle arrows on the toolbar search input when its dropdown is closed:
  // non-editing (readOnly) TV pills participate in all 4 directions; editing
  // empty search boxes still let ArrowDown exit cleanly to the lane.
  if (
    target instanceof HTMLInputElement &&
    toolbar !== null &&
    toolbar.contains(target) &&
    target.getAttribute("aria-expanded") !== "true"
  ) {
    if (target.readOnly) {
      handleToolbarArrow(
        event,
        target,
        toolbar,
        lane,
        times,
        board,
        dockControl,
        sideBySideDock,
      );
      return;
    }
    if (key === "ArrowDown" && target.value === "") {
      const next =
        preferredLaneTarget(lane, target) ??
        preferredTimesTarget(times) ??
        firstVisibleBoardTarget(board, target);
      if (next !== null) {
        consumeAndFocus(event, next, board);
      } else {
        clampArrow(event);
      }
      return;
    }
  }

  if (target.closest(IGNORE_NAVIGATION_SELECTOR) !== null) {
    return;
  }

  if (toolbar !== null && toolbar.contains(target)) {
    handleToolbarArrow(
      event,
      target,
      toolbar,
      lane,
      times,
      board,
      dockControl,
      sideBySideDock,
    );
    return;
  }
  if (lane !== null && lane.contains(target)) {
    handleLaneArrow(
      event,
      target,
      toolbar,
      lane,
      times,
      board,
      sideBySideDock ? dockControl : null,
      television,
    );
    return;
  }
  if (times !== null && times.contains(target)) {
    handleTimesArrow(
      event,
      target,
      lane,
      times,
      board,
      sideBySideDock ? dockControl : null,
    );
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
      sideBySideDock ? dockControl : null,
      hasMore,
      onRequestMore,
    );
  }
}

/**
 * Routes arrow keys from Stage's `.stage__undock` preview button into the
 * sibling ProgrammeGuide while staying inside React's synthetic event tree.
 */
export function handleGuideDockArrowKey(
  event: ArrowKeyEvent,
  stage: HTMLElement,
): void {
  if (
    event.defaultPrevented ||
    event.ctrlKey ||
    event.metaKey ||
    event.altKey ||
    isComposingEvent(event) ||
    !(event.target instanceof HTMLElement)
  ) {
    return;
  }
  if (
    event.key !== "ArrowDown" &&
    event.key !== "ArrowUp" &&
    event.key !== "ArrowLeft" &&
    event.key !== "ArrowRight"
  ) {
    return;
  }
  const shell = stage.closest<HTMLElement>(".shell");
  if (shell !== null && shell.getAttribute("data-mode") !== "guide") {
    return;
  }
  const workspace = stage.closest<HTMLElement>(".shell__workspace");
  const section = workspace?.querySelector<HTMLElement>(".programme-guide") ?? null;
  if (section === null) return;
  const toolbar = section.querySelector<HTMLElement>(".programme-guide__toolbar");
  const lane = section.querySelector<HTMLElement>(".programme-guide__lane");
  const times = section.querySelector<HTMLElement>(".programme-guide__times");
  const board = section.querySelector<HTMLElement>(".programme-guide__board");
  const sideBySide = isSideBySideDock(event.target, workspace);
  handleDockArrow(
    event,
    event.target,
    toolbar,
    lane,
    times,
    board,
    sideBySide,
  );
}

/** Options for resolving the preferred fallback focus target inside a Guide. */
export interface GuideFallbackOptions {
  /** Also check error/empty notice buttons (e.g. "Try again") inside the board. */
  readonly includeNotice?: boolean;
  /** Fall back to the checked Channel Group when no Channel rows are rendered. */
  readonly includeActiveGroup?: boolean;
  /** Skip active-group and Sources fallbacks while the board is `aria-busy="true"`. */
  readonly respectBusyBoard?: boolean;
  /** Require non-empty layout boxes when filtering candidate controls. */
  readonly requireLayoutBox?: boolean;
}

/** Finds the preferred visible Guide focus target in priority order. */
export function findGuideFallback(
  scope: ParentNode | null,
  options: GuideFallbackOptions = {},
): HTMLElement | null {
  if (scope === null) return null;
  const requireLayoutBox = options.requireLayoutBox === true;
  const boardBusy =
    options.respectBusyBoard === true &&
    scope.querySelector('.programme-guide__board[aria-busy="true"]') !== null;
  return (
    firstVisibleIn(
      scope,
      '.programme-guide__row[data-playing="true"] .programme-guide__channel',
      { requireLayoutBox },
    ) ??
    firstVisibleIn(
      scope,
      '.programme-guide__row[data-last-played="true"] .programme-guide__channel',
      { requireLayoutBox },
    ) ??
    firstVisibleIn(scope, ".programme-guide__row .programme-guide__channel", {
      requireLayoutBox,
    }) ??
    (options.includeNotice === true
      ? firstVisibleIn(scope, ".programme-guide__notice button", {
          requireLayoutBox,
        })
      : null) ??
    (!boardBusy && options.includeActiveGroup !== false
      ? firstVisibleIn(scope, ".programme-guide__group[data-checked]", {
          allowNegativeTabIndex: true,
          requireLayoutBox,
        })
      : null) ??
    (!boardBusy
      ? firstVisibleIn(scope, ".feeds-trigger", { requireLayoutBox })
      : null)
  );
}

/**
 * Restores focus to the preferred visible Guide control (playing row,
 * last-played row, first row, optional notice/group, or Sources) and scrolls
 * the board to reveal the destination.
 */
export function focusGuideFallback(
  guide: ParentNode | null,
  options?: GuideFallbackOptions,
): boolean {
  const destination = findGuideFallback(guide, options);
  if (destination === null) return false;
  destination.focus({ preventScroll: true });
  const board = destination.closest<HTMLElement>(".programme-guide__board");
  if (board !== null) {
    scrollBoardToReveal(board, destination);
  }
  return true;
}

function firstVisibleIn(
  root: ParentNode,
  selector: string,
  options: {
    readonly allowNegativeTabIndex?: boolean;
    readonly requireLayoutBox?: boolean;
  } = {},
): HTMLElement | null {
  for (const el of root.querySelectorAll<HTMLElement>(selector)) {
    if (isVisibleControl(el, options)) {
      return el;
    }
  }
  return null;
}

function handleDockArrow(
  event: ArrowKeyEvent,
  dockControl: HTMLElement,
  toolbar: HTMLElement | null,
  lane: HTMLElement | null,
  times: HTMLElement | null,
  board: HTMLElement | null,
  sideBySide: boolean,
): void {
  if (!sideBySide) {
    if (event.key === "ArrowDown") {
      const next =
        nearestToolbarTarget(toolbar, dockControl) ??
        preferredLaneTarget(lane, dockControl) ??
        preferredTimesTarget(times) ??
        firstVisibleBoardTarget(board, dockControl);
      if (next !== null) {
        consumeAndFocus(event, next, board);
      } else {
        clampArrow(event);
      }
      return;
    }
    clampArrow(event);
    return;
  }
  // The TV band: the docked picture beside the toolbar, the lane under both.
  const laneBelow = liesBelow(preferredLaneTarget(lane, dockControl), dockControl);
  if (event.key === "ArrowRight") {
    const whenButton = lane?.querySelector<HTMLElement>(LEAD_SELECTOR) ?? null;
    const next =
      (laneBelow ? firstToolbarTarget(toolbar) : null) ??
      (whenButton !== null && isVisibleFocusable(whenButton) ? whenButton : null) ??
      checkedGroupInLane(lane) ??
      nearestToolbarTarget(toolbar, dockControl);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
    return;
  }
  if (event.key === "ArrowDown") {
    const next =
      preferredTimesTarget(times) ??
      (laneBelow ? preferredLaneTarget(lane, dockControl) : null) ??
      firstVisibleBoardTarget(board, dockControl);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
    return;
  }
  clampArrow(event);
}

function handleToolbarArrow(
  event: ArrowKeyEvent,
  target: HTMLElement,
  toolbar: HTMLElement,
  lane: HTMLElement | null,
  times: HTMLElement | null,
  board: HTMLElement | null,
  dockControl: HTMLElement | null,
  sideBySideDock: boolean,
): void {
  if (event.key === "ArrowUp") {
    if (dockControl !== null && !sideBySideDock) {
      consumeAndFocus(event, dockControl, board);
    } else {
      clampArrow(event);
    }
    return;
  }
  if (event.key === "ArrowDown") {
    const next =
      // Under the toolbar of the TV timeline lies the timeline, the rail beside it.
      (lane?.dataset.orientation === "vertical" ? timelineEntry(board, target) : null) ??
      preferredLaneTarget(lane, target) ??
      preferredTimesTarget(times) ??
      firstVisibleBoardTarget(board, target);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
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
    if (event.key === "ArrowLeft" && index === 0) {
      if (dockControl !== null && sideBySideDock) {
        consumeAndFocus(event, dockControl, board);
      } else {
        clampArrow(event);
      }
      return;
    }
    const nextIndex = event.key === "ArrowRight" ? index + 1 : index - 1;
    const next = controls[nextIndex] ?? null;
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
  }
}

function handleLaneArrow(
  event: ArrowKeyEvent,
  target: HTMLElement,
  toolbar: HTMLElement | null,
  lane: HTMLElement,
  times: HTMLElement | null,
  board: HTMLElement | null,
  dockControl: HTMLElement | null,
  television: boolean,
): void {
  if (lane.dataset.orientation === "vertical") {
    handleRailArrow(event, target, toolbar, lane, board, dockControl);
    return;
  }
  if (event.key === "ArrowDown") {
    const next =
      preferredTimesTarget(times) ?? firstVisibleBoardTarget(board, target);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
    return;
  }
  if (event.key === "ArrowUp") {
    // Under the docked picture of the TV band, Up returns to it.
    const next =
      (dockControl !== null &&
      overlapsDockBand(dockControl, target) &&
      liesBelow(target, dockControl)
        ? dockControl
        : null) ?? nearestToolbarTarget(toolbar, target);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
    return;
  }

  const whenButton = lane.querySelector<HTMLElement>(LEAD_SELECTOR);
  const rosterButton = lane.querySelector<HTMLElement>(".group-roster-trigger");
  const groups = visibleFocusables(lane, ".programme-guide__group");
  const activeGroup =
    groups.find((group) => group.hasAttribute("data-checked")) ??
    groups[0] ??
    null;

  if (target === whenButton) {
    if (event.key === "ArrowLeft") {
      if (dockControl !== null) {
        consumeAndFocus(event, dockControl, board);
      } else {
        clampArrow(event);
      }
      return;
    }
    if (event.key === "ArrowRight") {
      const next = (television ? groups[0] ?? null : activeGroup) ?? rosterButton;
      if (next !== null && isVisibleFocusable(next)) {
        consumeAndFocus(event, next, board);
      } else {
        clampArrow(event);
      }
      return;
    }
  }
  if (target === rosterButton) {
    if (event.key === "ArrowRight") {
      clampArrow(event);
      return;
    }
    if (event.key === "ArrowLeft") {
      const next =
        (television ? groups[groups.length - 1] ?? null : activeGroup) ??
        whenButton;
      if (next !== null && isVisibleFocusable(next)) {
        consumeAndFocus(event, next, board);
      } else {
        clampArrow(event);
      }
      return;
    }
  }

  const groupIndex = groups.indexOf(target);
  if (groupIndex === -1) {
    return;
  }
  // At the outer edges of the Channel Group RadioGroup, move cleanly to the
  // adjacent lane controls instead of wrapping across all groups.
  if (event.key === "ArrowLeft") {
    if (groupIndex === 0) {
      const edge =
        whenButton !== null && isVisibleFocusable(whenButton)
          ? whenButton
          : dockControl;
      if (edge !== null) {
        consumeAndFocus(event, edge, board);
      } else {
        clampArrow(event);
      }
      return;
    }
    if (television) {
      consumeAndFocus(event, groups[groupIndex - 1], board);
    }
    return;
  }
  if (event.key === "ArrowRight") {
    if (groupIndex === groups.length - 1) {
      if (rosterButton !== null && isVisibleFocusable(rosterButton)) {
        consumeAndFocus(event, rosterButton, board);
      } else {
        clampArrow(event);
      }
      return;
    }
    if (television) {
      consumeAndFocus(event, groups[groupIndex + 1], board);
    }
  }
}

function handleTimesArrow(
  event: ArrowKeyEvent,
  target: HTMLElement,
  lane: HTMLElement | null,
  times: HTMLElement,
  board: HTMLElement | null,
  dockControl: HTMLElement | null,
): void {
  if (event.key === "ArrowDown") {
    const next = firstVisibleBoardTarget(board, target);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
    return;
  }
  if (event.key === "ArrowUp") {
    const next =
      (dockControl !== null && overlapsDockBand(dockControl, target)
        ? dockControl
        : null) ??
      lane?.querySelector<HTMLElement>(LEAD_SELECTOR) ??
      preferredLaneTarget(lane, target);
    if (next !== null && isVisibleFocusable(next)) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
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
    } else {
      clampArrow(event);
    }
  }
}

function handleBoardArrow(
  event: ArrowKeyEvent,
  target: HTMLElement,
  layout: StageLayout,
  lane: HTMLElement | null,
  times: HTMLElement | null,
  board: HTMLElement,
  dockControl: HTMLElement | null,
  hasMore: boolean,
  onRequestMore: () => void,
): void {
  const channels = Array.from(
    board.querySelectorAll<HTMLElement>(
      ".programme-guide__row .programme-guide__channel",
    ),
  );

  if (
    board.dataset.view === "timeline" &&
    (target.classList.contains("programme-guide__channel") ||
      target.classList.contains("programme-guide__programme"))
  ) {
    handleTimelineArrow(event, target, lane, board, hasMore, onRequestMore);
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
      dockControl,
      hasMore,
      onRequestMore,
    );
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
        dockControl,
        hasMore,
        onRequestMore,
      );
    }
    return;
  }

  const retainedButton = visibleBoardFooterButton(
    board,
    ".programme-guide__retained button",
  );
  const moreButton = visibleBoardFooterButton(board, ".programme-guide__more");

  if (target === retainedButton) {
    if (event.key === "ArrowUp") {
      const next =
        findVisibleChannelInRange(channels, channels.length - 1, 0, -1) ??
        preferredTimesTarget(times) ??
        preferredLaneTarget(lane, target);
      if (next !== null) {
        consumeAndFocus(event, next, board);
      } else {
        clampArrow(event);
      }
      return;
    }
    if (event.key === "ArrowDown" && moreButton !== null) {
      consumeAndFocus(event, moreButton, board);
      return;
    }
    clampArrow(event);
    return;
  }

  if (target === moreButton) {
    if (event.key === "ArrowUp") {
      const next =
        retainedButton ??
        findVisibleChannelInRange(channels, channels.length - 1, 0, -1) ??
        preferredTimesTarget(times) ??
        preferredLaneTarget(lane, target);
      if (next !== null) {
        consumeAndFocus(event, next, board);
      } else {
        clampArrow(event);
      }
      return;
    }
    clampArrow(event);
    return;
  }

  // Empty/error notice retry button inside the board: ArrowUp returns to the lane.
  if (event.key === "ArrowUp") {
    const next =
      preferredTimesTarget(times) ?? preferredLaneTarget(lane, target);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
    return;
  }
  clampArrow(event);
}

function visibleBoardFooterButton(
  board: HTMLElement,
  selector: string,
): HTMLElement | null {
  const candidate = board.querySelector<HTMLElement>(selector);
  return candidate !== null && isVisibleFocusable(candidate) ? candidate : null;
}

function findVisibleChannelInRange(
  channels: readonly HTMLElement[],
  startIndex: number,
  endInclusive: number,
  step: number,
): HTMLElement | null {
  if (step > 0) {
    for (
      let i = startIndex;
      i <= endInclusive && i < channels.length;
      i += step
    ) {
      if (isVisibleFocusable(channels[i])) {
        return channels[i];
      }
    }
  } else if (step < 0) {
    for (let i = startIndex; i >= endInclusive && i >= 0; i += step) {
      if (isVisibleFocusable(channels[i])) {
        return channels[i];
      }
    }
  }
  return null;
}

function handleChannelGridArrow(
  event: ArrowKeyEvent,
  current: HTMLElement,
  channels: readonly HTMLElement[],
  layout: StageLayout,
  lane: HTMLElement | null,
  times: HTMLElement | null,
  board: HTMLElement,
  dockControl: HTMLElement | null,
  hasMore: boolean,
  onRequestMore: () => void,
): void {
  const index = channels.indexOf(current);
  if (index === -1) {
    return;
  }
  const columns = gridColumnCount(channels, current);

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
    const rowEnd = Math.min(channels.length - 1, index - col + columns - 1);
    const next =
      col + 1 < columns
        ? findVisibleChannelInRange(channels, index + 1, rowEnd, 1)
        : null;
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
    return;
  }

  if (event.key === "ArrowLeft") {
    const col = index % columns;
    const rowStart = index - col;
    const next =
      col > 0
        ? findVisibleChannelInRange(channels, index - 1, rowStart, -1)
        : null;
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
    return;
  }

  if (event.key === "ArrowDown") {
    const next = findVisibleChannelInRange(
      channels,
      index + columns,
      channels.length - 1,
      columns,
    );
    if (next !== null) {
      consumeAndFocus(event, next, board);
      return;
    }
    const currentRow = Math.floor(index / columns);
    const lastRow = Math.floor((channels.length - 1) / columns);
    if (currentRow < lastRow) {
      const lastVisible = findVisibleChannelInRange(
        channels,
        channels.length - 1,
        lastRow * columns,
        -1,
      );
      if (lastVisible !== null) {
        consumeAndFocus(event, lastVisible, board);
        return;
      }
    }
    const retainedButton = visibleBoardFooterButton(
      board,
      ".programme-guide__retained button",
    );
    if (retainedButton !== null) {
      consumeAndFocus(event, retainedButton, board);
      return;
    }
    if (hasMore) {
      onRequestMore();
      const moreButton = visibleBoardFooterButton(
        board,
        ".programme-guide__more",
      );
      if (moreButton !== null) {
        consumeAndFocus(event, moreButton, board);
        return;
      }
    }
    clampArrow(event);
    return;
  }

  if (event.key === "ArrowUp") {
    const prev = findVisibleChannelInRange(
      channels,
      index - columns,
      0,
      -columns,
    );
    if (prev !== null) {
      consumeAndFocus(event, prev, board);
      return;
    }
    const laneTarget = preferredLaneTarget(lane, current);
    const next =
      preferredTimesTarget(times) ??
      (dockControl !== null &&
      overlapsDockBand(dockControl, current) &&
      // The TV band's lane lies between the rows and the picture.
      !liesBelow(laneTarget, dockControl)
        ? dockControl
        : null) ??
      laneTarget;
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
  }
}

function visibleDockControl(workspace: HTMLElement | null): HTMLElement | null {
  const dock = workspace?.querySelector<HTMLElement>(".stage__undock") ?? null;
  if (dock === null || !isVisibleFocusable(dock)) {
    return null;
  }
  const rect = dock.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0 ? dock : null;
}

function isSideBySideDock(
  dockControl: HTMLElement,
  workspace: HTMLElement | null,
): boolean {
  const shell = dockControl.closest<HTMLElement>(".shell");
  if (shell?.getAttribute("data-dock") === "false") {
    return false;
  }
  const stage = dockControl.closest<HTMLElement>(".stage");
  if (stage !== null && workspace !== null) {
    const stageWidth = stage.getBoundingClientRect().width;
    const workspaceWidth = workspace.getBoundingClientRect().width;
    if (stageWidth > 0 && workspaceWidth > 0 && stageWidth >= workspaceWidth - 4) {
      return false;
    }
  }
  return true;
}

function overlapsDockBand(dockControl: HTMLElement, origin: HTMLElement): boolean {
  // Check the whole docked preview band (picture + metadata + undock icon) so
  // controls directly under the preview picture still step up to the preview.
  const band = dockControl.closest<HTMLElement>(".stage");
  const bandRect = band?.getBoundingClientRect();
  const candidateRect =
    bandRect !== undefined && bandRect.width > 0
      ? bandRect
      : dockControl.getBoundingClientRect();
  const originRect = origin.getBoundingClientRect();
  if (candidateRect.width === 0 || originRect.width === 0) {
    return false;
  }
  const originCenter = originRect.left + originRect.width / 2;
  return originCenter >= candidateRect.left && originCenter <= candidateRect.right;
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
  const whenButton = lane.querySelector<HTMLElement>(LEAD_SELECTOR);
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
  const channels = Array.from(
    board.querySelectorAll<HTMLElement>(
      ".programme-guide__row .programme-guide__channel",
    ),
  );
  if (channels.length === 0) {
    return visibleFocusables(board, "button")[0] ?? null;
  }
  const columns = gridColumnCount(channels);
  const boardRect = board.getBoundingClientRect();
  if (boardRect.height === 0) {
    return findVisibleChannelInRange(channels, 0, channels.length - 1, 1);
  }
  let topRowIndex = 0;
  for (let i = 0; i < channels.length; i += 1) {
    const rect = channels[i].getBoundingClientRect();
    if (rect.bottom > boardRect.top + ROW_TOP_TOLERANCE_PX) {
      topRowIndex = i;
      break;
    }
  }
  let firstVisible: HTMLElement | null = null;
  let firstTop = 0;
  for (let i = topRowIndex; i < channels.length; i += 1) {
    if (isVisibleFocusable(channels[i])) {
      firstVisible = channels[i];
      firstTop = channels[i].getBoundingClientRect().top;
      topRowIndex = i;
      break;
    }
  }
  if (firstVisible === null) {
    return visibleFocusables(board, "button")[0] ?? null;
  }
  const topBand: HTMLElement[] = [firstVisible];
  for (
    let i = topRowIndex + 1;
    i < Math.min(channels.length, topRowIndex + columns);
    i += 1
  ) {
    const candidate = channels[i];
    const rect = candidate.getBoundingClientRect();
    if (Math.abs(rect.top - firstTop) <= ROW_TOP_TOLERANCE_PX) {
      if (isVisibleFocusable(candidate)) {
        topBand.push(candidate);
      }
    } else {
      break;
    }
  }
  return nearestByHorizontalCenter(topBand, origin) ?? firstVisible;
}

function gridColumnCount(
  channels: readonly HTMLElement[],
  current?: HTMLElement,
): number {
  const rowsContainer =
    (current ?? channels[0])?.closest<HTMLElement>(".programme-guide__rows") ??
    null;
  if (rowsContainer !== null) {
    const style = window.getComputedStyle(rowsContainer);
    if (
      style.display === "grid" &&
      style.gridTemplateColumns.length > 0 &&
      !/[(]/u.test(style.gridTemplateColumns)
    ) {
      const tracks = style.gridTemplateColumns
        .trim()
        .split(/\s+/u)
        .filter((track) => track.length > 0 && track !== "none");
      if (tracks.length > 0) {
        return tracks.length;
      }
    }
  }
  if (channels.length < 2) {
    return 1;
  }
  const anchorIndex =
    current === undefined ? 0 : Math.max(0, channels.indexOf(current));
  const anchor = channels[anchorIndex].getBoundingClientRect();
  if (anchor.width === 0 && anchor.height === 0) {
    return 1;
  }
  let columns = 1;
  for (let i = anchorIndex - 1; i >= 0; i -= 1) {
    const rect = channels[i].getBoundingClientRect();
    if (Math.abs(rect.top - anchor.top) <= ROW_TOP_TOLERANCE_PX) {
      columns += 1;
    } else {
      break;
    }
  }
  for (let i = anchorIndex + 1; i < channels.length; i += 1) {
    const rect = channels[i].getBoundingClientRect();
    if (Math.abs(rect.top - anchor.top) <= ROW_TOP_TOLERANCE_PX) {
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
  return isVisibleControl(element, {
    allowNegativeTabIndex: element.classList.contains("programme-guide__group"),
  });
}

function clampArrow(event: ArrowKeyEvent): void {
  event.preventDefault();
  event.stopPropagation();
}

function consumeAndFocus(
  event: ArrowKeyEvent,
  next: HTMLElement,
  board: HTMLElement | null,
): void {
  event.preventDefault();
  event.stopPropagation();
  next.focus({ preventScroll: true });
  // Group radios already reveal themselves inside ChannelGroupLane's onFocus;
  // only time-row chips need horizontal reveal here.
  if (!next.classList.contains("programme-guide__group")) {
    const times = next.closest<HTMLElement>(".programme-guide__times");
    if (times !== null) {
      revealInHorizontalScroller(times, next);
    }
  }
  if (board !== null && board.contains(next)) {
    scrollBoardToReveal(board, next);
  }
}

/** Scrolls the guide board directly to keep `target` clear of sticky footers. */
export function scrollBoardToReveal(board: HTMLElement, target: HTMLElement): void {
  const row = target.closest<HTMLElement>(".programme-guide__row") ?? target;
  const retained = board.querySelector<HTMLElement>(".programme-guide__retained");
  const retainedHeight =
    retained !== null && !retained.contains(target) ? retained.offsetHeight : 0;
  const ruler = board.querySelector<HTMLElement>(".programme-guide__timeline-ruler");
  revealInScrollContainer(board, row, {
    marginPx: BOARD_FOCUS_MARGIN_PX + (ruler?.offsetHeight ?? 0),
    bottomMarginPx: BOARD_FOCUS_MARGIN_PX + retainedHeight,
  });
}

/**
 * Brings a TV timeline cell into the stretch of time the timeline shows, in
 * half-hour steps, and remembers it as the cell to return to from the rail.
 * Only the board's `--tl-offset`, the first minute shown, changes.
 */
export function revealTimelineCell(board: HTMLElement, cell: HTMLElement): void {
  if (board.dataset.view !== "timeline") return;
  lastTimelineCell.set(board, cell);
  const start = Number(cell.dataset.start);
  const end = Number(cell.dataset.end);
  if (!Number.isFinite(start) || !Number.isFinite(end)) return;
  const total = Number(board.dataset.windowMinutes) || TIMELINE_VIEW_MINUTES;
  const current = timelineOffset(board);
  let offset = current;
  if (start >= current + TIMELINE_VIEW_MINUTES - 15) {
    // A cell starting near or past the right edge: show it with half an hour
    // of what came before it.
    offset = Math.floor(start / 30) * 30 - 30;
  } else if (end <= current + 5) {
    offset = Math.floor(start / 30) * 30;
  }
  offset = Math.min(Math.max(offset, 0), Math.max(0, total - TIMELINE_VIEW_MINUTES));
  if (offset === current) return;
  board.style.setProperty("--tl-offset", String(offset));
  const nowMinutes = Number(board.dataset.nowMinutes);
  board.dataset.nowHidden = String(
    Number.isFinite(nowMinutes) && (nowMinutes < offset || nowMinutes > offset + TIMELINE_VIEW_MINUTES),
  );
}

function timelineOffset(board: HTMLElement): number {
  const value = Number(board.style.getPropertyValue("--tl-offset"));
  return Number.isFinite(value) ? value : 0;
}

/** What a timeline row offers the D-pad: its cells, or its Channel when it has none. */
function timelineStops(row: HTMLElement): HTMLElement[] {
  const cells = visibleFocusables(row, ".programme-guide__programme");
  if (cells.length > 0) return cells;
  const channel = row.querySelector<HTMLElement>(".programme-guide__channel");
  return channel !== null && isVisibleFocusable(channel) ? [channel] : [];
}

/** The cell of a row on at `minute`, else the one starting nearest to it. */
function timelineCellAt(row: HTMLElement, minute: number): HTMLElement | null {
  const stops = timelineStops(row);
  let best: HTMLElement | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const stop of stops) {
    const start = Number(stop.dataset.start);
    const end = Number(stop.dataset.end);
    if (!Number.isFinite(start) || !Number.isFinite(end)) return stop;
    if (start <= minute && minute < end) return stop;
    const distance = Math.min(Math.abs(start - minute), Math.abs(end - minute));
    if (distance < bestDistance) {
      bestDistance = distance;
      best = stop;
    }
  }
  return best;
}

/** The minute a move up or down keeps to: the focused cell's, within view. */
function timelineAnchor(board: HTMLElement, target: HTMLElement): number {
  const offset = timelineOffset(board);
  const start = Number(target.dataset.start);
  if (Number.isFinite(start)) return Math.max(start, offset);
  const now = Number(board.dataset.nowMinutes);
  return Math.max(Number.isFinite(now) ? now : 0, offset);
}

/** Where Right from the rail lands: the cell last focused, else what is on now in view. */
function timelineEntry(board: HTMLElement | null, origin: HTMLElement): HTMLElement | null {
  if (board === null || board.dataset.view !== "timeline") return null;
  const last = lastTimelineCell.get(board);
  if (last !== undefined && last.isConnected && isVisibleFocusable(last)) {
    return last;
  }
  const channel = firstVisibleBoardTarget(board, origin);
  const row = channel?.closest<HTMLElement>(".programme-guide__row") ?? null;
  return row === null ? channel : (timelineCellAt(row, timelineAnchor(board, origin)) ?? channel);
}

/**
 * The TV timeline: Up and Down change Channel and keep to the time in view,
 * Left and Right step one cell through time, and Left from a row's first
 * cell reaches the rail of Channel Groups.
 */
function handleTimelineArrow(
  event: ArrowKeyEvent,
  target: HTMLElement,
  lane: HTMLElement | null,
  board: HTMLElement,
  hasMore: boolean,
  onRequestMore: () => void,
): void {
  const row = target.closest<HTMLElement>(".programme-guide__row");
  if (row === null) return;
  const stops = timelineStops(row);
  const index = stops.indexOf(target);
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    if (target.classList.contains("programme-guide__channel")) {
      const next =
        event.key === "ArrowRight"
          ? (timelineCellAt(row, timelineAnchor(board, target)) ?? null)
          : railTarget(lane);
      if (next !== null && next !== target) {
        consumeAndFocus(event, next, board);
      } else {
        clampArrow(event);
      }
      return;
    }
    const next =
      event.key === "ArrowRight"
        ? (stops[index + 1] ?? null)
        : index > 0
          ? stops[index - 1]
          : railTarget(lane);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
    return;
  }
  const rows = Array.from(
    board.querySelectorAll<HTMLElement>(".programme-guide__row"),
  ).filter((candidate) => timelineStops(candidate).length > 0);
  const rowIndex = rows.indexOf(row);
  const anchor = timelineAnchor(board, target);
  if (event.key === "ArrowDown") {
    const below = rows[rowIndex + 1];
    if (below !== undefined) {
      const next = timelineCellAt(below, anchor);
      if (next !== null) {
        consumeAndFocus(event, next, board);
        return;
      }
    }
    const footer =
      visibleBoardFooterButton(board, ".programme-guide__retained button") ??
      (hasMore ? (onRequestMore(), visibleBoardFooterButton(board, ".programme-guide__more")) : null);
    if (footer !== null) {
      consumeAndFocus(event, footer, board);
    } else {
      clampArrow(event);
    }
    return;
  }
  if (event.key === "ArrowUp") {
    const above = rows[rowIndex - 1];
    const next =
      above === undefined ? null : timelineCellAt(above, anchor);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      // The top row: the lead of the rail, where the band is one press up.
      const lead = lane?.querySelector<HTMLElement>(LEAD_SELECTOR) ?? null;
      if (lead !== null && isVisibleFocusable(lead)) {
        consumeAndFocus(event, lead, board);
      } else {
        clampArrow(event);
      }
    }
  }
}

/** The rail's stop for a move into it: the chosen Channel Group. */
function railTarget(lane: HTMLElement | null): HTMLElement | null {
  return (
    checkedGroupInLane(lane) ??
    (lane === null ? null : visibleFocusables(lane, LEAD_SELECTOR)[0] ?? null)
  );
}

/**
 * The lane stood on its side beside the TV timeline: Up and Down step
 * through the layout switch, the Channel Groups and Choose groups; Right
 * returns to the timeline; Up from the top reaches the band.
 */
function handleRailArrow(
  event: ArrowKeyEvent,
  target: HTMLElement,
  toolbar: HTMLElement | null,
  lane: HTMLElement,
  board: HTMLElement | null,
  dockControl: HTMLElement | null,
): void {
  const stops = visibleFocusables(
    lane,
    `${LEAD_SELECTOR}, .programme-guide__group, .group-roster-trigger`,
  );
  const index = stops.indexOf(target);
  if (index === -1) return;
  if (event.key === "ArrowRight") {
    const next = timelineEntry(board, target) ?? firstVisibleBoardTarget(board, target);
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
    return;
  }
  if (event.key === "ArrowLeft") {
    clampArrow(event);
    return;
  }
  if (event.key === "ArrowDown") {
    const next = stops[index + 1] ?? null;
    if (next !== null) {
      consumeAndFocus(event, next, board);
    } else {
      clampArrow(event);
    }
    return;
  }
  if (index > 0) {
    consumeAndFocus(event, stops[index - 1], board);
    return;
  }
  const next =
    (dockControl !== null && overlapsDockBand(dockControl, target) ? dockControl : null) ??
    nearestToolbarTarget(toolbar, target);
  if (next !== null) {
    consumeAndFocus(event, next, board);
  } else {
    clampArrow(event);
  }
}

/** Whether `element` lies wholly under `above`, as the TV lane lies under the band. */
function liesBelow(element: HTMLElement | null, above: HTMLElement): boolean {
  if (element === null) return false;
  const box = element.getBoundingClientRect();
  const over = above.getBoundingClientRect();
  if (box.height === 0 || over.height === 0) return false;
  return box.top >= over.bottom - ROW_TOP_TOLERANCE_PX;
}

/** The toolbar's first control from the left: where Right from the TV band's picture goes. */
function firstToolbarTarget(toolbar: HTMLElement | null): HTMLElement | null {
  if (toolbar === null) return null;
  return (
    visibleFocusables(
      toolbar,
      'button, input, a[href], [tabindex]:not([tabindex="-1"])',
    )[0] ?? null
  );
}
