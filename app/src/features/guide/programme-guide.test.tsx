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
import type { GuideTime } from "./channel-group-lane";
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

describe("ProgrammeGuide pocket list", () => {
  const SD = variant("svt1", "SVT1", "sd", 101);
  const HD = variant("svt1", "SVT1", "hd", 101, [
    programme("Rapport", -15, 45),
    programme("Sportnytt", 45, 75),
  ]);
  const LONE_HD = variant("eurosport", "Eurosport", "hd", 102, [
    programme("Snooker", 0, 120),
  ]);

  it("lists what each Channel has on, what follows and how long is left, with no timeline", () => {
    const view = renderGuide({
      layout: "pocket",
      rows: [SD, HD, LONE_HD],
      playingChannel: HD.channel.id,
    });

    const svt1 = requireRow(screen.getByRole("button", { name: "Tune SVT1" }));
    expect(svt1).toHaveAttribute("data-body", "list");
    expect(svt1).toHaveAttribute("data-playing", "true");
    // The preferred variant has no guide data, so the row shows its sibling's.
    expect(rowText(svt1)).toEqual({
      number: "101",
      name: "SVT1",
      on: "Rapport",
      after: `${clockLabel(minutesFromStart(45))} Sportnytt`,
      left: "30 min left",
      progress: "50%",
    });
    // The button is named by its Channel alone; a screen reader hears what
    // is on as its description.
    expect(
      screen.getByRole("button", { name: "Tune SVT1" }),
    ).toHaveAccessibleDescription(
      `Rapport ${clockLabel(minutesFromStart(45))} Sportnytt 30 min left`,
    );
    // A lone Quality Variant has nothing to switch to: its quality is a tag.
    expect(
      rowText(requireRow(screen.getByRole("button", { name: "Tune Eurosport" }))),
    ).toEqual({
      number: "102",
      name: "EurosportHD",
      on: "Snooker",
      after: null,
      left: "105 min left",
      progress: "12.5%",
    });

    expect(view.container.querySelector("time")).not.toBeInTheDocument();
    expect(
      view.container.querySelector(".programme-guide__playhead"),
    ).not.toBeInTheDocument();
    // Nothing in the list is a Programme cell: every button tunes by name.
    expect(
      within(requirePart(view.container, ".programme-guide__rows"))
        .getAllByRole("button")
        .map((button) => button.getAttribute("aria-label")),
    ).toEqual([
      "Tune SVT1",
      "Tune SVT1 SD",
      "Tune SVT1 HD",
      "Tune Eurosport",
    ]);
  });

  it("marks one button per Channel: a lone Channel's row, or each chip of a quality switch", async () => {
    const user = userEvent.setup();
    const onTune = vi.fn();
    const onTuneVariant = vi.fn();
    const view = renderGuide({
      layout: "pocket",
      rows: [SD, HD, LONE_HD, LONG_CHANNEL_ROW],
      playingChannel: HD.channel.id,
      onTune,
      onTuneVariant,
    });

    expect(
      Array.from(
        view.container.querySelectorAll("button[data-acceptance-channel]"),
        (button) => [
          button.getAttribute("aria-label"),
          button.getAttribute("aria-pressed"),
        ],
      ),
    ).toEqual([
      ["Tune SVT1 SD", "false"],
      ["Tune SVT1 HD", "true"],
      ["Tune Eurosport", "false"],
      [`Tune ${LONG_CHANNEL_NAME}`, "false"],
    ]);

    await user.click(screen.getByRole("button", { name: "Tune SVT1" }));
    expect(onTune).toHaveBeenLastCalledWith(HD.channel);
    await user.click(screen.getByRole("button", { name: "Tune SVT1 SD" }));
    expect(onTuneVariant).toHaveBeenCalledWith(SD.channel);
    expect(onTune).toHaveBeenCalledTimes(1);
  });

  it("shows what each Channel has on at a chosen time, without progress", () => {
    renderGuide({
      layout: "pocket",
      rows: [SD, HD, LONE_HD],
      time: timeChoice({ chosen: minutesFromStart(60) }),
    });

    expect(
      rowText(requireRow(screen.getByRole("button", { name: "Tune SVT1" }))),
    ).toEqual({
      number: "101",
      name: "SVT1",
      on: "Sportnytt",
      after: `${clockLabel(minutesFromStart(45))} to ${clockLabel(minutesFromStart(75))}`,
      left: null,
      progress: null,
    });
  });

  it("titles a row by its Channel when nothing is listed, and claims nothing while the rows are being replaced", () => {
    const view = renderGuide({
      layout: "pocket",
      rows: [SD, HD, LONG_CHANNEL_ROW],
      time: timeChoice({ chosen: minutesFromStart(90) }),
    });
    const afterOf = (name: string) =>
      rowText(requireRow(screen.getByRole("button", { name: `Tune ${name}` })));

    expect(afterOf("SVT1")).toMatchObject({
      name: "",
      on: "SVT1",
      after: `Nothing listed at ${clockLabel(minutesFromStart(90))}`,
    });
    expect(afterOf(LONG_CHANNEL_NAME)).toMatchObject({
      name: "",
      on: LONG_CHANNEL_NAME,
      after: "No guide data. The channel still plays.",
    });

    // A longer read is on its way: it may yet list something at that time.
    view.rerender({ replacing: true });
    expect(afterOf("SVT1")).toMatchObject({ on: "SVT1", after: null });
    expect(afterOf(LONG_CHANNEL_NAME)).toMatchObject({ after: null });
  });

  it("says the guide is not loaded for a time past the Programmes a Channel's row was cut to", () => {
    // The catalog keeps the earliest hundred Programmes of a Channel: with
    // three minutes each they end five hours into the window.
    const shorts = Array.from({ length: 100 }, (_, index) =>
      programme(`Short ${index + 1}`, index * 3, index * 3 + 3),
    );
    renderGuide({
      layout: "pocket",
      rows: [
        { ...variant("clips", "Clips", "hd", 103, shorts), programmesTruncated: true },
        variant("eurosport", "Eurosport", "hd", 102, shorts),
      ],
      time: timeChoice({ chosen: minutesFromStart(360) }),
    });
    const afterOf = (name: string) =>
      rowText(requireRow(screen.getByRole("button", { name: `Tune ${name}` })));

    expect(afterOf("Clips")).toMatchObject({
      on: "Clips",
      after: "The guide for this time is not loaded.",
    });
    // The same Programmes, known to be all there are: nothing is on then.
    expect(afterOf("Eurosport")).toMatchObject({
      on: "Eurosport",
      after: `Nothing listed at ${clockLabel(minutesFromStart(360))}`,
    });
  });

  it("offers the times behind a chip, apart from the Channel Groups", async () => {
    const user = userEvent.setup();
    const onToggle = vi.fn();
    const onChoose = vi.fn();
    const later = [minutesFromStart(60), minutesFromStart(120)];
    const view = renderGuide({
      layout: "pocket",
      groups: GROUPS,
      time: timeChoice({ options: later, onToggle, onChoose }),
    });

    const chip = screen.getByRole("button", { name: "Now", expanded: false });
    expect(chip).toHaveAttribute("aria-pressed", "false");
    expect(screen.queryByRole("group", { name: "Time" })).not.toBeInTheDocument();
    await user.click(chip);
    expect(onToggle).toHaveBeenCalledTimes(1);

    view.rerender({
      time: timeChoice({ open: true, chosen: later[1], options: later, onToggle, onChoose }),
    });
    // The chip says which time the list shows, whether or not the row is open.
    const chosen = screen.getByRole("button", {
      name: clockLabel(minutesFromStart(120)),
      expanded: true,
    });
    expect(chosen).toHaveAttribute("aria-pressed", "true");
    const times = screen.getByRole("group", { name: "Time" });
    expect(
      within(times)
        .getAllByRole("button")
        .map((button) => [button.textContent, button.getAttribute("aria-pressed")]),
    ).toEqual([
      ["Now", "false"],
      [clockLabel(minutesFromStart(60)), "false"],
      [clockLabel(minutesFromStart(120)), "true"],
    ]);
    // Android acceptance counts the group buttons: a time is not one of them.
    expect(times.querySelector("[data-acceptance-group]")).toBeNull();
    expect(
      screen.getByRole("radiogroup", { name: "Channel groups" }),
    ).not.toContainElement(chip);

    await user.click(
      within(times).getByRole("button", { name: clockLabel(minutesFromStart(60)) }),
    );
    expect(onChoose).toHaveBeenLastCalledWith(later[0]);
    await user.click(within(times).getByRole("button", { name: "Now" }));
    expect(onChoose).toHaveBeenLastCalledWith(null);
  });

  it("has no time chip where the guide is a timeline", () => {
    renderGuide({ groups: GROUPS });

    expect(screen.queryByRole("button", { name: "Now" })).not.toBeInTheDocument();
  });

  it("brings the playing row to the middle of the list on entering guide mode, and leaves a row in view alone", () => {
    const rows = [SD, HD, LONE_HD, LONG_CHANNEL_ROW];
    const view = renderGuide({
      layout: "pocket",
      mode: "watch",
      rows,
      playingChannel: LONG_CHANNEL_ROW.channel.id,
    });
    // jsdom lays nothing out: a 300 px view over three 100 px rows.
    const board = requirePart(view.container, ".programme-guide__board");
    layOut(board, { top: () => 0, height: 300 });
    Array.from(
      view.container.querySelectorAll<HTMLElement>(".programme-guide__row"),
      (row, index) =>
        layOut(row, { top: () => index * 100 - board.scrollTop, height: 100 }),
    );

    view.rerender({ mode: "guide" });
    // The third row, with 100 px of the view above and below it.
    expect(board.scrollTop).toBe(100);

    // The Channel chosen from the list is in view: the list stays put.
    view.rerender({ mode: "guide", playingChannel: LONE_HD.channel.id });
    expect(board.scrollTop).toBe(100);

    // One that is cut off is brought to the middle.
    view.rerender({ mode: "guide", playingChannel: HD.channel.id });
    expect(board.scrollTop).toBe(0);
  });

  it("starts another Channel Group at the top of the list", () => {
    const view = renderGuide({ layout: "pocket", groups: GROUPS });
    const board = requirePart(view.container, ".programme-guide__board");
    board.scrollTop = 400;

    view.rerender({ now: new Date(NOW.getTime() + 30_000) });
    expect(board.scrollTop).toBe(400);

    view.rerender({ activeGroup: "News" });
    expect(board.scrollTop).toBe(0);
  });

  function variant(
    id: string,
    baseName: string,
    quality: ChannelQuality,
    number: number,
    programmes: readonly GuideProgramme[] = [],
  ): GuideWindowChannel {
    return {
      channel: channelFixture({
        id: `${id}-${quality}`,
        name: `${baseName} ${quality.toUpperCase()}`,
        group: "Sweden",
        number,
        variant: { quality, baseName },
      }),
      programmes,
      programmesTruncated: false,
    };
  }

  function timeChoice(overrides: Partial<GuideTime> = {}): GuideTime {
    return {
      open: false,
      chosen: null,
      options: [],
      onToggle: vi.fn(),
      onChoose: vi.fn(),
      ...overrides,
    };
  }

  /** What one list row shows, part by part; null for a part it leaves out. */
  function rowText(row: HTMLElement) {
    const text = (part: string) =>
      row.querySelector(`.programme-guide__${part}`)?.textContent ?? null;
    return {
      number: text("number"),
      name: text("name"),
      on: text("on"),
      after: text("after"),
      left: text("left"),
      progress:
        row
          .querySelector<HTMLElement>(".programme-guide__elapsed")
          ?.style.getPropertyValue("--progress") ?? null,
    };
  }

  /** Gives an element the box a browser would have laid out for it. */
  function layOut(
    element: HTMLElement,
    box: { readonly top: () => number; readonly height: number },
  ): void {
    Object.defineProperty(element, "clientHeight", {
      configurable: true,
      value: box.height,
    });
    element.getBoundingClientRect = () =>
      DOMRect.fromRect({ y: box.top(), height: box.height });
  }
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
  /** The timeline unless a test is about pocket's list. */
  readonly layout?: "theater" | "pocket";
  readonly mode?: "watch" | "guide";
  readonly time?: GuideTime;
  readonly replacing?: boolean;
  readonly rows?: readonly GuideWindowChannel[];
  readonly groups?: readonly ChannelGroup[];
  readonly activeGroup?: string;
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
      activeGroup={overrides.activeGroup ?? null}
      window={overrides.window ?? WINDOW}
      now={overrides.now ?? NOW}
      playingChannel={overrides.playingChannel ?? null}
      variantPreferences={overrides.variantPreferences ?? new Map()}
      layout={overrides.layout ?? "theater"}
      mode={overrides.mode ?? "guide"}
      time={overrides.time ?? null}
      loading={false}
      replacing={overrides.replacing ?? false}
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

function requirePart(container: HTMLElement, selector: string): HTMLElement {
  const part = container.querySelector<HTMLElement>(selector);
  if (part === null) {
    throw new Error(`expected ${selector} in the guide`);
  }
  return part;
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
