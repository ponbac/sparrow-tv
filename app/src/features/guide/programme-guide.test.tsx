import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
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

describe("ProgrammeGuide TV pagination", () => {
  it.each(["now", "timeline"] as const)("hands focus from More to the next page of a long %s guide", (view) => {
    const rows = Array.from({ length: 90 }, (_, index) => ({
      ...LONG_CHANNEL_ROW,
      channel: channelFixture({ id: `station-${index}`, name: `Station ${index + 1}`, group: "Stations", number: index + 1 }),
    }));
    const guide = renderGuide({ television: true, layout: "pocket", view, rows: rows.slice(0, 60), hasMore: true });
    const more = screen.getByRole("button", { name: "More channels" });
    act(() => more.focus());
    guide.rerender({ loadingMore: true });
    expect(more).toHaveFocus();
    guide.rerender({ rows });
    expect(screen.getByRole("button", { name: "Tune Station 61" })).toHaveFocus();
  });

  it("keeps More channels focused while loading, then focuses the first new row", async () => {
    const user = userEvent.setup();
    const loadMore = vi.fn();
    const view = renderGuide({
      television: true,
      layout: "pocket",
      hasMore: true,
      loadMore,
    });
    const more = screen.getByRole("button", { name: "More channels" });
    more.focus();
    await user.keyboard("{Enter}");
    expect(loadMore).toHaveBeenCalledTimes(1);
    view.rerender({ loadingMore: true });
    expect(more).toHaveFocus();
    expect(more).toHaveAttribute("aria-disabled", "true");
    await user.keyboard("{Enter}");
    expect(loadMore).toHaveBeenCalledTimes(1);

    view.rerender({
      hasMore: false,
      rows: [
        LONG_CHANNEL_ROW,
        {
          ...LONG_CHANNEL_ROW,
          channel: channelFixture({ id: "news", name: "News", group: "News", number: 2 }),
        },
      ],
    });
    expect(screen.getByRole("button", { name: "Tune News" })).toHaveFocus();
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

  it("keeps distant TV groups reachable from the lane and the complete roster", async () => {
    const groups = Array.from({ length: 100 }, (_, index) => ({
      name: `Group ${index + 1}`, channelCount: 1,
    }));
    const { rerender } = renderGuide({ television: true, layout: "pocket", groups });
    const lane = within(screen.getByRole("radiogroup", { name: "Channel groups" }));
    const firstPage = lane.getAllByRole("radio");
    expect(firstPage.length).toBeLessThan(groups.length);
    expect(firstPage[0]).toHaveAttribute("aria-setsize", "101");
    const last = firstPage[firstPage.length - 1];
    act(() => last.focus());
    const next = lane.getAllByRole("radio")[firstPage.length];
    fireEvent.keyDown(last, { key: "ArrowRight" });
    expect(next).toHaveFocus();
    fireEvent.keyDown(next, { key: "ArrowLeft" });
    expect(last).toHaveFocus();

    await userEvent.click(screen.getByRole("button", { name: "Choose groups" }));
    expect(await screen.findByRole("button", { name: "Group 100, 1 channels" })).toBeVisible();
    rerender({ activeGroup: "Group 100" });
    expect(lane.getByRole("radio", { name: /Group 100/, hidden: true })).toHaveAttribute("aria-checked", "true");
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

describe("ProgrammeGuide automatic pagination", () => {
  it("loads more channels when scrolled near the bottom, not at initial scrollTop 0", () => {
    const onLoadMore = vi.fn();
    const { container } = renderGuide({
      layout: "pocket",
      hasMore: true,
      onLoadMore,
    });
    const board = requirePart(container, ".programme-guide__board");
    expect(onLoadMore).not.toHaveBeenCalled();

    Object.defineProperty(board, "clientHeight", {
      configurable: true,
      value: 500,
    });
    Object.defineProperty(board, "scrollHeight", {
      configurable: true,
      value: 2_000,
    });

    // At scrollTop 0 (cold start), scrolling does not eagerly load page 2.
    board.scrollTop = 0;
    fireEvent.scroll(board);
    expect(onLoadMore).not.toHaveBeenCalled();

    // Still far from the bottom: does not trigger yet.
    board.scrollTop = 200;
    fireEvent.scroll(board);
    expect(onLoadMore).not.toHaveBeenCalled();

    // Within 1.5 viewports of the bottom: automatically loads more channels.
    board.scrollTop = 1_100;
    fireEvent.scroll(board);
    expect(onLoadMore).toHaveBeenCalledTimes(1);
  });

  it("loads more channels on D-pad focus near the end without dropping button focus", () => {
    const onLoadMore = vi.fn();
    const { rerender } = renderGuide({
      layout: "pocket",
      hasMore: true,
      onLoadMore,
    });

    fireEvent.focus(
      screen.getByRole("button", { name: `Tune ${LONG_CHANNEL_ROW.channel.name}` }),
    );
    expect(onLoadMore).toHaveBeenCalledTimes(1);

    rerender({
      layout: "pocket",
      hasMore: true,
      loadingMore: true,
      onLoadMore,
    });
    const more = screen.getByRole("button", { name: "Loading more channels…" });
    expect(more).toHaveAttribute("aria-disabled", "true");
    expect(more).not.toBeDisabled();
    fireEvent.click(more);
    expect(onLoadMore).toHaveBeenCalledTimes(1);
  });
});

describe("ProgrammeGuide D-pad and arrow navigation", () => {
  const SVT1_SD: GuideWindowChannel = {
    channel: channelFixture({
      id: "nav-svt1-sd",
      name: "SVT1 SD",
      group: "Sweden",
      number: 1,
      variant: { quality: "sd", baseName: "SVT1" },
    }),
    programmes: [],
    programmesTruncated: false,
  };
  const SVT1_HD: GuideWindowChannel = {
    channel: channelFixture({
      id: "nav-svt1-hd",
      name: "SVT1 HD",
      group: "Sweden",
      number: 1,
      variant: { quality: "hd", baseName: "SVT1" },
    }),
    programmes: [],
    programmesTruncated: false,
  };
  const SVT2_HD: GuideWindowChannel = {
    channel: channelFixture({
      id: "nav-svt2-hd",
      name: "SVT2 HD",
      group: "Sweden",
      number: 2,
      variant: { quality: "hd", baseName: "SVT2" },
    }),
    programmes: [],
    programmesTruncated: false,
  };
  const SVT2_FHD: GuideWindowChannel = {
    channel: channelFixture({
      id: "nav-svt2-fhd",
      name: "SVT2 FHD",
      group: "Sweden",
      number: 2,
      variant: { quality: "fhd", baseName: "SVT2" },
    }),
    programmes: [],
    programmesTruncated: false,
  };

  it("moves straight down from a Channel Group chip into the channels without switching groups", () => {
    const onSelectGroup = vi.fn();
    renderGuide({
      layout: "pocket",
      groups: GROUPS,
      rows: [SVT1_SD, SVT1_HD, SVT2_HD, SVT2_FHD],
      onSelectGroup,
    });

    const lane = screen.getByRole("radiogroup", { name: "Channel groups" });
    const allChip = within(lane).getByRole("radio", { name: "All" });
    allChip.focus();

    fireEvent.keyDown(allChip, { key: "ArrowDown" });

    const svt1Button = screen.getByRole("button", { name: "Tune SVT1" });
    expect(svt1Button).toHaveFocus();
    expect(onSelectGroup).not.toHaveBeenCalled();

    // Moving down again steps straight to the next channel, skipping inline quality chips.
    fireEvent.keyDown(svt1Button, { key: "ArrowDown" });
    const svt2Button = screen.getByRole("button", { name: "Tune SVT2" });
    expect(svt2Button).toHaveFocus();

    // Moving back up returns to the first channel, then straight back to the active group chip.
    fireEvent.keyDown(svt2Button, { key: "ArrowUp" });
    expect(svt1Button).toHaveFocus();
    fireEvent.keyDown(svt1Button, { key: "ArrowUp" });
    expect(allChip).toHaveFocus();
  });

  it("moves down from Choose groups into channels and keeps inline quality chips out of tab/D-pad order", () => {
    renderGuide({
      layout: "pocket",
      groups: GROUPS,
      rows: [SVT1_SD, SVT1_HD, SVT2_HD, SVT2_FHD],
    });

    for (const chip of within(
      screen.getAllByRole("group", { name: "Picture quality" })[0],
    ).getAllByRole("button")) {
      expect(chip).toHaveAttribute("tabindex", "-1");
    }

    const rosterButton = screen.getByRole("button", { name: "Choose groups" });
    rosterButton.focus();
    fireEvent.keyDown(rosterButton, { key: "ArrowDown" });
    expect(screen.getByRole("button", { name: "Tune SVT1" })).toHaveFocus();
  });

  it("prefers toolbar buttons on ArrowUp to avoid popping up TV keyboards, while still letting ArrowLeft reach search and ArrowDown leave it", () => {
    render(
      <ProgrammeGuide
        rows={[SVT1_SD, SVT1_HD]}
        groups={GROUPS}
        activeGroup={null}
        window={WINDOW}
        now={NOW}
        playingChannel={null}
        variantPreferences={new Map()}
        layout="pocket"
        mode="guide"
        time={null}
        loading={false}
        replacing={false}
        error={null}
        hasMore={false}
        loadingMore={false}
        onSelectGroup={vi.fn()}
        onPrefetchGroup={vi.fn()}
        excludedGroups={new Set()}
        onSetGroupExcluded={vi.fn()}
        onRestoreExcludedGroups={vi.fn()}
        onPreparePlayback={vi.fn()}
        onTune={vi.fn()}
        onTuneVariant={vi.fn()}
        onRetry={vi.fn()}
        onLoadMore={vi.fn()}
        search={
          <input
            type="search"
            aria-label="Search channels"
            aria-expanded="false"
            defaultValue=""
          />
        }
        feeds={<button type="button">Sources</button>}
        status={null}
      />,
    );

    const lane = screen.getByRole("radiogroup", { name: "Channel groups" });
    const allChip = within(lane).getByRole("radio", { name: "All" });
    const sourcesButton = screen.getByRole("button", { name: "Sources" });
    const searchInput = screen.getByRole("searchbox", {
      name: "Search channels",
    });

    allChip.focus();
    fireEvent.keyDown(allChip, { key: "ArrowUp" });
    expect(sourcesButton).toHaveFocus();

    fireEvent.keyDown(sourcesButton, { key: "ArrowLeft" });
    expect(searchInput).toHaveFocus();

    fireEvent.keyDown(searchInput, { key: "ArrowDown" });
    expect(allChip).toHaveFocus();
  });
});

interface GuideOverrides {
  readonly television?: boolean;
  readonly hasMore?: boolean;
  readonly loadingMore?: boolean;
  readonly loadMore?: () => void;
  /** The timeline unless a test is about pocket's list. */
  readonly layout?: "theater" | "pocket";
  readonly mode?: "watch" | "guide";
  readonly view?: "now" | "timeline";
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
  readonly onLoadMore?: () => void;
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
      television={overrides.television ?? false}
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
      view={overrides.view === undefined ? null : { current: overrides.view, onToggle: vi.fn() }}
      loading={false}
      replacing={overrides.replacing ?? false}
      error={null}
      hasMore={overrides.hasMore ?? false}
      loadingMore={overrides.loadingMore ?? false}
      onSelectGroup={overrides.onSelectGroup ?? vi.fn()}
      onPrefetchGroup={vi.fn()}
      excludedGroups={overrides.excludedGroups ?? new Set()}
      onSetGroupExcluded={overrides.onSetGroupExcluded ?? vi.fn()}
      onRestoreExcludedGroups={vi.fn()}
      onPreparePlayback={vi.fn()}
      onTune={overrides.onTune ?? vi.fn()}
      onTuneVariant={overrides.onTuneVariant ?? vi.fn()}
      onRetry={vi.fn()}
      onLoadMore={overrides.onLoadMore ?? overrides.loadMore ?? vi.fn()}
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

describe("TV Guide browsing through a long list", () => {
  it("keeps the focused origin through a virtual-window jump when six Down presses wait for a page", () => {
    const rows = Array.from({ length: 65 }, (_, index) => ({
      channel: channelFixture({ id: `number-${index}`, name: `Number ${index + 1}`, group: "North", number: index + 1 }),
      programmes: [], programmesTruncated: false,
    }));
    const height = vi.spyOn(Element.prototype, "clientHeight", "get").mockImplementation(function (this: Element) {
      return this.matches(".programme-guide__board") ? 180 : 0;
    });
    const bounds = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      return DOMRect.fromRect({ height: this.matches(".programme-guide__row") ? 60 : 0 });
    });
    try {
      const firstPage = rows.slice(0, 17);
      const guide = renderGuide({ layout: "pocket", television: true, view: "now", hasMore: true, rows: firstPage });
      const origin = screen.getByRole("button", { name: "Tune Number 16" });
      act(() => origin.focus());
      for (let press = 0; press < 6; press++) fireEvent.keyDown(origin, { key: "ArrowDown" });
      expect(origin).toHaveFocus();

      guide.rerender({ rows, hasMore: false });
      expect(screen.getByRole("button", { name: "Tune Number 34" })).toHaveFocus();
    } finally {
      bounds.mockRestore();
      height.mockRestore();
    }
  });

  it.each(["now", "timeline"] as const)("moves through the %s view and requests more only near the loaded end", (view) => {
    const columns = view === "now" ? 3 : 1;
    const rows = Array.from({ length: 121 }, (_, index) => ({
      channel: channelFixture({ id: `north-${index}`, name: `North ${index + 1}`, group: "North", number: index + 1 }),
      programmes: [programme("On now", 0, 60)], programmesTruncated: false,
    }));
    const loadMore = vi.fn();
    // jsdom supplies no geometry. The TV has room for four 60px lines.
    const height = vi.spyOn(Element.prototype, "clientHeight", "get").mockImplementation(function (this: Element) {
      return this.matches(".programme-guide__board") ? 240 : 0;
    });
    const bounds = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const board = this.closest<HTMLElement>(".programme-guide__board");
      const row = this.closest<HTMLElement>(".programme-guide__row");
      if (board === this) return DOMRect.fromRect({ width: 960, height: 240 });
      if (row && board) {
        const index = Number(row.dataset.number) - 1;
        return DOMRect.fromRect({
          x: index % columns * 960 / columns,
          y: Math.floor(index / columns) * 60 - board.scrollTop,
          width: 960 / columns, height: 60,
        });
      }
      return DOMRect.fromRect();
    });
    try {
      const guide = renderGuide({ layout: "pocket", television: true, view, rows, hasMore: true, onLoadMore: loadMore });
      const board = requirePart(guide.container, ".programme-guide__board");
      const focusedNumber = () => Number(document.activeElement?.closest<HTMLElement>(".programme-guide__row")?.dataset.number);
      const move = (key: "ArrowDown" | "ArrowUp", expected: number) => {
        fireEvent.keyDown(document.activeElement ?? document.body, { key });
        expect(focusedNumber()).toBe(expected);
      };
      const start = view === "timeline"
        ? requirePart(board, ".programme-guide__programme")
        : screen.getByRole("button", { name: "Tune North 1" });
      act(() => start.focus());
      expect(board.querySelectorAll(".programme-guide__row").length).toBeLessThan(rows.length);
      for (let n = 1 + columns; n <= 61; n += columns) move("ArrowDown", n);
      move("ArrowUp", 61 - columns);
      expect(loadMore).not.toHaveBeenCalled();
      expect(board.querySelectorAll(".programme-guide__row").length).toBeLessThan(rows.length);
      for (let n = 61; n <= 121; n += columns) move("ArrowDown", n);
      expect(loadMore).toHaveBeenCalled();
      move("ArrowUp", 121 - columns);
      if (view === "timeline") expect(document.activeElement).toHaveClass("programme-guide__programme");

      const south = channelFixture({ id: "south", name: "South Coast", group: "South", number: 200 });
      guide.rerender({ activeGroup: "South", hasMore: false, rows: [{ channel: south, programmes: [], programmesTruncated: false }] });
      expect(screen.getByRole("button", { name: "Tune South Coast" })).toBeVisible();
      expect(board.querySelectorAll(".programme-guide__row")).toHaveLength(1);
    } finally {
      bounds.mockRestore();
      height.mockRestore();
    }
  });
});
