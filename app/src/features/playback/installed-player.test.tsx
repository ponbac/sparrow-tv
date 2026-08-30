import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clientSchemas,
  type ClientResult,
  type PlaybackDescriptor,
} from "../../client/contracts";
import {
  InstalledPlayer,
  type InstalledPlayerProps,
} from "./installed-player";
import type { NativePlaybackEngine } from "./native-mpegts-engine";

afterEach(cleanup);

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

describe("InstalledPlayer", () => {
  it("moves from starting to playing through one opaque native session", async () => {
    const client = playbackClient(async () => success(DESCRIPTOR));
    const engineStop = vi.fn();
    const engineStart = vi.fn<NativePlaybackEngine["start"]>((request) => {
      request.video.dispatchEvent(new Event("playing"));
      return { stop: engineStop };
    });
    const onStop = vi.fn();
    const view = render(
      <InstalledPlayer
        channel={CHANNEL}
        client={client}
        engine={{ start: engineStart }}
        onStop={onStop}
      />,
    );

    expect(screen.getByText("Opening the live signal")).toBeVisible();
    expect(await screen.findByText("ON AIR")).toBeVisible();
    expect(client.startPlayback).toHaveBeenCalledTimes(1);
    expect(client.startPlayback).toHaveBeenCalledWith({
      id: CHANNEL.id,
      signal: expect.any(AbortSignal),
    });
    expect(engineStart).toHaveBeenCalledTimes(1);
    expect(engineStart.mock.calls[0]?.[0].descriptor).toEqual(DESCRIPTOR);
    expect(document.body.innerHTML).not.toContain(DESCRIPTOR.sessionId);
    expect(document.body.innerHTML).not.toContain(DESCRIPTOR.streamHandle);

    await userEvent.setup().click(
      screen.getByRole("button", { name: "Stop stream" }),
    );
    expect(onStop).toHaveBeenCalledTimes(1);
    view.unmount();
    expect(engineStop).toHaveBeenCalledTimes(1);
  });

  it("aborts an opening session when its player is removed", async () => {
    const start = deferred<ClientResult<PlaybackDescriptor>>();
    const client = playbackClient(() => start.promise);
    const engineStart = vi.fn<NativePlaybackEngine["start"]>(() => ({
      stop: vi.fn(),
    }));
    const view = render(
      <InstalledPlayer
        channel={CHANNEL}
        client={client}
        engine={{ start: engineStart }}
        onStop={vi.fn()}
      />,
    );
    const signal = requireStartSignal(client.startPlayback);

    view.unmount();
    expect(signal.aborted).toBe(true);
    start.resolve({ ok: false, error: { _tag: "cancelled" } });
    await start.promise;
    await Promise.resolve();
    expect(engineStart).not.toHaveBeenCalled();
  });

  it("renders only safe typed start failures and never starts the engine", async () => {
    const privateMessage = "https://user:secret@provider.invalid/live";
    const client = playbackClient(async () => ({
      ok: false,
      error: {
        _tag: "transport",
        retryable: true,
        message: privateMessage,
      },
    }));
    const engineStart = vi.fn<NativePlaybackEngine["start"]>(() => ({
      stop: vi.fn(),
    }));

    render(
      <InstalledPlayer
        channel={CHANNEL}
        client={client}
        engine={{ start: engineStart }}
        onStop={vi.fn()}
      />,
    );

    expect(await screen.findByText("SOURCE OFFLINE")).toBeVisible();
    expect(document.body.textContent).not.toContain("provider.invalid");
    expect(engineStart).not.toHaveBeenCalled();
  });
});

function playbackClient(
  startPlayback: InstalledPlayerProps["client"]["startPlayback"],
): InstalledPlayerProps["client"] & {
  readonly startPlayback: ReturnType<typeof vi.fn>;
} {
  return {
    startPlayback: vi.fn(startPlayback),
    readPlayback: vi.fn(async () => success(new ArrayBuffer(0))),
    stopPlayback: vi.fn(async () => success(undefined)),
  };
}

function requireStartSignal(
  startPlayback: ReturnType<typeof vi.fn>,
): AbortSignal {
  const input = startPlayback.mock.calls[0]?.[0];
  if (typeof input !== "object" || input === null || !("signal" in input)) {
    throw new Error("expected a start signal");
  }
  const signal = input.signal;
  if (!(signal instanceof AbortSignal)) {
    throw new Error("expected an AbortSignal");
  }
  return signal;
}

function success<Value>(value: Value): { readonly ok: true; readonly value: Value } {
  return { ok: true, value };
}

function deferred<Value>(): {
  readonly promise: Promise<Value>;
  readonly resolve: (value: Value) => void;
} {
  let resolve: ((value: Value) => void) | undefined;
  const promise = new Promise<Value>((next) => {
    resolve = next;
  });
  return {
    promise,
    resolve: (value) => {
      if (resolve === undefined) {
        throw new Error("deferred fixture was not initialized");
      }
      resolve(value);
    },
  };
}
