import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { flushSync } from "react-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useStageKeys, type StageKeys } from "./use-stage-keys";

afterEach(cleanup);

describe("useStageKeys", () => {
  it("opens and closes the guide on G, unless the guide is forced or the key is held", () => {
    const keys = renderKeys();

    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "G" });
    expect(keys.onToggleGuide).toHaveBeenCalledTimes(2);

    fireEvent.keyDown(document.body, { key: "g", repeat: true });
    expect(keys.onToggleGuide).toHaveBeenCalledTimes(2);

    keys.set({ mode: "guide", guideForced: true });
    fireEvent.keyDown(document.body, { key: "g" });
    expect(keys.onToggleGuide).toHaveBeenCalledTimes(2);
  });

  it("leaves a key alone that is typed into a field, pressed in a dialog, or held with a modifier", () => {
    const keys = renderKeys();

    fireEvent.keyDown(screen.getByLabelText("Notes"), { key: "g" });
    fireEvent.keyDown(screen.getByLabelText("Audio track"), {
      key: "ArrowDown",
    });
    fireEvent.keyDown(screen.getByRole("button", { name: "In a dialog" }), {
      key: "g",
    });
    fireEvent.keyDown(screen.getByRole("menuitem"), { key: "ArrowDown" });
    fireEvent.keyDown(document.body, { key: "g", ctrlKey: true });
    fireEvent.keyDown(document.body, { key: "g", metaKey: true });
    fireEvent.keyDown(document.body, { key: "ArrowDown", altKey: true });

    expect(keys.onToggleGuide).not.toHaveBeenCalled();
    expect(keys.onZap).not.toHaveBeenCalled();
    // The same keys do act from a plain button.
    fireEvent.keyDown(screen.getByRole("button", { name: "Mute" }), {
      key: "g",
    });
    expect(keys.onToggleGuide).toHaveBeenCalledTimes(1);
  });

  it("leaves a key alone that a control has already handled", () => {
    const keys = renderKeys();
    const handled = (event: KeyboardEvent) => event.preventDefault();
    document.body.addEventListener("keydown", handled);

    try {
      fireEvent.keyDown(document.body, { key: "g" });
    } finally {
      document.body.removeEventListener("keydown", handled);
    }

    expect(keys.onToggleGuide).not.toHaveBeenCalled();
  });

  it("goes to the search field on /, without typing the slash into it", () => {
    renderKeys();

    const delivered = fireEvent.keyDown(document.body, { key: "/" });

    expect(screen.getByLabelText("Search")).toHaveFocus();
    expect(delivered).toBe(false);
  });

  it("presses the player's Full screen button on F, once per key press", () => {
    const keys = renderKeys();

    fireEvent.keyDown(document.body, { key: "f" });
    fireEvent.keyDown(document.body, { key: "f", repeat: true });

    expect(keys.onFullscreen).toHaveBeenCalledTimes(1);
  });

  it("makes the window itself fullscreen on F while there is no player to ask", () => {
    renderKeys({ player: false });
    const request = vi.fn(() => Promise.resolve());
    // jsdom has no fullscreen of its own.
    Object.defineProperty(document.documentElement, "requestFullscreen", {
      configurable: true,
      value: request,
    });

    try {
      fireEvent.keyDown(document.body, { key: "f" });
    } finally {
      Reflect.deleteProperty(document.documentElement, "requestFullscreen");
    }

    expect(request).toHaveBeenCalledTimes(1);
  });

  it("changes Channel on the arrows in watch mode, not in the guide and not while they move the volume", () => {
    const keys = renderKeys();

    const delivered = fireEvent.keyDown(document.body, { key: "ArrowDown" });
    fireEvent.keyDown(document.body, { key: "ArrowUp" });
    // Holding an arrow keeps moving.
    fireEvent.keyDown(document.body, { key: "ArrowDown", repeat: true });
    expect(keys.onZap.mock.calls).toEqual([[1], [-1], [1]]);
    expect(delivered).toBe(false);

    const volume = screen.getByLabelText("Volume");
    fireEvent.keyDown(volume, { key: "ArrowUp" });
    expect(keys.onZap).toHaveBeenCalledTimes(3);
    // Only the arrows belong to the slider.
    fireEvent.keyDown(volume, { key: "g" });
    expect(keys.onToggleGuide).toHaveBeenCalledTimes(1);

    keys.set({ mode: "guide" });
    expect(fireEvent.keyDown(document.body, { key: "ArrowDown" })).toBe(true);
    expect(keys.onZap).toHaveBeenCalledTimes(3);
  });

  it("returns to the picture on Esc from a guide the viewer opened", () => {
    const keys = renderKeys({ mode: "guide" });

    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(keys.onWatch).toHaveBeenCalledTimes(1);

    keys.set({ mode: "guide", guideForced: true });
    fireEvent.keyDown(document.body, { key: "Escape" });
    keys.set({ mode: "watch", guideForced: false });
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(keys.onWatch).toHaveBeenCalledTimes(1);
  });

  it("hands focus from the search field to the picture on Esc, once its results are closed", () => {
    const keys = renderKeys({ mode: "guide" });
    const search = screen.getByLabelText("Search");
    search.focus();

    search.setAttribute("aria-expanded", "true");
    fireEvent.keyDown(search, { key: "Escape" });
    expect(search).toHaveFocus();

    search.setAttribute("aria-expanded", "false");
    fireEvent.keyDown(search, { key: "Escape" });
    expect(screen.getByTestId("monitor")).toHaveFocus();
    // Leaving the field is all that key press does.
    expect(keys.onWatch).not.toHaveBeenCalled();
  });

  it("handles no key outside the Theater layout", () => {
    const keys = renderKeys({ active: false });

    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "f" });
    fireEvent.keyDown(document.body, { key: "ArrowDown" });
    expect(fireEvent.keyDown(document.body, { key: "/" })).toBe(true);

    expect(keys.onToggleGuide).not.toHaveBeenCalled();
    expect(keys.onFullscreen).not.toHaveBeenCalled();
    expect(keys.onZap).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Search")).not.toHaveFocus();
  });

  it("still hears a key when the shell renders again while the key is being delivered", () => {
    // A browser runs React's pending render between two listeners of one key
    // press. This listener comes first and renders in the middle of it.
    let renderAgain = (): void => undefined;
    const earlier = () => flushSync(renderAgain);
    document.addEventListener("keydown", earlier);
    const zapped: number[] = [];

    function Shell() {
      const [renders, setRenders] = useState(1);
      renderAgain = () => setRenders((count) => count + 1);
      useStageKeys({
        active: true,
        mode: "watch",
        guideForced: false,
        onToggleGuide: () => undefined,
        onWatch: () => undefined,
        // A new function on every render, as the shell's own is.
        onZap: () => zapped.push(renders),
      });
      return null;
    }

    try {
      render(<Shell />);
      fireEvent.keyDown(document.body, { key: "ArrowDown" });
    } finally {
      document.removeEventListener("keydown", earlier);
    }

    // Heard once, by the handler of the render that was current by then.
    expect(zapped).toEqual([2]);
  });
});

type KeysState = Pick<StageKeys, "active" | "mode" | "guideForced"> & {
  /** Whether a player, with its Full screen button, is on the stage. */
  readonly player: boolean;
};

/** Mounts the keys beside the parts of the shell they look for. */
function renderKeys(initial: Partial<KeysState> = {}) {
  const onToggleGuide = vi.fn<StageKeys["onToggleGuide"]>();
  const onWatch = vi.fn<StageKeys["onWatch"]>();
  const onZap = vi.fn<StageKeys["onZap"]>();
  const onFullscreen = vi.fn<() => void>();
  let state: KeysState = {
    active: true,
    mode: "watch",
    guideForced: false,
    player: true,
    ...initial,
  };

  function Host({ player, ...keys }: KeysState) {
    useStageKeys({ ...keys, onToggleGuide, onWatch, onZap });
    return (
      <>
        <div className="shell__search">
          <input aria-label="Search" aria-expanded="false" />
        </div>
        <div className="stage__monitor" data-testid="monitor" tabIndex={-1} />
        {player ? (
          <button
            type="button"
            data-stage-action="fullscreen"
            onClick={onFullscreen}
          >
            Full screen
          </button>
        ) : null}
        <button type="button">Mute</button>
        <input type="range" aria-label="Volume" />
        <select aria-label="Audio track" />
        <input aria-label="Notes" />
        <div role="dialog">
          <button type="button">In a dialog</button>
        </div>
        <div role="menu">
          <div role="menuitem" tabIndex={-1}>
            Restart
          </div>
        </div>
      </>
    );
  }

  const view = render(<Host {...state} />);
  return {
    onToggleGuide,
    onWatch,
    onZap,
    onFullscreen,
    set(next: Partial<KeysState>) {
      state = { ...state, ...next };
      view.rerender(<Host {...state} />);
    },
  };
}
