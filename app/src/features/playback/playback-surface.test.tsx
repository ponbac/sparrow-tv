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
  Reflect.deleteProperty(document, "fullscreenElement");
});

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

it.each(["top", "bottom"] as const)(
  "opens the More menu on the %s side when the shell asks for it",
  async (menuSide) => {
    const user = userEvent.setup();
    render(
      <CompactChrome menuSide={menuSide}>
        <PlaybackSurface {...surfaceProps()} {...everyControl().slots} />
      </CompactChrome>,
    );

    await user.click(screen.getByRole("button", { name: "More" }));

    expect(await screen.findByRole("menu")).toHaveAttribute(
      "data-side",
      menuSide,
    );
  },
);

it("shows the inline bar while the player section itself is fullscreen, whatever the shell asks for", () => {
  const props = { ...surfaceProps(), ...everyControl().slots };
  const view = render(
    <CompactChrome>
      <PlaybackSurface {...props} />
    </CompactChrome>,
  );
  const player = screen.getByRole("region", { name: "Demo" });
  const video = screen.getByLabelText("Demo live video");
  const controls = () =>
    screen.getByRole("group", { name: "Playback controls" });
  expect(controls()).toHaveAttribute("data-variant", "compact");
  expect(screen.getByTestId("controls-slot")).toContainElement(controls());

  // No fullscreen target from the shell: the section is the fullscreen
  // element, and only what is inside it is drawn.
  enterFullscreen(player);
  view.rerender(
    <CompactChrome>
      <PlaybackSurface {...props} fullscreen />
    </CompactChrome>,
  );
  expect(controls()).toHaveAttribute("data-variant", "bar");
  expect(player).toContainElement(controls());
  expect(screen.getByTestId("controls-slot")).toBeEmptyDOMElement();
  expect(buttonText(controls())).toEqual([
    "Pause",
    "Mute",
    "Exit fullscreen",
    "Open in mpv",
    "Restart",
    "Copy diagnostics",
    "Stop stream",
  ]);
  expect(screen.queryByRole("button", { name: "More" })).not.toBeInTheDocument();
  expect(screen.getByLabelText("Demo live video")).toBe(video);

  enterFullscreen(null);
  view.rerender(
    <CompactChrome>
      <PlaybackSurface {...props} />
    </CompactChrome>,
  );
  expect(controls()).toHaveAttribute("data-variant", "compact");
  expect(screen.getByTestId("controls-slot")).toContainElement(controls());
  expect(screen.getByLabelText("Demo live video")).toBe(video);
});

it("keeps the shell's controls while the shell's own fullscreen target is fullscreen", () => {
  const props = { ...surfaceProps(), ...everyControl().slots };
  render(
    <CompactChrome fullscreenTarget={document.documentElement}>
      <PlaybackSurface {...props} fullscreen />
    </CompactChrome>,
  );
  enterFullscreen(document.documentElement);

  const controls = screen.getByRole("group", { name: "Playback controls" });
  expect(controls).toHaveAttribute("data-variant", "compact");
  expect(screen.getByTestId("controls-slot")).toContainElement(controls);
  expect(screen.getByRole("button", { name: "More" })).toBeInTheDocument();
});

it("keeps the bar in a fullscreen player section when the shell gains a target of its own, and leaves that section", async () => {
  const user = userEvent.setup();
  const props = { ...surfaceProps(), ...everyControl().slots };
  // An installed player before the device has said the picture may be
  // covered: no target, so Full screen takes the player section.
  const view = render(
    <CompactChrome>
      <PlaybackSurface {...props} />
    </CompactChrome>,
  );
  const player = screen.getByRole("region", { name: "Demo" });
  const controls = () =>
    screen.getByRole("group", { name: "Playback controls" });
  await user.click(screen.getByRole("button", { name: "Full screen" }));
  expect(props.onRequestFullscreen).toHaveBeenLastCalledWith(player);
  enterFullscreen(player);

  // The device answers: the shell's target is now the document root. The
  // section is still what is fullscreen, so its bar stays inside it.
  view.rerender(
    <CompactChrome fullscreenTarget={document.documentElement}>
      <PlaybackSurface {...props} fullscreen />
    </CompactChrome>,
  );
  expect(controls()).toHaveAttribute("data-variant", "bar");
  expect(player).toContainElement(controls());
  expect(screen.getByTestId("controls-slot")).toBeEmptyDOMElement();

  // Leaving fullscreen acts on the section, not on the new target.
  await user.click(screen.getByRole("button", { name: "Exit fullscreen" }));
  expect(props.onRequestFullscreen).toHaveBeenCalledTimes(2);
  expect(props.onRequestFullscreen).toHaveBeenLastCalledWith(player);

  enterFullscreen(null);
  view.rerender(
    <CompactChrome fullscreenTarget={document.documentElement}>
      <PlaybackSurface {...props} />
    </CompactChrome>,
  );
  expect(controls()).toHaveAttribute("data-variant", "compact");
  expect(screen.getByTestId("controls-slot")).toContainElement(controls());
  await user.click(screen.getByRole("button", { name: "Full screen" }));
  expect(props.onRequestFullscreen).toHaveBeenLastCalledWith(
    document.documentElement,
  );
});

it("tells the shell the state of its picture and where it is, until it unmounts", () => {
  const reportPicture = vi.fn<StageChrome["reportPicture"]>();
  const props = surfaceProps();
  const view = render(
    <CompactChrome reportPicture={reportPicture}>
      <PlaybackSurface {...props} state={{ _tag: "starting" }} />
    </CompactChrome>,
  );
  expect(reportPicture).toHaveBeenLastCalledWith({
    state: "starting",
    status: "Tuning",
    external: false,
  });

  view.rerender(
    <CompactChrome reportPicture={reportPicture}>
      <PlaybackSurface {...props} external />
    </CompactChrome>,
  );
  expect(reportPicture).toHaveBeenLastCalledWith({
    state: "playing",
    status: "On air",
    external: true,
  });

  // A failure is reported under the name the player's heading gives it.
  view.rerender(
    <CompactChrome reportPicture={reportPicture}>
      <PlaybackSurface
        {...props}
        state={{
          _tag: "failed",
          failure: "stream-interrupted",
          retryable: true,
        }}
      />
    </CompactChrome>,
  );
  expect(reportPicture).toHaveBeenLastCalledWith({
    state: "failed",
    status: "Signal lost",
    external: false,
  });
  expect(reportPicture).not.toHaveBeenCalledWith(null);

  view.unmount();
  expect(reportPicture).toHaveBeenLastCalledWith(null);
});

/** A stand-in for the shell: compact controls portalled into a slot it owns. */
function CompactChrome({
  children,
  fullscreenTarget = null,
  menuSide = "top",
  reportPicture = ignorePicture,
}: {
  readonly children: ReactNode;
  readonly fullscreenTarget?: HTMLElement | null;
  readonly menuSide?: StageChrome["menuSide"];
  readonly reportPicture?: StageChrome["reportPicture"];
}) {
  const [controlsSlot, setControlsSlot] = useState<HTMLElement | null>(null);
  const chrome = useMemo<StageChrome>(
    () => ({
      controlsSlot,
      controls: "compact",
      fullscreenTarget,
      menuSide,
      reportPicture,
    }),
    [controlsSlot, fullscreenTarget, menuSide, reportPicture],
  );
  return (
    <StageChromeProvider value={chrome}>
      {children}
      <div data-testid="controls-slot" ref={setControlsSlot} />
    </StageChromeProvider>
  );
}

function ignorePicture(): void {}

/** jsdom has no fullscreen: makes an element the document's, or none, as a browser reports it. */
function enterFullscreen(element: Element | null): void {
  act(() => {
    Object.defineProperty(document, "fullscreenElement", {
      configurable: true,
      value: element,
    });
    document.dispatchEvent(new Event("fullscreenchange"));
  });
}

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
