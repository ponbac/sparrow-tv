import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useIdleChrome, type ChromeVisibility } from "./use-idle-chrome";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("useIdleChrome", () => {
  it("hides the chrome three seconds after it is armed", () => {
    renderChrome(true);
    expect(chrome()).toBe("shown");

    wait(2_999);
    expect(chrome()).toBe("shown");

    wait(1);
    expect(chrome()).toBe("hidden");
  });

  it.each([
    ["a pointer move", () => fireEvent.pointerMove(document.body, { screenX: 40, screenY: 40 })],
    ["a pointer press", () => fireEvent.pointerDown(document.body)],
    ["a key press", () => fireEvent.keyDown(document.body, { key: "Shift" })],
    ["focus moving", () => fireEvent.focusIn(screen.getByTestId("monitor"))],
    ["a changed control", () => fireEvent.change(screen.getByLabelText("Volume"), { target: { value: "40" } })],
  ])("shows the chrome again on %s and waits three more seconds", (_, input) => {
    renderChrome(true);
    wait(3_000);
    expect(chrome()).toBe("hidden");

    act(() => {
      input();
    });
    expect(chrome()).toBe("shown");

    wait(2_999);
    expect(chrome()).toBe("shown");
    wait(1);
    expect(chrome()).toBe("hidden");
  });

  it("counts the three seconds from the last input", () => {
    renderChrome(true);

    wait(2_000);
    fireEvent.keyDown(document.body, { key: "Shift" });
    wait(2_000);
    expect(chrome()).toBe("shown");

    wait(1_000);
    expect(chrome()).toBe("hidden");
  });

  it("ignores a pointer move that goes nowhere", () => {
    renderChrome(true);
    fireEvent.pointerMove(document.body, { screenX: 40, screenY: 40 });
    wait(3_000);
    expect(chrome()).toBe("hidden");

    // What lies under a resting pointer changed; the pointer did not move.
    fireEvent.pointerMove(document.body, { screenX: 40, screenY: 40 });
    expect(chrome()).toBe("hidden");

    fireEvent.pointerMove(document.body, { screenX: 41, screenY: 40 });
    expect(chrome()).toBe("shown");
  });

  it("never hides the chrome while not armed, and shows it at once when disarmed", () => {
    const view = renderChrome(false);
    wait(10_000);
    expect(chrome()).toBe("shown");

    view.arm(true);
    wait(3_000);
    expect(chrome()).toBe("hidden");

    // Not for a single render does the chrome stay hidden once disarmed, or
    // start out hidden when armed again.
    view.rendered.length = 0;
    view.arm(false);
    view.arm(true);
    expect(view.rendered).not.toContain("hidden");

    wait(2_999);
    expect(chrome()).toBe("shown");
    wait(1);
    expect(chrome()).toBe("hidden");
  });

  it("holds no timer once disarmed", () => {
    const view = renderChrome(true);
    wait(1_000);

    view.arm(false);

    expect(vi.getTimerCount()).toBe(0);
    fireEvent.keyDown(document.body, { key: "Shift" });
    expect(vi.getTimerCount()).toBe(0);
  });

  it("keeps the chrome while the pointer rests on it", () => {
    renderChrome(true);
    const mute = screen.getByRole("button", { name: "Mute" });

    fireEvent.pointerOver(mute);
    wait(10_000);
    expect(chrome()).toBe("shown");

    fireEvent.pointerOver(screen.getByTestId("monitor"));
    wait(3_000);
    expect(chrome()).toBe("hidden");

    // A pointer that leaves the window from the chrome no longer rests on it.
    fireEvent.pointerMove(mute, { screenX: 7, screenY: 7 });
    fireEvent.pointerOver(mute);
    fireEvent.pointerOut(mute, { relatedTarget: null });
    wait(3_000);
    expect(chrome()).toBe("hidden");
  });

  it("keeps the chrome while the search field has focus", () => {
    renderChrome(true);
    const search = screen.getByLabelText("Search");

    act(() => search.focus());
    wait(10_000);
    expect(chrome()).toBe("shown");

    act(() => search.blur());
    wait(3_000);
    expect(chrome()).toBe("hidden");
  });

  it.each(["menu", "listbox", "dialog"])(
    "keeps the chrome while a %s is open",
    (role) => {
      renderChrome(true);
      const popup = document.createElement("div");
      popup.setAttribute("role", role);
      document.body.append(popup);

      try {
        wait(10_000);
        expect(chrome()).toBe("shown");
      } finally {
        popup.remove();
      }

      wait(3_000);
      expect(chrome()).toBe("hidden");
    },
  );

  it("keeps the chrome around a picker and returns focus to the picture once a choice is made", () => {
    renderChrome(true);
    const picker = screen.getByLabelText("Audio track");

    // jsdom cannot say whether the picker is open, so it counts as open.
    act(() => picker.focus());
    wait(10_000);
    expect(chrome()).toBe("shown");

    fireEvent.change(picker, { target: { value: "two" } });
    expect(screen.getByTestId("monitor")).toHaveFocus();
    wait(3_000);
    expect(chrome()).toBe("hidden");
  });

  it("returns focus to the picture when the volume is released", () => {
    renderChrome(true);
    const volume = screen.getByLabelText("Volume");
    act(() => volume.focus());

    fireEvent.pointerUp(volume);

    expect(screen.getByTestId("monitor")).toHaveFocus();
  });

  it("returns focus to the picture when the chrome hides around it, and stays hidden", () => {
    renderChrome(true);
    act(() => screen.getByRole("button", { name: "Mute" }).focus());

    wait(3_000);

    expect(chrome()).toBe("hidden");
    expect(screen.getByTestId("monitor")).toHaveFocus();
    wait(10_000);
    expect(chrome()).toBe("hidden");
  });

  it("leaves focus alone when it is not inside the hiding chrome", () => {
    renderChrome(true);
    const outside = screen.getByRole("button", { name: "Outside the chrome" });
    act(() => outside.focus());

    wait(3_000);

    expect(chrome()).toBe("hidden");
    expect(outside).toHaveFocus();
  });
});

function wait(milliseconds: number): void {
  act(() => {
    vi.advanceTimersByTime(milliseconds);
  });
}

function chrome(): string | null {
  return screen.getByTestId("shell").getAttribute("data-chrome");
}

/** Mounts the hook inside the parts of the shell it looks for. */
function renderChrome(armed: boolean): {
  /** What every render so far showed. */
  readonly rendered: ChromeVisibility[];
  arm(next: boolean): void;
} {
  const rendered: ChromeVisibility[] = [];

  function Host({ armed }: { readonly armed: boolean }) {
    const visibility = useIdleChrome(armed);
    rendered.push(visibility);
    return (
      <div data-testid="shell" data-chrome={visibility}>
        <header className="shell__masthead">
          <div className="shell__search">
            <input aria-label="Search" />
          </div>
        </header>
        <div className="stage__monitor" data-testid="monitor" tabIndex={-1} />
        <div className="now-playing">
          <button type="button">Mute</button>
          <input type="range" aria-label="Volume" defaultValue="80" />
          <select aria-label="Audio track" defaultValue="one">
            <option value="one">One</option>
            <option value="two">Two</option>
          </select>
        </div>
        <button type="button">Outside the chrome</button>
      </div>
    );
  }

  const view = render(<Host armed={armed} />);
  return {
    rendered,
    arm(next) {
      view.rerender(<Host armed={next} />);
    },
  };
}
