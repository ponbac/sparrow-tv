import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ChannelId } from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import { createHttpSparrowClient } from "../../client/http";
import { HostedPlayer, type HostedPlayerProps } from "./hosted-player";
import type { HostedPlaybackEngine, HostedPlaybackFailure } from "./mpegts-engine";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("HostedPlayer", () => {
  it("reconnects an interrupted signal after releasing the old request and waiting", async () => {
    vi.useFakeTimers();
    const engine = controlledEngine();
    render(
      <HostedPlayer channel={channel("channel-one", "World News")}
        client={createHttpSparrowClient()} engine={engine.value} onStop={() => undefined} />,
    );
    await act(async () => { await Promise.resolve(); });
    act(() => engine.fail("stream-interrupted"));

    expect(engine.events).toEqual(["start", "stop"]);
    expect(screen.getByText("Reconnecting")).toBeVisible();
    await act(async () => { await vi.advanceTimersByTimeAsync(999); });
    expect(engine.events).toEqual(["start", "stop"]);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(engine.events).toEqual(["start", "stop", "start"]);
    expect(screen.getByText("On air")).toBeVisible();
  });

  it("keeps manual retry immediate without leaving a second scheduled reconnect", async () => {
    vi.useFakeTimers();
    const engine = controlledEngine();
    render(<HostedPlayer channel={channel("channel-one", "World News")}
      client={createHttpSparrowClient()} engine={engine.value} onStop={() => undefined} />);
    await act(async () => { await Promise.resolve(); });
    act(() => engine.fail("stream-interrupted"));
    fireEvent.click(screen.getByRole("button", { name: "Reconnect signal" }));
    await act(async () => { await Promise.resolve(); });
    expect(engine.events).toEqual(["start", "stop", "start"]);
    await act(async () => { await vi.advanceTimersByTimeAsync(21_000); });
    expect(engine.events).toEqual(["start", "stop", "start"]);
    expect(screen.getByText("On air")).toBeVisible();
  });

  it.each(["stop", "unmount", "channel-change"] as const)(
    "cancels a pending reconnect on %s", async (action) => {
      vi.useFakeTimers();
      const engine = controlledEngine();
      const client = createHttpSparrowClient();
      let stops = 0;
      const view = render(<HostedPlayer channel={channel("channel-one", "World News")}
        client={client} engine={engine.value} onStop={() => { stops += 1; }} />);
      await act(async () => { await Promise.resolve(); });
      act(() => engine.fail("stream-interrupted"));
      if (action === "stop") {
        // Stop releases immediately even if the parent does not unmount yet.
        fireEvent.click(screen.getByRole("button", { name: "Stop stream" }));
        expect(stops).toBe(1);
      } else if (action === "unmount") {
        view.unmount();
      } else {
        view.rerender(<HostedPlayer channel={channel("channel-two", "Cinema One")}
          client={client} engine={engine.value} onStop={() => undefined} />);
      }
      await act(async () => { await vi.advanceTimersByTimeAsync(21_000); });
      expect(engine.events).toEqual(action === "channel-change"
        ? ["start", "stop", "start"] : ["start", "stop"]);
      if (action === "channel-change") expect(screen.getByLabelText("Cinema One live video")).toBeVisible();
    },
  );

  it.each([
    ["backoff", "AbortError"],
    ["backoff", "NotSupportedError"],
    ["new-transport", "AbortError"],
    ["new-transport", "NotSupportedError"],
  ] as const)("ignores a retired gesture's %s rejection (%s)", async (timing, errorName) => {
    vi.useFakeTimers();
    let reject: ((cause: unknown) => void) | undefined;
    const pending = new Promise<void>((_resolve, no) => { reject = no; });
    const events: string[] = [];
    let interrupt: (() => void) | undefined;
    const engine: HostedPlaybackEngine = {
      start: ({ video, onAutoplayBlocked, onFailure }) => {
        events.push("start");
        interrupt = () => onFailure("stream-interrupted");
        if (events.length === 1) {
          video.play = () => {
            video.dispatchEvent(new Event("play"));
            return pending;
          };
          onAutoplayBlocked();
        } else {
          video.dispatchEvent(new Event("playing"));
        }
        return { stop: () => { events.push("stop"); } };
      },
    };
    render(<HostedPlayer channel={channel("channel-one", "World News")}
      client={createHttpSparrowClient()} engine={engine} onStop={() => undefined} />);
    await act(async () => { await Promise.resolve(); });
    fireEvent.click(screen.getByRole("button", { name: "Start audio & video" }));
    act(() => interrupt?.());
    expect(screen.getByText("Reconnecting")).toBeVisible();
    if (timing === "new-transport") {
      await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
    }
    await act(async () => {
      reject?.(new DOMException("retired playback", errorName));
      await Promise.resolve();
    });
    expect(screen.getByText(timing === "backoff" ? "Reconnecting" : "On air")).toBeVisible();
    if (timing === "backoff") {
      await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
    }
    expect(screen.getByText("On air")).toBeVisible();
    expect(events).toEqual(["start", "stop", "start"]);
  });

  it.each(["NotAllowedError", "NotSupportedError", "AbortError"] as const)(
    "classifies a current gesture's %s without suppressing genuine failures", async (errorName) => {
      const events: string[] = [];
      const engine: HostedPlaybackEngine = {
        start: ({ video, onAutoplayBlocked }) => {
          events.push("start");
          video.play = () => {
            video.dispatchEvent(new Event("play"));
            video.dispatchEvent(new Event("playing"));
            return Promise.reject(new DOMException("current playback", errorName));
          };
          onAutoplayBlocked();
          return { stop: () => { events.push("stop"); } };
        },
      };
      render(<HostedPlayer channel={channel("channel-one", "World News")}
        client={createHttpSparrowClient()} engine={engine} onStop={() => undefined} />);
      fireEvent.click(await screen.findByRole("button", { name: "Start audio & video" }));
      await act(async () => { await Promise.resolve(); });
      if (errorName === "NotSupportedError") {
        expect(screen.getByText("Format missed")).toBeVisible();
        expect(events).toEqual(["start", "stop"]);
        expect(screen.queryByRole("button", { name: "Start audio & video" })).not.toBeInTheDocument();
      } else {
        if (errorName === "NotAllowedError") {
          expect(screen.getByRole("button", { name: "Start audio & video" })).toBeVisible();
        } else {
          expect(screen.getByText("On air")).toBeVisible();
        }
        expect(events).toEqual(["start"]);
      }
    },
  );

  it("ignores a prior Channel's late user-gesture play rejection", async () => {
    let reject: ((cause: unknown) => void) | undefined;
    const playing = new Promise<void>((_resolve, no) => { reject = no; });
    const events: string[] = [];
    const engine: HostedPlaybackEngine = {
      start: ({ endpoint, video, onAutoplayBlocked }) => {
        events.push(`start:${endpoint}`);
        if (endpoint === "/api/v1/play/channel-one") {
          video.play = () => playing;
          onAutoplayBlocked();
        } else {
          video.dispatchEvent(new Event("playing"));
        }
        return { stop: () => { events.push(`stop:${endpoint}`); } };
      },
    };
    const client = createHttpSparrowClient();
    const view = render(<HostedPlayer channel={channel("channel-one", "World News")}
      client={client} engine={engine} onStop={() => undefined} />);
    fireEvent.click(await screen.findByRole("button", { name: "Start audio & video" }));
    view.rerender(<HostedPlayer channel={channel("channel-two", "Cinema One")}
      client={client} engine={engine} onStop={() => undefined} />);
    await screen.findByText("On air");
    await act(async () => {
      reject?.(new Error("old video failed"));
      await Promise.resolve();
    });
    expect(screen.getByText("On air")).toBeVisible();
    expect(events).toEqual([
      "start:/api/v1/play/channel-one", "stop:/api/v1/play/channel-one",
      "start:/api/v1/play/channel-two",
    ]);
  });

  it("starts from only a Channel Identifier and renders a playing monitor", async () => {
    const engine = recordingEngine();
    let fetchCalls = 0;
    const client = createHttpSparrowClient({
      fetch: async () => {
        fetchCalls += 1;
        throw new Error("the descriptor must remain local");
      },
    });

    render(
      <StrictMode>
        <HostedPlayer
          channel={channel("channel-one", "World News")}
          client={client}
          engine={engine.value}
          onStop={vi.fn()}
        />
      </StrictMode>,
    );

    expect(await screen.findByText("On air")).toBeVisible();
    expect(engine.events).toEqual(["start:/api/v1/play/channel-one"]);
    expect(fetchCalls).toBe(0);
    expect(document.body.textContent).not.toContain("provider.invalid");
    expect(screen.getByLabelText("World News live video")).toBeInTheDocument();
  });

  it("fully stops the old stream before opening a newly selected Channel", async () => {
    const engine = recordingEngine();
    const client = createHttpSparrowClient();
    const view = render(
      <HostedPlayer
        channel={channel("channel-one", "World News")}
        client={client}
        engine={engine.value}
        onStop={vi.fn()}
      />,
    );
    await screen.findByText("On air");

    view.rerender(
      <HostedPlayer
        channel={channel("channel-two", "Cinema One")}
        client={client}
        engine={engine.value}
        onStop={vi.fn()}
      />,
    );

    await waitFor(() =>
      expect(engine.events).toEqual([
        "start:/api/v1/play/channel-one",
        "stop:/api/v1/play/channel-one",
        "start:/api/v1/play/channel-two",
      ]),
    );
    expect(screen.getByRole("heading", { name: "Cinema One" })).toBeVisible();
  });

  it("keeps the viewer's volume when the Channel changes", async () => {
    const engine = recordingEngine();
    const client = createHttpSparrowClient();
    const view = render(
      <HostedPlayer
        channel={channel("channel-one", "World News")}
        client={client}
        engine={engine.value}
        onStop={vi.fn()}
      />,
    );
    await screen.findByText("On air");
    fireEvent.change(screen.getByRole("slider", { name: "Volume" }), {
      target: { value: "35" },
    });
    expect(screen.getByLabelText("World News live video")).toHaveProperty(
      "volume",
      0.35,
    );

    view.rerender(
      <HostedPlayer
        channel={channel("channel-two", "Cinema One")}
        client={client}
        engine={engine.value}
        onStop={vi.fn()}
      />,
    );

    expect(screen.getByRole("slider", { name: "Volume" })).toHaveValue("35");
    expect(screen.getByLabelText("Cinema One live video")).toHaveProperty(
      "volume",
      0.35,
    );
  });

  it("shows fullscreen that began before the player mounted", async () => {
    Object.defineProperty(document, "fullscreenElement", {
      configurable: true,
      value: document.documentElement,
    });
    try {
      render(
        <HostedPlayer
          channel={channel("channel-one", "World News")}
          client={createHttpSparrowClient()}
          engine={recordingEngine().value}
          onStop={vi.fn()}
        />,
      );

      expect(
        await screen.findByRole("button", { name: "Exit fullscreen" }),
      ).toHaveAttribute("aria-pressed", "true");
    } finally {
      Reflect.deleteProperty(document, "fullscreenElement");
    }
  });

  it("releases playback when the user stops the monitor", async () => {
    const user = userEvent.setup();
    const engine = recordingEngine();
    render(<StoppingHarness engine={engine.value} />);
    await screen.findByText("On air");

    await user.click(screen.getByRole("button", { name: "Stop stream" }));

    expect(await screen.findByText("Monitor stopped")).toBeVisible();
    expect(engine.events).toEqual([
      "start:/api/v1/play/channel-one",
      "stop:/api/v1/play/channel-one",
    ]);
  });

  it("turns exhausted engine recovery into actionable safe copy", async () => {
    vi.useFakeTimers();
    const privateDiagnostic =
      "https://viewer:secret@provider.invalid/live?token=private";
    const engine: HostedPlaybackEngine = {
      start: ({ onFailure }) => {
        onFailure("source-timeout");
        return { stop: () => undefined };
      },
    };

    render(
      <HostedPlayer
        channel={channel("channel-one", "World News")}
        client={createHttpSparrowClient()}
        engine={engine}
        onStop={vi.fn()}
      />,
    );

    await act(async () => { await vi.advanceTimersByTimeAsync(21_000); });
    expect(
      screen.getByText("The signal took too long to answer"),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: /Try signal again/ })).toBeVisible();
    expect(document.body.textContent).not.toContain(privateDiagnostic);
    expect(document.body.textContent).not.toContain("viewer:secret");
  });

  it("leaves the on-air state when a live response ends", async () => {
    let interrupt: (() => void) | undefined;
    const engine: HostedPlaybackEngine = {
      start: ({ onFailure, video }) => {
        interrupt = () => onFailure("stream-interrupted");
        video.dispatchEvent(new Event("playing"));
        return { stop: () => undefined };
      },
    };
    render(
      <HostedPlayer
        channel={channel("channel-one", "World News")}
        client={createHttpSparrowClient()}
        engine={engine}
        onStop={vi.fn()}
      />,
    );
    expect(await screen.findByText("On air")).toBeVisible();

    act(() => interrupt?.());

    expect(await screen.findByText("Reconnecting")).toBeVisible();
    expect(
      screen.getByRole("button", { name: /Reconnect signal/ }),
    ).toBeVisible();
  });

  it("does not offer a futile retry for unsupported media", async () => {
    const engine: HostedPlaybackEngine = {
      start: () => "media-unsupported",
    };
    render(
      <HostedPlayer
        channel={channel("channel-one", "World News")}
        client={createHttpSparrowClient()}
        engine={engine}
        onStop={vi.fn()}
      />,
    );

    expect(
      await screen.findByText("This signal cannot play in the browser"),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: /Try signal again/ }),
    ).not.toBeInTheDocument();
  });

  it("preserves a non-retryable client transport decision", async () => {
    const client: HostedPlayerProps["client"] = {
      startPlayback: async () => ({
        ok: false,
        error: {
          _tag: "transport",
          retryable: false,
          message: "The playback route could not be encoded.",
        },
      }),
    };

    render(
      <HostedPlayer
        channel={channel("channel-one", "World News")}
        client={client}
        engine={recordingEngine().value}
        onStop={vi.fn()}
      />,
    );

    expect(await screen.findByText("Source offline")).toBeVisible();
    expect(
      screen.getByText("Choose another channel or refresh the sources."),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: /Try signal again/ }),
    ).not.toBeInTheDocument();
  });
});

function StoppingHarness({ engine }: { readonly engine: HostedPlaybackEngine }) {
  const [active, setActive] = useState(true);
  return active ? (
    <HostedPlayer
      channel={channel("channel-one", "World News")}
      client={createHttpSparrowClient()}
      engine={engine}
      onStop={() => setActive(false)}
    />
  ) : (
    <p>Monitor stopped</p>
  );
}

function controlledEngine() {
  const events: string[] = [];
  let request: Parameters<HostedPlaybackEngine["start"]>[0] | undefined;
  return {
    events,
    fail: (failure: HostedPlaybackFailure) => request?.onFailure(failure),
    value: {
      start: (next) => {
        request = next;
        events.push("start");
        next.video.dispatchEvent(new Event("playing"));
        return { stop: () => { events.push("stop"); } };
      },
    } satisfies HostedPlaybackEngine,
  };
}

function recordingEngine(): {
  readonly value: HostedPlaybackEngine;
  readonly events: string[];
} {
  const events: string[] = [];
  return {
    events,
    value: {
      start: ({ endpoint, video }) => {
        events.push(`start:${endpoint}`);
        video.dispatchEvent(new Event("playing"));
        return {
          stop: () => events.push(`stop:${endpoint}`),
        };
      },
    },
  };
}

function channel(id: string, name: string): {
  readonly id: ChannelId;
  readonly name: string;
} {
  return {
    id: channelFixture({ id, name, group: "Fixtures" }).id,
    name,
  };
}
