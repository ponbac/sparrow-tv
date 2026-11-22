import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ChannelIcon } from "./channel-icon";

afterEach(cleanup);

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
