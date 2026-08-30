import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { lazy, Suspense } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PlaybackLoadBoundary } from "./playback-load-boundary";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("PlaybackLoadBoundary", () => {
  it("contains a player chunk failure without taking down surrounding browsing", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const user = userEvent.setup();
    const stop = vi.fn();
    const reload = vi.fn();
    const RejectedPlayer = lazy(async () => {
      throw new Error("private chunk diagnostic");
    });

    render(
      <main>
        <p>Catalog remains available</p>
        <PlaybackLoadBoundary
          resetKey="channel-one"
          onStop={stop}
          onReload={reload}
        >
          <Suspense fallback={<p>Loading player</p>}>
            <RejectedPlayer />
          </Suspense>
        </PlaybackLoadBoundary>
      </main>,
    );

    expect(screen.getByText("Catalog remains available")).toBeVisible();
    expect(
      await screen.findByRole("heading", {
        name: "The live player could not be loaded",
      }),
    ).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Close player" }));
    expect(stop).toHaveBeenCalledOnce();
    await user.click(screen.getByRole("button", { name: "Reload Sparrow" }));
    expect(reload).toHaveBeenCalledOnce();
    expect(document.body.textContent).not.toContain("private chunk diagnostic");
  });

  it("tries a newly selected Channel after an earlier player failure", () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const view = render(
      <PlaybackLoadBoundary
        resetKey="channel-one"
        onStop={vi.fn()}
        onReload={vi.fn()}
      >
        <BrokenPlayer />
      </PlaybackLoadBoundary>,
    );
    expect(
      screen.getByText("The live player could not be loaded"),
    ).toBeVisible();

    view.rerender(
      <PlaybackLoadBoundary
        resetKey="channel-two"
        onStop={vi.fn()}
        onReload={vi.fn()}
      >
        <p>Player module restored</p>
      </PlaybackLoadBoundary>,
    );

    expect(screen.getByText("Player module restored")).toBeVisible();
  });
});

function BrokenPlayer(): never {
  throw new Error("private chunk diagnostic");
}
