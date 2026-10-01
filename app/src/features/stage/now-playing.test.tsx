import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { clientSchemas } from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import { NowPlaying, type NowPlayingSubject } from "./now-playing";
import type { NowPlayingProgramme } from "./use-now-playing";

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
    ]);
    expect(screen.queryByText(/Ended Quiz/u)).not.toBeInTheDocument();
  });

  it("names the Channel between Programmes and still says what is next", () => {
    const gap = {
      now: "2026-09-01T20:45:00.000Z",
      programmes: [
        programme("Ended Quiz", "20:00", "20:30"),
        programme("Late News", "21:30", "22:00"),
      ],
    };
    const { rerender } = renderInfo({ ...gap, loading: true });

    // The reads have not settled, so the absence of a Programme says nothing yet.
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      "Discovery",
    );
    expect(
      screen.queryByText("Live channel, no guide data"),
    ).not.toBeInTheDocument();

    rerender(info({ ...gap, loading: false }));

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
  readonly loading?: boolean;
}

function renderInfo(input: InfoInput) {
  return render(info(input));
}

function info({ now, programmes, loading = false }: InfoInput) {
  const subject: NowPlayingSubject = {
    channel: DISCOVERY,
    variants: [DISCOVERY],
    programmes,
    loading,
  };
  return (
    <NowPlaying
      subject={subject}
      playingChannel={DISCOVERY.id}
      now={new Date(now)}
      layout="stacked"
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
