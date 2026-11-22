import { QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createRef, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clientSchemas,
  type CatalogStatus,
  type ChannelGroup,
  type GuideWindowChannel,
  type InstalledSparrowClient,
} from "../../client/contracts";
import { createSparrowQueryClient } from "../../client/query-client";
import { androidPortFixture } from "../../test/android-port-fixture";
import { channelFixture } from "../../test/channel-fixture";
import { BoardSearch } from "../guide/board-search";
import { BoardSearchDesk } from "../guide/board-search-desk";
import { FeedsDialog } from "../guide/feeds-dialog";
import { GroupRosterDialog } from "../guide/group-roster-dialog";
import { ProgrammeGuide } from "../guide/programme-guide";
import { createAndroidPlayerBridge } from "../playback/android-player-bridge";
import { InstalledPlayer } from "../playback/installed-player";
import { PlaybackLoadBoundary } from "../playback/playback-load-boundary";
import { PlaybackSurface } from "../playback/playback-surface";
import { ChannelBar } from "./channel-bar";
import { NowPlaying } from "./now-playing";
import { Stage } from "./stage";
import { StageChromeProvider, type StageChrome } from "./stage-chrome";
import { showTelevisionPicture } from "./stage-dom";
import { useStageKeys } from "./use-stage-keys";
import { useTelevisionControls } from "./use-television-controls";

const NOW = new Date("2026-08-30T10:15:00Z");
const WINDOW = {
  startsAt: new Date("2026-08-30T10:00:00Z"),
  endsAt: new Date("2026-08-30T13:00:00Z"),
};
const GROUPS: readonly ChannelGroup[] = [
  { name: "News", channelCount: 4 },
  { name: "Sports", channelCount: 4 },
  { name: "Movies", channelCount: 4 },
];

function makeRows(count: number, startNumber = 1): GuideWindowChannel[] {
  return Array.from({ length: count }, (_, index) => {
    const number = startNumber + index;
    return {
      channel: channelFixture({
        id: `ch-${number}`,
        name: `Channel ${number}`,
        group: GROUPS[index % GROUPS.length].name,
        number,
      }),
      programmes: [],
      programmesTruncated: false,
    };
  });
}

function giveBox(element: Element, rect: DOMRect): void {
  Object.defineProperty(element, "getClientRects", {
    configurable: true,
    value: () => [rect],
  });
  Object.defineProperty(element, "getBoundingClientRect", {
    configurable: true,
    value: () => rect,
  });
}

/** The Timeline row whose Channel button is named `Tune ${name}`. */
function requireTimelineRow(name: string): HTMLElement {
  const row = screen
    .getByRole("button", { name: `Tune ${name}` })
    .closest<HTMLElement>(".programme-guide__row");
  if (row === null) throw new Error(`no row for ${name}`);
  return row;
}

afterEach(() => {
  cleanup();
  if (vi.isFakeTimers()) {
    act(() => {
      vi.runOnlyPendingTimers();
    });
    vi.useRealTimers();
  }
  document.documentElement.removeAttribute("data-television");
  document.documentElement.style.removeProperty("--pocket-pinned-w");
  document.documentElement.style.removeProperty("--pocket-pinned-h");
  Reflect.deleteProperty(document, "fullscreenElement");
  Reflect.deleteProperty(document, "exitFullscreen");
});

describe("Shield TV integrated D-pad navigation regressions", () => {
  it("keeps open time row reachable from Channel Groups and row 0, and reaches docked preview on ArrowUp", () => {
    document.documentElement.setAttribute("data-television", "");
    const onSelectGroup = vi.fn();
    const onShowPicture = vi.fn();

    function GuideWithTv() {
      const [timeOpen, setTimeOpen] = useState(true);
      const [chosenTime, setChosenTime] = useState<Date | null>(null);
      return (
        <main className="shell__workspace">
          <Stage
            television
            playingChannel={channelFixture({ id: "ch-1", name: "Channel 1", group: "News" }).id}
            player={null}
            info={null}
            rail={null}
            keyHints={false}
            onShowPicture={onShowPicture}
          />
          <ProgrammeGuide
            television
            rows={makeRows(6)}
            groups={GROUPS}
            activeGroup={null}
            window={WINDOW}
            now={NOW}
            playingChannel={null}
            variantPreferences={new Map()}
            layout="pocket"
            mode="guide"
            time={{
              open: timeOpen,
              chosen: chosenTime,
              options: [new Date("2026-08-30T11:00:00Z"), new Date("2026-08-30T12:00:00Z")],
              onToggle: () => setTimeOpen((open) => !open),
              onChoose: setChosenTime,
            }}
            loading={false}
            replacing={false}
            error={null}
            hasMore={false}
            loadingMore={false}
            onSelectGroup={onSelectGroup}
            onPrefetchGroup={() => undefined}
            excludedGroups={new Set()}
            onSetGroupExcluded={() => undefined}
            onRestoreExcludedGroups={() => undefined}
            onPreparePlayback={() => undefined}
            onTune={() => undefined}
            onTuneVariant={() => undefined}
            onRetry={() => undefined}
            onLoadMore={() => undefined}
            search={null}
            feeds={<button type="button" className="feeds-trigger">Sources</button>}
            status={null}
          />
        </main>
      );
    }

    render(<GuideWithTv />);
    const grid = document.querySelector<HTMLElement>(".programme-guide__rows")!;
    grid.style.display = "grid";
    grid.style.gridTemplateColumns = "200px 200px 200px";

    const stage = document.querySelector<HTMLElement>(".stage")!;
    giveBox(stage, new DOMRect(0, 0, 200, 80));
    const undock = screen.getByRole("button", { name: "Watch full screen" });
    giveBox(undock, new DOMRect(160, 0, 40, 80));
    const when = screen.getByRole("button", { name: "Now", expanded: true });
    giveBox(when, new DOMRect(210, 40, 80, 40));
    const allRadio = screen.getByRole("radio", { name: "All" });
    const newsRadio = screen.getByRole("radio", { name: /News/u });
    giveBox(allRadio, new DOMRect(300, 40, 80, 40));
    giveBox(newsRadio, new DOMRect(390, 40, 80, 40));
    const times = within(screen.getByRole("group", { name: "Time" })).getAllByRole("button");
    giveBox(times[0], new DOMRect(10, 90, 80, 40));
    giveBox(times[1], new DOMRect(100, 90, 80, 40));
    const tiles = screen.getAllByRole("button", { name: /^Tune Channel /u });
    // Simulate content-visibility: auto collapsing row 0's first tile while grid tracks stay 3-col.
    giveBox(tiles[0], new DOMRect(0, 0, 0, 0));
    tiles.slice(1).forEach((tile, i) => {
      const idx = i + 1;
      giveBox(tile, new DOMRect((idx % 3) * 200, 140 + Math.floor(idx / 3) * 80, 200, 80));
    });

    // P0-2: Moving sideways in TV group lane moves focus WITHOUT immediately selecting the group
    // (even after earlier Arrow keys inside RadioGroup), reaches Now/Choose groups at the edges,
    // and commits only on Enter/Space.
    act(() => allRadio.focus());
    fireEvent.keyDown(allRadio, { key: "ArrowLeft" });
    expect(when).toHaveFocus();
    fireEvent.keyDown(when, { key: "ArrowRight" });
    expect(allRadio).toHaveFocus();
    fireEvent.keyDown(allRadio, { key: "ArrowRight" });
    expect(newsRadio).toHaveFocus();
    expect(onSelectGroup).not.toHaveBeenCalled();
    // Second sideways step after RadioGroup saw Arrow keys still must not auto-commit.
    const sportsRadio = screen.getByRole("radio", { name: /Sports/u });
    giveBox(sportsRadio, new DOMRect(480, 40, 80, 40));
    fireEvent.keyDown(newsRadio, { key: "ArrowRight" });
    expect(sportsRadio).toHaveFocus();
    expect(onSelectGroup).not.toHaveBeenCalled();
    fireEvent.keyDown(sportsRadio, { key: "ArrowLeft" });
    expect(newsRadio).toHaveFocus();
    expect(onSelectGroup).not.toHaveBeenCalled();
    fireEvent.keyDown(newsRadio, { key: "Enter" });
    expect(onSelectGroup).toHaveBeenCalledWith("News");

    // Pointer click on a TV Channel Group chip still selects immediately.
    fireEvent.click(sportsRadio);
    expect(onSelectGroup).toHaveBeenCalledWith("Sports");

    const moviesRadio = screen.getByRole("radio", { name: /Movies/u });
    const chooser = screen.getByRole("button", { name: "Choose groups" });
    giveBox(moviesRadio, new DOMRect(570, 40, 80, 40));
    giveBox(chooser, new DOMRect(660, 40, 120, 40));
    act(() => moviesRadio.focus());
    fireEvent.keyDown(moviesRadio, { key: "ArrowRight" });
    expect(chooser).toHaveFocus();
    fireEvent.keyDown(chooser, { key: "ArrowLeft" });
    expect(moviesRadio).toHaveFocus();

    // P0-1: Pressing Down from Channel Group enters open Time row instead of jumping over it.
    act(() => newsRadio.focus());
    fireEvent.keyDown(newsRadio, { key: "ArrowDown" });
    expect(times[0]).toHaveFocus();

    // P1-4: Horizontal movement in 3-col grid still works even if tiles[0] collapsed to 0x0.
    act(() => tiles[3].focus());
    fireEvent.keyDown(tiles[3], { key: "ArrowRight" });
    expect(tiles[4]).toHaveFocus();

    // P0-1 + R1 Should 2: ArrowUp from row 0 enters open Time row first.
    giveBox(tiles[0], new DOMRect(0, 140, 200, 80));
    act(() => tiles[0].focus());
    fireEvent.keyDown(tiles[0], { key: "ArrowUp" });
    expect(times[0]).toHaveFocus();

    // ArrowUp from col-0 time button reaches docked preview; Back inside ProgrammeGuide's
    // own Time row returns focus to Now. (Sibling-preview Back is exercised through CatalogBrowser.)
    fireEvent.keyDown(times[0], { key: "ArrowUp" });
    expect(undock).toHaveFocus();
    fireEvent.keyDown(undock, { key: "ArrowDown" });
    expect(times[0]).toHaveFocus();
    fireEvent.keyDown(times[0], { key: "Escape", isComposing: true });
    expect(screen.getByRole("group", { name: "Time" })).toBeInTheDocument();
    fireEvent.keyDown(times[0], { key: "Escape" });
    expect(screen.queryByRole("group", { name: "Time" })).toBeNull();
    expect(when).toHaveFocus();
  });

  it("walks the TV timeline by Channel and by time, reaches the group rail, and brings later cells into view", () => {
    document.documentElement.setAttribute("data-television", "");
    const at = (minutes: number) =>
      clientSchemas.isoInstant.parse(
        new Date(WINDOW.startsAt.getTime() + minutes * 60_000).toISOString(),
      );
    const programme = (title: string, from: number, to: number) => ({
      title,
      startsAt: at(from),
      endsAt: at(to),
      titleTruncated: false,
    });
    const rows: GuideWindowChannel[] = [
      {
        channel: channelFixture({ id: "ch-1", name: "Channel 1", group: "News", number: 1 }),
        programmes: [
          programme("Morning", 0, 60),
          programme("Midday", 60, 120),
          programme("Afternoon", 120, 180),
        ],
        programmesTruncated: false,
      },
      {
        channel: channelFixture({ id: "ch-2", name: "Channel 2", group: "Sports", number: 2 }),
        programmes: [programme("Short", 0, 30), programme("Long", 30, 120)],
        programmesTruncated: false,
      },
      {
        channel: channelFixture({ id: "ch-3", name: "Channel 3", group: "Movies", number: 3 }),
        programmes: [],
        programmesTruncated: false,
      },
    ];
    const onTune = vi.fn();
    const onToggle = vi.fn();
    render(
      <ProgrammeGuide
        television
        rows={rows}
        groups={GROUPS}
        activeGroup={null}
        window={WINDOW}
        now={NOW}
        playingChannel={null}
        variantPreferences={new Map()}
        layout="pocket"
        mode="guide"
        time={null}
        view={{ current: "timeline", onToggle }}
        loading={false}
        replacing={false}
        error={null}
        hasMore={false}
        loadingMore={false}
        onSelectGroup={() => undefined}
        onPrefetchGroup={() => undefined}
        excludedGroups={new Set()}
        onSetGroupExcluded={() => undefined}
        onRestoreExcludedGroups={() => undefined}
        onPreparePlayback={() => undefined}
        onTune={onTune}
        onTuneVariant={() => undefined}
        onRetry={() => undefined}
        onLoadMore={() => undefined}
        search={null}
        feeds={null}
        status={null}
      />,
    );
    const board = document.querySelector<HTMLElement>(".programme-guide__board")!;
    expect(board).toHaveAttribute("data-view", "timeline");
    // One mark per Channel, on its own button, in catalog order; the cells
    // tune the Channel without claiming its name or its mark.
    expect(
      Array.from(document.querySelectorAll("[data-acceptance-channel]"), (el) =>
        el.getAttribute("aria-label"),
      ),
    ).toEqual(["Tune Channel 1", "Tune Channel 2", "Tune Channel 3"]);
    const cell = (name: string) =>
      screen.getByRole("button", { name: new RegExp(`^${name}, `, "u") });
    for (const button of document.querySelectorAll(".programme-guide__programme")) {
      expect(button).not.toHaveAttribute("data-acceptance-channel");
      expect(button.getAttribute("aria-label")).not.toMatch(/^Tune /u);
    }
    const lead = screen.getByRole("button", { name: "Timeline" });
    expect(lead).toHaveAttribute("aria-pressed", "true");
    const allRadio = screen.getByRole("radio", { name: "All" });
    const newsRadio = screen.getByRole("radio", { name: /News/u });

    // Left and Right step one cell through time; Left from the first cell
    // reaches the rail, and Right from the rail returns to that cell.
    act(() => cell("Morning").focus());
    fireEvent.keyDown(cell("Morning"), { key: "ArrowRight" });
    expect(cell("Midday")).toHaveFocus();
    fireEvent.keyDown(cell("Midday"), { key: "ArrowLeft" });
    expect(cell("Morning")).toHaveFocus();
    fireEvent.keyDown(cell("Morning"), { key: "ArrowLeft" });
    expect(allRadio).toHaveFocus();
    fireEvent.keyDown(allRadio, { key: "ArrowRight" });
    expect(cell("Morning")).toHaveFocus();

    // Up and Down change Channel and keep to the time of the focused cell; a
    // Channel without Programmes offers its own button.
    fireEvent.keyDown(cell("Morning"), { key: "ArrowDown" });
    expect(cell("Short")).toHaveFocus();
    fireEvent.keyDown(cell("Short"), { key: "ArrowDown" });
    const channel3 = screen.getByRole("button", { name: "Tune Channel 3" });
    expect(channel3).toHaveFocus();
    fireEvent.keyDown(channel3, { key: "ArrowUp" });
    expect(cell("Short")).toHaveFocus();
    fireEvent.keyDown(cell("Short"), { key: "ArrowRight" });
    expect(cell("Long")).toHaveFocus();
    fireEvent.keyDown(cell("Long"), { key: "ArrowUp" });
    expect(cell("Morning")).toHaveFocus();
    fireEvent.keyDown(cell("Morning"), { key: "ArrowUp" });
    expect(lead).toHaveFocus();

    // The rail stands on its side: Up and Down walk it.
    fireEvent.keyDown(lead, { key: "ArrowDown" });
    expect(allRadio).toHaveFocus();
    fireEvent.keyDown(allRadio, { key: "ArrowDown" });
    expect(newsRadio).toHaveFocus();
    fireEvent.keyDown(newsRadio, { key: "ArrowUp" });
    expect(allRadio).toHaveFocus();
    fireEvent.keyDown(allRadio, { key: "ArrowUp" });
    expect(lead).toHaveFocus();

    // A cell near the right edge brings later time into view in half hours,
    // no further than the window allows; the last cell holds.
    expect(board.style.getPropertyValue("--tl-offset")).toBe("0");
    act(() => cell("Afternoon").focus());
    expect(board.style.getPropertyValue("--tl-offset")).toBe("60");
    fireEvent.keyDown(cell("Afternoon"), { key: "ArrowRight" });
    expect(cell("Afternoon")).toHaveFocus();
    act(() => cell("Morning").focus());
    expect(board.style.getPropertyValue("--tl-offset")).toBe("0");

    // Center on a cell tunes its Channel now.
    fireEvent.click(cell("Midday"));
    expect(onTune).toHaveBeenCalledWith(rows[0].channel);
    fireEvent.click(lead);
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it("passes over a sliver at the timeline's left edge for the cell drawn across it", () => {
    document.documentElement.setAttribute("data-television", "");
    const at = (minutes: number) =>
      clientSchemas.isoInstant.parse(
        new Date(WINDOW.startsAt.getTime() + minutes * 60_000).toISOString(),
      );
    const programme = (title: string, from: number, to: number) => ({
      title,
      startsAt: at(from),
      endsAt: at(to),
      titleTruncated: false,
    });
    const rows: GuideWindowChannel[] = [
      {
        channel: channelFixture({ id: "edge-1", name: "Edge 1", group: "News", number: 1 }),
        programmes: [programme("Morning", 0, 60)],
        programmesTruncated: false,
      },
      {
        channel: channelFixture({ id: "edge-2", name: "Edge 2", group: "News", number: 2 }),
        // Five minutes of "Lead-out" are in view: the CSS draws "Feature"
        // from the edge over it.
        programmes: [programme("Lead-out", -55, 5), programme("Feature", 5, 60)],
        programmesTruncated: false,
      },
    ];
    render(
      <ProgrammeGuide
        television
        rows={rows}
        groups={GROUPS}
        activeGroup={null}
        window={WINDOW}
        now={NOW}
        playingChannel={null}
        variantPreferences={new Map()}
        layout="pocket"
        mode="guide"
        time={null}
        view={{ current: "timeline", onToggle: () => undefined }}
        loading={false}
        replacing={false}
        error={null}
        hasMore={false}
        loadingMore={false}
        onSelectGroup={() => undefined}
        onPrefetchGroup={() => undefined}
        excludedGroups={new Set()}
        onSetGroupExcluded={() => undefined}
        onRestoreExcludedGroups={() => undefined}
        onPreparePlayback={() => undefined}
        onTune={() => undefined}
        onTuneVariant={() => undefined}
        onRetry={() => undefined}
        onLoadMore={() => undefined}
        search={null}
        feeds={null}
        status={null}
      />,
    );
    const cell = (name: string) =>
      screen.getByRole("button", { name: new RegExp(`^${name}, `, "u") });

    act(() => cell("Morning").focus());
    // A Timeline move reads the rows beside it, never every Channel.
    const queries = vi.spyOn(Element.prototype, "querySelectorAll");
    fireEvent.keyDown(cell("Morning"), { key: "ArrowDown" });
    expect(cell("Feature")).toHaveFocus();
    expect(queries.mock.calls.map(([selector]) => selector)).not.toContain(
      ".programme-guide__row .programme-guide__channel",
    );
    queries.mockRestore();
    // At the first minute shown there is no earlier time to bring into view,
    // so Left from the cell drawn across the sliver reaches the rail.
    fireEvent.keyDown(cell("Feature"), { key: "ArrowLeft" });
    expect(screen.getByRole("radio", { name: "All" })).toHaveFocus();
  });

  it("keeps showing the same time when the window moves on under a panned timeline", () => {
    document.documentElement.setAttribute("data-television", "");
    const at = (minutes: number) =>
      clientSchemas.isoInstant.parse(
        new Date(WINDOW.startsAt.getTime() + minutes * 60_000).toISOString(),
      );
    const programme = (title: string, from: number, to: number) => ({
      title,
      startsAt: at(from),
      endsAt: at(to),
      titleTruncated: false,
    });
    const rows: GuideWindowChannel[] = [
      {
        channel: channelFixture({ id: "roll-1", name: "Roll 1", group: "News", number: 1 }),
        programmes: [
          programme("Morning", 0, 65),
          programme("Bulletin", 65, 80),
          programme("Afternoon", 150, 170),
        ],
        programmesTruncated: false,
      },
    ];
    const guide = (window: typeof WINDOW, now: Date) => (
      <ProgrammeGuide
        television
        rows={rows}
        groups={GROUPS}
        activeGroup={null}
        window={window}
        now={now}
        playingChannel={null}
        variantPreferences={new Map()}
        layout="pocket"
        mode="guide"
        time={null}
        view={{ current: "timeline", onToggle: () => undefined }}
        loading={false}
        replacing={false}
        error={null}
        hasMore={false}
        loadingMore={false}
        onSelectGroup={() => undefined}
        onPrefetchGroup={() => undefined}
        excludedGroups={new Set()}
        onSetGroupExcluded={() => undefined}
        onRestoreExcludedGroups={() => undefined}
        onPreparePlayback={() => undefined}
        onTune={() => undefined}
        onTuneVariant={() => undefined}
        onRetry={() => undefined}
        onLoadMore={() => undefined}
        search={null}
        feeds={null}
        status={null}
      />
    );
    const { rerender } = render(guide(WINDOW, NOW));
    const board = document.querySelector<HTMLElement>(".programme-guide__board")!;
    const cell = (name: string) =>
      screen.getByRole("button", { name: new RegExp(`^${name}, `, "u") });

    // Panned to 11:00, with 11:05 to 11:20 focused.
    act(() => cell("Afternoon").focus());
    act(() => cell("Bulletin").focus());
    expect(board.style.getPropertyValue("--tl-offset")).toBe("60");
    const bulletin = cell("Bulletin");
    expect(bulletin).toHaveAttribute("data-end", "80");

    // The window moves on half an hour: the same button now ends at minute
    // 50 of the window, and the timeline still shows it from 11:00.
    rerender(
      guide(
        {
          startsAt: new Date("2026-08-30T10:30:00Z"),
          endsAt: new Date("2026-08-30T13:30:00Z"),
        },
        new Date("2026-08-30T10:30:00Z"),
      ),
    );
    expect(cell("Bulletin")).toBe(bulletin);
    expect(bulletin).toHaveFocus();
    expect(bulletin).toHaveAttribute("data-end", "50");
    expect(board.style.getPropertyValue("--tl-offset")).toBe("30");
  });

  it("checks visibility lazily on large Guide boards without reading styles for every loaded Channel", () => {
    document.documentElement.setAttribute("data-television", "");
    render(
      <ProgrammeGuide
        television
        rows={makeRows(60)}
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
        onSelectGroup={() => undefined}
        onPrefetchGroup={() => undefined}
        excludedGroups={new Set()}
        onSetGroupExcluded={() => undefined}
        onRestoreExcludedGroups={() => undefined}
        onPreparePlayback={() => undefined}
        onTune={() => undefined}
        onTuneVariant={() => undefined}
        onRetry={() => undefined}
        onLoadMore={() => undefined}
        search={null}
        feeds={null}
        status={null}
      />,
    );
    const grid = document.querySelector<HTMLElement>(".programme-guide__rows")!;
    grid.style.display = "grid";
    grid.style.gridTemplateColumns = "200px 200px 200px";
    const tiles = screen.getAllByRole("button", { name: /^Tune Channel /u });
    tiles.forEach((tile, idx) =>
      giveBox(tile, new DOMRect((idx % 3) * 200, 100 + Math.floor(idx / 3) * 80, 200, 80)),
    );

    const styleSpy = vi.spyOn(window, "getComputedStyle");
    try {
      act(() => tiles[0].focus());
      styleSpy.mockClear();
      fireEvent.keyDown(tiles[0], { key: "ArrowDown" });
      expect(tiles[3]).toHaveFocus();
      // Only reads grid container + ancestors of the single adjacent candidate (not all 60 tiles).
      expect(styleSpy.mock.calls.length).toBeLessThan(25);
    } finally {
      styleSpy.mockRestore();
    }
  });

  it("reaches a row's quality chips with the D-pad in the Now grid and the Timeline, keeping one mark per Channel", async () => {
    document.documentElement.setAttribute("data-television", "");
    const user = (await import("@testing-library/user-event")).default.setup();
    const at = (minutes: number) =>
      clientSchemas.isoInstant.parse(
        new Date(WINDOW.startsAt.getTime() + minutes * 60_000).toISOString(),
      );
    const programme = (title: string, from: number, to: number) => ({
      title,
      startsAt: at(from),
      endsAt: at(to),
      titleTruncated: false,
    });
    const variant = (quality: "sd" | "hd") => ({
      channel: channelFixture({
        id: `svt-${quality}`,
        name: `SVT ${quality.toUpperCase()}`,
        group: "News",
        number: 2,
        variant: { quality, baseName: "SVT" },
      }),
      programmes: [programme("Bulletin", 0, 60), programme("Late", 60, 120)],
      programmesTruncated: false,
    });
    const lone = (number: number) => ({
      channel: channelFixture({ id: `chip-${number}`, name: `Channel ${number}`, group: "News", number }),
      programmes: [programme(`Show ${number}`, 0, 120)],
      programmesTruncated: false,
    });
    const rows: GuideWindowChannel[] = [lone(1), variant("sd"), variant("hd"), lone(3), lone(4)];
    const onTune = vi.fn();
    const onTuneVariant = vi.fn();
    const guide = (view: "now" | "timeline") => (
      <ProgrammeGuide
        television
        rows={rows}
        groups={GROUPS}
        activeGroup={null}
        window={WINDOW}
        now={NOW}
        playingChannel={null}
        variantPreferences={new Map()}
        layout="pocket"
        mode="guide"
        time={null}
        view={{ current: view, onToggle: () => undefined }}
        loading={false}
        replacing={false}
        error={null}
        hasMore={false}
        loadingMore={false}
        onSelectGroup={() => undefined}
        onPrefetchGroup={() => undefined}
        excludedGroups={new Set()}
        onSetGroupExcluded={() => undefined}
        onRestoreExcludedGroups={() => undefined}
        onPreparePlayback={() => undefined}
        onTune={onTune}
        onTuneVariant={onTuneVariant}
        onRetry={() => undefined}
        onLoadMore={() => undefined}
        search={null}
        feeds={null}
        status={null}
      />
    );
    const marks = () =>
      Array.from(document.querySelectorAll("[data-acceptance-channel]"), (el) =>
        el.getAttribute("aria-label"),
      );
    const view = render(guide("now"));
    expect(marks()).toEqual([
      "Tune Channel 1",
      "Tune SVT SD",
      "Tune SVT HD",
      "Tune Channel 3",
      "Tune Channel 4",
    ]);
    // The Now grid, three cells wide.
    const grid = document.querySelector<HTMLElement>(".programme-guide__rows")!;
    grid.style.display = "grid";
    grid.style.gridTemplateColumns = "200px 200px 200px";
    const svt = screen.getByRole("button", { name: "Tune SVT" });
    const sd = screen.getByRole("button", { name: "Tune SVT SD" });
    const hd = screen.getByRole("button", { name: "Tune SVT HD" });
    const channel = (number: number) =>
      screen.getByRole("button", { name: `Tune Channel ${number}` });

    // Right walks a cell, then its chips, then the next cell; Left comes back
    // the same way. Left and Up and Down from a cell never stop on chips.
    act(() => channel(1).focus());
    fireEvent.keyDown(channel(1), { key: "ArrowRight" });
    expect(svt).toHaveFocus();
    fireEvent.keyDown(svt, { key: "ArrowRight" });
    expect(sd).toHaveFocus();
    fireEvent.keyDown(sd, { key: "ArrowRight" });
    expect(hd).toHaveFocus();
    fireEvent.keyDown(hd, { key: "ArrowRight" });
    expect(channel(3)).toHaveFocus();
    fireEvent.keyDown(channel(3), { key: "ArrowLeft" });
    expect(svt).toHaveFocus();
    fireEvent.keyDown(svt, { key: "ArrowRight" });
    fireEvent.keyDown(sd, { key: "ArrowRight" });
    fireEvent.keyDown(hd, { key: "ArrowLeft" });
    expect(sd).toHaveFocus();
    fireEvent.keyDown(sd, { key: "ArrowLeft" });
    expect(svt).toHaveFocus();
    // Down from a chip moves as Down from its cell.
    fireEvent.keyDown(svt, { key: "ArrowRight" });
    fireEvent.keyDown(sd, { key: "ArrowDown" });
    expect(screen.getByRole("button", { name: "Tune Channel 4" })).toHaveFocus();

    // OK on a chip tunes that Quality Variant, and focus stays on it.
    fireEvent.keyDown(channel(4), { key: "ArrowUp" });
    expect(channel(1)).toHaveFocus();
    fireEvent.keyDown(channel(1), { key: "ArrowRight" });
    fireEvent.keyDown(svt, { key: "ArrowRight" });
    fireEvent.keyDown(sd, { key: "ArrowRight" });
    expect(hd).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(onTuneVariant).toHaveBeenLastCalledWith(rows[2].channel);
    expect(onTune).not.toHaveBeenCalled();
    expect(hd).toHaveFocus();

    // The Timeline: Left and Right step through time, and Left from the
    // row's first cell reaches its chips (the last one first), then the rail.
    view.rerender(guide("timeline"));
    expect(marks()).toEqual([
      "Tune Channel 1",
      "Tune SVT SD",
      "Tune SVT HD",
      "Tune Channel 3",
      "Tune Channel 4",
    ]);
    const cell = (name: string) =>
      within(requireTimelineRow("SVT")).getByRole("button", {
        name: new RegExp(`^${name}, `, "u"),
      });
    const timelineSd = screen.getByRole("button", { name: "Tune SVT SD" });
    const timelineHd = screen.getByRole("button", { name: "Tune SVT HD" });
    act(() => cell("Late").focus());
    fireEvent.keyDown(cell("Late"), { key: "ArrowLeft" });
    expect(cell("Bulletin")).toHaveFocus();
    fireEvent.keyDown(cell("Bulletin"), { key: "ArrowLeft" });
    expect(timelineHd).toHaveFocus();
    fireEvent.keyDown(timelineHd, { key: "ArrowLeft" });
    expect(timelineSd).toHaveFocus();
    fireEvent.keyDown(timelineSd, { key: "ArrowLeft" });
    expect(screen.getByRole("radio", { name: "All" })).toHaveFocus();
    // Right from the rail returns to the last cell; from the chips, Right
    // enters the cells at the time in view.
    fireEvent.keyDown(screen.getByRole("radio", { name: "All" }), { key: "ArrowRight" });
    expect(cell("Bulletin")).toHaveFocus();
    fireEvent.keyDown(cell("Bulletin"), { key: "ArrowLeft" });
    fireEvent.keyDown(timelineHd, { key: "ArrowLeft" });
    fireEvent.keyDown(timelineSd, { key: "ArrowRight" });
    expect(timelineHd).toHaveFocus();
    fireEvent.keyDown(timelineHd, { key: "ArrowRight" });
    expect(cell("Bulletin")).toHaveFocus();
    // Up and Down from a chip change Channel at the time in view.
    fireEvent.keyDown(cell("Bulletin"), { key: "ArrowLeft" });
    fireEvent.keyDown(timelineHd, { key: "ArrowDown" });
    expect(screen.getByRole("button", { name: /^Show 3, /u })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole("button", { name: /^Show 3, /u }), { key: "ArrowUp" });
    expect(cell("Bulletin")).toHaveFocus();
    fireEvent.keyDown(cell("Bulletin"), { key: "ArrowLeft" });
    fireEvent.keyDown(timelineHd, { key: "ArrowLeft" });
    await user.keyboard("{Enter}");
    expect(onTuneVariant).toHaveBeenLastCalledWith(rows[1].channel);
  });

  it("keeps a focused future Timeline cell through the window's move to the next half hour", () => {
    document.documentElement.setAttribute("data-television", "");
    const programme = (title: string, from: string, to: string) => ({
      title,
      startsAt: clientSchemas.isoInstant.parse(from),
      endsAt: clientSchemas.isoInstant.parse(to),
      titleTruncated: false,
    });
    const early = programme("Early", "2026-08-30T09:30:00Z", "2026-08-30T10:30:00Z");
    const now = programme("Now", "2026-08-30T10:30:00Z", "2026-08-30T11:00:00Z");
    const later = programme("Later", "2026-08-30T11:00:00Z", "2026-08-30T12:00:00Z");
    const channel = channelFixture({ id: "roll-1", name: "Roll", group: "News", number: 1 });
    const guide = (window: typeof WINDOW, programmes: GuideWindowChannel["programmes"], at: Date) => (
      <ProgrammeGuide
        television
        rows={[{ channel, programmes, programmesTruncated: false }]}
        groups={GROUPS}
        activeGroup={null}
        window={window}
        now={at}
        playingChannel={null}
        variantPreferences={new Map()}
        layout="pocket"
        mode="guide"
        time={null}
        view={{ current: "timeline", onToggle: () => undefined }}
        loading={false}
        replacing={false}
        error={null}
        hasMore={false}
        loadingMore={false}
        onSelectGroup={() => undefined}
        onPrefetchGroup={() => undefined}
        excludedGroups={new Set()}
        onSetGroupExcluded={() => undefined}
        onRestoreExcludedGroups={() => undefined}
        onPreparePlayback={() => undefined}
        onTune={() => undefined}
        onTuneVariant={() => undefined}
        onRetry={() => undefined}
        onLoadMore={() => undefined}
        search={null}
        feeds={null}
        status={null}
      />
    );
    const view = render(guide(WINDOW, [early, now, later], NOW));
    const future = screen.getByRole("button", { name: /^Later, /u });
    act(() => future.focus());

    // Half an hour on, the window starts at 10:30 and the Programme that
    // ended leaves the row: the focused cell is the same element.
    view.rerender(
      guide(
        {
          startsAt: new Date("2026-08-30T10:30:00Z"),
          endsAt: new Date("2026-08-30T13:30:00Z"),
        },
        [now, later],
        new Date("2026-08-30T10:45:00Z"),
      ),
    );
    expect(future).toBeInTheDocument();
    expect(future).toHaveFocus();
    expect(future).toHaveAttribute("data-start", "30");
  });

  it("moves Up and Down the Timeline reading only the rows it passes", () => {
    document.documentElement.setAttribute("data-television", "");
    const rows = makeRows(60).map((row) => ({
      ...row,
      programmes: [
        {
          title: `${row.channel.name} show`,
          startsAt: clientSchemas.isoInstant.parse("2026-08-30T10:00:00Z"),
          endsAt: clientSchemas.isoInstant.parse("2026-08-30T11:00:00Z"),
          titleTruncated: false,
        },
      ],
    }));
    render(
      <ProgrammeGuide
        television
        rows={rows}
        groups={GROUPS}
        activeGroup={null}
        window={WINDOW}
        now={NOW}
        playingChannel={null}
        variantPreferences={new Map()}
        layout="pocket"
        mode="guide"
        time={null}
        view={{ current: "timeline", onToggle: () => undefined }}
        loading={false}
        replacing={false}
        error={null}
        hasMore={false}
        loadingMore={false}
        onSelectGroup={() => undefined}
        onPrefetchGroup={() => undefined}
        excludedGroups={new Set()}
        onSetGroupExcluded={() => undefined}
        onRestoreExcludedGroups={() => undefined}
        onPreparePlayback={() => undefined}
        onTune={() => undefined}
        onTuneVariant={() => undefined}
        onRetry={() => undefined}
        onLoadMore={() => undefined}
        search={null}
        feeds={null}
        status={null}
      />,
    );
    const cells = document.querySelectorAll<HTMLElement>(".programme-guide__programme");
    expect(cells).toHaveLength(60);
    const styleSpy = vi.spyOn(window, "getComputedStyle");
    try {
      act(() => cells[30].focus());
      styleSpy.mockClear();
      fireEvent.keyDown(cells[30], { key: "ArrowDown" });
      expect(cells[31]).toHaveFocus();
      fireEvent.keyDown(cells[31], { key: "ArrowUp" });
      expect(cells[30]).toHaveFocus();
      // Two moves read the focused and the neighbouring rows, not all sixty.
      expect(styleSpy.mock.calls.length).toBeLessThan(60);
    } finally {
      styleSpy.mockRestore();
    }
  });

  it("reaches retained error Try again from last row and returns to last row on ArrowUp", () => {
    document.documentElement.setAttribute("data-television", "");
    render(
      <ProgrammeGuide
        television
        rows={makeRows(3)}
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
        error={{ _tag: "transport", retryable: true, message: "offline" }}
        hasMore
        loadingMore={false}
        onSelectGroup={() => undefined}
        onPrefetchGroup={() => undefined}
        excludedGroups={new Set()}
        onSetGroupExcluded={() => undefined}
        onRestoreExcludedGroups={() => undefined}
        onPreparePlayback={() => undefined}
        onTune={() => undefined}
        onTuneVariant={() => undefined}
        onRetry={() => undefined}
        onLoadMore={() => undefined}
        search={null}
        feeds={null}
        status={null}
      />,
    );
    const grid = document.querySelector<HTMLElement>(".programme-guide__rows")!;
    grid.style.display = "grid";
    grid.style.gridTemplateColumns = "200px 200px 200px";
    const tiles = screen.getAllByRole("button", { name: /^Tune Channel /u });
    const retry = screen.getByRole("button", { name: "Try again" });
    const more = screen.getByRole("button", { name: "More channels" });
    tiles.forEach((tile, i) => giveBox(tile, new DOMRect(i * 200, 100, 200, 80)));
    giveBox(retry, new DOMRect(0, 190, 200, 40));
    giveBox(more, new DOMRect(0, 240, 600, 40));

    act(() => tiles[2].focus());
    fireEvent.keyDown(tiles[2], { key: "ArrowDown" });
    expect(retry).toHaveFocus();
    fireEvent.keyDown(retry, { key: "ArrowDown" });
    expect(more).toHaveFocus();
    fireEvent.keyDown(more, { key: "ArrowUp" });
    expect(retry).toHaveFocus();
    fireEvent.keyDown(retry, { key: "ArrowUp" });
    expect(tiles[2]).toHaveFocus();
  });

  it("hands focus off when More channels is focused while auto-load is already in flight", () => {
    document.documentElement.setAttribute("data-television", "");
    const rows = makeRows(4);
    const baseProps = {
      television: true,
      groups: GROUPS,
      activeGroup: null,
      window: WINDOW,
      now: NOW,
      playingChannel: null,
      variantPreferences: new Map(),
      layout: "pocket" as const,
      mode: "guide" as const,
      time: null,
      loading: false,
      replacing: false,
      error: null,
      onSelectGroup: () => undefined,
      onPrefetchGroup: () => undefined,
      excludedGroups: new Set<string>(),
      onSetGroupExcluded: () => undefined,
      onRestoreExcludedGroups: () => undefined,
      onPreparePlayback: () => undefined,
      onTune: () => undefined,
      onTuneVariant: () => undefined,
      onRetry: () => undefined,
      onLoadMore: () => undefined,
      search: null,
      feeds: null,
      status: null,
    };
    const view = render(<ProgrammeGuide {...baseProps} rows={rows} hasMore loadingMore />);
    const more = screen.getByRole("button", { name: "Loading more channels…" });
    act(() => more.focus());
    expect(more).toHaveFocus();

    // Final page completes with 2 new channels and removes More channels.
    view.rerender(<ProgrammeGuide {...baseProps} rows={makeRows(6)} hasMore={false} loadingMore={false} />);
    expect(screen.getByRole("button", { name: "Tune Channel 5" })).toHaveFocus();
  });

  it("wakes fullscreen controls on Center when hidden, and leaves fullscreen on one Back whether or not the bar shows", async () => {
    vi.useFakeTimers();
    document.documentElement.setAttribute("data-television", "");
    const port = androidPortFixture();
    const bridge = createAndroidPlayerBridge(port.value);
    const onPause = vi.fn();
    const onZap = vi.fn();
    const onShowGuide = vi.fn();
    const exitFullscreen = vi.fn(async () => {
      Reflect.deleteProperty(document, "fullscreenElement");
      document.dispatchEvent(new Event("fullscreenchange"));
    });
    Object.defineProperty(document, "exitFullscreen", { configurable: true, value: exitFullscreen });

    function FullscreenPlayer() {
      const videoRef = createRef<HTMLVideoElement>();
      useStageKeys({
        active: false,
        television: true,
        mode: "watch",
        guideForced: false,
        onToggleGuide: () => undefined,
        onWatch: () => undefined,
        onZap,
      });
      useTelevisionControls(
        { enabled: true, onShowGuide, onZap },
        bridge,
      );
      const chrome: StageChrome = {
        television: true,
        controls: "compact",
        controlsSlot: null,
        fullscreenTarget: null,
        menuSide: "bottom",
        reportPicture: () => undefined,
      };
      return (
        <StageChromeProvider value={chrome}>
          <PlaybackSurface
            channel={channelFixture({ id: "ch-1", name: "Channel 1", group: "News" })}
            state={{ _tag: "playing" }}
            videoKey="ch-1"
            videoRef={videoRef}
            privacyCopy="Private"
            onPlaying={() => undefined}
            pause={{ onPause }}
            volume={1}
            muted={false}
            fullscreen
            nativeVideo
            onVolumeChange={() => undefined}
            onToggleMuted={() => undefined}
            onRequestFullscreen={() => undefined}
            onStop={() => undefined}
            onAutoplayFailure={() => undefined}
          />
        </StageChromeProvider>
      );
    }

    render(<FullscreenPlayer />);
    const section = screen.getByRole("region", { name: "Channel 1" });
    Object.defineProperty(document, "fullscreenElement", { configurable: true, value: section });
    act(() => {
      document.dispatchEvent(new Event("fullscreenchange"));
    });
    expect(section).toHaveFocus();

    // Untouched controls auto-hide on fresh tune.
    act(() => vi.advanceTimersByTime(3_000));
    expect(section).toHaveAttribute("data-controls-visible", "false");

    // A2: Center while controls hidden wakes controls and focuses Pause without pausing playback.
    fireEvent.keyDown(section, { key: "Enter" });
    expect(section).toHaveAttribute("data-controls-visible", "true");
    expect(onPause).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(50));
    const pauseButton = screen.getByRole("button", { name: "Pause" });
    expect(pauseButton).toHaveFocus();

    // P1-1: ArrowUp on a fullscreen control button returns to section without zapping.
    fireEvent.keyDown(pauseButton, { key: "ArrowUp" });
    expect(onZap).not.toHaveBeenCalled();
    expect(section).toHaveFocus();

    // Extended idle timer: focus inside controls bar stays visible past 3s, then hides and focuses section.
    fireEvent.keyDown(section, { key: "Enter" });
    act(() => vi.advanceTimersByTime(50));
    expect(pauseButton).toHaveFocus();
    act(() => vi.advanceTimersByTime(3_000));
    expect(section).toHaveAttribute("data-controls-visible", "true");
    expect(pauseButton).toHaveFocus();
    act(() => vi.advanceTimersByTime(5_500));
    expect(section).toHaveAttribute("data-controls-visible", "false");
    expect(section).toHaveFocus();

    // DOM Escape while the bar is hidden leaves fullscreen for the Guide
    // without showing the bar on the way out.
    fireEvent.keyDown(section, { key: "Escape" });
    expect(section).toHaveAttribute("data-controls-visible", "false");
    expect(exitFullscreen).toHaveBeenCalledTimes(1);
    await act(async () => {
      await Promise.resolve();
    });
    expect(onShowGuide).toHaveBeenCalledTimes(1);

    // Native Back while the bar shows, with focus in it, leaves at once too:
    // one Back from fullscreen is the Guide, as on the Shield itself.
    Object.defineProperty(document, "fullscreenElement", { configurable: true, value: section });
    act(() => {
      document.dispatchEvent(new Event("fullscreenchange"));
    });
    fireEvent.keyDown(section, { key: "ArrowRight" });
    expect(section).toHaveAttribute("data-controls-visible", "true");
    act(() => vi.advanceTimersByTime(50));
    expect(pauseButton).toHaveFocus();
    act(() => port.deliver("tv:back"));
    expect(exitFullscreen).toHaveBeenCalledTimes(2);
    expect(port.sent).not.toContain("tv:default-back");
    await act(async () => {
      await Promise.resolve();
    });
    expect(onShowGuide).toHaveBeenCalledTimes(2);

    // And so does native Back while the bar is hidden.
    Object.defineProperty(document, "fullscreenElement", { configurable: true, value: section });
    act(() => {
      document.dispatchEvent(new Event("fullscreenchange"));
    });
    act(() => vi.advanceTimersByTime(3_000));
    expect(section).toHaveAttribute("data-controls-visible", "false");
    act(() => port.deliver("tv:back"));
    expect(section).toHaveAttribute("data-controls-visible", "false");
    expect(exitFullscreen).toHaveBeenCalledTimes(3);
    await act(async () => {
      await Promise.resolve();
    });
    expect(onShowGuide).toHaveBeenCalledTimes(3);
  });

  it("moves fullscreen focus from section to Close player on non-retryable failure", () => {
    vi.useFakeTimers();
    document.documentElement.setAttribute("data-television", "");

    function FailingFullscreen({ failed }: { readonly failed: boolean }) {
      const videoRef = createRef<HTMLVideoElement>();
      useTelevisionControls(
        { enabled: true, onShowGuide: () => undefined, onZap: () => undefined },
        null,
      );
      const chrome: StageChrome = {
        television: true,
        controls: "bar",
        controlsSlot: null,
        fullscreenTarget: null,
        menuSide: "bottom",
        reportPicture: () => undefined,
      };
      return (
        <StageChromeProvider value={chrome}>
          <PlaybackSurface
            channel={channelFixture({ id: "ch-fail", name: "Fail Channel", group: "News" })}
            state={
              failed
                ? { _tag: "failed", failure: "cleanup-unconfirmed", retryable: false }
                : { _tag: "playing" }
            }
            videoKey="ch-fail"
            videoRef={videoRef}
            privacyCopy="Private"
            onPlaying={() => undefined}
            showMediaControls={!failed}
            stopLabel={failed ? "Close player" : "Stop stream"}
            volume={1}
            muted={false}
            fullscreen
            nativeVideo
            onVolumeChange={() => undefined}
            onToggleMuted={() => undefined}
            onRequestFullscreen={() => undefined}
            onStop={() => undefined}
            onAutoplayFailure={() => undefined}
          />
        </StageChromeProvider>
      );
    }

    const view = render(<FailingFullscreen failed={false} />);
    const section = screen.getByRole("region", { name: "Fail Channel" });
    giveBox(section, new DOMRect(0, 0, 960, 540));
    Object.defineProperty(document, "fullscreenElement", { configurable: true, value: section });
    act(() => document.dispatchEvent(new Event("fullscreenchange")));
    expect(section).toHaveFocus();

    // Transition to non-retryable failure while section still owns focus -> focus moves to Close player.
    view.rerender(<FailingFullscreen failed />);
    const closeBtn = screen.getByRole("button", { name: "Close player" });
    giveBox(closeBtn, new DOMRect(10, 490, 120, 40));
    act(() => {
      vi.advanceTimersByTime(50);
    });
    expect(closeBtn).toHaveFocus();
  });

  it("marks Exit fullscreen while the signal opens, then gives focus back to the picture once it plays", () => {
    vi.useFakeTimers();
    document.documentElement.setAttribute("data-television", "");

    function OpeningFullscreen({ playing }: { readonly playing: boolean }) {
      const videoRef = createRef<HTMLVideoElement>();
      useTelevisionControls(
        { enabled: true, onShowGuide: () => undefined, onZap: () => undefined },
        null,
      );
      const chrome: StageChrome = {
        television: true,
        controls: "bar",
        controlsSlot: null,
        fullscreenTarget: null,
        menuSide: "bottom",
        reportPicture: () => undefined,
      };
      return (
        <StageChromeProvider value={chrome}>
          <PlaybackSurface
            channel={channelFixture({ id: "ch-open", name: "Opening Channel", group: "News" })}
            state={playing ? { _tag: "playing" } : { _tag: "starting" }}
            videoKey="ch-open"
            videoRef={videoRef}
            privacyCopy="Private"
            onPlaying={() => undefined}
            volume={1}
            muted={false}
            fullscreen
            nativeVideo
            onVolumeChange={() => undefined}
            onToggleMuted={() => undefined}
            onRequestFullscreen={() => undefined}
            onStop={() => undefined}
            onAutoplayFailure={() => undefined}
          />
        </StageChromeProvider>
      );
    }

    const view = render(<OpeningFullscreen playing={false} />);
    const section = screen.getByRole("region", { name: "Opening Channel" });
    giveBox(section, new DOMRect(0, 0, 960, 540));
    const exit = screen.getByRole("button", { name: "Exit fullscreen" });
    giveBox(exit, new DOMRect(340, 300, 140, 38));
    Object.defineProperty(document, "fullscreenElement", { configurable: true, value: section });
    act(() => document.dispatchEvent(new Event("fullscreenchange")));
    act(() => {
      vi.advanceTimersByTime(50);
    });
    // No picture yet: one action is marked, the way out of fullscreen.
    expect(exit).toHaveFocus();

    view.rerender(<OpeningFullscreen playing />);
    // The picture plays: OK shows the bar again instead of leaving fullscreen.
    expect(section).toHaveFocus();
    act(() => vi.advanceTimersByTime(3_000));
    expect(section).toHaveAttribute("data-controls-visible", "false");
  });

  it("scrolls an offscreen playing row into view during global TV Guide focus recovery", () => {
    vi.useFakeTimers();
    document.documentElement.setAttribute("data-television", "");

    function OffscreenRecoveryHarness() {
      useTelevisionControls(
        { enabled: true, onShowGuide: () => undefined, onZap: () => undefined },
        null,
      );
      return (
        <ProgrammeGuide
          television
          rows={makeRows(4)}
          groups={GROUPS}
          activeGroup={null}
          window={WINDOW}
          now={NOW}
          playingChannel={channelFixture({ id: "ch-4", name: "Channel 4", group: "News", number: 4 }).id}
          variantPreferences={new Map()}
          layout="pocket"
          mode="guide"
          time={null}
          loading={false}
          replacing={false}
          error={null}
          hasMore={false}
          loadingMore={false}
          onSelectGroup={() => undefined}
          onPrefetchGroup={() => undefined}
          excludedGroups={new Set()}
          onSetGroupExcluded={() => undefined}
          onRestoreExcludedGroups={() => undefined}
          onPreparePlayback={() => undefined}
          onTune={() => undefined}
          onTuneVariant={() => undefined}
          onRetry={() => undefined}
          onLoadMore={() => undefined}
          search={null}
          feeds={null}
          status={null}
        />
      );
    }

    render(<OffscreenRecoveryHarness />);
    const board = document.querySelector<HTMLElement>(".programme-guide__board")!;
    const playingButton = screen.getByRole("button", { name: "Tune Channel 4" });
    const playingRow = playingButton.closest<HTMLElement>(".programme-guide__row")!;
    Object.defineProperty(board, "clientHeight", { configurable: true, value: 200 });
    board.scrollTop = 0;
    giveBox(board, new DOMRect(0, 100, 800, 200));
    giveBox(playingRow, new DOMRect(0, 620, 800, 60));
    giveBox(playingButton, new DOMRect(0, 620, 800, 60));

    act(() => {
      vi.advanceTimersByTime(50);
    });
    expect(playingButton).toHaveFocus();
    expect(board.scrollTop).toBeGreaterThan(0);
  });

  it("keeps Back on closed inline search inside the app, preserves open Time row during search Back, reveals offscreen row, and blocks CH+/CH- behind dialogs", () => {
    document.documentElement.setAttribute("data-television", "");
    const port = androidPortFixture();
    const bridge = createAndroidPlayerBridge(port.value);
    const onZap = vi.fn();
    const status: CatalogStatus = clientSchemas.status.parse({
      generation: 1,
      configuration: { configured: true, epgConfigured: false },
      m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
      epg: null,
    });
    const generation = status.generation!;
    const client = {
      search: vi.fn(async () => ({
        ok: true as const,
        value: {
          generation,
          channels: { generation, items: [], next: null },
          programmes: { generation, items: [], next: null },
        },
      })),
      searchChannels: vi.fn(async () => ({
        ok: true as const,
        value: { generation, items: [], next: null },
      })),
    };
    const installedClient = {
      replaceSourceConfiguration: vi.fn(async () => ({ ok: true as const, value: status })),
    } as unknown as Pick<InstalledSparrowClient, "replaceSourceConfiguration">;

    function Harness() {
      const [timeOpen, setTimeOpen] = useState(true);
      useTelevisionControls(
        { enabled: true, onShowGuide: () => undefined, onZap },
        bridge,
      );
      return (
        <QueryClientProvider client={createSparrowQueryClient("installed")}>
          <main className="shell__workspace">
            <ProgrammeGuide
              television
              rows={makeRows(2)}
              groups={GROUPS}
              activeGroup={null}
              window={WINDOW}
              now={NOW}
              playingChannel={channelFixture({ id: "ch-2", name: "Channel 2", group: "Sports", number: 2 }).id}
              variantPreferences={new Map()}
              layout="pocket"
              mode="guide"
              time={{
                open: timeOpen,
                chosen: null,
                options: [new Date("2026-08-30T11:00:00Z")],
                onToggle: () => setTimeOpen((open) => !open),
                onChoose: () => undefined,
              }}
              loading={false}
              replacing={false}
              error={null}
              hasMore={false}
              loadingMore={false}
              onSelectGroup={() => undefined}
              onPrefetchGroup={() => undefined}
              excludedGroups={new Set()}
              onSetGroupExcluded={() => undefined}
              onRestoreExcludedGroups={() => undefined}
              onPreparePlayback={() => undefined}
              onTune={() => undefined}
              onTuneVariant={() => undefined}
              onRetry={() => undefined}
              onLoadMore={() => undefined}
              search={
                <BoardSearch
                  television
                  client={client}
                  generation={status.generation}
                  excludedGroups={new Set()}
                  onGenerationMismatch={() => undefined}
                  onPreparePlayback={() => undefined}
                  onTune={() => undefined}
                />
              }
              feeds={
                <FeedsDialog
                  television
                  runtime="installed"
                  client={installedClient}
                  status={status}
                  refreshing={false}
                  refreshResult={null}
                  latestEvent={null}
                  onRefresh={() => undefined}
                  onApplied={() => undefined}
                />
              }
              status={null}
            />
          </main>
        </QueryClientProvider>
      );
    }

    render(<Harness />);
    const input = screen.getByRole("combobox", { name: "Search channels and programmes" });
    const board = document.querySelector<HTMLElement>(".programme-guide__board")!;
    const playingButton = screen.getByRole("button", { name: "Tune Channel 2" });
    const playingRow = playingButton.closest<HTMLElement>(".programme-guide__row")!;
    Object.defineProperty(board, "clientHeight", { configurable: true, value: 200 });
    giveBox(board, new DOMRect(0, 100, 800, 200));
    giveBox(input, new DOMRect(10, 10, 200, 40));
    giveBox(playingRow, new DOMRect(0, 500, 800, 60));
    giveBox(playingButton, new DOMRect(0, 500, 800, 60));

    // Press Center to edit inline search while Time row is open; composing Escape and ArrowDown
    // must NOT exit edit mode, leave the field, or close the Time row.
    act(() => input.focus());
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input).not.toHaveAttribute("readonly");
    fireEvent.keyDown(input, { key: "Escape", isComposing: true });
    expect(input).not.toHaveAttribute("readonly");
    expect(input).toHaveFocus();
    expect(screen.getByRole("group", { name: "Time" })).toBeInTheDocument();
    fireEvent.keyDown(input, { key: "ArrowDown", isComposing: true });
    expect(input).toHaveFocus();
    expect(input).not.toHaveAttribute("readonly");

    // Non-composing Back exits search edit mode ONLY, Time row stays open!
    act(() => port.deliver("tv:back"));
    expect(input).toHaveAttribute("readonly");
    expect(screen.getByRole("group", { name: "Time" })).toBeInTheDocument();

    // Second Back on closed read-only inline search jumps to playing row and reveals it (Time row STILL open).
    act(() => port.deliver("tv:back"));
    expect(port.sent).not.toContain("tv:default-back");
    expect(playingButton).toHaveFocus();
    expect(board.scrollTop).toBeGreaterThan(0);
    expect(screen.getByRole("group", { name: "Time" })).toBeInTheDocument();

    // Third Back on Channel row now dismisses open Time row cleanly.
    act(() => port.deliver("tv:back"));
    expect(screen.queryByRole("group", { name: "Time" })).toBeNull();
    expect(port.sent).not.toContain("tv:default-back");

    // Opening Sources dialog blocks native CH+/CH- zapping.
    fireEvent.click(screen.getByRole("button", { name: "Sources" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    act(() => port.deliver("tv:channel-up"));
    expect(onZap).not.toHaveBeenCalled();
  });

  it("handles TV pagination error, variant-only pages, and navigated-away guard without stealing focus", () => {
    document.documentElement.setAttribute("data-television", "");
    const baseProps = {
      television: true,
      groups: GROUPS,
      activeGroup: null,
      window: WINDOW,
      now: NOW,
      playingChannel: null,
      variantPreferences: new Map(),
      layout: "pocket" as const,
      mode: "guide" as const,
      time: null,
      loading: false,
      replacing: false,
      onSelectGroup: () => undefined,
      onPrefetchGroup: () => undefined,
      excludedGroups: new Set<string>(),
      onSetGroupExcluded: () => undefined,
      onRestoreExcludedGroups: () => undefined,
      onPreparePlayback: () => undefined,
      onTune: () => undefined,
      onTuneVariant: () => undefined,
      onRetry: () => undefined,
      onLoadMore: () => undefined,
      search: null,
      feeds: null,
      status: null,
    };

    // Non-final variant-only page (0 new visible rows, More stays mounted) keeps focus on More channels.
    const rows = makeRows(2);
    const view = render(
      <ProgrammeGuide {...baseProps} rows={rows} error={null} hasMore loadingMore />,
    );
    const moreNonFinal = screen.getByRole("button", { name: "Loading more channels…" });
    act(() => moreNonFinal.focus());
    view.rerender(
      <ProgrammeGuide {...baseProps} rows={rows} error={null} hasMore loadingMore={false} />,
    );
    expect(screen.getByRole("button", { name: "More channels" })).toHaveFocus();

    // Variant-only final page (same family count, More removed) focuses last visible tile.
    fireEvent.click(screen.getByRole("button", { name: "More channels" }));
    view.rerender(
      <ProgrammeGuide {...baseProps} rows={rows} error={null} hasMore loadingMore />,
    );
    view.rerender(
      <ProgrammeGuide {...baseProps} rows={rows} error={null} hasMore={false} loadingMore={false} />,
    );
    expect(screen.getByRole("button", { name: "Tune Channel 2" })).toHaveFocus();

    // Error completion focuses retained Try again.
    view.rerender(
      <ProgrammeGuide {...baseProps} rows={rows} error={null} hasMore loadingMore />,
    );
    const moreAgain = screen.getByRole("button", { name: "Loading more channels…" });
    act(() => moreAgain.focus());
    view.rerender(
      <ProgrammeGuide
        {...baseProps}
        rows={rows}
        error={{ _tag: "transport", retryable: true, message: "offline" }}
        hasMore
        loadingMore={false}
      />,
    );
    expect(screen.getByRole("button", { name: "Try again" })).toHaveFocus();

    // Navigated-away guard: moving to a row before completion keeps focus on that row.
    view.rerender(
      <ProgrammeGuide {...baseProps} rows={rows} error={null} hasMore loadingMore />,
    );
    const moreThird = screen.getByRole("button", { name: "Loading more channels…" });
    act(() => moreThird.focus());
    const ch1 = screen.getByRole("button", { name: "Tune Channel 1" });
    act(() => ch1.focus());
    view.rerender(
      <ProgrammeGuide {...baseProps} rows={makeRows(4)} error={null} hasMore={false} loadingMore={false} />,
    );
    expect(ch1).toHaveFocus();
  });

  it("keeps Pause/Resume button identity across suspending, and reaches load recovery in the docked box", () => {
    document.documentElement.setAttribute("data-television", "");

    function PlayerWatchAndFullscreen({
      phase,
    }: {
      readonly phase: "playing" | "suspending" | "paused";
    }) {
      const [controlsSlot, setControlsSlot] = useState<HTMLElement | null>(null);
      const videoRef = createRef<HTMLVideoElement>();
      const ch = channelFixture({
        id: "svt-hd",
        name: "SVT1 HD",
        group: "News",
        variant: { quality: "hd", baseName: "SVT1" },
      });
      const chSd = channelFixture({
        id: "svt-sd",
        name: "SVT1 SD",
        group: "News",
        variant: { quality: "sd", baseName: "SVT1" },
      });
      const chrome: StageChrome = {
        television: true,
        controls: "compact",
        controlsSlot,
        fullscreenTarget: null,
        menuSide: "bottom",
        reportPicture: () => undefined,
      };
      return (
        <StageChromeProvider value={chrome}>
          <Stage
            television
            playingChannel={ch.id}
            player={
              <PlaybackSurface
                channel={ch}
                displayName="Pending Zap Channel"
                state={{ _tag: phase === "paused" ? "paused" : "playing" }}
                videoKey={ch.id}
                videoRef={videoRef}
                privacyCopy="Private"
                onPlaying={() => undefined}
                pause={
                  phase === "paused"
                    ? undefined
                    : { onPause: () => undefined, pending: phase === "suspending" }
                }
                recoveryAction={
                  phase === "paused"
                    ? { label: "Resume", onAction: () => undefined }
                    : undefined
                }
                audio={
                  <label
                    className="hosted-player__audio-track"
                  >
                    <span>Audio</span>
                    <select aria-label="Audio track" defaultValue="en">
                      <option value="en">English</option>
                      <option value="sv">Swedish</option>
                    </select>
                  </label>
                }
                volume={1}
                muted={false}
                fullscreen={false}
                nativeVideo
                onVolumeChange={() => undefined}
                onToggleMuted={() => undefined}
                onRequestFullscreen={() => undefined}
                onStop={() => undefined}
                onAutoplayFailure={() => undefined}
              />
            }
            info={
              <NowPlaying
                subject={{
                  channel: ch,
                  variants: [chSd, ch],
                  programmes: [],
                  reading: "done",
                }}
                playingChannel={ch.id}
                now={NOW}
                layout="pocket"
                picture={{ state: "playing", status: "On air", silent: false }}
                controlsRef={setControlsSlot}
                onPreparePlayback={() => undefined}
                onTuneVariant={() => undefined}
              />
            }
            rail={
              <ChannelBar
                stops={[]}
                current={ch.id}
                now={NOW}
                onZap={() => undefined}
                onShowGuide={() => undefined}
              />
            }
            keyHints={false}
            onShowPicture={() => undefined}
          />
        </StageChromeProvider>
      );
    }

    const view = render(<PlayerWatchAndFullscreen phase="playing" />);
    // Pending zap displayName renders immediately in the player heading.
    expect(screen.getByRole("heading", { level: 2, name: "Pending Zap Channel" })).toBeInTheDocument();

    // Volume slider stays mounted in DOM for ADR 0007, removed from TV tab/accessibility flow.
    const volume = document.querySelector<HTMLInputElement>('input[aria-label="Volume"]')!;
    expect(volume).toBeInTheDocument();
    expect(volume).toHaveAttribute("tabindex", "-1");
    expect(volume.closest("label")).toHaveAttribute("aria-hidden", "true");

    // Pause -> suspending -> paused preserves exact same DOM button and focus.
    const primaryButton = screen.getByRole("button", { name: "Pause" });
    act(() => primaryButton.focus());
    view.rerender(<PlayerWatchAndFullscreen phase="suspending" />);
    expect(screen.getByRole("button", { name: "Pause" })).toBe(primaryButton);
    expect(primaryButton).toHaveAttribute("aria-disabled", "true");
    expect(primaryButton).toHaveFocus();
    view.rerender(<PlayerWatchAndFullscreen phase="paused" />);
    expect(screen.getByRole("button", { name: "Resume" })).toBe(primaryButton);
    expect(primaryButton).toHaveFocus();

    // A TV has no watch screen: the docked picture's box claims no arrows
    // for controls the Guide hides.
    act(() => primaryButton.focus());
    const up = fireEvent.keyDown(primaryButton, { key: "ArrowUp" });
    expect(up).toBe(true);
    expect(primaryButton).toHaveFocus();
    view.unmount();

    // Player load-boundary failure: its Reload and Close actions stay in the
    // docked box. Left and Right step between them; Down and Back return to
    // the playing Channel's row instead of leaving the app.
    function ThrowingPlayer(): never {
      throw new Error("chunk failed");
    }
    const ch = channelFixture({ id: "load-1", name: "Load One", group: "News" });
    const spy = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      render(
        <>
          <Stage
            television
            playingChannel={ch.id}
            player={
              <PlaybackLoadBoundary
                resetKey={ch.id}
                onReload={() => undefined}
                onStop={() => undefined}
              >
                <ThrowingPlayer />
              </PlaybackLoadBoundary>
            }
            info={null}
            rail={null}
            keyHints={false}
            onShowPicture={() => undefined}
          />
          <section className="programme-guide">
            <div className="programme-guide__row" data-playing="true">
              <button type="button" className="programme-guide__channel">
                Tune Load One
              </button>
            </div>
          </section>
        </>,
      );
      const row = screen.getByRole("button", { name: "Tune Load One" });
      giveBox(row, new DOMRect(0, 400, 300, 80));
      const reloadBtn = screen.getByRole("button", { name: "Reload Sparrow" });
      const closeBtn = screen.getByRole("button", { name: "Close player" });
      // Center on the Channel, or on the band's caption, finds no picture to
      // take fullscreen: focus goes to the notice's actions instead.
      act(() => row.focus());
      expect(showTelevisionPicture()).toBe("load-notice");
      expect(reloadBtn).toHaveFocus();
      fireEvent.keyDown(reloadBtn, { key: "ArrowRight" });
      expect(closeBtn).toHaveFocus();
      fireEvent.keyDown(closeBtn, { key: "ArrowLeft" });
      expect(reloadBtn).toHaveFocus();
      fireEvent.keyDown(reloadBtn, { key: "ArrowUp" });
      expect(reloadBtn).toHaveFocus();
      fireEvent.keyDown(reloadBtn, { key: "ArrowDown" });
      expect(row).toHaveFocus();
      act(() => closeBtn.focus());
      // Back: the shell's dispatch is cancelled, so Android keeps the app.
      expect(fireEvent.keyDown(closeBtn, { key: "Escape" })).toBe(false);
      expect(row).toHaveFocus();
    } finally {
      spy.mockRestore();
    }
  });

  it("omits Copy diagnostics and PiP on TV InstalledPlayer and suppresses picker activation while audio is replacing", async () => {
    document.documentElement.setAttribute("data-television", "");
    const english = clientSchemas.audioTrackId.parse(`atrk1_${"1".repeat(32)}`);
    const swedish = clientSchemas.audioTrackId.parse(`atrk1_${"2".repeat(32)}`);
    const descriptor = clientSchemas.nativePlaybackDescriptor.parse({
      _tag: "tauri-native-stream",
      sessionId: `play1_${"a".repeat(32)}_1`,
      streamHandle: `stream1_${"b".repeat(16)}`,
      presentation: "android-media3",
      tracks: [
        { id: english, language: "eng", codec: "aac-adts", selected: true },
        { id: swedish, language: "swe", codec: "ac-3", selected: false },
      ],
      selection: { _tag: "selected", trackId: english, reason: "first-available" },
    });
    let resolveRestart: (() => void) | null = null;
    const restart = vi.fn(
      () =>
        new Promise<{ readonly ok: true; readonly value: typeof descriptor }>((resolve) => {
          resolveRestart = () => resolve({ ok: true, value: descriptor });
        }),
    );
    const port = androidPortFixture();
    const bridge = createAndroidPlayerBridge(port.value);
    const chrome: StageChrome = {
      television: true,
      controls: "bar",
      controlsSlot: null,
      fullscreenTarget: null,
      menuSide: "bottom",
      reportPicture: () => undefined,
    };

    render(
      <StageChromeProvider value={chrome}>
        <InstalledPlayer
          channel={channelFixture({ id: "tv-1", name: "TV One", group: "News" })}
          client={{
            capabilities: async () => ({
              ok: true,
              value: {
                sourceConfiguration: "device-writable",
                playbackTransport: "platform-native",
                audioTrackSelection: true,
                mpvFailover: false,
                pictureOverlay: false,
              },
            }),
            createPlaybackSession: () => ({
              start: async () => ({ ok: true, value: descriptor }),
              reopen: async () => ({ ok: true, value: descriptor }),
              restart,
              read: async () => ({ ok: true, value: new ArrayBuffer(0) }),
              startAndroidPresentation: async () => ({
                ok: true,
                value: {
                  status: async () => ({
                    ok: true,
                    value: {
                      state: "playing" as const,
                      decodedFrames: 10,
                      droppedFrames: 0,
                      bufferedDurationMs: 500,
                      silent: false,
                      audio: "device-decoder" as const,
                    },
                  }),
                  pause: async () => ({ ok: true, value: undefined }),
                  resume: async () => ({ ok: true, value: undefined }),
                  setVolume: async () => ({ ok: true, value: undefined }),
                  setMuted: async () => ({ ok: true, value: undefined }),
                  setViewport: async () => ({ ok: true, value: undefined }),
                  stop: async () => ({ ok: true, value: undefined }),
                },
              }),
              controlMpv: async () => ({ ok: true, value: undefined }),
              suspend: async () => ({ ok: true, value: undefined }),
              setActivity: async () => ({ ok: true, value: undefined }),
              stop: async () => ({ ok: true, value: undefined }),
            }),
          }}
          engine={{
            start: ({ onPlaying }) => {
              onPlaying();
              return { stop: () => undefined };
            },
          }}
          lifecycleEvents={{ subscribe: async () => () => undefined }}
          androidPlayer={bridge}
          onStop={() => undefined}
        />
      </StageChromeProvider>,
    );

    expect(await screen.findByText("On air")).toBeVisible();
    act(() => port.deliver("capabilities:picture-in-picture"));
    expect(screen.queryByRole("button", { name: "Copy diagnostics" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Picture in picture" })).toBeNull();

    const audio = screen.getByRole("combobox", { name: "Audio track" });
    act(() => audio.focus());
    fireEvent.change(audio, { target: { value: swedish } });
    expect(audio).not.toBeDisabled();
    expect(audio).toHaveAttribute("aria-disabled", "true");
    const enterEvent = new KeyboardEvent("keydown", {
      key: "Enter",
      bubbles: true,
      cancelable: true,
    });
    audio.dispatchEvent(enterEvent);
    expect(enterEvent.defaultPrevented).toBe(true);
    await act(async () => {
      resolveRestart?.();
    });
  });

  it("keeps a focused folded desk row when a sibling Quality Variant arrives on the next page", async () => {
    document.documentElement.setAttribute("data-television", "");
    const generation = clientSchemas.status.parse({
      generation: 1,
      configuration: { configured: true, epgConfigured: false },
      m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
      epg: null,
    }).generation!;
    const svt = (quality: "sd" | "hd") =>
      channelFixture({
        id: `fold-svt-${quality}`,
        name: `SVT1 ${quality.toUpperCase()}`,
        group: "Sweden",
        number: 5,
        variant: { quality, baseName: "SVT1" },
      });
    const other = channelFixture({ id: "fold-svt2", name: "SVT2", group: "Sweden", number: 6 });
    const page1 = clientSchemas.channelsPage.parse({
      generation: 1,
      items: [svt("hd"), other],
      next: "cursor-2",
    });
    const page2 = clientSchemas.channelsPage.parse({
      generation: 1,
      items: [svt("sd")],
      next: null,
    });
    const client = {
      searchChannels: vi.fn(async () => ({ ok: true as const, value: page2 })),
    };
    const queryClient = createSparrowQueryClient("installed");
    queryClient.setQueryData(
      ["catalog", "search", "desk-channels", "svt", generation, 40],
      { pages: [{ ok: true, value: page1 }], pageParams: [null] },
    );
    const onTune = vi.fn();
    render(
      <QueryClientProvider client={queryClient}>
        <BoardSearchDesk
          television
          client={client}
          generation={generation}
          term="svt"
          excludedGroups={new Set()}
          open
          onOpenChange={() => undefined}
          onTermChange={() => undefined}
          onGenerationMismatch={() => undefined}
          onPreparePlayback={() => undefined}
          returnFocusElement={() => null}
          onTune={onTune}
        />
      </QueryClientProvider>,
    );
    const row = await screen.findByRole("button", { name: "Tune SVT1 HD" });
    await waitFor(() => expect(row).toHaveFocus());

    // Focus near the end of the list reads the next page by itself.
    await waitFor(() => expect(client.searchChannels).toHaveBeenCalledTimes(1));
    // Another of the guide row's variants arrived, sorted first among them:
    // the row is the same button, and it still tunes the best one.
    await waitFor(() =>
      expect(document.querySelector(".board-search__qualities")).toHaveTextContent(/SD.*HD/u),
    );
    expect(row).toBeInTheDocument();
    expect(row).toHaveAccessibleName("Tune SVT1 HD");
    expect(row).toHaveFocus();
    expect(screen.getAllByRole("button", { name: /^Tune SVT1/u })).toHaveLength(1);
  });

  it("handles TV BoardSearchDesk initial/final focus, press-Center-to-edit, and Search again pagination outcome", async () => {
    document.documentElement.setAttribute("data-television", "");
    const generation = clientSchemas.status.parse({
      generation: 1,
      configuration: { configured: true, epgConfigured: false },
      m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
      epg: null,
    }).generation!;
    const ch1 = channelFixture({ id: "s-1", name: "Sports 1", group: "Sports", number: 1 });
    const ch2 = channelFixture({ id: "s-2", name: "Sports 2", group: "Sports", number: 2 });
    const page1 = clientSchemas.channelsPage.parse({
      generation: 1,
      items: [ch1, ch2],
      next: "cursor-2",
    });
    const nextGeneration = clientSchemas.status.parse({
      generation: 2,
      configuration: { configured: true, epgConfigured: false },
      m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
      epg: null,
    }).generation!;
    let resolvePage2:
      | ((value: {
          readonly ok: false;
          readonly error: { readonly _tag: "stale-cursor"; readonly current: typeof nextGeneration };
        }) => void)
      | null = null;
    const client = {
      searchChannels: vi.fn(async () => {
        return await new Promise<{
          readonly ok: false;
          readonly error: { readonly _tag: "stale-cursor"; readonly current: typeof nextGeneration };
        }>((resolve) => {
          resolvePage2 = resolve;
        });
      }),
    };
    const trigger = document.createElement("button");
    trigger.textContent = "Return here";
    document.body.append(trigger);

    const queryClient = createSparrowQueryClient("installed");
    queryClient.setQueryData(
      ["catalog", "search", "desk-channels", "sports", generation, 40],
      { pages: [{ ok: true, value: page1 }], pageParams: [null] },
    );

    function DeskHarness({ television = true }: { readonly television?: boolean }) {
      const [open, setOpen] = useState(true);
      const [term, setTerm] = useState("sports");
      return (
        <QueryClientProvider client={queryClient}>
          <BoardSearchDesk
            television={television}
            client={client}
            generation={generation}
            term={term}
            excludedGroups={new Set()}
            open={open}
            onOpenChange={setOpen}
            onTermChange={setTerm}
            onGenerationMismatch={() => undefined}
            onPreparePlayback={() => undefined}
            returnFocusElement={() => trigger}
            onTune={() => undefined}
          />
        </QueryClientProvider>
      );
    }

    try {
      render(<DeskHarness />);
      const searchInput = screen.getByRole("searchbox", { name: "Search channels" });
      // TV opens with cached results focusing the first result (NOT Close or search input).
      expect(searchInput).toHaveAttribute("readonly");
      const firstPick = await screen.findByRole("button", { name: "Tune Sports 1" });
      await waitFor(() => expect(firstPick).toHaveFocus());
      // More results are coming, so the list does not say it is whole.
      expect(screen.queryByText(/every channel found/u)).toBeNull();

      // Press Center on search input enters edit mode, Escape exits edit mode without closing dialog.
      act(() => searchInput.focus());
      fireEvent.keyDown(searchInput, { key: "Enter" });
      expect(searchInput).not.toHaveAttribute("readonly");
      fireEvent.keyDown(searchInput, { key: "Escape" });
      expect(searchInput).toHaveAttribute("readonly");
      expect(screen.getByRole("dialog")).toBeInTheDocument();

      // Tail prefetch already started page 2 when firstPick focused; focusing busy button still arms handoff.
      const more = await screen.findByRole("button", { name: "Loading more channels…" });
      act(() => more.focus());
      expect(more).toHaveFocus();
      await act(async () => {
        resolvePage2?.({
          ok: false,
          error: { _tag: "stale-cursor", current: nextGeneration },
        });
      });
      const searchAgain = await screen.findByRole("button", { name: "Search again" });
      expect(searchAgain).toHaveFocus();

      // Dismissing dialog returns focus to the resolved toolbar element.
      fireEvent.keyDown(searchAgain, { key: "Escape" });
      expect(screen.queryByRole("dialog")).toBeNull();
      await waitFor(() => expect(trigger).toHaveFocus());

      // Uncached TV open lands on Close while loading, then hands off to first pick once results arrive.
      cleanup();
      let resolveUncached:
        | ((result: { readonly ok: true; readonly value: typeof page1 }) => void)
        | undefined;
      const uncachedClient = {
        searchChannels: vi.fn(
          () =>
            new Promise<{ readonly ok: true; readonly value: typeof page1 }>(
              (resolve) => {
                resolveUncached = resolve;
              },
            ),
        ),
      };
      render(
        <QueryClientProvider client={createSparrowQueryClient("installed")}>
          <BoardSearchDesk
            television
            client={uncachedClient}
            generation={generation}
            term="sports"
            excludedGroups={new Set()}
            open
            onOpenChange={() => undefined}
            onTermChange={() => undefined}
            onGenerationMismatch={() => undefined}
            onPreparePlayback={() => undefined}
            onTune={() => undefined}
          />
        </QueryClientProvider>,
      );
      const closeButton = screen.getByRole("button", {
        name: "Close channel search",
      });
      await waitFor(() => expect(closeButton).toHaveFocus());
      await act(async () => {
        resolveUncached?.({ ok: true, value: page1 });
      });
      const asyncFirstPick = await screen.findByRole("button", {
        name: "Tune Sports 1",
      });
      await waitFor(() => expect(asyncFirstPick).toHaveFocus());

      // Moving focus away from Close before async results arrive prevents stealing focus.
      cleanup();
      let resolveSecondUncached:
        | ((result: { readonly ok: true; readonly value: typeof page1 }) => void)
        | undefined;
      const secondUncachedClient = {
        searchChannels: vi.fn(
          () =>
            new Promise<{ readonly ok: true; readonly value: typeof page1 }>(
              (resolve) => {
                resolveSecondUncached = resolve;
              },
            ),
        ),
      };
      render(
        <QueryClientProvider client={createSparrowQueryClient("installed")}>
          <BoardSearchDesk
            television
            client={secondUncachedClient}
            generation={generation}
            term="sports"
            excludedGroups={new Set()}
            open
            onOpenChange={() => undefined}
            onTermChange={() => undefined}
            onGenerationMismatch={() => undefined}
            onPreparePlayback={() => undefined}
            onTune={() => undefined}
          />
        </QueryClientProvider>,
      );
      const secondClose = screen.getByRole("button", {
        name: "Close channel search",
      });
      await waitFor(() => expect(secondClose).toHaveFocus());
      const secondInput = screen.getByRole("searchbox", {
        name: "Search channels",
      });
      act(() => secondInput.focus());
      expect(secondInput).toHaveFocus();
      await act(async () => {
        resolveSecondUncached?.({ ok: true, value: page1 });
      });
      await screen.findByRole("button", { name: "Tune Sports 1" });
      expect(secondInput).toHaveFocus();

      // Non-TV desktop/phone dialog does NOT intercept Shift+ArrowDown in the search field.
      cleanup();
      render(<DeskHarness television={false} />);
      const desktopInput = screen.getByRole("searchbox", { name: "Search channels" });
      act(() => desktopInput.focus());
      const shiftDown = new KeyboardEvent("keydown", {
        key: "ArrowDown",
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      });
      desktopInput.dispatchEvent(shiftDown);
      expect(shiftDown.defaultPrevented).toBe(false);
    } finally {
      trigger.remove();
    }
  });

  it("ends a whole TV search list with a hint that Up and OK lead to a new search", async () => {
    document.documentElement.setAttribute("data-television", "");
    const generation = clientSchemas.status.parse({
      generation: 1,
      configuration: { configured: true, epgConfigured: false },
      m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
      epg: null,
    }).generation!;
    const page = clientSchemas.channelsPage.parse({
      generation: 1,
      items: [
        channelFixture({ id: "s-1", name: "Sports 1", group: "Sports", number: 1 }),
        channelFixture({ id: "s-2", name: "Sports 2", group: "Sports", number: 2 }),
      ],
      next: null,
    });
    const queryClient = createSparrowQueryClient("installed");
    queryClient.setQueryData(
      ["catalog", "search", "desk-channels", "sports", generation, 40],
      { pages: [{ ok: true, value: page }], pageParams: [null] },
    );

    render(
      <QueryClientProvider client={queryClient}>
        <BoardSearchDesk
          television
          client={{ searchChannels: vi.fn() }}
          generation={generation}
          term="sports"
          excludedGroups={new Set()}
          open
          onOpenChange={() => undefined}
          onTermChange={() => undefined}
          onGenerationMismatch={() => undefined}
          onPreparePlayback={() => undefined}
          onTune={() => undefined}
        />
      </QueryClientProvider>,
    );

    const firstPick = await screen.findByRole("button", { name: "Tune Sports 1" });
    await waitFor(() => expect(firstPick).toHaveFocus());
    expect(
      screen.getByText(
        "That is every channel found. To search again, go up to the search field and press OK.",
      ),
    ).toBeVisible();

    // What the hint says: Up reaches the field, and OK there starts typing.
    fireEvent.keyDown(firstPick, { key: "ArrowUp" });
    const field = screen.getByRole("searchbox", { name: "Search channels" });
    expect(field).toHaveFocus();
    fireEvent.keyDown(field, { key: "Enter" });
    expect(field).not.toHaveAttribute("readonly");
  });

  it("preserves the highlighted suggestion during IME composition in populated inline search", async () => {
    document.documentElement.setAttribute("data-television", "");
    const generation = clientSchemas.status.parse({
      generation: 1,
      configuration: { configured: true, epgConfigured: false },
      m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
      epg: null,
    }).generation!;
    const ch1 = channelFixture({ id: "s-1", name: "Sports 1", group: "Sports", number: 1 });
    const ch2 = channelFixture({ id: "s-2", name: "Sports 2", group: "Sports", number: 2 });
    const client = {
      search: vi.fn(async () => ({
        ok: true as const,
        value: {
          generation,
          channels: { generation, items: [ch1, ch2], next: null },
          programmes: { generation, items: [], next: null },
        },
      })),
      searchChannels: vi.fn(async () => ({
        ok: true as const,
        value: { generation, items: [ch1, ch2], next: null },
      })),
    };

    const onTune = vi.fn();
    render(
      <QueryClientProvider client={createSparrowQueryClient("installed")}>
        <BoardSearch
          television
          client={client}
          generation={generation}
          excludedGroups={new Set()}
          onGenerationMismatch={() => undefined}
          onPreparePlayback={() => undefined}
          onTune={onTune}
        />
      </QueryClientProvider>,
    );

    const input = screen.getByRole("combobox", { name: "Search channels and programmes" });
    act(() => input.focus());
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.change(input, { target: { value: "Sports" } });

    await screen.findByText("Sports 1");
    await screen.findByText("Sports 2");
    await waitFor(() =>
      expect(input.getAttribute("aria-activedescendant")).toBeTruthy(),
    );
    const activeBeforeComposition = input.getAttribute("aria-activedescendant");

    // Composing ArrowDown must stay on input and preserve Base UI's highlighted suggestion.
    fireEvent.compositionStart(input);
    fireEvent.keyDown(input, { key: "ArrowDown", isComposing: true, which: 40 });
    expect(input).toHaveFocus();
    expect(input.getAttribute("aria-activedescendant")).toBe(
      activeBeforeComposition,
    );

    // Composing Escape must not close suggestions or exit edit mode.
    fireEvent.keyDown(input, { key: "Escape", isComposing: true, which: 27 });
    expect(input).toHaveFocus();
    expect(input).not.toHaveAttribute("readonly");
    expect(input.getAttribute("aria-expanded")).toBe("true");
    expect(input.getAttribute("aria-activedescendant")).toBe(
      activeBeforeComposition,
    );
    fireEvent.compositionEnd(input);

    // On TV, non-composing ArrowDown moves real DOM focus into the option list (setting
    // input readOnly) so Android TV KEYCODE_DPAD_CENTER activates suggestions instead of
    // re-opening the soft keyboard.
    fireEvent.keyDown(input, { key: "ArrowDown", which: 40 });
    const options = screen.getAllByRole("option");
    expect(options[0]).toHaveFocus();
    expect(input).toHaveAttribute("readonly");

    // ArrowUp from the top option returns to the read-only input cleanly.
    fireEvent.keyDown(options[0], { key: "ArrowUp" });
    expect(input).toHaveFocus();
    expect(input).toHaveAttribute("readonly");

    // ArrowDown into options, step to Sports 1, and press Enter -> tunes Sports 1 directly.
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(options[0]).toHaveFocus();
    fireEvent.keyDown(options[0], { key: "ArrowDown" });
    expect(options[1]).toHaveFocus();
    fireEvent.keyDown(options[1], { key: "Enter" });
    expect(onTune).toHaveBeenCalledWith(ch1);

    // Non-TV composition guard still prevents Base UI's virtual activeDescendant from advancing.
    cleanup();
    document.documentElement.removeAttribute("data-television");
    render(
      <QueryClientProvider client={createSparrowQueryClient("installed")}>
        <BoardSearch
          television={false}
          client={client}
          generation={generation}
          excludedGroups={new Set()}
          onGenerationMismatch={() => undefined}
          onPreparePlayback={() => undefined}
          onTune={() => undefined}
        />
      </QueryClientProvider>,
    );
    const desktopInput = screen.getByRole("combobox", { name: "Search channels and programmes" });
    act(() => desktopInput.focus());
    fireEvent.change(desktopInput, { target: { value: "Sports" } });
    await screen.findByText("Sports 1");
    await waitFor(() =>
      expect(desktopInput.getAttribute("aria-activedescendant")).toBeTruthy(),
    );
    const firstDesktopActive = desktopInput.getAttribute("aria-activedescendant");
    fireEvent.keyDown(desktopInput, { key: "ArrowDown", which: 40 });
    if (desktopInput.getAttribute("aria-activedescendant") === firstDesktopActive) {
      fireEvent.keyDown(desktopInput, { key: "ArrowDown", which: 40 });
    }
    const trackedActive = desktopInput.getAttribute("aria-activedescendant");
    expect(trackedActive).not.toBe(firstDesktopActive);
    fireEvent.compositionStart(desktopInput);
    fireEvent.keyDown(desktopInput, { key: "ArrowDown", isComposing: true, which: 40 });
    expect(desktopInput.getAttribute("aria-activedescendant")).toBe(trackedActive);
    fireEvent.compositionEnd(desktopInput);
    fireEvent.keyDown(desktopInput, { key: "ArrowDown", which: 40 });
    await waitFor(() =>
      expect(desktopInput.getAttribute("aria-activedescendant")).not.toBe(trackedActive),
    );
  });

  it("steps through large BoardSearchDesk result lists lazily without reading styles for every result", async () => {
    document.documentElement.setAttribute("data-television", "");
    const generation = clientSchemas.status.parse({
      generation: 1,
      configuration: { configured: true, epgConfigured: false },
      m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
      epg: null,
    }).generation!;
    const channels = Array.from({ length: 60 }, (_, idx) =>
      channelFixture({
        id: `desk-${idx + 1}`,
        name: `Desk Channel ${idx + 1}`,
        group: "News",
        number: idx + 1,
      }),
    );
    const page1 = clientSchemas.channelsPage.parse({
      generation: 1,
      items: channels,
      next: null,
    });
    const queryClient = createSparrowQueryClient("installed");
    queryClient.setQueryData(
      ["catalog", "search", "desk-channels", "desk", generation, 40],
      { pages: [{ ok: true, value: page1 }], pageParams: [null] },
    );
    const client = {
      searchChannels: vi.fn(async () => ({ ok: true as const, value: page1 })),
    };

    render(
      <QueryClientProvider client={queryClient}>
        <BoardSearchDesk
          television
          client={client}
          generation={generation}
          term="desk"
          excludedGroups={new Set()}
          open
          onOpenChange={() => undefined}
          onTermChange={() => undefined}
          onGenerationMismatch={() => undefined}
          onPreparePlayback={() => undefined}
          onTune={() => undefined}
        />
      </QueryClientProvider>,
    );

    const first = await screen.findByRole("button", { name: "Tune Desk Channel 1" });
    const second = screen.getByRole("button", { name: "Tune Desk Channel 2" });
    const third = screen.getByRole("button", { name: "Tune Desk Channel 3" });
    await waitFor(() => expect(first).toHaveFocus());

    // Mark second result hidden to also prove lazy walking still skips ineligible controls.
    const styleSpy = vi.spyOn(window, "getComputedStyle");
    try {
      styleSpy.mockClear();
      fireEvent.keyDown(first, { key: "ArrowDown" });
      expect(second).toHaveFocus();
      expect(styleSpy.mock.calls.length).toBeLessThan(25);

      second.setAttribute("hidden", "");
      act(() => first.focus());
      styleSpy.mockClear();
      fireEvent.keyDown(first, { key: "ArrowDown" });
      expect(third).toHaveFocus();
      expect(styleSpy.mock.calls.length).toBeLessThan(25);
    } finally {
      styleSpy.mockRestore();
    }
  });

  it("handles TV GroupRosterDialog active-row focus, 2-column arrows, Show all handoff, and FeedsDialog TV busy guards", async () => {
    document.documentElement.setAttribute("data-television", "");
    const onRestoreAll = vi.fn();
    const onRefresh = vi.fn();

    function RosterAndFeeds({ refreshing }: { readonly refreshing: boolean }) {
      const [excluded, setExcluded] = useState<ReadonlySet<string>>(new Set(["Sports"]));
      const status = clientSchemas.status.parse({
        generation: 1,
        configuration: { configured: true, epgConfigured: false },
        m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
        epg: null,
      });
      return (
        <div>
          <GroupRosterDialog
            television
            groups={GROUPS}
            activeGroup="News"
            excluded={excluded}
            onSelectGroup={() => undefined}
            onPrefetchGroup={() => undefined}
            onSetExcluded={() => undefined}
            onRestoreAll={() => {
              onRestoreAll();
              setExcluded(new Set());
            }}
          />
          <FeedsDialog
            television
            runtime="installed"
            client={{
              replaceSourceConfiguration: async () => ({ ok: true, value: status }),
            }}
            status={status}
            refreshing={refreshing}
            refreshResult={null}
            latestEvent={null}
            onRefresh={onRefresh}
            onApplied={() => undefined}
          />
        </div>
      );
    }

    const view = render(<RosterAndFeeds refreshing />);
    const chooseGroups = screen.getByRole("button", { name: "Choose groups" });
    fireEvent.click(chooseGroups);
    const newsPick = await screen.findByRole("button", { name: "News, 4 channels" });
    await waitFor(() => expect(newsPick).toHaveFocus());

    // 2-column arrow moves to sibling Hide button; ArrowUp from the FIRST Hide button reaches Show all!
    fireEvent.keyDown(newsPick, { key: "ArrowRight" });
    const hideNews = screen.getByRole("button", { name: "Hide News" });
    expect(hideNews).toHaveFocus();
    const showAll = screen.getByRole("button", { name: "Show all" });
    fireEvent.keyDown(hideNews, { key: "ArrowUp" });
    expect(showAll).toHaveFocus();
    // ArrowDown from Show all enters the All channel groups row; ArrowDown reaches News; ArrowLeft from Hide returns to pick.
    fireEvent.keyDown(showAll, { key: "ArrowDown" });
    expect(screen.getByRole("button", { name: "All channel groups" })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole("button", { name: "All channel groups" }), { key: "ArrowDown" });
    expect(newsPick).toHaveFocus();
    fireEvent.keyDown(newsPick, { key: "ArrowRight" });
    expect(hideNews).toHaveFocus();
    fireEvent.keyDown(hideNews, { key: "ArrowLeft" });
    expect(newsPick).toHaveFocus();

    // Clicking Show all hands focus to an offscreen active pick AND reveals it inside the list scroller.
    const rosterList = document.querySelector<HTMLElement>(".group-roster__list")!;
    Object.defineProperty(rosterList, "clientHeight", { configurable: true, value: 180 });
    rosterList.scrollTop = 0;
    giveBox(rosterList, new DOMRect(0, 100, 400, 180));
    giveBox(newsPick, new DOMRect(0, 520, 300, 44));
    act(() => showAll.focus());
    fireEvent.click(showAll);
    expect(onRestoreAll).toHaveBeenCalledTimes(1);
    expect(newsPick).toHaveFocus();
    expect(rosterList.scrollTop).toBeGreaterThan(0);

    // With Show all gone (nothing excluded), ArrowUp from the first Hide button falls back to search input.
    fireEvent.keyDown(newsPick, { key: "ArrowRight" });
    expect(hideNews).toHaveFocus();
    fireEvent.keyDown(hideNews, { key: "ArrowUp" });
    expect(screen.getByRole("searchbox", { name: "Search channel groups" })).toHaveFocus();

    fireEvent.keyDown(newsPick, { key: "Escape" });
    await waitFor(() => expect(chooseGroups).toHaveFocus());

    // Sources dialog: TV noValidate, inputs start readOnly so D-pad browsing doesn't open IME,
    // first Enter enters edit mode, composing Enter stays on M3U, next Enter advances to EPG (editing) and reveals it.
    const sourcesTrigger = screen.getByRole("button", { name: "Sources" });
    fireEvent.click(sourcesTrigger);
    const m3u = await screen.findByLabelText("Channel source (required)");
    const epg = screen.getByLabelText("Guide source (optional)");
    const feedsScroller = document.querySelector<HTMLElement>(".feeds-dialog__content")!;
    Object.defineProperty(feedsScroller, "clientHeight", { configurable: true, value: 160 });
    feedsScroller.scrollTop = 0;
    giveBox(feedsScroller, new DOMRect(0, 80, 500, 160));
    giveBox(epg, new DOMRect(0, 420, 420, 40));
    expect(m3u.closest("form")).toHaveAttribute("novalidate");
    expect(m3u).toHaveAttribute("readonly");
    expect(epg).toHaveAttribute("readonly");
    act(() => m3u.focus());
    fireEvent.keyDown(m3u, { key: "Enter" });
    expect(m3u).not.toHaveAttribute("readonly");
    expect(m3u).toHaveFocus();
    fireEvent.keyDown(m3u, { key: "Enter", isComposing: true });
    expect(m3u).toHaveFocus();
    expect(feedsScroller.scrollTop).toBe(0);
    fireEvent.keyDown(m3u, { key: "Enter" });
    expect(epg).toHaveFocus();
    expect(epg).not.toHaveAttribute("readonly");
    expect(feedsScroller.scrollTop).toBeGreaterThan(0);

    const refresh = screen.getByRole("button", { name: "Refresh in progress" });
    expect(refresh).not.toBeDisabled();
    expect(refresh).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(refresh);
    expect(onRefresh).not.toHaveBeenCalled();

    // Clipboard failure on TV shows "Clipboard unavailable".
    const origClipboard = navigator.clipboard;
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });
    try {
      fireEvent.click(screen.getByText("Diagnostics you can copy"));
      fireEvent.click(screen.getByRole("button", { name: "Copy diagnostics" }));
      expect(await screen.findByRole("button", { name: "Clipboard unavailable" })).toBeVisible();
    } finally {
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: origClipboard });
    }

    fireEvent.keyDown(refresh, { key: "Escape" });
    await waitFor(() => expect(sourcesTrigger).toHaveFocus());
    view.unmount();
  });
});
