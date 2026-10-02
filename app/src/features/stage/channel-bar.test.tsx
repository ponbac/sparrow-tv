import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import {
  clientSchemas,
  type ChannelSummary,
  type GuideWindowChannel,
} from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import { ChannelBar } from "./channel-bar";
import { zapStops } from "./zap";

afterEach(cleanup);

const NOW = new Date("2026-09-01T20:45:00.000Z");
const NEWS = channelFixture({ id: "news", name: "News", group: "A", number: 4 });
const FILM = channelFixture({ id: "film", name: "Film", group: "A", number: 5 });
const SPORT = channelFixture({
  id: "sport",
  name: "Sport",
  group: "A",
  number: 6,
});

describe("ChannelBar", () => {
  it("names the Channel on either side with what it shows now", () => {
    renderBar(
      [row(NEWS, "Evening News"), row(FILM, "Late Film"), row(SPORT, null)],
      FILM,
    );

    const bar = screen.getByRole("group", { name: "Channels" });
    // The label names the Channel; what it has on is read out after it.
    expect(
      within(bar).getByRole("button", { name: "Previous channel, 4 News" }),
    ).toHaveAccessibleDescription("Evening News");
    expect(
      within(bar).getByRole("button", { name: "Next channel, 6 Sport" }),
    ).toHaveAccessibleDescription("No guide data");
    expect(within(bar).getByRole("button", { name: "Guide" })).toBeEnabled();
    // Only the guide's own buttons are acceptance-marked Channels.
    expect(bar.querySelector("[data-acceptance-channel]")).toBeNull();
  });

  it("offers no Channel past either end, or before the rows around it are known", () => {
    const { rerender } = renderBar(
      [row(NEWS, "Evening News"), row(FILM, "Late Film")],
      NEWS,
    );
    expect(
      screen.getByRole("button", { name: "Previous channel" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Next channel, 5 Film" }),
    ).toBeEnabled();

    rerender(bar([row(NEWS, "Evening News"), row(FILM, "Late Film")], FILM));
    expect(
      screen.getByRole("button", { name: "Previous channel, 4 News" }),
    ).toBeEnabled();
    expect(screen.getByRole("button", { name: "Next channel" })).toBeDisabled();

    rerender(bar([], FILM));
    expect(
      screen.getByRole("button", { name: "Previous channel" }),
    ).toBeDisabled();
    expect(screen.getByRole("button", { name: "Next channel" })).toBeDisabled();
  });
});

function renderBar(rows: readonly GuideWindowChannel[], current: ChannelSummary) {
  return render(bar(rows, current));
}

function bar(rows: readonly GuideWindowChannel[], current: ChannelSummary) {
  return (
    <ChannelBar
      stops={
        rows.length === 0
          ? []
          : zapStops(
              { rows, startReached: true, endReached: true },
              current.id,
              new Set(),
              new Map(),
            )
      }
      current={current.id}
      now={NOW}
      onZap={() => undefined}
      onShowGuide={() => undefined}
    />
  );
}

/** A guide row with one Programme airing at `NOW`, or none. */
function row(channel: ChannelSummary, live: string | null): GuideWindowChannel {
  return {
    channel,
    programmes:
      live === null
        ? []
        : [
            {
              title: live,
              titleTruncated: false,
              startsAt: clientSchemas.isoInstant.parse(
                "2026-09-01T20:30:00.000Z",
              ),
              endsAt: clientSchemas.isoInstant.parse(
                "2026-09-01T21:30:00.000Z",
              ),
            },
          ],
    programmesTruncated: false,
  };
}
