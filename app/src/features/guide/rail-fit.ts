/**
 * The TV timeline's rail of Channel Groups shows whole groups only: its view
 * ends on a whole row, the room left under that row is covered (and fades
 * only over the next row's edge), and scrolling moves by whole rows.
 */

/** How many whole rows a view holds, and the room under the last of them. */
export interface RailFit {
  readonly rows: number;
  readonly rest: number;
}

/**
 * Fits rows `rowHeight` tall, one every `pitch` (the row and the gap after
 * it), into a view `viewHeight` tall.
 */
export function railFit(
  viewHeight: number,
  rowHeight: number,
  pitch: number,
): RailFit {
  if (!(viewHeight > 0) || !(rowHeight > 0) || !(pitch >= rowHeight)) {
    return { rows: 0, rest: 0 };
  }
  if (viewHeight < rowHeight) {
    return { rows: 0, rest: viewHeight };
  }
  const rows = Math.floor((viewHeight - rowHeight) / pitch) + 1;
  return { rows, rest: viewHeight - ((rows - 1) * pitch + rowHeight) };
}

/**
 * The scroll offset that keeps row `index` of `count` wholly in view while
 * the first row shown stays a whole row: unchanged when it is already in
 * view, else the fewest rows of movement that bring it in.
 */
export function railScrollTop(
  index: number,
  count: number,
  scrollTop: number,
  fit: RailFit,
  pitch: number,
): number {
  if (fit.rows === 0 || !(pitch > 0)) return scrollTop;
  const lastFirst = Math.max(0, count - fit.rows);
  let first = Math.min(lastFirst, Math.max(0, Math.round(scrollTop / pitch)));
  if (index < first) {
    first = index;
  } else if (index > first + fit.rows - 1) {
    first = index - fit.rows + 1;
  }
  return Math.min(lastFirst, Math.max(0, first)) * pitch;
}
