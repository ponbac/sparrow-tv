import { Dialog } from "@base-ui/react/dialog";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { createAndroidPlayerBridge } from "../playback/android-player-bridge";
import { androidPortFixture } from "../../test/android-port-fixture";
import { useTelevisionControls } from "./use-television-controls";

afterEach(() => {
  cleanup();
  Reflect.deleteProperty(document, "fullscreenElement");
  Reflect.deleteProperty(document, "exitFullscreen");
});

it("gives dialogs first refusal of Back, then shows the Guide, then lets Android leave", async () => {
  const port = androidPortFixture();
  const bridge = createAndroidPlayerBridge(port.value);
  const onShowGuide = vi.fn();
  function Television({ mode }: { readonly mode: "guide" | "watch" }) {
    useTelevisionControls({ enabled: true, mode, onShowGuide, onZap: () => undefined }, bridge);
    return (
      <Dialog.Root defaultOpen>
        <Dialog.Portal>
          <Dialog.Popup>
            <Dialog.Title>Sources</Dialog.Title>
            <Dialog.Close>Close sources</Dialog.Close>
          </Dialog.Popup>
        </Dialog.Portal>
      </Dialog.Root>
    );
  }
  const view = render(<Television mode="watch" />);
  expect(screen.getByRole("dialog")).toBeInTheDocument();
  act(() => port.deliver("tv:back"));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(onShowGuide).not.toHaveBeenCalled();

  const exitFullscreen = vi.fn(async () => {
    Reflect.deleteProperty(document, "fullscreenElement");
  });
  Object.defineProperty(document, "fullscreenElement", {
    configurable: true,
    value: document.createElement("section"),
  });
  Object.defineProperty(document, "exitFullscreen", { configurable: true, value: exitFullscreen });
  await act(async () => port.deliver("tv:back"));
  expect(exitFullscreen).toHaveBeenCalledTimes(1);
  expect(onShowGuide).toHaveBeenCalledTimes(1);
  expect(port.sent).toEqual(["ready"]);

  act(() => port.deliver("tv:back"));
  expect(onShowGuide).toHaveBeenCalledTimes(2);
  view.rerender(<Television mode="guide" />);
  act(() => port.deliver("tv:back"));
  expect(port.sent).toEqual(["ready", "tv:default-back"]);
});

it("moves down within a Guide column, then reaches its recovery action", () => {
  function Guide() {
    useTelevisionControls({ enabled: true, mode: "guide", onShowGuide: () => undefined, onZap: () => undefined }, null);
    return (
      <section className="programme-guide">
        <div style={{ gridTemplateColumns: "200px 200px" }}>
          {["One", "Two", "Three", "Four"].map((name) => (
            <div key={name} className="programme-guide__row" data-body="list">
              <button type="button" className="programme-guide__channel">{name}</button>
            </div>
          ))}
        </div>
        <div className="programme-guide__retained"><button type="button">Try again</button></div>
        <button type="button" className="programme-guide__more">More channels</button>
      </section>
    );
  }
  render(<Guide />);
  const second = screen.getByRole("button", { name: "Two" });
  const fourth = screen.getByRole("button", { name: "Four" });
  const retry = screen.getByRole("button", { name: "Try again" });
  giveLayout(retry, screen.getByRole("button", { name: "More channels" }));

  act(() => second.focus());
  fireEvent.keyDown(second, { key: "ArrowDown" });
  expect(fourth).toHaveFocus();
  fireEvent.keyDown(fourth, { key: "ArrowDown" });
  expect(retry).toHaveFocus();
});

it("reaches the time and group chooser from the edges of the group lane", () => {
  function Guide() {
    useTelevisionControls({ enabled: true, mode: "guide", onShowGuide: () => undefined, onZap: () => undefined }, null);
    return (
      <section className="programme-guide">
        <button type="button" className="programme-guide__when">Now</button>
        <div className="programme-guide__groups" role="radiogroup">
          <button type="button" role="radio" aria-checked="true">All</button>
          <button type="button" role="radio" aria-checked="false">News</button>
        </div>
        <button type="button" className="group-roster-trigger">Choose groups</button>
      </section>
    );
  }
  render(<Guide />);
  const now = screen.getByRole("button", { name: "Now" });
  const chooser = screen.getByRole("button", { name: "Choose groups" });
  giveLayout(now, chooser);
  const first = screen.getByRole("radio", { name: "All" });
  const last = screen.getByRole("radio", { name: "News" });
  first.focus();
  fireEvent.keyDown(first, { key: "ArrowLeft" });
  expect(now).toHaveFocus();
  last.focus();
  fireEvent.keyDown(last, { key: "ArrowRight" });
  expect(chooser).toHaveFocus();
});

// jsdom has no boxes. A box marks an action available to spatial focus.
function giveLayout(...controls: HTMLElement[]): void {
  for (const control of controls) {
    Object.defineProperty(control, "getClientRects", {
      value: () => [new DOMRect(0, 0, 100, 40)],
    });
  }
}
