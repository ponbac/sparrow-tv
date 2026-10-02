import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { clientSchemas } from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import { NowPlaying, type NowPlayingSubject } from "./now-playing";
import type { StagePicture } from "./stage-chrome";
import type {
  NowPlayingProgramme,
  NowPlayingReading,
} from "./use-now-playing";
import type { StageLayout } from "./use-stage-layout";

afterEach(cleanup);

const DISCOVERY = channelFixture({
  id: "discovery-hd",
  name: "Discovery HD",
  group: "Documentary",
  variant: { quality: "hd", baseName: "Discovery" },
});

describe("NowPlaying", () => {
  it("describes the Programme airing now, not the first one listed", () => {
    renderInfo({
      now: "2026-09-01T20:45:00.000Z",
      programmes: [
        programme("Ended Quiz", "20:00", "20:30"),
        programme("Evening Film", "20:30", "21:30"),
        programme("Late News", "21:30", "22:00"),
        programme("Night Talk", "22:00", "23:00"),
        programme("Small Hours", "23:00", "23:30"),
        programme("Dawn Chorus", "23:30", "23:59"),
      ],
    });

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      "Evening Film",
    );
    expect(screen.getByText("45 min left")).toBeVisible();
    expect(screen.getByText("About Evening Film.")).toBeInTheDocument();
    expect(
      document
        .querySelector<HTMLElement>(".now-playing__progress")
        ?.style.getPropertyValue("--progress"),
    ).toBe("25%");
    expect(
      screen.getAllByRole("listitem").map((item) => item.textContent),
    ).toEqual([
      expect.stringMatching(/^Next at \d\d:\d\d Late News$/u),
      expect.stringMatching(/^\d\d:\d\d Night Talk$/u),
      expect.stringMatching(/^\d\d:\d\d Small Hours$/u),
      expect.stringMatching(/^\d\d:\d\d Dawn Chorus$/u),
    ]);
    expect(screen.queryByText(/Ended Quiz/u)).not.toBeInTheDocument();
  });

  it("lists three upcoming Programmes in Theater, which has one line for them", () => {
    renderInfo({
      now: "2026-09-01T20:45:00.000Z",
      layout: "theater",
      programmes: [
        programme("Evening Film", "20:30", "21:30"),
        programme("Late News", "21:30", "22:00"),
        programme("Night Talk", "22:00", "23:00"),
        programme("Small Hours", "23:00", "23:30"),
        programme("Dawn Chorus", "23:30", "23:59"),
      ],
    });

    expect(
      screen.getAllByRole("listitem").map((item) => item.textContent),
    ).toEqual([
      expect.stringMatching(/Late News$/u),
      expect.stringMatching(/Night Talk$/u),
      expect.stringMatching(/Small Hours$/u),
    ]);
  });

  it("words the player's state in pocket while the picture is not simply playing", () => {
    const paused = {
      now: "2026-09-01T20:45:00.000Z",
      programmes: [programme("Evening Film", "20:30", "21:30")],
      picture: { state: "paused", status: "Paused" },
    } as const;
    const { rerender } = renderInfo(paused);

    expect(screen.getByText("Paused")).toHaveAttribute("data-state", "paused");

    rerender(info({ ...paused, picture: { state: "playing", status: "On air" } }));
    expect(screen.queryByText("On air")).not.toBeInTheDocument();

    // Theater shows the state over the picture, inside the player.
    rerender(info({ ...paused, layout: "theater" }));
    expect(screen.queryByText("Paused")).not.toBeInTheDocument();
  });

  it("marks what follows as unsettled while the description may still arrive", () => {
    const guideRowOnly = {
      now: "2026-09-01T20:45:00.000Z",
      programmes: [
        { ...programme("Quiz Night", "20:30", "21:30"), description: null },
        programme("Late News", "21:30", "22:00"),
      ],
    };
    const { container, rerender } = renderInfo({
      ...guideRowOnly,
      reading: "descriptions",
    });
    const later = container.querySelector(".now-playing__later");

    // The lines above follow at once; pocket holds this block back.
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      "Quiz Night",
    );
    expect(later).toHaveAttribute("data-settled", "false");

    rerender(info({ ...guideRowOnly, reading: "done" }));

    expect(later).toHaveAttribute("data-settled", "true");
  });

  it("names the Channel between Programmes and still says what is next", () => {
    const gap = {
      now: "2026-09-01T20:45:00.000Z",
      programmes: [
        programme("Ended Quiz", "20:00", "20:30"),
        programme("Late News", "21:30", "22:00"),
      ],
    };
    const { rerender } = renderInfo({ ...gap, reading: "programmes" });

    // The reads have not settled, so the absence of a Programme says nothing yet.
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      "Discovery",
    );
    expect(
      screen.queryByText("Live channel, no guide data"),
    ).not.toBeInTheDocument();

    rerender(info({ ...gap, reading: "done" }));

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      "Discovery",
    );
    expect(screen.getByText("Live channel, no guide data")).toBeVisible();
    expect(screen.queryByText(/min left/u)).not.toBeInTheDocument();
    expect(screen.getByRole("list", { name: "Up next" })).toHaveTextContent(
      /^Next at \d\d:\d\d Late News$/u,
    );
  });
});

interface InfoInput {
  readonly now: string;
  readonly programmes: readonly NowPlayingProgramme[];
  readonly reading?: NowPlayingReading;
  readonly layout?: StageLayout;
  readonly picture?: Pick<StagePicture, "state" | "status">;
}

function renderInfo(input: InfoInput) {
  return render(info(input));
}

function info({
  now,
  programmes,
  reading = "done",
  layout = "pocket",
  picture,
}: InfoInput) {
  const subject: NowPlayingSubject = {
    channel: DISCOVERY,
    variants: [DISCOVERY],
    programmes,
    reading,
  };
  return (
    <NowPlaying
      subject={subject}
      playingChannel={DISCOVERY.id}
      now={new Date(now)}
      layout={layout}
      picture={picture ?? null}
      controlsRef={() => undefined}
      onPreparePlayback={() => undefined}
      onTuneVariant={() => undefined}
    />
  );
}

/** A described Programme on 1 September 2026, between two UTC clock times. */
function programme(
  title: string,
  startsAt: string,
  endsAt: string,
): NowPlayingProgramme {
  return {
    title,
    description: `About ${title}.`,
    startsAt: clientSchemas.isoInstant.parse(`2026-09-01T${startsAt}:00.000Z`),
    endsAt: clientSchemas.isoInstant.parse(`2026-09-01T${endsAt}:00.000Z`),
  };
}
