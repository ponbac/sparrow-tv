import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { usePicturePin } from "./use-picture-pin";

afterEach(() => {
  cleanup();
  Reflect.deleteProperty(document, "fullscreenElement");
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

  it("pins a paused TV picture in Guide coordinates after fullscreen exits", () => {
    const monitor = monitorOfSize(960, 540);
    Object.defineProperty(document, "fullscreenElement", {
      configurable: true,
      value: document.createElement("section"),
    });
    renderHook(() => usePicturePin(false, false, true));
    expect(pinnedWidth()).toBe("");

    monitor.size(1920, 600);
    Reflect.deleteProperty(document, "fullscreenElement");
    document.dispatchEvent(new Event("fullscreenchange"));
    expect(pinnedWidth()).toBe("1920px");
  });

  it("preserves a paused phone pin across mode changes and avoids overwriting ordinary TV pins", () => {
    const monitor = monitorOfSize(360, 203);
    const phone = renderHook(
      ({ mode }: { readonly mode: "watch" | "guide" }) =>
        usePicturePin(false, false, false, mode),
      { initialProps: { mode: "watch" } },
    );
    expect(pinnedWidth()).toBe("360px");

    // Phone keyboard resize + mode switch must keep the originally pinned box.
    monitor.size(360, 140);
    phone.rerender({ mode: "guide" });
    expect(pinnedWidth()).toBe("360px");
    expect(document.documentElement.style.getPropertyValue("--pocket-pinned-h")).toBe("203px");
    phone.unmount();

    // Non-exit TV mode switch while already held keeps the held box.
    monitor.size(1920, 700);
    const tv = renderHook(
      ({ mode }: { readonly mode: "watch" | "guide" }) =>
        usePicturePin(false, false, true, mode),
      { initialProps: { mode: "watch" } },
    );
    expect(pinnedWidth()).toBe("1920px");
    monitor.size(640, 160);
    tv.rerender({ mode: "guide" });
    expect(pinnedWidth()).toBe("1920px");
  });

  it("remeasures once against committed TV Guide layout after fullscreen exit", () => {
    const monitor = monitorOfSize(1920, 700);
    Object.defineProperty(document, "fullscreenElement", {
      configurable: true,
      value: document.createElement("section"),
    });
    const view = renderHook(
      ({ mode }: { readonly mode: "watch" | "guide" }) =>
        usePicturePin(false, false, true, mode),
      { initialProps: { mode: "watch" } },
    );

    Reflect.deleteProperty(document, "fullscreenElement");
    document.dispatchEvent(new Event("fullscreenchange"));
    monitor.size(640, 160);
    view.rerender({ mode: "guide" });
    expect(pinnedWidth()).toBe("640px");
    expect(
      document.documentElement.style.getPropertyValue("--pocket-pinned-h"),
    ).toBe("160px");
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
