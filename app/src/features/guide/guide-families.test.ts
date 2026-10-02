import { describe, expect, it } from "vitest";
import {
  clientSchemas,
  type ChannelQuality,
  type GuideProgramme,
  type GuideWindowChannel,
} from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import {
  familyKey,
  familyProgrammes,
  guideFamilies,
  preferredVariant,
  type GuideFamily,
} from "./guide-families";

const SVT1_NUMBER = 101;

const BULLETIN: GuideProgramme = {
  title: "Bulletin",
  titleTruncated: false,
  startsAt: clientSchemas.isoInstant.parse("2026-09-01T20:00:00Z"),
  endsAt: clientSchemas.isoInstant.parse("2026-09-01T21:00:00Z"),
};

describe("guide families", () => {
  it("folds adjacent rows that share a Channel Number into one family", () => {
    const news = row(
      channelFixture({ id: "world-news", name: "World News", group: "News" }),
    );
    const sd = svt1("sd");
    const hd = svt1("hd");
    const cinema = row(
      channelFixture({ id: "cinema-one", name: "Cinema One", group: "Cinema" }),
    );

    expect(guideFamilies([news, sd, hd, cinema])).toEqual([
      {
        number: news.channel.number,
        title: "World News",
        group: "News",
        variants: [news],
      },
      {
        number: SVT1_NUMBER,
        title: "SVT1",
        group: "Sweden",
        variants: [sd, hd],
      },
      {
        number: cinema.channel.number,
        title: "Cinema One",
        group: "Cinema",
        variants: [cinema],
      },
    ]);
  });

  it("keeps rows with one Channel Number apart when another row sits between them", () => {
    const sd = svt1("sd");
    const news = row(
      channelFixture({ id: "world-news", name: "World News", group: "News" }),
    );
    const hd = svt1("hd");

    expect(
      guideFamilies([sd, news, hd]).map((family) => family.variants),
    ).toEqual([[sd], [news], [hd]]);
  });

  it("prefers the highest quality until the viewer has chosen one the family has", () => {
    const family = svt1Family([svt1("sd"), svt1("fhd"), svt1("hd")]);
    const key = familyKey(family);

    expect(preferredVariant(family, new Map()).channel.id).toBe("svt1-fhd");
    expect(preferredVariant(family, new Map([[key, "sd"]])).channel.id).toBe(
      "svt1-sd",
    );
    // A stored quality the catalog no longer offers falls back to the best.
    expect(preferredVariant(family, new Map([[key, "uhd"]])).channel.id).toBe(
      "svt1-fhd",
    );
  });

  it("shows the preferred variant's Programmes, else the first variant's that has any", () => {
    const sd = svt1("sd");
    const hd = svt1("hd", [BULLETIN]);
    const fhd = svt1("fhd");
    const family = svt1Family([sd, hd, fhd]);
    const later: GuideProgramme = { ...BULLETIN, title: "Late Bulletin" };
    const fhdWithGuide = svt1("fhd", [later]);

    expect(familyProgrammes(family, fhd)).toEqual([BULLETIN]);
    expect(
      familyProgrammes(svt1Family([sd, hd, fhdWithGuide]), fhdWithGuide),
    ).toEqual([later]);
    expect(familyProgrammes(svt1Family([sd, fhd]), fhd)).toEqual([]);
  });

  it("keys a family by its Channel Group and title, ignoring letter case", () => {
    expect(familyKey({ group: "Sweden", title: "SVT1" })).toBe(
      familyKey({ group: "Sweden", title: "svt1" }),
    );
    expect(familyKey({ group: "Sweden", title: "SVT1" })).not.toBe(
      familyKey({ group: "Norway", title: "SVT1" }),
    );
  });
});

function svt1(
  quality: ChannelQuality,
  programmes: readonly GuideProgramme[] = [],
): GuideWindowChannel {
  return row(
    channelFixture({
      id: `svt1-${quality}`,
      name: `SVT1 ${quality.toUpperCase()}`,
      group: "Sweden",
      number: SVT1_NUMBER,
      variant: { quality, baseName: "SVT1" },
    }),
    programmes,
  );
}

function svt1Family(
  variants: readonly [GuideWindowChannel, ...GuideWindowChannel[]],
): GuideFamily {
  return { number: SVT1_NUMBER, title: "SVT1", group: "Sweden", variants };
}

function row(
  channel: GuideWindowChannel["channel"],
  programmes: readonly GuideProgramme[] = [],
): GuideWindowChannel {
  return { channel, programmes, programmesTruncated: false };
}
