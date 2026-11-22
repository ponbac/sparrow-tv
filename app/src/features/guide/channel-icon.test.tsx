import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ChannelIcon } from "./channel-icon";
import { iconShape } from "./icon-shape";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.documentElement.removeAttribute("data-television");
});

const ICON = "http://logos.example/svt1.png";

function tile(container: HTMLElement): HTMLElement {
  const element = container.querySelector<HTMLElement>(".channel-icon");
  if (element === null) throw new Error("no tile");
  return element;
}

describe("ChannelIcon", () => {
  it("shows the Channel Number when the Channel has no icon", () => {
    const { container } = render(<ChannelIcon icon={null} number={7} />);

    expect(tile(container)).toHaveAttribute("data-icon", "none");
    expect(tile(container)).toHaveTextContent(/^7$/u);
    expect(container.querySelector("img")).toBeNull();
  });

  it("loads the icon lazily, without a referrer, behind the number", () => {
    const { container } = render(<ChannelIcon icon={ICON} number={1} />);
    const image = container.querySelector("img");

    expect(tile(container)).toHaveAttribute("data-icon", "loading");
    expect(image).toHaveAttribute("src", ICON);
    expect(image).toHaveAttribute("loading", "lazy");
    expect(image).toHaveAttribute("decoding", "async");
    expect(image).toHaveAttribute("referrerpolicy", "no-referrer");
    expect(image).toHaveAttribute("alt", "");

    fireEvent.load(image as HTMLImageElement);
    expect(tile(container)).toHaveAttribute("data-icon", "loaded");
  });

  it("says a loaded icon's outline, so the plate can fit it", () => {
    const { container } = render(<ChannelIcon icon={ICON} number={1} />);
    const image = container.querySelector("img") as HTMLImageElement;
    Object.defineProperty(image, "naturalWidth", { value: 512 });
    Object.defineProperty(image, "naturalHeight", { value: 177 });

    expect(tile(container)).not.toHaveAttribute("data-shape");
    fireEvent.load(image);

    expect(tile(container)).toHaveAttribute("data-shape", "wide");
  });

  it("falls back to the Channel Number when the icon fails to load", () => {
    const { container } = render(<ChannelIcon icon={ICON} number={12} />);

    fireEvent.error(container.querySelector("img") as HTMLImageElement);

    expect(tile(container)).toHaveAttribute("data-icon", "failed");
    expect(container.querySelector("img")).toBeNull();
    expect(tile(container)).toHaveTextContent(/^12$/u);
  });

  it("tries again when the tile is given another icon", () => {
    const { container, rerender } = render(
      <ChannelIcon icon={ICON} number={1} />,
    );
    fireEvent.error(container.querySelector("img") as HTMLImageElement);

    rerender(<ChannelIcon icon="https://logos.example/svt2.png" number={2} />);

    expect(tile(container)).toHaveAttribute("data-icon", "loading");
    expect(container.querySelector("img")).toHaveAttribute(
      "src",
      "https://logos.example/svt2.png",
    );
  });
});

describe("ChannelIcon on a TV", () => {
  it("never reads or re-requests a cross-origin icon's pixels", () => {
    // A CORS retry of an icon its host serves without CORS makes the WebView
    // log the icon's full location, which Android forwards to Logcat.
    document.documentElement.setAttribute("data-television", "");
    const crossOrigin = "https://logos.example/cross-origin.png";
    const { container } = render(<ChannelIcon icon={crossOrigin} number={3} />);
    const image = container.querySelector("img") as HTMLImageElement;
    Object.defineProperty(image, "naturalWidth", { value: 400 });
    Object.defineProperty(image, "naturalHeight", { value: 300 });
    const createElement = vi.spyOn(document, "createElement");
    const Image = vi.spyOn(window, "Image");

    fireEvent.load(image);

    expect(tile(container)).toHaveAttribute("data-icon", "loaded");
    expect(tile(container)).toHaveAttribute("data-shape", "wide");
    expect(tile(container)).not.toHaveAttribute("data-fit");
    expect(Image).not.toHaveBeenCalled();
    expect(createElement).not.toHaveBeenCalledWith("canvas");
    expect(createElement).not.toHaveBeenCalledWith("img");
    expect(container.querySelectorAll("img")).toHaveLength(1);
    expect(image).not.toHaveAttribute("crossorigin");
  });

  it("takes a square cross-origin icon for an app tile, by its size alone", () => {
    // The plate is filled by a square icon (`television.css`); nothing but
    // the image's natural size decides it.
    document.documentElement.setAttribute("data-television", "");
    const { container } = render(
      <ChannelIcon icon="https://logos.example/tv4.png" number={4} />,
    );
    const image = container.querySelector("img") as HTMLImageElement;
    Object.defineProperty(image, "naturalWidth", { value: 304 });
    Object.defineProperty(image, "naturalHeight", { value: 304 });
    const createElement = vi.spyOn(document, "createElement");

    fireEvent.load(image);

    expect(tile(container)).toHaveAttribute("data-shape", "square");
    expect(tile(container)).not.toHaveAttribute("data-fit");
    expect(createElement).not.toHaveBeenCalledWith("canvas");
  });

  it("fits an icon the page holds itself by the ink its pixels show", () => {
    // A 40 by 30 white tile with a black 20 by 10 wordmark in its middle.
    const width = 40;
    const height = 30;
    const pixels = new Uint8ClampedArray(width * height * 4).fill(255);
    for (let y = 10; y < 20; y += 1) {
      for (let x = 10; x < 30; x += 1) {
        pixels.set([0, 0, 0, 255], (y * width + x) * 4);
      }
    }
    const drawImage = vi.fn();
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
      drawImage,
      getImageData: () => ({ data: pixels, width, height }),
    } as unknown as CanvasRenderingContext2D);
    document.documentElement.setAttribute("data-television", "");
    const { container } = render(
      <ChannelIcon icon={`${window.location.origin}/logos/own.png`} number={4} />,
    );
    const image = container.querySelector("img") as HTMLImageElement;
    Object.defineProperty(image, "naturalWidth", { value: width });
    Object.defineProperty(image, "naturalHeight", { value: height });

    fireEvent.load(image);

    expect(drawImage).toHaveBeenCalledWith(image, 0, 0, width, height);
    const plate = tile(container);
    expect(plate).toHaveAttribute("data-fit", "ink");
    expect(plate).toHaveAttribute("data-ground", "true");
    // The plate takes the tile's white, and the wordmark, half the image
    // wide, is sized to 76 % of the plate's width and centred on it.
    expect(plate.style.getPropertyValue("--fit-ground")).toBe("rgb(255 255 255)");
    expect(plate.style.getPropertyValue("--fit-width")).toBe("152.000%");
    expect(plate.style.getPropertyValue("--fit-height")).toBe("152.000%");
    expect(plate.style.getPropertyValue("--fit-left")).toBe("-26.000%");
    expect(plate.style.getPropertyValue("--fit-top")).toBe("-26.000%");
    expect(plate.style.getPropertyValue("--fit-clip")).toBe(
      "inset(30.33% 22.00% 30.33% 22.00%)",
    );
  });
});

describe("iconShape", () => {
  it("tells square tiles from wide and tall wordmarks", () => {
    expect(iconShape(304, 304)).toBe("square");
    expect(iconShape(512, 513)).toBe("square");
    expect(iconShape(512, 396)).toBe("wide");
    expect(iconShape(512, 757)).toBe("tall");
    expect(iconShape(0, 0)).toBeNull();
  });

  it("shows a nearly square full-bleed logo whole rather than as a tile", () => {
    // Covering the plate would cut a badge drawn to its canvas's edge.
    expect(iconShape(149, 139)).toBe("wide");
    expect(iconShape(111, 95)).toBe("wide");
    expect(iconShape(332, 400)).toBe("tall");
    expect(iconShape(168, 190)).toBe("tall");
  });
});
