/** A row's place in a scrolling list, measured from the top of what scrolls. */
export interface ListItemBox {
  readonly top: number;
  readonly height: number;
}

/** The part of a scrolling list that is in view. */
export interface ListView {
  readonly scrollTop: number;
  readonly height: number;
}

/**
 * Returns the scroll offset that puts a row in the middle of its list's view.
 * With `always` false a row that is already wholly in view stays where it is:
 * the offset comes back unchanged.
 */
export function revealScrollTop(
  item: ListItemBox,
  view: ListView,
  always: boolean,
): number {
  const inView =
    item.top >= view.scrollTop &&
    item.top + item.height <= view.scrollTop + view.height;
  if (inView && !always) {
    return view.scrollTop;
  }
  return Math.max(0, Math.round(item.top - (view.height - item.height) / 2));
}
