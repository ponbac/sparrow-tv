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

  it("reads the pixels of an icon the page holds itself", () => {
    document.documentElement.setAttribute("data-television", "");
    const { container } = render(
      <ChannelIcon icon={`${window.location.origin}/logos/own.png`} number={4} />,
    );
    const image = container.querySelector("img") as HTMLImageElement;
    Object.defineProperty(image, "naturalWidth", { value: 40 });
    Object.defineProperty(image, "naturalHeight", { value: 30 });
    const createElement = vi.spyOn(document, "createElement");

    fireEvent.load(image);

    expect(createElement).toHaveBeenCalledWith("canvas");
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
