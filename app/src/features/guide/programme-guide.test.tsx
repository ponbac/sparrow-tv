import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clientSchemas,
  type ChannelGroup,
  type GuideWindowChannel,
} from "../../client/contracts";
import { ProgrammeGuide } from "./programme-guide";
import type { ClockWindow } from "./guide-window";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

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

const GROUPS: readonly ChannelGroup[] = [
  { name: "", channelCount: 18 },
  { name: "News", channelCount: 4 },
  { name: "Cinema", channelCount: 2 },
  { name: "4K (Nordicsubs) (Serie)", channelCount: 556 },
];

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

describe("ProgrammeGuide Channel Groups", () => {
  it("hides excluded groups from the lane while keeping them in the roster", async () => {
    const user = userEvent.setup();
    const onSetGroupExcluded = vi.fn();
    renderGuide({
      groups: GROUPS,
      excludedGroups: new Set(["News"]),
      onSetGroupExcluded,
    });

    const lane = screen.getByRole("radiogroup", { name: "Channel groups" });
    expect(within(lane).getByRole("radio", { name: "All" })).toBeVisible();
    expect(within(lane).getByRole("radio", { name: /Ungrouped/ })).toBeVisible();
    expect(within(lane).queryByRole("radio", { name: /News/ })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Channel Group roster" }));
    expect(await screen.findByRole("heading", { name: "Channel Groups" })).toBeVisible();
    expect(
      screen.getByRole("button", { name: "News, 4 channels" }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Restore News to the board" }));
    expect(onSetGroupExcluded).toHaveBeenCalledWith("News", false);
  });

  it("lets the roster search, select, and exclude Channel Groups", async () => {
    const user = userEvent.setup();
    const onSelectGroup = vi.fn();
    const onSetGroupExcluded = vi.fn();
    renderGuide({
      groups: GROUPS,
      onSelectGroup,
      onSetGroupExcluded,
    });

    await user.click(screen.getByRole("button", { name: "Channel Group roster" }));
    const search = await screen.findByRole("searchbox", {
      name: "Search Channel Groups",
    });
    await user.type(search, "cine");
    expect(
      screen.queryByRole("button", { name: "News, 4 channels" }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Exclude Cinema" }));
    expect(onSetGroupExcluded).toHaveBeenCalledWith("Cinema", true);
    await user.click(screen.getByRole("button", { name: "Cinema, 2 channels" }));
    expect(onSelectGroup).toHaveBeenCalledWith("Cinema");
    expect(screen.queryByRole("heading", { name: "Channel Groups" })).not.toBeInTheDocument();
  });

  it("steps the group lane when Channel Groups overflow", async () => {
    renderGuide({ groups: GROUPS });
    const scroller = screen.getByRole("radiogroup", {
      name: "Channel groups",
    }).parentElement;
    expect(scroller).not.toBeNull();
    mockScrollerOverflow(scroller as HTMLElement, {
      clientWidth: 200,
      scrollWidth: 800,
    });
    fireEvent(window, new Event("resize"));

    const later = await screen.findByRole("button", {
      name: "Later Channel Groups",
    });
    expect(later).toBeEnabled();
    fireEvent.click(later);
    expect((scroller as HTMLElement).scrollLeft).toBeGreaterThan(0);
  });
});

function renderGuide(
  overrides: Partial<{
    readonly groups: readonly ChannelGroup[];
    readonly excludedGroups: ReadonlySet<string>;
    readonly onSelectGroup: (group: string | null) => void;
    readonly onSetGroupExcluded: (name: string, exclude: boolean) => void;
  }> = {},
) {
  render(
    <ProgrammeGuide
      rows={[LONG_CHANNEL_ROW]}
      groups={overrides.groups ?? []}
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
      onSelectGroup={overrides.onSelectGroup ?? vi.fn()}
      onPrefetchGroup={vi.fn()}
      excludedGroups={overrides.excludedGroups ?? new Set()}
      onSetGroupExcluded={overrides.onSetGroupExcluded ?? vi.fn()}
      onRestoreExcludedGroups={vi.fn()}
      onPreparePlayback={vi.fn()}
      onTune={vi.fn()}
      onRetry={vi.fn()}
      onLoadMore={vi.fn()}
      search={<div />}
      feeds={<div />}
    />,
  );
}

function mockScrollerOverflow(
  scroller: HTMLElement,
  size: { readonly clientWidth: number; readonly scrollWidth: number },
): void {
  let scrollLeft = 0;
  Object.defineProperty(scroller, "clientWidth", {
    configurable: true,
    get: () => size.clientWidth,
  });
  Object.defineProperty(scroller, "scrollWidth", {
    configurable: true,
    get: () => size.scrollWidth,
  });
  Object.defineProperty(scroller, "scrollLeft", {
    configurable: true,
    get: () => scrollLeft,
    set: (value: number) => {
      scrollLeft = value;
      scroller.dispatchEvent(new Event("scroll"));
    },
  });
}
