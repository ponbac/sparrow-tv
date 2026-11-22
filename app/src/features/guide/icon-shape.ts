/**
 * The outline of a loaded Channel Icon, from its own width and height, the
 * one thing known of a cross-origin icon without reading its pixels. A
 * square icon is most often an app-style tile with its own light ground and
 * a frame at its rim, so the TV lets it cover its plate, zoomed until the
 * frame lies outside (`television.css`); a wide or tall one is most often a
 * wordmark drawn for a dark ground, so it keeps the dark plate and is shown
 * whole.
 *
 * Tiles are drawn square to the pixel, so only an icon within a few percent
 * of square counts. A logo nearly square but not quite (a full-bleed station
 * badge, a tall numeral on a transparent canvas) mostly fills its canvas to
 * the edge, and covering the plate would cut its artwork, so it is shown
 * whole as a wordmark is. A round logo on a transparent square canvas cannot
 * be told from a tile by its size; it covers the plate too.
 */
export type IconShape = "square" | "wide" | "tall";

// Within this ratio of width to height either way an icon counts as square.
const SQUARE_RATIO = 1.05;

/** The outline of an icon `width` by `height` pixels, or null when unknown. */
export function iconShape(width: number, height: number): IconShape | null {
  if (!(width > 0) || !(height > 0)) return null;
  const ratio = width / height;
  if (ratio > SQUARE_RATIO) return "wide";
  if (ratio < 1 / SQUARE_RATIO) return "tall";
  return "square";
}
