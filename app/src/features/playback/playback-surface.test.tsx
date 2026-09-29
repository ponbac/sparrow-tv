import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { clientSchemas } from "../../client/contracts";
import { PlaybackSurface, type PlaybackSurfaceProps } from "./playback-surface";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

it("hides fullscreen chrome only during playback and restores it for touch, keyboard, and recovery", () => {
  vi.useFakeTimers();
  const props: PlaybackSurfaceProps = {
    channel: clientSchemas.channel.parse({ id: "demo", name: "Demo", group: "Demo" }),
    state: { _tag: "playing" },
    videoKey: "demo",
    videoRef: createRef<HTMLVideoElement>(),
    transportLabel: "Android Media3",
    privacyCopy: "",
    nativeVideo: true,
    additionalControls: <select aria-label="Audio track"><option>English</option><option>Swedish</option></select>,
    volume: 1,
    muted: true,
    fullscreen: true,
    onPlaying: vi.fn(),
    onVolumeChange: vi.fn(),
    onToggleMuted: vi.fn(),
    onRequestFullscreen: vi.fn(),
    onStop: vi.fn(),
    onAutoplayFailure: vi.fn(),
  };
  const view = render(<PlaybackSurface {...props} />);
  const surface = screen.getByRole("region", { name: "Demo" });
  act(() => vi.advanceTimersByTime(3_000));
  expect(surface).toHaveAttribute("data-controls-visible", "false");
  fireEvent.pointerDown(surface);
  expect(surface).toHaveAttribute("data-controls-visible", "true");
  const audio = screen.getByRole("combobox", { name: "Audio track" });
  act(() => audio.focus());
  act(() => vi.advanceTimersByTime(4_000));
  expect(surface).toHaveAttribute("data-controls-visible", "true");
  act(() => audio.blur());
  fireEvent.change(audio, { target: { value: "Swedish" } });
  act(() => vi.advanceTimersByTime(3_000));
  expect(surface).toHaveAttribute("data-controls-visible", "false");
  fireEvent.keyDown(surface, { key: "Tab" });
  expect(surface).toHaveAttribute("data-controls-visible", "true");

  view.rerender(<PlaybackSurface {...props} state={{ _tag: "paused" }} />);
  act(() => vi.advanceTimersByTime(10_000));
  expect(surface).toHaveAttribute("data-controls-visible", "true");
  view.rerender(<PlaybackSurface {...props} state={{ _tag: "failed", failure: "stream-interrupted", retryable: true }} />);
  act(() => vi.advanceTimersByTime(10_000));
  expect(surface).toHaveAttribute("data-controls-visible", "true");
  view.rerender(<PlaybackSurface {...props} />);
  act(() => vi.advanceTimersByTime(3_000));
  expect(surface).toHaveAttribute("data-controls-visible", "false");
  view.rerender(<PlaybackSurface {...props} fullscreen={false} />);
  expect(surface).toHaveAttribute("data-controls-visible", "true");
});
