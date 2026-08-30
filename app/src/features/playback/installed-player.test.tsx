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
} from "../../client/contracts";
import {
  InstalledPlayer,
  type InstalledPlayerProps,
} from "./installed-player";
import type { NativePlaybackEngine } from "./native-mpegts-engine";

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
});
const REOPENED_DESCRIPTOR = clientSchemas.nativePlaybackDescriptor.parse({
  _tag: "tauri-native-stream",
  sessionId: DESCRIPTOR.sessionId,
  streamHandle: `stream1_${"b".repeat(16)}`,
});

describe("InstalledPlayer", () => {
  it("owns pause, live-edge resume, controls, diagnostics, and confirmed stop", async () => {
    const session = fixtureSession();
    const client = fixtureClient(() => session.value);
    const engine = playingEngine();
    const onStop = vi.fn();
    const user = userEvent.setup();
    const clipboard = vi.fn<(text: string) => Promise<void>>(async () => undefined);
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
    expect(client.createPlaybackSession).toHaveBeenCalledWith({ id: CHANNEL.id });
    expect(session.start).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: "Pause" }));
    expect(await screen.findByText("PAUSED")).toBeVisible();
    expect(engine.stops).toBe(1);
    expect(session.suspend).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: "Resume" }));
    expect(await screen.findByText("ON AIR")).toBeVisible();
    expect(session.reopen).toHaveBeenCalledTimes(1);

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

    await userEvent.setup().click(
      screen.getByRole("button", { name: "Stop stream" }),
    );

    expect(await screen.findByText("CLEANUP NEEDED")).toBeVisible();
    expect(onStop).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Restart" })).not.toBeInTheDocument();
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
    expect(resources.every((resource) => resource.stop.mock.calls.length === 1)).toBe(
      true,
    );
  });
});

function fixtureClient(
  create: () => InstalledPlaybackSession,
): InstalledPlayerProps["client"] & {
  readonly createPlaybackSession: ReturnType<typeof vi.fn>;
} {
  return { createPlaybackSession: vi.fn(create) };
}

function fixtureSession(): {
  readonly value: InstalledPlaybackSession;
  readonly start: ReturnType<typeof vi.fn>;
  readonly reopen: ReturnType<typeof vi.fn>;
  readonly suspend: ReturnType<typeof vi.fn>;
  readonly stop: ReturnType<typeof vi.fn>;
} {
  const start = vi.fn(async () => success(DESCRIPTOR));
  const reopen = vi.fn(async () => success(REOPENED_DESCRIPTOR));
  const suspend = vi.fn(async () => success(undefined));
  const stop = vi.fn(async () => success(undefined));
  return {
    value: {
      start,
      reopen,
      read: vi.fn(async () => success(new ArrayBuffer(0))),
      suspend,
      stop,
    },
    start,
    reopen,
    suspend,
    stop,
  };
}

function playingEngine(): {
  readonly value: NativePlaybackEngine;
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

function success<Value>(
  value: Value,
): { readonly ok: true; readonly value: Value } {
  return { ok: true, value };
}
