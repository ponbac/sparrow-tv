import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { clientSchemas, type GuideProgramme } from "../../client/contracts";
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
