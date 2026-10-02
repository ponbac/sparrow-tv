import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clientSchemas,
  type ChannelGroup,
  type ChannelId,
  type ChannelQuality,
  type ChannelSummary,
  type GuideProgramme,
  type GuideWindowChannel,
} from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import { familyKey, type VariantPreferences } from "./guide-families";
import { ProgrammeGuide } from "./programme-guide";
import { clockLabel, type ClockWindow } from "./guide-window";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

const WINDOW: ClockWindow = {
  startsAt: new Date("2026-09-01T08:00:00.000Z"),
  endsAt: new Date("2026-09-01T11:00:00.000Z"),
};

const NOW = new Date("2026-09-01T08:15:00.000Z");

const LONG_CHANNEL_NAME = "[4K] 1883 (Nordicsubs) (Serie)";

const LONG_CHANNEL_ROW: GuideWindowChannel = {
  channel: channelFixture({
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

describe("ProgrammeGuide Quality Variants", () => {
  const SD = svt1("sd");
  const HD = svt1("hd");
  const FHD = svt1("fhd");

  it("shows the variants of one Channel as one numbered row with a quality switch", () => {
    renderGuide({ rows: [SD, HD, FHD, LONG_CHANNEL_ROW], playingChannel: HD.channel.id });

    const row = requireRow(screen.getByRole("button", { name: "Tune SVT1" }));
    expect(row).toHaveAttribute("data-playing", "true");
    expect(within(row).getByText(String(SD.channel.number))).toBeInTheDocument();
    const chips = within(
      within(row).getByRole("group", { name: "Picture quality" }),
    ).getAllByRole("button");
    expect(
      chips.map((chip) => [
        chip.textContent,
        chip.getAttribute("aria-label"),
        chip.getAttribute("aria-pressed"),
      ]),
    ).toEqual([
      ["SD", "Tune SVT1 SD", "false"],
      ["HD", "Tune SVT1 HD", "true"],
      ["FHD", "Tune SVT1 FHD", "false"],
    ]);
    expect(
      within(
        requireRow(screen.getByRole("button", { name: `Tune ${LONG_CHANNEL_NAME}` })),
      ).queryByRole("group", { name: "Picture quality" }),
    ).not.toBeInTheDocument();
  });

  it("tunes the preferred variant from the row and its cells, and any variant from its chip", async () => {
    const user = userEvent.setup();
    const onTune = vi.fn();
    const onTuneVariant = vi.fn();
    const view = renderGuide({
      rows: [SD, svt1("hd", [programme("Bulletin", 0, 60)]), FHD],
      onTune,
      onTuneVariant,
    });

    await user.click(screen.getByRole("button", { name: "Tune SVT1" }));
    expect(onTune).toHaveBeenLastCalledWith(FHD.channel);
    // The preferred variant has no guide data, so the row shows its sibling's.
    await user.click(screen.getByRole("button", { name: /^Bulletin, .*, SVT1$/u }));
    expect(onTune).toHaveBeenLastCalledWith(FHD.channel);

    await user.click(screen.getByRole("button", { name: "Tune SVT1 SD" }));
    expect(onTuneVariant).toHaveBeenCalledWith(SD.channel);
    expect(onTune).toHaveBeenCalledTimes(2);

    view.rerender({
      variantPreferences: new Map([
        [familyKey({ group: "Sweden", title: "SVT1" }), "sd"],
      ]),
    });
    await user.click(screen.getByRole("button", { name: "Tune SVT1" }));
    expect(onTune).toHaveBeenLastCalledWith(SD.channel);
  });

  function svt1(
    quality: ChannelQuality,
    programmes: readonly GuideProgramme[] = [],
  ): GuideWindowChannel {
    return {
      channel: channelFixture({
        id: `svt1-${quality}`,
        name: `SVT1 ${quality.toUpperCase()}`,
        group: "Sweden",
        number: 101,
        variant: { quality, baseName: "SVT1" },
      }),
      programmes,
      programmesTruncated: false,
    };
  }
});

describe("ProgrammeGuide timeline", () => {
  it("gives a cell its time range only when half an hour of it is in view", () => {
    renderGuide({
      rows: [
        {
          ...LONG_CHANNEL_ROW,
          programmes: [
            programme("Lead-in", -30, 29),
            programme("Feature", 29, 59),
          ],
        },
      ],
    });

    const leadIn = screen.getByRole("button", { name: /^Lead-in,/u });
    const feature = screen.getByRole("button", { name: /^Feature,/u });
    expect(leadIn).toHaveTextContent(/^Lead-in$/u);
    expect(feature).toHaveTextContent(
      `Feature${clockLabel(minutesFromStart(29))} to ${clockLabel(minutesFromStart(59))}`,
    );
  });

  it("marks the hours on the ruler and puts the clock on the now-line", () => {
    const startsAt = new Date(2026, 8, 1, 20, 30);
    const now = new Date(2026, 8, 1, 20, 47);
    const view = renderGuide({
      window: {
        startsAt,
        endsAt: new Date(startsAt.getTime() + 3 * 60 * 60 * 1_000),
      },
      now,
    });

    const marks = Array.from(view.container.querySelectorAll("time"));
    expect(
      marks.map((mark) => [mark.textContent, mark.classList.contains("is-hour")]),
    ).toEqual([
      ["20:30", false],
      ["21:00", true],
      ["21:30", false],
      ["22:00", true],
      ["22:30", false],
      ["23:00", true],
    ]);
    expect(
      view.container.querySelector(".programme-guide__now"),
    ).toHaveTextContent(/^20:47$/u);
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

    await user.click(screen.getByRole("button", { name: "Choose groups" }));
    expect(await screen.findByRole("heading", { name: "Channel groups" })).toBeVisible();
    expect(
      screen.getByRole("button", { name: "News, 4 channels" }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Show News" }));
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

    await user.click(screen.getByRole("button", { name: "Choose groups" }));
    const search = await screen.findByRole("searchbox", {
      name: "Search channel groups",
    });
    await user.type(search, "cine");
    expect(
      screen.queryByRole("button", { name: "News, 4 channels" }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Hide Cinema" }));
    expect(onSetGroupExcluded).toHaveBeenCalledWith("Cinema", true);
    await user.click(screen.getByRole("button", { name: "Cinema, 2 channels" }));
    expect(onSelectGroup).toHaveBeenCalledWith("Cinema");
    expect(screen.queryByRole("heading", { name: "Channel groups" })).not.toBeInTheDocument();
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

interface GuideOverrides {
  readonly rows?: readonly GuideWindowChannel[];
  readonly groups?: readonly ChannelGroup[];
  readonly window?: ClockWindow;
  readonly now?: Date;
  readonly playingChannel?: ChannelId;
  readonly variantPreferences?: VariantPreferences;
  readonly excludedGroups?: ReadonlySet<string>;
  readonly onSelectGroup?: (group: string | null) => void;
  readonly onSetGroupExcluded?: (name: string, exclude: boolean) => void;
  readonly onTune?: (channel: ChannelSummary) => void;
  readonly onTuneVariant?: (channel: ChannelSummary) => void;
}

function renderGuide(overrides: GuideOverrides = {}) {
  const view = render(guide(overrides));
  return {
    container: view.container,
    /** Renders again with these overrides on top of the first ones. */
    rerender: (next: GuideOverrides) =>
      view.rerender(guide({ ...overrides, ...next })),
  };
}

function guide(overrides: GuideOverrides) {
  return (
    <ProgrammeGuide
      rows={overrides.rows ?? [LONG_CHANNEL_ROW]}
      groups={overrides.groups ?? []}
      activeGroup={null}
      window={overrides.window ?? WINDOW}
      now={overrides.now ?? NOW}
      playingChannel={overrides.playingChannel ?? null}
      variantPreferences={overrides.variantPreferences ?? new Map()}
      layout="stacked"
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
      onTune={overrides.onTune ?? vi.fn()}
      onTuneVariant={overrides.onTuneVariant ?? vi.fn()}
      onRetry={vi.fn()}
      onLoadMore={vi.fn()}
      search={<div />}
      feeds={<div />}
      status={null}
    />
  );
}

/** A Programme placed in minutes from the start of the default window. */
function programme(
  title: string,
  startsAfterMinutes: number,
  endsAfterMinutes: number,
): GuideProgramme {
  return {
    title,
    titleTruncated: false,
    startsAt: clientSchemas.isoInstant.parse(
      minutesFromStart(startsAfterMinutes).toISOString(),
    ),
    endsAt: clientSchemas.isoInstant.parse(
      minutesFromStart(endsAfterMinutes).toISOString(),
    ),
  };
}

function minutesFromStart(minutes: number): Date {
  return new Date(WINDOW.startsAt.getTime() + minutes * 60_000);
}

function requireRow(element: HTMLElement): HTMLElement {
  const row = element.closest<HTMLElement>(".programme-guide__row");
  if (row === null) {
    throw new Error("expected the element inside a guide row");
  }
  return row;
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
