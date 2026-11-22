import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clientSchemas,
  type GuideProgramme,
  type ScheduleInput,
} from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import type { GuideFamily } from "./guide-families";
import { GuidePreview } from "./guide-preview";
import { createPreviewStore } from "./guide-preview-store";
import { clockLabel, programmeKeys } from "./guide-window";

afterEach(cleanup);

const NOW = new Date("2026-11-22T08:04:00Z");

function programme(title: string, startsAt: string, endsAt: string): GuideProgramme {
  return {
    title,
    titleTruncated: false,
    startsAt: clientSchemas.isoInstant.parse(startsAt),
    endsAt: clientSchemas.isoInstant.parse(endsAt),
  };
}

const LIVE = programme("Morgon", "2026-11-22T07:30:00Z", "2026-11-22T08:30:00Z");
const LATER = programme("Fiskeläget", "2026-11-22T11:45:00Z", "2026-11-22T12:40:00Z");

const FAMILY: GuideFamily = {
  number: 1,
  title: "SVT1",
  group: "Sverige",
  variants: [
    {
      channel: channelFixture({ id: "svt1", name: "SVT1", group: "Sverige", number: 1 }),
      programmes: [LIVE, LATER],
      programmesTruncated: false,
    },
  ],
};

/** The key the Timeline gives the cell of `family`'s `index`-th Programme. */
function cellKey(family: GuideFamily, index: number): string {
  return programmeKeys(family.variants[0]?.programmes ?? [])[index] as string;
}

function renderPreview(focused: number | null) {
  const store = createPreviewStore();
  store.set({ number: 1, programme: focused === null ? null : cellKey(FAMILY, focused) });
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <GuidePreview
        source={{ client: { schedule: () => new Promise(() => undefined) }, generation: null }}
        store={store}
        families={[FAMILY]}
        variantPreferences={new Map()}
        playingChannel={null}
        now={NOW}
      />
    </QueryClientProvider>,
  );
}

function meter(container: HTMLElement): HTMLElement {
  const element = container.querySelector<HTMLElement>(".programme-guide__preview-meter");
  if (element === null) throw new Error("no meter");
  return element;
}

describe("GuidePreview", () => {
  it("shows a Programme on now with its progress and the minutes left", () => {
    const { container } = renderPreview(null);

    expect(meter(container)).toHaveAttribute("data-when", "live");
    expect(container.querySelector(".programme-guide__preview-progress")).not.toBeNull();
    expect(meter(container)).toHaveTextContent("26 min left");
  });

  it("gives a Programme not on yet a start cue instead of an empty bar", () => {
    const { container } = renderPreview(1);

    expect(meter(container)).toHaveAttribute("data-when", "later");
    expect(container.querySelector(".programme-guide__preview-progress")).toBeNull();
    expect(meter(container)).toHaveTextContent(
      `Starts ${clockLabel(LATER.startsAt)}·in 3 h 41 min55 min`,
    );
  });
});

describe("GuidePreview's description", () => {
  const generation = clientSchemas.status.parse({
    generation: 7,
    configuration: { configured: true, epgConfigured: true },
    m3u: { _tag: "fresh", validatedAt: "2026-11-22T07:00:00Z" },
    epg: null,
  }).generation!;
  const summary = (
    channelId: string,
    item: GuideProgramme,
    description: string,
  ) => ({
    channelId,
    title: item.title,
    description,
    startsAt: item.startsAt,
    endsAt: item.endsAt,
  });

  function renderWith(
    family: GuideFamily,
    schedule: (input: ScheduleInput) => Promise<unknown>,
    focused: number | null = null,
  ) {
    const store = createPreviewStore();
    store.set({
      number: family.number,
      programme: focused === null ? null : cellKey(family, focused),
    });
    return render(
      <QueryClientProvider client={new QueryClient()}>
        <GuidePreview
          source={{
            client: { schedule: schedule as never },
            generation,
          }}
          store={store}
          families={[family]}
          variantPreferences={new Map()}
          playingChannel={null}
          now={NOW}
        />
      </QueryClientProvider>,
    );
  }

  it("reads it from the Quality Variant whose Programme is shown, and matches that Programme", async () => {
    const sd = channelFixture({
      id: "svt1-sd",
      name: "SVT1 SD",
      group: "Sverige",
      number: 1,
      variant: { quality: "sd", baseName: "SVT1" },
    });
    const hd = channelFixture({
      id: "svt1-hd",
      name: "SVT1 HD",
      group: "Sverige",
      number: 1,
      variant: { quality: "hd", baseName: "SVT1" },
    });
    // The preferred variant (HD) has no guide data: the panel borrows SD's.
    const family: GuideFamily = {
      number: 1,
      title: "SVT1",
      group: "Sverige",
      variants: [
        { channel: sd, programmes: [LIVE, LATER], programmesTruncated: false },
        { channel: hd, programmes: [], programmesTruncated: false },
      ],
    };
    const earlier = programme("Natt", "2026-11-22T06:00:00Z", "2026-11-22T07:45:00Z");
    const schedule = vi.fn(async (input: ScheduleInput) => {
      void input;
      return {
        ok: true,
        value: clientSchemas.schedulePage.parse({
          generation: 7,
          // A Programme still on at the shown one's start comes first.
          items: [
            summary("svt1-sd", earlier, "Not this one."),
            summary("svt1-sd", LIVE, "Morning news from Sweden."),
          ],
          next: null,
        }),
      };
    });

    renderWith(family, schedule);

    const details = await screen.findByRole("region", { name: "About Morgon" });
    expect(
      await within(details).findByText("Morning news from Sweden."),
    ).toBeInTheDocument();
    expect(schedule).toHaveBeenCalledWith(
      expect.objectContaining({ id: "svt1-sd", from: LIVE.startsAt }),
    );
    expect(schedule.mock.calls[0]?.[0].limit).toBeGreaterThan(1);
  });

  it("shows the focused cell's own Programme when two share a slot, and its own description", async () => {
    // As in the core's fixture: distinct titles in one slot.
    const first = programme("Nyheter", "2026-11-22T09:00:00Z", "2026-11-22T10:00:00Z");
    const second = programme("Sporten", "2026-11-22T09:00:00Z", "2026-11-22T10:00:00Z");
    const family: GuideFamily = {
      ...FAMILY,
      variants: [
        {
          channel: channelFixture({ id: "svt1", name: "SVT1", group: "Sverige", number: 1 }),
          programmes: [LIVE, first, second, LATER],
          programmesTruncated: false,
        },
      ],
    };
    const schedule = vi.fn(async (input: ScheduleInput) => {
      void input;
      return {
        ok: true,
        value: clientSchemas.schedulePage.parse({
          generation: 7,
          items: [
            summary("svt1", first, "The news."),
            summary("svt1", second, "The sport."),
          ],
          next: null,
        }),
      };
    });

    const { container } = renderWith(family, schedule, 2);

    expect(container.querySelector(".programme-guide__preview-title")).toHaveTextContent(
      /^Sporten$/u,
    );
    const details = screen.getByRole("region", { name: "About Sporten" });
    expect(await within(details).findByText("The sport.")).toBeInTheDocument();
    expect(within(details).queryByText("The news.")).toBeNull();
  });

  it("reads on past a first page of Programmes that overlap the shown one's start", async () => {
    const overlapping = (index: number) =>
      programme(`Earlier ${index}`, "2026-11-22T07:00:00Z", `2026-11-22T08:0${index}:00Z`);
    const schedule = vi.fn(async (input: ScheduleInput) => ({
      ok: true,
      value: clientSchemas.schedulePage.parse(
        input.cursor === undefined
          ? {
              generation: 7,
              items: [0, 1, 2, 3].map((index) =>
                summary("svt1", overlapping(index + 1), "Not this one."),
              ),
              next: "page-2",
            }
          : {
              generation: 7,
              items: [
                summary("svt1", LIVE, "Morning news from Sweden."),
                summary("svt1", LATER, "Not this one either."),
              ],
              next: "page-3",
            },
      ),
    }));

    renderWith(FAMILY, schedule);

    const details = await screen.findByRole("region", { name: "About Morgon" });
    expect(
      await within(details).findByText("Morning news from Sweden."),
    ).toBeInTheDocument();
    expect(schedule).toHaveBeenCalledTimes(2);
    expect(schedule.mock.calls.map(([input]) => [input.limit, input.cursor])).toEqual([
      [4, undefined],
      [4, "page-2"],
    ]);
    expect(schedule.mock.calls[1]?.[0]).toMatchObject({
      from: LIVE.startsAt,
      afterStartsAt: "2026-11-22T07:00:00Z",
    });
  });

  it("offers the description and what follows to a screen reader as a named region that is not live", () => {
    renderPreview(null);

    const details = screen.getByRole("region", { name: "About Morgon" });
    expect(details).not.toHaveAttribute("aria-live");
    expect(details.closest("[aria-hidden='true']")).toBeNull();
    expect(
      within(details).getByRole("list", { name: "Later" }),
    ).toHaveTextContent("Fiskeläget");
  });
});
