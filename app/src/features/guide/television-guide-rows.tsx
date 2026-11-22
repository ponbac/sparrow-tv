import {
  useCallback,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type FocusEvent,
  type ReactNode,
  type Ref,
} from "react";
import type { GuideFamily } from "./guide-families";

const OVERSCAN_LINES = 2;
const INITIAL_VISIBLE_LINES = 6;

export interface TelevisionRowsHandle {
  readonly revealIndex: (index: number) => void;
}

/**
 * The TV has three fixed-height cards per line in Now and one in Timeline.
 * Keep the natural scroll height of all loaded Channels while mounting the
 * viewport and a small margin for the next remote presses. A focused or
 * playing Channel stays available even when the viewer scrolls away from it.
 */
export function TelevisionGuideRows({
  families,
  timeline,
  playingIndex,
  firstNewRow,
  handle,
  onCommit,
  onFocus,
  renderRow,
}: {
  readonly families: readonly GuideFamily[];
  readonly timeline: boolean;
  readonly playingIndex: number;
  readonly firstNewRow: number;
  readonly handle: Ref<TelevisionRowsHandle>;
  readonly onCommit: (board: HTMLElement) => void;
  readonly onFocus: (event: FocusEvent<HTMLDivElement>) => void;
  readonly renderRow: (family: GuideFamily, index: number) => ReactNode;
}) {
  const rowsRef = useRef<HTMLDivElement>(null);
  const columns = timeline ? 1 : 3;
  const lineCount = Math.ceil(families.length / columns);
  const [viewport, setViewport] = useState({
    first: 0,
    visible: INITIAL_VISIBLE_LINES,
    retained: -1,
  });
  const capacity = viewport.visible + 2 * OVERSCAN_LINES;
  const first = Math.min(viewport.first, Math.max(0, lineCount - capacity));
  const end = Math.min(lineCount, first + capacity);

  const revealIndex = useCallback((index: number) => {
    const row = document.activeElement?.closest<HTMLElement>("[data-guide-index]");
    const retained = row && rowsRef.current?.contains(row) ? Number(row.dataset.guideIndex) : -1;
    const line = Math.floor(index / columns);
    setViewport((view) => {
      const size = view.visible + 2 * OVERSCAN_LINES;
      const max = Math.max(0, lineCount - size);
      const current = Math.min(view.first, max);
      // Keep the adjacent line ready even during key repeats that arrive
      // before the browser dispatches its next scroll event.
      if ((line > current || current === 0) &&
          (line < current + size - 1 || current === max)) return view;
      const first = Math.min(max, Math.max(0, line - Math.floor(size / 2)));
      return current === first ? view : { ...view, first, retained };
    });
  }, [columns, lineCount]);

  useImperativeHandle(handle, () => ({ revealIndex }), [revealIndex]);

  useLayoutEffect(() => {
    const rows = rowsRef.current;
    const board = rows?.parentElement;
    if (!rows || !board) return;
    let pitch = 0;
    let visible = INITIAL_VISIBLE_LINES;
    let top = 0;
    const followScroll = () => {
      if (pitch <= 0) return;
      const firstVisible = Math.max(0, Math.floor((board.scrollTop - top) / pitch));
      // Move in two-line increments so adjacent presses reuse the same DOM.
      const first = Math.max(0, 2 * Math.floor((firstVisible - OVERSCAN_LINES) / 2));
      const focus = document.activeElement?.closest<HTMLElement>("[data-guide-index]");
      const retained = focus && rows.contains(focus) ? Number(focus.dataset.guideIndex) : -1;
      setViewport((current) => current.first === first && current.visible === visible
        ? current
        : { first, visible, retained });
    };
    const measure = () => {
      const row = rows.querySelector<HTMLElement>(".programme-guide__row");
      if (!row) return;
      const gap = timeline ? 0 : parseFloat(getComputedStyle(rows).rowGap) || 0;
      pitch = row.getBoundingClientRect().height + gap;
      if (pitch <= 0 || board.clientHeight <= 0) return;
      top = rows.offsetTop;
      visible = Math.max(1, Math.ceil(board.clientHeight / pitch) + 1);
      followScroll();
    };
    measure();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(board);
    // A root font-size change can alter the pitch without resizing the board.
    const row = rows.querySelector<HTMLElement>(".programme-guide__row");
    if (row) observer?.observe(row);
    board.addEventListener("scroll", followScroll, { passive: true });
    return () => {
      observer?.disconnect();
      board.removeEventListener("scroll", followScroll);
    };
  }, [timeline]);

  useLayoutEffect(() => {
    const board = rowsRef.current?.parentElement;
    if (board) onCommit(board);
  });

  const indices = new Set<number>();
  for (let index = first * columns; index < Math.min(families.length, end * columns); index++) {
    indices.add(index);
  }
  // Keep destinations for focus returning from playback, dialogs or the More button.
  for (const index of [0, playingIndex, viewport.retained, firstNewRow]) {
    if (index >= 0 && index < families.length) indices.add(index);
  }

  return (
    <div
      className="programme-guide__rows"
      ref={rowsRef}
      data-windowed=""
      style={{
        height: `calc(${lineCount} * var(--guide-pitch) - var(--guide-row-gap))`,
        "--guide-columns": columns,
      }}
      onFocus={(event) => {
        const row = event.target.closest<HTMLElement>("[data-guide-index]");
        if (row) revealIndex(Number(row.dataset.guideIndex));
        onFocus(event);
      }}
    >
      {timeline && <div className="programme-guide__now-band" aria-hidden="true" />}
      {Array.from(indices).sort((a, b) => a - b).map((index) => renderRow(families[index], index))}
    </div>
  );
}
