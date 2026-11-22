import { afterEach, describe, expect, it, vi } from "vitest";
import { handleGuideArrowKey, settleGuideNavigation } from "./guide-navigation";

afterEach(() => document.body.replaceChildren());

function boardFixture(columns: number, timeline = false) {
  const section = document.body.appendChild(document.createElement("section"));
  const board = section.appendChild(document.createElement("div"));
  board.className = "programme-guide__board";
  if (timeline) board.dataset.view = "timeline";
  const onRequestMore = vi.fn();
  const revealIndex = vi.fn();
  let familyCount = 0;
  const add = (index: number) => {
    const row = board.appendChild(document.createElement("div"));
    row.className = "programme-guide__row";
    row.dataset.guideIndex = String(index);
    familyCount = Math.max(familyCount, index + 1);
    const channel = row.appendChild(document.createElement("button"));
    channel.className = "programme-guide__channel";
    return {
      channel,
      cell(from: number, to: number) {
        const button = row.appendChild(document.createElement("button"));
        button.className = "programme-guide__programme";
        button.dataset.start = String(from);
        button.dataset.end = String(to);
        return button;
      },
    };
  };
  section.addEventListener("keydown", (event) => handleGuideArrowKey(event, {
    section, board, layout: "pocket", mode: "guide", television: true, hasMore: true,
    televisionWindow: { columns, familyCount, revealIndex }, onRequestMore,
  }));
  const key = (key: string) => document.activeElement?.dispatchEvent(
    new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
  );
  const settle = (hasMore = false) => settleGuideNavigation(board, {
    familyCount, busy: false, hasError: false, hasMore, onRequestMore, revealIndex,
  });
  return { add, key, settle, onRequestMore, revealIndex };
}

describe("TV Guide navigation across a page boundary", () => {
  it("keeps focus on the Channel and coalesces repeated Down presses into the requested column and distance", () => {
    const fixture = boardFixture(3);
    const rows = Array.from({ length: 5 }, (_, i) => fixture.add(i));
    rows[2].channel.focus();
    fixture.key("ArrowDown");
    fixture.key("ArrowDown");
    expect(rows[2].channel).toHaveFocus();
    expect(fixture.onRequestMore).toHaveBeenCalledTimes(1);
    fixture.add(5);
    fixture.add(6);
    fixture.add(7);
    const next = fixture.add(8);
    fixture.settle();
    expect(next.channel).toHaveFocus();
  });

  it("bounds queued movement when a remote is held throughout a slow page fetch", () => {
    const fixture = boardFixture(1);
    fixture.add(0).channel.focus();
    for (let press = 0; press < 40; press++) fixture.key("ArrowDown");
    const page = Array.from({ length: 20 }, (_, i) => fixture.add(i + 1));
    fixture.settle();
    expect(fixture.onRequestMore).toHaveBeenCalledTimes(1);
    expect(page[7].channel).toHaveFocus();
  });

  it("retains the original Timeline minute when a delayed page arrives", () => {
    const fixture = boardFixture(1, true);
    const start = fixture.add(0).cell(75, 100);
    start.focus();
    fixture.key("ArrowDown");
    expect(start).toHaveFocus();
    const next = fixture.add(1);
    next.cell(0, 60);
    const showingAt75 = next.cell(60, 120);
    fixture.settle();
    expect(showingAt75).toHaveFocus();
  });

  it("a reversal cancels a pending move and skips no Channels when the data arrives", () => {
    const fixture = boardFixture(1);
    const previous = fixture.add(0).channel;
    const end = fixture.add(1).channel;
    end.focus();
    fixture.key("ArrowDown");
    fixture.key("ArrowUp");
    expect(previous).toHaveFocus();
    fixture.add(2);
    fixture.settle();
    expect(previous).toHaveFocus();
  });

  it("reveals the logical neighbour across an unmounted gap instead of focusing a distant pinned row", () => {
    const fixture = boardFixture(3);
    const first = fixture.add(0).channel;
    fixture.add(23);
    first.focus();
    fixture.key("ArrowDown");
    expect(fixture.revealIndex).toHaveBeenCalledWith(3);
    expect(fixture.onRequestMore).not.toHaveBeenCalled();
    expect(first).toHaveFocus();
    const destination = fixture.add(3);
    fixture.settle(true);
    expect(destination.channel).toHaveFocus();
  });
});
