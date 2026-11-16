import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { usePicturePin } from "./use-picture-pin";

afterEach(() => {
  cleanup();
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

describe("usePicturePin", () => {
  it("holds the box while the picture cannot follow it", () => {
    monitorOfSize(360, 203);
    const { rerender } = renderHook(
      ({ follows }) => usePicturePin(follows, false),
      { initialProps: { follows: true } },
    );
    expect(pinnedWidth()).toBe("");
    rerender({ follows: false });
    expect(pinnedWidth()).toBe("360px");
    rerender({ follows: true });
    expect(pinnedWidth()).toBe("");
  });

  it("does not pin a box picture-in-picture shrank until the picture follows again", () => {
    const monitor = monitorOfSize(360, 203);
    const { rerender } = renderHook(
      ({ follows, pictureInPicture }) => usePicturePin(follows, pictureInPicture),
      { initialProps: { follows: false, pictureInPicture: false } },
    );
    expect(pinnedWidth()).toBe("360px");

    rerender({ follows: false, pictureInPicture: true });
    expect(pinnedWidth()).toBe("");
    // Leaving the window can reach the page before the window has grown.
    monitor.size(120, 68);
    rerender({ follows: false, pictureInPicture: false });
    expect(pinnedWidth()).toBe("");

    rerender({ follows: true, pictureInPicture: false });
    monitor.size(360, 203);
    rerender({ follows: false, pictureInPicture: false });
    expect(pinnedWidth()).toBe("360px");
  });
});

function monitorOfSize(width: number, height: number) {
  const monitor = document.createElement("div");
  monitor.className = "stage__monitor";
  document.body.append(monitor);
  let box = { width, height };
  vi.spyOn(monitor, "getBoundingClientRect").mockImplementation(
    () => ({ ...box, x: 0, y: 0, top: 0, left: 0, right: box.width, bottom: box.height }) as DOMRect,
  );
  return {
    size: (nextWidth: number, nextHeight: number) => {
      box = { width: nextWidth, height: nextHeight };
    },
  };
}

function pinnedWidth(): string {
  return document.documentElement.style.getPropertyValue("--pocket-pinned-w");
}
