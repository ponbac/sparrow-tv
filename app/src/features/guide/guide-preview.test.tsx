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
import { clockLabel } from "./guide-window";

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

function renderPreview(startsAt: string | null) {
  const store = createPreviewStore();
  store.set({ number: 1, startsAt });
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
    const { container } = renderPreview(LATER.startsAt);

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
  ) {
    const store = createPreviewStore();
    store.set({ number: family.number, startsAt: null });
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
