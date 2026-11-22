/**
 * The outline of a loaded Channel Icon, from its own width and height. A
 * square icon is most often an app-style tile with its own light ground, so
 * the TV gives it a light plate it can fill; a wide or tall one is most often
 * a wordmark drawn for a dark ground, so it keeps the dark plate.
 */
export type IconShape = "square" | "wide" | "tall";

// Within this ratio of width to height either way an icon counts as square.
const SQUARE_RATIO = 1.25;

/** The outline of an icon `width` by `height` pixels, or null when unknown. */
export function iconShape(width: number, height: number): IconShape | null {
  if (!(width > 0) || !(height > 0)) return null;
  const ratio = width / height;
  if (ratio > SQUARE_RATIO) return "wide";
  if (ratio < 1 / SQUARE_RATIO) return "tall";
  return "square";
}
