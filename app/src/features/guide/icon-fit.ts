/**
 * How a Channel Icon sits on its plate, read from the icon's own pixels.
 *
 * Logos arrive with whatever padding their artist gave them: a wordmark in a
 * thin strip across a large white tile, or a coin with a wide transparent
 * margin. Fitting the whole image makes those look small beside a logo that
 * fills its own canvas. So the TV finds the artwork's ink (what is drawn,
 * without the padding, the tile's ground or the frame at its rim), fits that
 * ink by one rule, and, when the icon is a tile with a solid ground, paints
 * the plate in that ground so tile and plate are one surface.
 *
 * Pixels are read only from an icon whose pixels the page may read without
 * asking another origin: a same-origin, `data:` or `blob:` location. A
 * cross-origin icon is never drawn to a canvas and never fetched again with
 * CORS: the WebView logs a refused CORS request with the icon's full
 * location, and Android forwards that line to Logcat. Those icons take the
 * outline-only fit in `television.css`. An icon's location is never logged.
 */

/** A part of an image, as fractions of its width and height. */
export interface IconBox {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
}

/** What an icon's pixels say: where its ink lies, and its tile's ground. */
export interface IconInk {
  readonly box: IconBox;
  /** The tile's solid ground as `rgb(r g b)`, or null for a transparent icon. */
  readonly ground: string | null;
}

/** Where the image goes on the plate, as fractions of the plate. */
export interface IconFit {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
  /** The image's part to show, a CSS `inset()`, for a tile with a ground. */
  readonly clip: string | null;
  readonly ground: string | null;
}

/** Every plate is 4:3 (see `television.css`). */
export const PLATE_ASPECT = 4 / 3;

// The side of the square the image is read at.
const READ_SIZE = 96;
// The band along each side where a tile's ground is looked for.
const BAND = 0.15;
// How much of every side's band the ground must cover.
const GROUND_SHARE = 0.4;
// How far a pixel may stray from the ground and still be ground.
const GROUND_TOLERANCE = 20;
// How far a pixel must stray from the ground to count as ink.
const INK_CONTRAST = 32;
// The least alpha that counts as drawn.
const INK_ALPHA = 24;
// The room kept around the ink when a tile is cut to it.
const CLIP_MARGIN = 0.03;

/**
 * Finds the ink in RGBA `pixels` of a `width` by `height` image, or null
 * when there is none or it cannot be told safely from its frame.
 *
 * A tile has a ground when one opaque colour covers much of the band along
 * every side. Its ink is then what the ground encloses and stands apart from
 * it: the frame at the rim and the transparent corners lie outside the ground
 * and are left out. When something drawn reaches from the rim far into the
 * tile, the ink cannot be told from the frame, and the icon is left whole.
 * Without a ground, the ink is everything drawn.
 */
export function iconInk(
  pixels: ArrayLike<number>,
  width: number,
  height: number,
): IconInk | null {
  if (width <= 0 || height <= 0 || pixels.length < width * height * 4) {
    return null;
  }
  const ground = tileGround(pixels, width, height);
  const inked = (index: number) => {
    const at = index * 4;
    if ((pixels[at + 3] ?? 0) < INK_ALPHA) return false;
    if (ground === null) return true;
    return colourDistance(pixels, at, ground) > INK_CONTRAST;
  };
  const isGround = (index: number) =>
    ground !== null &&
    (pixels[index * 4 + 3] ?? 0) >= 250 &&
    colourDistance(pixels, index * 4, ground) <= GROUND_TOLERANCE;

  // What lies outside the ground: reached from the edge without crossing it.
  const outside = new Uint8Array(width * height);
  if (ground !== null) {
    const queue: number[] = [];
    const visit = (index: number) => {
      if (outside[index] === 1 || isGround(index)) return;
      outside[index] = 1;
      queue.push(index);
    };
    for (let x = 0; x < width; x += 1) {
      visit(x);
      visit((height - 1) * width + x);
    }
    for (let y = 0; y < height; y += 1) {
      visit(y * width);
      visit(y * width + width - 1);
    }
    while (queue.length > 0) {
      const index = queue.pop() as number;
      const x = index % width;
      const y = (index - x) / width;
      if (x > 0) visit(index - 1);
      if (x < width - 1) visit(index + 1);
      if (y > 0) visit(index - width);
      if (y < height - 1) visit(index + width);
    }
  }

  let minX = width;
  let minY = height;
  let maxX = -1;
  let maxY = -1;
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const index = y * width + x;
      if (!inked(index)) continue;
      if (outside[index] === 1) {
        // Drawing outside the ground deep inside the tile: not a frame.
        if (inside(x / width, y / height)) return null;
        continue;
      }
      if (x < minX) minX = x;
      if (x > maxX) maxX = x;
      if (y < minY) minY = y;
      if (y > maxY) maxY = y;
    }
  }
  if (maxX < 0) return null;
  return {
    box: {
      x: minX / width,
      y: minY / height,
      w: (maxX + 1 - minX) / width,
      h: (maxY + 1 - minY) / height,
    },
    ground:
      ground === null
        ? null
        : `rgb(${Math.round(ground[0])} ${Math.round(ground[1])} ${Math.round(ground[2])})`,
  };
}

/**
 * Places an image of `naturalWidth` by `naturalHeight` pixels on a 4:3 plate
 * so its ink is centred and sized by one rule:
 *
 * - square or wide ink may be 62 % of the plate's height;
 * - it may take 76 % of the plate's width, and a thin strip more, rising to
 *   90 % for a strip five times as wide as it is tall, so a wordmark in a
 *   thin strip reads at the weight of its neighbours;
 * - tall ink may grow taller, by the square root of how tall it is, to at
 *   most 80 % of the plate's height.
 *
 * The ink keeps its own proportions within those limits.
 */
export function iconFit(
  ink: IconInk,
  naturalWidth: number,
  naturalHeight: number,
): IconFit {
  const { box } = ink;
  const aspect = (box.w * naturalWidth) / (box.h * naturalHeight);
  const thin = Math.min(1, Math.max(0, (aspect - 2.5) / 2.5));
  const maxWidth = PLATE_ASPECT * (0.76 + 0.14 * thin);
  const maxHeight = aspect >= 1 ? 0.62 : Math.min(0.8, 0.62 / Math.sqrt(aspect));
  // In plate heights.
  const inkWidth = Math.min(maxWidth, maxHeight * aspect);
  const inkHeight = inkWidth / aspect;
  const width = inkWidth / PLATE_ASPECT / box.w;
  const height = inkHeight / box.h;
  return {
    left: 0.5 - (box.x + box.w / 2) * width,
    top: 0.5 - (box.y + box.h / 2) * height,
    width,
    height,
    clip:
      ink.ground === null
        ? null
        : `inset(${percent(box.y - CLIP_MARGIN)} ${percent(1 - box.x - box.w - CLIP_MARGIN)} ${percent(1 - box.y - box.h - CLIP_MARGIN)} ${percent(box.x - CLIP_MARGIN)})`,
    ground: ink.ground,
  };
}

const known = new Map<string, IconInk | null>();

/**
 * The ink of a loaded icon, read from the image shown, or null when it
 * cannot be read. Each location is read once per session. A location whose
 * pixels belong to another origin is not read at all (see above).
 */
export function loadedIconInk(
  location: string,
  image: HTMLImageElement,
): IconInk | null {
  const seen = known.get(location);
  if (seen !== undefined) return seen;
  const ink = iconPixelsReadable(location) ? readImageInk(image) : null;
  known.set(location, ink);
  return ink;
}

/**
 * Whether the page may read an icon's pixels without a cross-origin
 * request: a `data:` or `blob:` location, or one on the page's own origin.
 */
export function iconPixelsReadable(location: string): boolean {
  let url: URL;
  try {
    url = new URL(location, document.baseURI);
  } catch {
    return false;
  }
  if (url.protocol === "data:" || url.protocol === "blob:") return true;
  return url.origin !== "null" && url.origin === window.location.origin;
}

function readImageInk(image: HTMLImageElement): IconInk | null {
  const { naturalWidth, naturalHeight } = image;
  if (!(naturalWidth > 0) || !(naturalHeight > 0)) return null;
  const scale = Math.min(1, READ_SIZE / Math.max(naturalWidth, naturalHeight));
  const width = Math.max(1, Math.round(naturalWidth * scale));
  const height = Math.max(1, Math.round(naturalHeight * scale));
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (context === null) return null;
  try {
    context.drawImage(image, 0, 0, width, height);
    return iconInk(context.getImageData(0, 0, width, height).data, width, height);
  } catch {
    // A same-origin location that redirected elsewhere taints the canvas.
    return null;
  }
}

/** The colour that covers much of the band along every side, if one does. */
function tileGround(
  pixels: ArrayLike<number>,
  width: number,
  height: number,
): readonly [number, number, number] | null {
  const bandX = Math.max(1, Math.round(width * BAND));
  const bandY = Math.max(1, Math.round(height * BAND));
  const counts = new Map<number, { n: number; r: number; g: number; b: number }>();
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      if (x >= bandX && x < width - bandX && y >= bandY && y < height - bandY) continue;
      const at = (y * width + x) * 4;
      if ((pixels[at + 3] ?? 0) < 250) continue;
      const r = pixels[at] ?? 0;
      const g = pixels[at + 1] ?? 0;
      const b = pixels[at + 2] ?? 0;
      const key = ((r >> 4) << 8) | ((g >> 4) << 4) | (b >> 4);
      const entry = counts.get(key);
      if (entry === undefined) counts.set(key, { n: 1, r, g, b });
      else {
        entry.n += 1;
        entry.r += r;
        entry.g += g;
        entry.b += b;
      }
    }
  }
  let best: { n: number; r: number; g: number; b: number } | null = null;
  for (const entry of counts.values()) {
    if (best === null || entry.n > best.n) best = entry;
  }
  if (best === null) return null;
  const ground = [best.r / best.n, best.g / best.n, best.b / best.n] as const;
  const sides = [
    { x0: 0, x1: width, y0: 0, y1: bandY },
    { x0: 0, x1: width, y0: height - bandY, y1: height },
    { x0: 0, x1: bandX, y0: 0, y1: height },
    { x0: width - bandX, x1: width, y0: 0, y1: height },
  ];
  for (const side of sides) {
    let covered = 0;
    let all = 0;
    for (let y = side.y0; y < side.y1; y += 1) {
      for (let x = side.x0; x < side.x1; x += 1) {
        const at = (y * width + x) * 4;
        all += 1;
        if ((pixels[at + 3] ?? 0) >= 250 && colourDistance(pixels, at, ground) <= GROUND_TOLERANCE) {
          covered += 1;
        }
      }
    }
    if (covered < all * GROUND_SHARE) return null;
  }
  return ground;
}

function colourDistance(
  pixels: ArrayLike<number>,
  at: number,
  colour: readonly [number, number, number],
): number {
  return Math.max(
    Math.abs((pixels[at] ?? 0) - colour[0]),
    Math.abs((pixels[at + 1] ?? 0) - colour[1]),
    Math.abs((pixels[at + 2] ?? 0) - colour[2]),
  );
}

/** Whether a point lies well inside the tile, clear of any frame. */
function inside(x: number, y: number): boolean {
  return x > BAND && x < 1 - BAND && y > BAND && y < 1 - BAND;
}

function percent(fraction: number): string {
  return `${(Math.max(0, fraction) * 100).toFixed(2)}%`;
}
