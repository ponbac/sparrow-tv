import { Dialog } from "@base-ui/react/dialog";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { createAndroidPlayerBridge } from "../playback/android-player-bridge";
import { androidPortFixture } from "../../test/android-port-fixture";
import { useTelevisionControls } from "./use-television-controls";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  Reflect.deleteProperty(document, "fullscreenElement");
  Reflect.deleteProperty(document, "exitFullscreen");
});

it("gives dialogs first refusal of Back, tolerates rejected fullscreen exit, then shows Guide, then lets Android leave", async () => {
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

  const rejectedExit = vi.fn(async () => {
    throw new Error("rejected");
  });
  Object.defineProperty(document, "fullscreenElement", {
    configurable: true,
    value: document.createElement("section"),
  });
  Object.defineProperty(document, "exitFullscreen", { configurable: true, value: rejectedExit });
  await act(async () => port.deliver("tv:back"));
  expect(rejectedExit).toHaveBeenCalledTimes(1);
  expect(onShowGuide).not.toHaveBeenCalled();
  expect(port.sent).toEqual(["ready"]);

  const exitFullscreen = vi.fn(async () => {
    Reflect.deleteProperty(document, "fullscreenElement");
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

it("contains focus inside open menus, clamps horizontal menu arrows, and guards CH+/CH- behind overlays", () => {
  const port = androidPortFixture();
  const bridge = createAndroidPlayerBridge(port.value);
  const onZap = vi.fn();

  function OverlayHarness({
    menuOpen,
    searchExpanded,
  }: {
    readonly menuOpen: boolean;
    readonly searchExpanded: boolean;
  }) {
    useTelevisionControls(
      { enabled: true, mode: "watch", onShowGuide: () => undefined, onZap },
      bridge,
    );
    return (
      <div>
        <button type="button">Outside</button>
        <div className="board-search">
          <input aria-label="Search" aria-expanded={searchExpanded} />
        </div>
        {menuOpen ? (
          <div role="menu" data-open="">
            <button type="button" role="menuitem">
              Restart
            </button>
          </div>
        ) : null}
      </div>
    );
  }

  const view = render(<OverlayHarness menuOpen searchExpanded={false} />);
  const outside = screen.getByRole("button", { name: "Outside" });
  const item = screen.getByRole("menuitem", { name: "Restart" });
  giveLayout(outside, item);

  act(() => item.focus());
  const leftEvent = new KeyboardEvent("keydown", {
    key: "ArrowLeft",
    bubbles: true,
    cancelable: true,
  });
  item.dispatchEvent(leftEvent);
  expect(leftEvent.defaultPrevented).toBe(true);

  act(() => outside.focus());
  expect(item).toHaveFocus();

  act(() => port.deliver("tv:channel-up"));
  expect(onZap).not.toHaveBeenCalled();

  view.rerender(<OverlayHarness menuOpen={false} searchExpanded />);
  act(() => port.deliver("tv:channel-down"));
  expect(onZap).not.toHaveBeenCalled();

  view.rerender(<OverlayHarness menuOpen={false} searchExpanded={false} />);
  act(() => port.deliver("tv:channel-up"));
  expect(onZap).toHaveBeenCalledWith(1);
});

it("restores focus to last-played row, Stop on non-retryable fullscreen failure, and respects busy-board before Sources", async () => {
  vi.useFakeTimers();

  function FallbackHarness({
    mode,
    busy,
    hasRows,
    fullscreenFailed,
  }: {
    readonly mode: "guide" | "watch";
    readonly busy: boolean;
    readonly hasRows: boolean;
    readonly fullscreenFailed: boolean;
  }) {
    useTelevisionControls(
      { enabled: true, mode, onShowGuide: () => undefined, onZap: () => undefined },
      null,
    );
    return (
      <div>
        {fullscreenFailed ? (
          <section className="hosted-player" tabIndex={-1}>
            <div className="hosted-player__state" data-state="failed" />
            <button type="button" className="hosted-player__stop">
              Close player
            </button>
          </section>
        ) : null}
        <section className="programme-guide">
          <button type="button" className="feeds-trigger">
            Sources
          </button>
          <div className="programme-guide__board" aria-busy={busy}>
            {hasRows ? (
              <>
                <div className="programme-guide__row">
                  <button type="button" className="programme-guide__channel">
                    Channel 1
                  </button>
                </div>
                <div className="programme-guide__row" data-last-played="true">
                  <button type="button" className="programme-guide__channel">
                    Channel 2
                  </button>
                </div>
              </>
            ) : null}
          </div>
        </section>
      </div>
    );
  }

  const view = render(
    <FallbackHarness mode="guide" busy hasRows={false} fullscreenFailed={false} />,
  );
  const sources = screen.getByRole("button", { name: "Sources" });
  giveLayout(sources);

  // Cold start busy board must NOT dump focus onto Sources prematurely.
  await act(async () => {
    await Promise.resolve();
    vi.advanceTimersByTime(50);
  });
  expect(sources).not.toHaveFocus();

  // Once rows arrive, last-played row receives focus over row 0.
  view.rerender(
    <FallbackHarness mode="guide" busy={false} hasRows fullscreenFailed={false} />,
  );
  const ch1 = screen.getByRole("button", { name: "Channel 1" });
  const ch2 = screen.getByRole("button", { name: "Channel 2" });
  giveLayout(ch1, ch2);
  await act(async () => {
    await Promise.resolve();
    vi.advanceTimersByTime(50);
  });
  expect(ch2).toHaveFocus();

  // Fullscreen non-retryable failure focuses Stop/Close player.
  view.rerender(
    <FallbackHarness mode="watch" busy={false} hasRows={false} fullscreenFailed />,
  );
  const player = document.querySelector<HTMLElement>(".hosted-player")!;
  const closePlayer = screen.getByRole("button", { name: "Close player" });
  giveLayout(player, closePlayer);
  Object.defineProperty(document, "fullscreenElement", {
    configurable: true,
    value: player,
  });
  await act(async () => {
    document.dispatchEvent(new Event("fullscreenchange"));
    await Promise.resolve();
    vi.advanceTimersByTime(50);
  });
  expect(closePlayer).toHaveFocus();
});

// jsdom has no boxes. A box marks an action available to spatial focus.
function giveLayout(...controls: HTMLElement[]): void {
  for (const control of controls) {
    Object.defineProperty(control, "getClientRects", {
      configurable: true,
      value: () => [new DOMRect(0, 0, 100, 40)],
    });
  }
}
