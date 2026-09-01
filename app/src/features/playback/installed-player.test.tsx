import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clientSchemas,
  type InstalledPlaybackSession,
  type InstalledPlaybackTransport,
} from "../../client/contracts";
import { InstalledPlayer, type InstalledPlayerProps } from "./installed-player";
import type {
  InstalledLifecycleEvents,
  InstalledLifecycleSignal,
} from "./installed-lifecycle";
import type { InstalledPlaybackEngine } from "./installed-playback-engine";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const CHANNEL = clientSchemas.channel.parse({
  id: "installed-news",
  name: "Installed News",
  group: "News",
});
const DESCRIPTOR = clientSchemas.nativePlaybackDescriptor.parse({
  _tag: "tauri-native-stream",
  sessionId: `play1_${"f".repeat(32)}_1`,
  streamHandle: `stream1_${"a".repeat(16)}`,
  presentation: "webview-mse",
  tracks: [],
  selection: { _tag: "none" },
});
const REOPENED_DESCRIPTOR = clientSchemas.nativePlaybackDescriptor.parse({
  _tag: "tauri-native-stream",
  sessionId: DESCRIPTOR.sessionId,
  streamHandle: `stream1_${"b".repeat(16)}`,
  presentation: "webview-mse",
  tracks: [],
  selection: { _tag: "none" },
});
const ENGLISH_AUDIO_ID = clientSchemas.audioTrackId.parse(
  `atrk1_${"3".repeat(32)}`,
);
const SPANISH_AUDIO_ID = clientSchemas.audioTrackId.parse(
  `atrk1_${"4".repeat(32)}`,
);
const AUDIO_TRACKS = [
  {
    id: ENGLISH_AUDIO_ID,
    language: "eng",
    label: "Original",
    codec: "aac-adts" as const,
    selected: true,
  },
  {
    id: SPANISH_AUDIO_ID,
    codec: "ac-3" as const,
    selected: false,
  },
] as const;
const MULTI_AUDIO_DESCRIPTOR = clientSchemas.nativePlaybackDescriptor.parse({
  _tag: "tauri-native-stream",
  sessionId: DESCRIPTOR.sessionId,
  streamHandle: `stream1_${"c".repeat(16)}`,
  presentation: "webview-mse",
  tracks: AUDIO_TRACKS,
  selection: {
    _tag: "selected",
    trackId: ENGLISH_AUDIO_ID,
    reason: "first-available",
  },
});
const SPANISH_AUDIO_DESCRIPTOR = clientSchemas.nativePlaybackDescriptor.parse({
  _tag: "tauri-native-stream",
  sessionId: DESCRIPTOR.sessionId,
  streamHandle: `stream1_${"d".repeat(16)}`,
  presentation: "webview-mse",
  tracks: AUDIO_TRACKS.map((track) => ({
    ...track,
    selected: track.id === SPANISH_AUDIO_ID,
  })),
  selection: {
    _tag: "selected",
    trackId: SPANISH_AUDIO_ID,
    reason: "requested",
  },
  preferenceStatus: "saved",
});
const LINUX_TRANSPORT = { _tag: "linux-mpv" } as const;

describe("InstalledPlayer", () => {
  it("brings a newly mounted player into the mobile viewport before playback starts", async () => {
    const originalMatchMedia = Object.getOwnPropertyDescriptor(
      window,
      "matchMedia",
    );
    const originalScrollIntoView = Object.getOwnPropertyDescriptor(
      HTMLVideoElement.prototype,
      "scrollIntoView",
    );
    const scrollIntoView = vi.fn();
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: (query: string): MediaQueryList => ({
        matches: query === "(max-width: 760px)",
        media: query,
        onchange: null,
        addListener: vi.fn(),
        removeListener: vi.fn(),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        dispatchEvent: vi.fn(() => true),
      }),
    });
    Object.defineProperty(HTMLVideoElement.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoView,
    });

    try {
      const session = fixtureSession();
      render(
        <InstalledPlayer
          channel={CHANNEL}
          client={fixtureClient(() => session.value)}
          engine={playingEngine().value}
          onStop={vi.fn()}
        />,
      );

      await waitFor(() => expect(session.start).toHaveBeenCalledTimes(1));
      expect(scrollIntoView).toHaveBeenCalledWith({
        behavior: "auto",
        block: "center",
        inline: "nearest",
      });
    } finally {
      restoreProperty(window, "matchMedia", originalMatchMedia);
      restoreProperty(
        HTMLVideoElement.prototype,
        "scrollIntoView",
        originalScrollIntoView,
      );
    }
  });

  it("owns pause, live-edge resume, controls, diagnostics, and confirmed stop", async () => {
    const session = fixtureSession();
    const client = fixtureClient(() => session.value);
    const engine = playingEngine();
    const onStop = vi.fn();
    const user = userEvent.setup();
    const clipboard = vi.fn<(text: string) => Promise<void>>(
      async () => undefined,
    );
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: clipboard },
    });
    render(
      <InstalledPlayer
        channel={CHANNEL}
        client={client}
        engine={engine.value}
        onStop={onStop}
      />,
    );

    expect(await screen.findByText("ON AIR")).toBeVisible();
    expect(client.createPlaybackSession).toHaveBeenCalledWith({
      id: CHANNEL.id,
    });
    expect(session.start).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: "Pause" }));
    expect(await screen.findByText("PAUSED")).toBeVisible();
    expect(engine.stops).toBe(1);
    expect(session.suspend).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: "Resume" }));
    expect(await screen.findByText("ON AIR")).toBeVisible();
    expect(session.reopen).toHaveBeenCalledTimes(1);

    expect(screen.getByRole("button", { name: "Mute" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    await user.click(screen.getByRole("button", { name: "Mute" }));
    expect(screen.getByRole("button", { name: "Unmute" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    fireEvent.change(screen.getByRole("slider", { name: "Volume" }), {
      target: { value: "35" },
    });

    await user.click(screen.getByRole("button", { name: "Copy diagnostics" }));
    await waitFor(() => expect(clipboard).toHaveBeenCalledTimes(1));
    const copied = String(clipboard.mock.calls[0]?.[0]);
    expect(copied).toContain('"engine":"mpegts-native"');
    expect(copied).not.toContain(CHANNEL.id);
    expect(copied).not.toContain(CHANNEL.name);
    expect(copied).not.toContain(DESCRIPTOR.sessionId);
    expect(copied).not.toContain(DESCRIPTOR.streamHandle);

    await user.click(screen.getByRole("button", { name: "Stop stream" }));
    await waitFor(() => expect(onStop).toHaveBeenCalledTimes(1));
    expect(session.stop).toHaveBeenCalledTimes(1);
  });

  it("surfaces unconfirmed cleanup and refuses to dismiss the player", async () => {
    const session = fixtureSession();
    session.stop.mockResolvedValue({
      ok: false,
      error: {
        _tag: "transport",
        retryable: false,
        message: "safe cleanup failure",
      },
    });
    const onStop = vi.fn();
    render(
      <InstalledPlayer
        channel={CHANNEL}
        client={fixtureClient(() => session.value)}
        engine={playingEngine().value}
        onStop={onStop}
      />,
    );
    expect(await screen.findByText("ON AIR")).toBeVisible();

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "Stop stream" }));

    expect(await screen.findByText("CLEANUP NEEDED")).toBeVisible();
    expect(onStop).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("button", { name: "Restart" }),
    ).not.toBeInTheDocument();
  });

  it("enumerates Audio Tracks, selects without opaque UI, and confirms persistence", async () => {
    const session = fixtureSession({
      start: MULTI_AUDIO_DESCRIPTOR,
      restart: SPANISH_AUDIO_DESCRIPTOR,
    });
    render(
      <InstalledPlayer
        channel={CHANNEL}
        client={fixtureClient(() => session.value)}
        engine={playingEngine().value}
        onStop={vi.fn()}
      />,
    );
    expect(await screen.findByText("ON AIR")).toBeVisible();
    const selector = screen.getByRole("combobox", { name: "Audio track" });
    expect(selector).toHaveValue(ENGLISH_AUDIO_ID);
    expect(
      screen.getByRole("option", { name: "Original · ENG · AAC" }),
    ).toBeVisible();
    expect(
      screen.getByRole("option", { name: "Audio 2 · AC-3" }),
    ).toBeVisible();
    expect(document.body.textContent).not.toContain(ENGLISH_AUDIO_ID);
    expect(document.body.textContent).not.toContain(SPANISH_AUDIO_ID);

    await userEvent.setup().selectOptions(selector, SPANISH_AUDIO_ID);
    await waitFor(() => expect(session.restart).toHaveBeenCalledTimes(1));
    expect(session.restart).toHaveBeenCalledWith({
      expectedStreamHandle: MULTI_AUDIO_DESCRIPTOR.streamHandle,
      intent: {
        _tag: "select-audio",
        audioTrackId: SPANISH_AUDIO_ID,
      },
      signal: expect.any(AbortSignal),
    });
    expect(await screen.findByText("Audio preference saved for this channel.")).toBeVisible();
    expect(selector).toHaveValue(SPANISH_AUDIO_ID);
  });

  it("visibly falls back when a saved Audio Track is no longer available", async () => {
    const fallback = clientSchemas.nativePlaybackDescriptor.parse({
      ...MULTI_AUDIO_DESCRIPTOR,
      tracks: [AUDIO_TRACKS[0]],
      selection: {
        _tag: "fallback",
        trackId: ENGLISH_AUDIO_ID,
        missing: "saved-preference",
      },
    });
    render(
      <InstalledPlayer
        channel={CHANNEL}
        client={fixtureClient(() => fixtureSession({ start: fallback }).value)}
        engine={playingEngine().value}
        onStop={vi.fn()}
      />,
    );

    expect(
      await screen.findByText(
        "Saved audio is unavailable. Using the first compatible track.",
      ),
    ).toBeVisible();
    expect(screen.getByRole("combobox", { name: "Audio track" })).toBeDisabled();
  });

  it("does not leak resources across React StrictMode setup replay", async () => {
    const resources: ReturnType<typeof fixtureSession>[] = [];
    const client = fixtureClient(() => {
      const session = fixtureSession();
      resources.push(session);
      return session.value;
    });
    const view = render(
      <StrictMode>
        <InstalledPlayer
          channel={CHANNEL}
          client={client}
          engine={playingEngine().value}
          onStop={vi.fn()}
        />
      </StrictMode>,
    );
    expect(await screen.findByText("ON AIR")).toBeVisible();

    view.unmount();
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(resources.length).toBeGreaterThanOrEqual(1);
    expect(
      resources.every((resource) => resource.stop.mock.calls.length === 1),
    ).toBe(true);
  });

  it("preserves one session across rotation and owns native lifecycle resume", async () => {
    const session = fixtureSession();
    const client = fixtureClient(() => session.value);
    const lifecycle = lifecycleFixture();
    const props = {
      channel: CHANNEL,
      client,
      engine: playingEngine().value,
      lifecycleEvents: lifecycle.value,
      onStop: vi.fn(),
    } satisfies InstalledPlayerProps;
    const view = render(<InstalledPlayer {...props} />);
    expect(await screen.findByText("ON AIR")).toBeVisible();
    await waitFor(() => expect(lifecycle.subscribe).toHaveBeenCalledTimes(1));

    fireEvent(window, new Event("resize"));
    view.rerender(
      <InstalledPlayer
        {...props}
        channel={{ id: CHANNEL.id, name: CHANNEL.name }}
      />,
    );
    expect(client.createPlaybackSession).toHaveBeenCalledTimes(1);
    expect(session.start).toHaveBeenCalledTimes(1);

    await act(async () => lifecycle.emit("suspended"));
    expect(await screen.findByText("PAUSED")).toBeVisible();
    expect(session.suspend).toHaveBeenCalledTimes(1);
    await act(async () => lifecycle.emit("suspended"));
    expect(session.suspend).toHaveBeenCalledTimes(1);

    await act(async () => lifecycle.emit("resumed"));
    expect(await screen.findByText("ON AIR")).toBeVisible();
    expect(session.reopen).toHaveBeenCalledTimes(1);
    await act(async () => lifecycle.emit("resumed"));
    expect(session.reopen).toHaveBeenCalledTimes(1);

    view.unmount();
    expect(lifecycle.release).toHaveBeenCalledTimes(1);
  });

  it("presents Linux mpv as the primary engine with separate-window controls", async () => {
    const session = fixtureSession({
      start: LINUX_TRANSPORT,
      reopen: LINUX_TRANSPORT,
      restart: LINUX_TRANSPORT,
    });
    const client = fixtureClient(() => session.value);
    const onStop = vi.fn();
    const user = userEvent.setup();
    render(
      <InstalledPlayer
        channel={CHANNEL}
        client={client}
        engine={playingEngine().value}
        onStop={onStop}
      />,
    );

    expect(await screen.findByText("ON AIR")).toBeVisible();
    expect(screen.getByText("Live monitor · system mpv")).toBeVisible();
    expect(screen.getByRole("button", { name: "Mute" })).toBeVisible();
    expect(screen.getByRole("slider", { name: "Volume" })).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Full screen" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Open in mpv" }),
    ).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Stop mpv" }));
    await waitFor(() => expect(onStop).toHaveBeenCalledTimes(1));
    expect(session.stop).toHaveBeenCalledTimes(1);
  });

  it("keeps a terminal primary failure on the restart-only path", async () => {
    const session = fixtureSession();
    const engine: InstalledPlaybackEngine = {
      start: ({ onFailure }) => {
        onFailure("media-unsupported", false);
        return { stop: () => undefined };
      },
    };
    render(
      <InstalledPlayer
        channel={CHANNEL}
        client={fixtureClient(() => session.value)}
        engine={engine}
        onStop={vi.fn()}
      />,
    );

    expect(await screen.findByText("FORMAT MISSED")).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Open in mpv" }),
    ).not.toBeInTheDocument();
  });

  it("gives actionable copy when the required Linux system player is missing", async () => {
    const session = fixtureSession();
    session.start.mockResolvedValueOnce({
      ok: false,
      error: {
        _tag: "mpv-failed",
        reason: "not-installed",
        retryable: false,
      },
    });
    render(
      <InstalledPlayer
        channel={CHANNEL}
        client={fixtureClient(() => session.value)}
        engine={playingEngine().value}
        onStop={vi.fn()}
      />,
    );

    expect(await screen.findByText("MPV MISSING")).toBeVisible();
    expect(
      screen.getByText("System mpv is required for Linux playback"),
    ).toBeVisible();
    expect(screen.getByText("Install mpv, then restart playback.")).toBeVisible();
  });
});

function restoreProperty(
  target: object,
  property: string,
  descriptor: PropertyDescriptor | undefined,
): void {
  if (descriptor === undefined) {
    Reflect.deleteProperty(target, property);
  } else {
    Object.defineProperty(target, property, descriptor);
  }
}

function lifecycleFixture(): {
  readonly value: InstalledLifecycleEvents;
  readonly subscribe: ReturnType<typeof vi.fn>;
  readonly release: ReturnType<typeof vi.fn>;
  readonly emit: (signal: InstalledLifecycleSignal) => void;
} {
  let listener: ((signal: InstalledLifecycleSignal) => void) | null = null;
  const release = vi.fn(() => {
    listener = null;
  });
  const subscribe = vi.fn<InstalledLifecycleEvents["subscribe"]>(
    async (next) => {
      listener = next;
      return release;
    },
  );
  return {
    value: { subscribe },
    subscribe,
    release,
    emit: (signal) => listener?.(signal),
  };
}

function fixtureClient(
  create: () => InstalledPlaybackSession,
): InstalledPlayerProps["client"] & {
  readonly createPlaybackSession: ReturnType<typeof vi.fn>;
} {
  return {
    createPlaybackSession: vi.fn(create),
  };
}

function fixtureSession(
  descriptors: {
    readonly start?: InstalledPlaybackTransport;
    readonly reopen?: InstalledPlaybackTransport;
    readonly restart?: InstalledPlaybackTransport;
  } = {},
): {
  readonly value: InstalledPlaybackSession;
  readonly start: ReturnType<typeof vi.fn>;
  readonly reopen: ReturnType<typeof vi.fn>;
  readonly restart: ReturnType<typeof vi.fn>;
  readonly startAndroidPresentation: ReturnType<typeof vi.fn>;
  readonly controlMpv: ReturnType<typeof vi.fn>;
  readonly suspend: ReturnType<typeof vi.fn>;
  readonly setActivity: ReturnType<typeof vi.fn>;
  readonly stop: ReturnType<typeof vi.fn>;
} {
  const start = vi.fn(async () => success(descriptors.start ?? DESCRIPTOR));
  const reopen = vi.fn(async () =>
    success(descriptors.reopen ?? REOPENED_DESCRIPTOR),
  );
  const restart = vi.fn(async () =>
    success(descriptors.restart ?? REOPENED_DESCRIPTOR),
  );
  const startAndroidPresentation = vi.fn(async () =>
    success(androidPresentationFixture()),
  );
  const controlMpv = vi.fn(async () => success(undefined));
  const suspend = vi.fn(async () => success(undefined));
  const setActivity = vi.fn(async () => success(undefined));
  const stop = vi.fn(async () => success(undefined));
  return {
    value: {
      start,
      reopen,
      restart,
      read: vi.fn(async () => success(new ArrayBuffer(0))),
      startAndroidPresentation,
      controlMpv,
      suspend,
      setActivity,
      stop,
    },
    start,
    reopen,
    restart,
    startAndroidPresentation,
    controlMpv,
    suspend,
    setActivity,
    stop,
  };
}

function playingEngine(): {
  readonly value: InstalledPlaybackEngine;
  readonly stops: number;
} {
  let stops = 0;
  return {
    value: {
      start: ({ onPlaying }) => {
        onPlaying();
        let active = true;
        return {
          stop: () => {
            if (active) {
              active = false;
              stops += 1;
            }
          },
        };
      },
    },
    get stops() {
      return stops;
    },
  };
}

function androidPresentationFixture() {
  return {
    status: async () =>
      success({
        state: "stopped" as const,
        decodedFrames: 0,
        droppedFrames: 0,
        bufferedDurationMs: 0,
        silent: true,
      }),
    pause: async () => success(undefined),
    resume: async () => success(undefined),
    setVolume: async () => success(undefined),
    setMuted: async () => success(undefined),
    setViewport: async () => success(undefined),
    stop: async () => success(undefined),
  };
}

function success<Value>(value: Value): {
  readonly ok: true;
  readonly value: Value;
} {
  return { ok: true, value };
}
