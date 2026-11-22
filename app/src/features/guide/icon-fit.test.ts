import { describe, expect, it } from "vitest";
import { iconFit, iconInk, PLATE_ASPECT, type IconInk } from "./icon-fit";

type Rgba = readonly [number, number, number, number];

const CLEAR: Rgba = [0, 0, 0, 0];
const WHITE: Rgba = [255, 255, 255, 255];
const INK: Rgba = [20, 20, 20, 255];
const FRAME: Rgba = [0, 0, 0, 153];

/** An image `width` by `height` whose pixels `paint` gives. */
function image(
  width: number,
  height: number,
  paint: (x: number, y: number) => Rgba,
): Uint8ClampedArray {
  const pixels = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      pixels.set(paint(x, y), (y * width + x) * 4);
    }
  }
  return pixels;
}

/** A white tile with a frame at its rim and clear corners, as many logo sets draw. */
function framedTile(ink: (x: number, y: number) => boolean): Uint8ClampedArray {
  return image(40, 40, (x, y) => {
    const corner = (x < 2 || x > 37) && (y < 2 || y > 37);
    if (corner) return CLEAR;
    if (x === 0 || y === 0 || x === 39 || y === 39) return FRAME;
    return ink(x, y) ? INK : WHITE;
  });
}

describe("iconInk", () => {
  it("finds a thin wordmark inside a framed white tile, and the tile's ground", () => {
    const pixels = framedTile((x, y) => x >= 8 && x < 32 && y >= 18 && y < 21);

    expect(iconInk(pixels, 40, 40)).toEqual({
      box: { x: 8 / 40, y: 18 / 40, w: 24 / 40, h: 3 / 40 },
      ground: "rgb(255 255 255)",
    });
  });

  it("finds what a transparent logo draws, with no ground", () => {
    const pixels = image(40, 20, (x, y) =>
      x >= 5 && x < 35 && y >= 4 && y < 16 ? [220, 30, 40, 255] : CLEAR,
    );

    expect(iconInk(pixels, 40, 20)).toEqual({
      box: { x: 5 / 40, y: 4 / 20, w: 30 / 40, h: 12 / 20 },
      ground: null,
    });
  });

  it("leaves a tile whole when its drawing runs from the rim deep inside", () => {
    // A bar from the left edge to the middle cannot be told from a frame.
    const pixels = framedTile((x, y) => x < 24 && y >= 18 && y < 22);

    expect(iconInk(pixels, 40, 40)).toBeNull();
  });

  it("finds nothing in an empty image", () => {
    expect(iconInk(image(8, 8, () => CLEAR), 8, 8)).toBeNull();
  });
});

describe("iconFit", () => {
  const ink = (box: IconInk["box"], ground: string | null = null): IconInk => ({
    box,
    ground,
  });
  // The ink's size on the plate, in plate widths and plate heights.
  const inkSize = (fit: ReturnType<typeof iconFit>, box: IconInk["box"]) => ({
    width: fit.width * box.w,
    height: fit.height * box.h,
  });

  it("gives square ink 62 % of the plate's height, centred", () => {
    const box = { x: 0.2, y: 0.2, w: 0.5, h: 0.5 };
    const fit = iconFit(ink(box), 100, 100);

    expect(inkSize(fit, box).height).toBeCloseTo(0.62);
    expect(inkSize(fit, box).width).toBeCloseTo(0.62 / PLATE_ASPECT);
    expect(fit.left + fit.width * (box.x + box.w / 2)).toBeCloseTo(0.5);
    expect(fit.top + fit.height * (box.y + box.h / 2)).toBeCloseTo(0.5);
  });

  it("lets a wordmark take 76 % of the width, and a thin strip up to 90 %", () => {
    const wordmark = { x: 0, y: 0, w: 1, h: 1 };
    const strip = { x: 0.1, y: 0.45, w: 0.8, h: 0.1 };

    expect(inkSize(iconFit(ink(wordmark), 250, 100), wordmark).width).toBeCloseTo(0.76);
    expect(inkSize(iconFit(ink(strip), 100, 100), strip).width).toBeCloseTo(0.9);
  });

  it("lets tall ink grow taller, to at most 80 % of the height", () => {
    const box = { x: 0, y: 0, w: 1, h: 1 };

    expect(inkSize(iconFit(ink(box), 50, 100), box).height).toBeCloseTo(0.8);
    expect(inkSize(iconFit(ink(box), 80, 100), box).height).toBeCloseTo(
      0.62 / Math.sqrt(0.8),
    );
  });

  it("cuts a tile with a ground to its ink and paints the plate in that ground", () => {
    const box = { x: 0.25, y: 0.4, w: 0.5, h: 0.2 };

    expect(iconFit(ink(box), 100, 100).clip).toBeNull();
    expect(iconFit(ink(box, "rgb(255 255 255)"), 100, 100)).toMatchObject({
      clip: "inset(37.00% 22.00% 37.00% 22.00%)",
      ground: "rgb(255 255 255)",
    });
  });
});
