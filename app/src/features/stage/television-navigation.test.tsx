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
    const undock = screen.getByRole("button", { name: "Back to the picture" });
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
    fireEvent.keyDown(cell("Morning"), { key: "ArrowDown" });
    expect(cell("Feature")).toHaveFocus();
    // At the first minute shown there is no earlier time to bring into view,
    // so Left from the cell drawn across the sliver reaches the rail.
    fireEvent.keyDown(cell("Feature"), { key: "ArrowLeft" });
    expect(screen.getByRole("radio", { name: "All" })).toHaveFocus();
  });

  it("changes Channel with Up and Down on the TV Watch control row, and keeps Left and Right for the row", () => {
    document.documentElement.setAttribute("data-television", "");
    const onZap = vi.fn();
    render(
      <Stage
        television
        playingChannel={channelFixture({ id: "ch-1", name: "Channel 1", group: "News" }).id}
        player={null}
        info={
          <div className="now-playing">
            <div className="now-playing__controls">
              <button type="button">Pause</button>
              <button type="button">Full screen</button>
            </div>
          </div>
        }
        rail={null}
        keyHints={false}
        onShowPicture={() => undefined}
        onZap={onZap}
      />,
    );
    const pause = screen.getByRole("button", { name: "Pause" });
    const fullscreen = screen.getByRole("button", { name: "Full screen" });
    act(() => fullscreen.focus());
    fireEvent.keyDown(fullscreen, { key: "ArrowUp" });
    expect(onZap).toHaveBeenLastCalledWith(-1);
    fireEvent.keyDown(fullscreen, { key: "ArrowDown" });
    expect(onZap).toHaveBeenLastCalledWith(1);
    expect(fullscreen).toHaveFocus();
    fireEvent.keyDown(fullscreen, { key: "ArrowLeft" });
    expect(pause).toHaveFocus();
    expect(onZap).toHaveBeenCalledTimes(2);
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

  it("wakes fullscreen controls on Center when hidden, dismisses visible controls on Back, and exits fullscreen on hidden Escape or Back", async () => {
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
        { enabled: true, mode: "watch", onShowGuide, onZap },
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

    // Reveal again, then A2: first Back hides visible controls instead of exiting fullscreen.
    fireEvent.keyDown(section, { key: "ArrowRight" });
    expect(section).toHaveAttribute("data-controls-visible", "true");
    act(() => vi.advanceTimersByTime(50));
    expect(pauseButton).toHaveFocus();
    act(() => port.deliver("tv:back"));
    expect(section).toHaveAttribute("data-controls-visible", "false");
    expect(exitFullscreen).not.toHaveBeenCalled();
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

    // A2 (Blocker R1 #1): DOM Escape while controls are hidden exits fullscreen to Guide without revealing controls.
    fireEvent.keyDown(section, { key: "Escape" });
    expect(section).toHaveAttribute("data-controls-visible", "false");
    expect(exitFullscreen).toHaveBeenCalledTimes(1);
    await act(async () => {
      await Promise.resolve();
    });
    expect(onShowGuide).toHaveBeenCalledTimes(1);

    // Re-enter fullscreen and verify native tv:back while hidden also exits to Guide cleanly.
    Object.defineProperty(document, "fullscreenElement", { configurable: true, value: section });
    act(() => {
      document.dispatchEvent(new Event("fullscreenchange"));
    });
    act(() => vi.advanceTimersByTime(3_000));
    expect(section).toHaveAttribute("data-controls-visible", "false");
    act(() => port.deliver("tv:back"));
    expect(section).toHaveAttribute("data-controls-visible", "false");
    expect(exitFullscreen).toHaveBeenCalledTimes(2);
    await act(async () => {
      await Promise.resolve();
    });
    expect(onShowGuide).toHaveBeenCalledTimes(2);
  });

  it("moves fullscreen focus from section to Close player on non-retryable failure", () => {
    vi.useFakeTimers();
    document.documentElement.setAttribute("data-television", "");

    function FailingFullscreen({ failed }: { readonly failed: boolean }) {
      const videoRef = createRef<HTMLVideoElement>();
      useTelevisionControls(
        { enabled: true, mode: "watch", onShowGuide: () => undefined, onZap: () => undefined },
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
        { enabled: true, mode: "watch", onShowGuide: () => undefined, onZap: () => undefined },
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

  it("restores focus to Watch Full screen when Exit fullscreen is clicked while playing and fullscreen prop updates", () => {
    vi.useFakeTimers();
    document.documentElement.setAttribute("data-television", "");

    function ExplicitExitWatchHarness() {
      const [controlsSlot, setControlsSlot] = useState<HTMLElement | null>(null);
      const [fullscreen, setFullscreen] = useState(true);
      const videoRef = createRef<HTMLVideoElement>();
      const ch = channelFixture({ id: "ch-exit", name: "Exit Channel", group: "News" });
      useTelevisionControls(
        { enabled: true, mode: "watch", onShowGuide: () => undefined, onZap: () => undefined },
        null,
      );
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
                state={{ _tag: "playing" }}
                videoKey={ch.id}
                videoRef={videoRef}
                privacyCopy="Private"
                onPlaying={() => undefined}
                pause={{ onPause: () => undefined }}
                volume={1}
                muted={false}
                fullscreen={fullscreen}
                nativeVideo
                onVolumeChange={() => undefined}
                onToggleMuted={() => undefined}
                onRequestFullscreen={() => {
                  Reflect.deleteProperty(document, "fullscreenElement");
                  setFullscreen(false);
                  document.dispatchEvent(new Event("fullscreenchange"));
                }}
                onStop={() => undefined}
                onAutoplayFailure={() => undefined}
              />
            }
            info={
              <NowPlaying
                subject={{
                  channel: ch,
                  variants: [ch],
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
            rail={null}
            keyHints={false}
            onShowPicture={() => undefined}
          />
        </StageChromeProvider>
      );
    }

    render(<ExplicitExitWatchHarness />);
    const section = document.querySelector<HTMLElement>("section.hosted-player")!;
    giveBox(section, new DOMRect(0, 0, 960, 540));
    Object.defineProperty(document, "fullscreenElement", { configurable: true, value: section });
    act(() => document.dispatchEvent(new Event("fullscreenchange")));

    const exitButton = screen.getByRole("button", { name: "Exit fullscreen" });
    act(() => exitButton.focus());
    fireEvent.click(exitButton);

    const watchFullscreenButton = screen.getByRole("button", { name: "Full screen" });
    giveBox(watchFullscreenButton, new DOMRect(120, 420, 40, 40));
    expect(watchFullscreenButton.closest(".now-playing__controls")).not.toBeNull();
    expect(watchFullscreenButton).toHaveFocus();
    // Stays focused after scheduled rAFs and hide-timer cleanups settle.
    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(watchFullscreenButton).toHaveFocus();
  });

  it("scrolls an offscreen playing row into view during global TV Guide focus recovery", () => {
    vi.useFakeTimers();
    document.documentElement.setAttribute("data-television", "");

    function OffscreenRecoveryHarness() {
      useTelevisionControls(
        { enabled: true, mode: "guide", onShowGuide: () => undefined, onZap: () => undefined },
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
        { enabled: true, mode: "guide", onShowGuide: () => undefined, onZap },
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

  it("routes stacked paused/failed Guide (data-dock=false) through lane and toolbar instead of jumping over them", () => {
    document.documentElement.setAttribute("data-television", "");
    const ch1 = channelFixture({ id: "ch-1", name: "Channel 1", group: "News", number: 1 });
    render(
      <div className="shell" data-layout="pocket" data-mode="guide" data-dock="false" data-playing="true">
        <main className="shell__workspace">
          <Stage
            television
            playingChannel={ch1.id}
            player={null}
            info={null}
            rail={null}
            keyHints={false}
            onShowPicture={() => undefined}
          />
          <ProgrammeGuide
            television
            rows={makeRows(3)}
            groups={GROUPS}
            activeGroup={null}
            window={WINDOW}
            now={NOW}
            playingChannel={ch1.id}
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
            feeds={<button type="button" className="feeds-trigger">Sources</button>}
            status={null}
          />
        </main>
      </div>,
    );

    const workspace = document.querySelector<HTMLElement>(".shell__workspace")!;
    const stage = document.querySelector<HTMLElement>(".stage")!;
    const undock = screen.getByRole("button", { name: "Back to the picture" });
    const sources = screen.getByRole("button", { name: "Sources" });
    const allRadio = screen.getByRole("radio", { name: "All" });
    const tile1 = screen.getByRole("button", { name: "Tune Channel 1" });
    giveBox(workspace, new DOMRect(0, 0, 1200, 800));
    giveBox(stage, new DOMRect(0, 0, 1200, 300));
    giveBox(undock, new DOMRect(10, 250, 120, 40));
    giveBox(sources, new DOMRect(1000, 310, 120, 40));
    giveBox(allRadio, new DOMRect(10, 360, 80, 40));
    giveBox(tile1, new DOMRect(0, 410, 400, 80));

    // Row 0 -> ArrowUp -> Channel Group lane -> ArrowUp -> Toolbar -> ArrowUp -> stacked preview -> ArrowDown -> Toolbar.
    act(() => tile1.focus());
    fireEvent.keyDown(tile1, { key: "ArrowUp" });
    expect(allRadio).toHaveFocus();
    fireEvent.keyDown(allRadio, { key: "ArrowUp" });
    expect(sources).toHaveFocus();
    fireEvent.keyDown(sources, { key: "ArrowUp" });
    expect(undock).toHaveFocus();
    fireEvent.keyDown(undock, { key: "ArrowDown" });
    expect(sources).toHaveFocus();
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

  it("keeps Pause/Resume button identity across suspending, supports TV Watch controls, skips hidden-ancestor Audio select, and reaches load recovery", () => {
    document.documentElement.setAttribute("data-television", "");

    function PlayerWatchAndFullscreen({
      phase,
      hideAudioWrapper = false,
    }: {
      readonly phase: "playing" | "suspending" | "paused";
      readonly hideAudioWrapper?: boolean;
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
                    style={hideAudioWrapper ? { display: "none" } : undefined}
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

    // Watch-mode TV arrows navigate across QualitySwitch <-> controls (incl. closed Audio select) <-> ChannelBar.
    const audio = screen.getByRole("combobox", { name: "Audio track" }) as HTMLSelectElement;
    act(() => audio.focus());
    fireEvent.keyDown(audio, { key: "ArrowDown" });
    expect(screen.getByRole("button", { name: "Guide" })).toHaveFocus();
    expect(audio.value).toBe("en");
    fireEvent.keyDown(screen.getByRole("button", { name: "Guide" }), { key: "ArrowUp" });
    expect(primaryButton).toHaveFocus();
    fireEvent.keyDown(primaryButton, { key: "ArrowUp" });
    expect(screen.getByRole("button", { name: "Tune SVT1 HD" })).toHaveFocus();

    // Narrow Watch mode: when .hosted-player__audio-track is display:none, ArrowRight from Mute skips select to Full screen.
    view.rerender(<PlayerWatchAndFullscreen phase="paused" hideAudioWrapper />);
    const mute = screen.getByRole("button", { name: "Mute" });
    act(() => mute.focus());
    fireEvent.keyDown(mute, { key: "ArrowRight" });
    expect(screen.getByRole("button", { name: "Full screen" })).toHaveFocus();
    view.unmount();

    // Player load-boundary failure: ChannelBar ArrowUp reaches Reload Sparrow / Close player.
    function ThrowingPlayer(): never {
      throw new Error("chunk failed");
    }
    const ch = channelFixture({ id: "load-1", name: "Load One", group: "News" });
    const spy = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      render(
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
        />,
      );
      const guideBtn = screen.getByRole("button", { name: "Guide" });
      act(() => guideBtn.focus());
      fireEvent.keyDown(guideBtn, { key: "ArrowUp" });
      const reloadBtn = screen.getByRole("button", { name: "Reload Sparrow" });
      expect(reloadBtn).toHaveFocus();
      fireEvent.keyDown(reloadBtn, { key: "ArrowRight" });
      const closeBtn = screen.getByRole("button", { name: "Close player" });
      expect(closeBtn).toHaveFocus();
      fireEvent.keyDown(closeBtn, { key: "ArrowDown" });
      expect(guideBtn).toHaveFocus();
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
