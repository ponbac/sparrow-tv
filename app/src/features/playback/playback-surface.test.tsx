import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef, useMemo, useState, type ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { channelFixture } from "../../test/channel-fixture";
import {
  StageChromeProvider,
  type StageChrome,
} from "../stage/stage-chrome";
import { PlaybackSurface, type PlaybackSurfaceProps } from "./playback-surface";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

it.each(["resolve", "AbortError", "NotAllowedError", "NotSupportedError"] as const)(
  "preserves default installed gesture playback on %s", async (outcome) => {
    const props = surfaceProps();
    let failures = 0;
    render(<PlaybackSurface {...props} state={{ _tag: "autoplay-blocked" }}
      onAutoplayFailure={() => { failures += 1; }} />);
    const video = props.videoRef.current;
    if (video === null) throw new Error("expected the playback video");
    let plays = 0;
    video.play = () => {
      plays += 1;
      return outcome === "resolve"
        ? Promise.resolve()
        : Promise.reject(new DOMException("fixture playback", outcome));
    };
    fireEvent.click(screen.getByRole("button", { name: "Start audio & video" }));
    // The play call stays inside user activation, not a deferred effect.
    expect(plays).toBe(1);
    await act(async () => { await Promise.resolve(); });
    expect(failures).toBe(outcome === "resolve" ? 0 : 1);
  },
);

it("hides fullscreen chrome only during playback and restores it for touch, keyboard, and recovery", () => {
  vi.useFakeTimers();
  const props: PlaybackSurfaceProps = {
    ...surfaceProps(),
    nativeVideo: true,
    audio: <select aria-label="Audio track"><option>English</option><option>Swedish</option></select>,
    muted: true,
    fullscreen: true,
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

it("keeps the button text that acceptance scripts match on in both control variants", () => {
  const props = { ...surfaceProps(), ...everyControl().slots };
  const view = render(<PlaybackSurface {...props} />);
  const bar = screen.getByRole("group", { name: "Playback controls" });
  expect(screen.getByRole("region", { name: "Demo" })).toContainElement(bar);
  expect(bar).toHaveAttribute("data-variant", "bar");
  expect(buttonText(bar)).toEqual([
    "Pause",
    "Mute",
    "Full screen",
    "Open in mpv",
    "Restart",
    "Copy diagnostics",
    "Stop stream",
  ]);
  view.unmount();

  render(
    <CompactChrome>
      <PlaybackSurface {...props} />
    </CompactChrome>,
  );
  const compact = screen.getByRole("group", { name: "Playback controls" });
  expect(compact).toHaveAttribute("data-variant", "compact");
  // The empty entry is the icon-only More trigger that replaces the two
  // secondary buttons.
  expect(buttonText(compact)).toEqual([
    "Pause",
    "Mute",
    "Full screen",
    "Open in mpv",
    "",
    "Stop stream",
  ]);
  expect(
    within(compact).getByRole("combobox", { name: "Audio track" }),
  ).toBeInTheDocument();
  expect(
    within(compact).getByRole("slider", { name: "Volume" }),
  ).toBeInTheDocument();
});

it("hands compact controls to the shell's slot and keeps them wired to the player", async () => {
  const user = userEvent.setup();
  const fullscreenTarget = document.createElement("div");
  const controls = everyControl();
  const props = { ...surfaceProps(), ...controls.slots };
  const view = render(
    <CompactChrome fullscreenTarget={fullscreenTarget}>
      <PlaybackSurface {...props} />
    </CompactChrome>,
  );
  const group = screen.getByRole("group", { name: "Playback controls" });
  expect(screen.getByTestId("controls-slot")).toContainElement(group);
  expect(screen.getByRole("region", { name: "Demo" })).not.toContainElement(
    group,
  );

  await user.click(screen.getByRole("button", { name: "Full screen" }));
  expect(props.onRequestFullscreen).toHaveBeenLastCalledWith(fullscreenTarget);
  // Keys pressed on a portalled control still reach the player section.
  fireEvent.keyDown(screen.getByRole("button", { name: "Mute" }), { key: "f" });
  expect(props.onRequestFullscreen).toHaveBeenCalledTimes(2);

  expect(screen.queryByRole("menuitem")).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "More" }));
  expect(
    (await screen.findAllByRole("menuitem")).map((item) => item.textContent),
  ).toEqual(["Restart", "Copy diagnostics"]);
  await user.click(screen.getByRole("menuitem", { name: "Restart" }));
  expect(controls.restart).toHaveBeenCalledTimes(1);
  expect(controls.copyDiagnostics).not.toHaveBeenCalled();

  // Hosted playback has no secondary actions, so it has no menu either.
  view.rerender(
    <CompactChrome fullscreenTarget={fullscreenTarget}>
      <PlaybackSurface {...props} secondaryActions={[]} />
    </CompactChrome>,
  );
  expect(screen.queryByRole("button", { name: "More" })).not.toBeInTheDocument();
});

it("tells the shell whether a picture is playing and where, until it unmounts", () => {
  const reportPicture = vi.fn<StageChrome["reportPicture"]>();
  const props = surfaceProps();
  const view = render(
    <CompactChrome reportPicture={reportPicture}>
      <PlaybackSurface {...props} state={{ _tag: "starting" }} />
    </CompactChrome>,
  );
  expect(reportPicture).toHaveBeenLastCalledWith({
    playing: false,
    external: false,
  });

  view.rerender(
    <CompactChrome reportPicture={reportPicture}>
      <PlaybackSurface {...props} external />
    </CompactChrome>,
  );
  expect(reportPicture).toHaveBeenLastCalledWith({
    playing: true,
    external: true,
  });
  expect(reportPicture).not.toHaveBeenCalledWith(null);

  view.unmount();
  expect(reportPicture).toHaveBeenLastCalledWith(null);
});

/** A stand-in for the Theater shell: compact controls portalled into a slot it owns. */
function CompactChrome({
  children,
  fullscreenTarget = null,
  reportPicture = ignorePicture,
}: {
  readonly children: ReactNode;
  readonly fullscreenTarget?: HTMLElement | null;
  readonly reportPicture?: StageChrome["reportPicture"];
}) {
  const [controlsSlot, setControlsSlot] = useState<HTMLElement | null>(null);
  const chrome = useMemo<StageChrome>(
    () => ({
      controlsSlot,
      controls: "compact",
      fullscreenTarget,
      reportPicture,
    }),
    [controlsSlot, fullscreenTarget, reportPicture],
  );
  return (
    <StageChromeProvider value={chrome}>
      {children}
      <div data-testid="controls-slot" ref={setControlsSlot} />
    </StageChromeProvider>
  );
}

function ignorePicture(): void {}

function surfaceProps() {
  return {
    channel: channelFixture({ id: "demo", name: "Demo", group: "Demo" }),
    state: { _tag: "playing" },
    videoKey: "demo",
    videoRef: createRef<HTMLVideoElement>(),
    privacyCopy: "",
    volume: 1,
    muted: false,
    fullscreen: false,
    onPlaying: vi.fn(),
    onVolumeChange: vi.fn(),
    onToggleMuted: vi.fn(),
    onRequestFullscreen: vi.fn(),
    onStop: vi.fn(),
    onAutoplayFailure: vi.fn(),
  } satisfies PlaybackSurfaceProps;
}

/** Every optional control an installed Linux player can show at once. */
function everyControl() {
  const restart = vi.fn();
  const copyDiagnostics = vi.fn();
  const slots = {
    pause: { onPause: vi.fn() },
    audio: (
      <label>
        <span>Audio</span>
        <select aria-label="Audio track">
          <option>English</option>
        </select>
      </label>
    ),
    playerSwitch: { label: "Open in mpv", onSwitch: vi.fn() },
    secondaryActions: [
      { key: "restart", label: "Restart", icon: null, onSelect: restart },
      {
        key: "copy-diagnostics",
        label: "Copy diagnostics",
        icon: null,
        onSelect: copyDiagnostics,
      },
    ],
  } satisfies Partial<PlaybackSurfaceProps>;
  return { slots, restart, copyDiagnostics };
}

function buttonText(group: HTMLElement): readonly string[] {
  return within(group)
    .getAllByRole("button")
    .map((button) => button.textContent ?? "");
}
