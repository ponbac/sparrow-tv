import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clientSchemas, type ChannelId } from "../../client/contracts";
import { createHttpSparrowClient } from "../../client/http";
import { HostedPlayer } from "./hosted-player";
import type { HostedPlaybackEngine } from "./mpegts-engine";

afterEach(cleanup);

describe("HostedPlayer", () => {
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

    expect(await screen.findByText("ON AIR")).toBeVisible();
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
    await screen.findByText("ON AIR");

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

  it("releases playback when the user stops the monitor", async () => {
    const user = userEvent.setup();
    const engine = recordingEngine();
    render(<StoppingHarness engine={engine.value} />);
    await screen.findByText("ON AIR");

    await user.click(screen.getByRole("button", { name: "Stop stream" }));

    expect(await screen.findByText("Monitor stopped")).toBeVisible();
    expect(engine.events).toEqual([
      "start:/api/v1/play/channel-one",
      "stop:/api/v1/play/channel-one",
    ]);
  });

  it("turns engine failures into actionable safe copy", async () => {
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

    expect(
      await screen.findByText("The signal took too long to answer"),
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
    expect(await screen.findByText("ON AIR")).toBeVisible();

    act(() => interrupt?.());

    expect(await screen.findByText("SIGNAL LOST")).toBeVisible();
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
    const client = createHttpSparrowClient();
    vi.spyOn(client, "startPlayback").mockResolvedValue({
      ok: false,
      error: {
        _tag: "transport",
        retryable: false,
        message: "The playback route could not be encoded.",
      },
    });

    render(
      <HostedPlayer
        channel={channel("channel-one", "World News")}
        client={client}
        engine={recordingEngine().value}
        onStop={vi.fn()}
      />,
    );

    expect(await screen.findByText("SOURCE OFFLINE")).toBeVisible();
    expect(
      screen.getByText("Choose another Channel or refresh the catalog status."),
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
    id: clientSchemas.channel.parse({ id, name, group: "Fixtures" }).id,
    name,
  };
}
