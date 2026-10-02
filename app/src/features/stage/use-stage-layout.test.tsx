import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { stubViewport } from "../../test/theater-viewport";
import { useStageLayout } from "./use-stage-layout";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "fullscreenElement");
});

describe("useStageLayout", () => {
  it("is stacked where the window cannot be measured", () => {
    expect(renderHook(() => useStageLayout(true)).result.current).toBe(
      "stacked",
    );
  });

  it("is Theater only in a large window whose picture may be covered", () => {
    stubViewport(true);

    expect(renderHook(() => useStageLayout(true)).result.current).toBe(
      "theater",
    );
    expect(renderHook(() => useStageLayout(false)).result.current).toBe(
      "stacked",
    );
  });

  it("follows the window across the size threshold", () => {
    const viewport = stubViewport(false);
    const layout = renderHook(() => useStageLayout(true));
    expect(layout.result.current).toBe("stacked");

    act(() => viewport.resize(true));
    expect(layout.result.current).toBe("theater");

    act(() => viewport.resize(false));
    expect(layout.result.current).toBe("stacked");
  });

  it("is Theater in a small window while the document root is fullscreen", () => {
    stubViewport(false);
    const layout = renderHook(() => useStageLayout(true));

    act(() => enterFullscreen(document.body));
    // Only the root counts: a fullscreen player section covers the shell.
    expect(layout.result.current).toBe("stacked");

    act(() => enterFullscreen(document.documentElement));
    expect(layout.result.current).toBe("theater");

    act(() => enterFullscreen(null));
    expect(layout.result.current).toBe("stacked");
  });
});

function enterFullscreen(element: Element | null): void {
  Object.defineProperty(document, "fullscreenElement", {
    configurable: true,
    value: element,
  });
  document.dispatchEvent(new Event("fullscreenchange"));
}
