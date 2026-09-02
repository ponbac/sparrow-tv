import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clientSchemas, type GuideWindowChannel } from "../../client/contracts";
import { ProgrammeGuide } from "./programme-guide";
import type { ClockWindow } from "./guide-window";

afterEach(cleanup);

const WINDOW: ClockWindow = {
  startsAt: new Date("2026-09-01T08:00:00.000Z"),
  endsAt: new Date("2026-09-01T11:00:00.000Z"),
};

const LONG_CHANNEL_NAME = "[4K] 1883 (Nordicsubs) (Serie)";

const LONG_CHANNEL_ROW: GuideWindowChannel = {
  channel: clientSchemas.channel.parse({
    id: "serie-1883",
    name: LONG_CHANNEL_NAME,
    group: "4K (Nordicsubs) (Serie)",
  }),
  programmes: [],
  programmesTruncated: false,
};

describe("ProgrammeGuide channel names", () => {
  it("keeps the full Channel name in the row and reveals it in a tooltip on hover", async () => {
    const user = userEvent.setup();
    renderGuide();

    const tune = screen.getByRole("button", {
      name: `Tune ${LONG_CHANNEL_NAME}`,
    });
    expect(tune).toHaveTextContent(LONG_CHANNEL_NAME);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();

    await user.hover(tune);

    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      LONG_CHANNEL_NAME,
    );
  });
});

function renderGuide() {
  render(
    <ProgrammeGuide
      rows={[LONG_CHANNEL_ROW]}
      groups={[]}
      activeGroup={null}
      window={WINDOW}
      now={new Date("2026-09-01T08:15:00.000Z")}
      selection={null}
      playingChannel={null}
      loading={false}
      replacing={false}
      error={null}
      hasMore={false}
      loadingMore={false}
      onSelectGroup={vi.fn()}
      onPrefetchGroup={vi.fn()}
      onPreparePlayback={vi.fn()}
      onTune={vi.fn()}
      onRetry={vi.fn()}
      onLoadMore={vi.fn()}
      search={<div />}
      feeds={<div />}
    />,
  );
}
