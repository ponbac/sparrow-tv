import { describe, expect, it } from "vitest";
import { clientSchemas } from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import { guideProgrammesByChannel, searchHitOnNow } from "./search-on-now";

const instant = (value: string) => clientSchemas.isoInstant.parse(value);

const NOW = new Date("2026-11-22T08:04:00Z");
const ON_NOW = {
  title: "Matresan: Norrland",
  startsAt: instant("2026-11-22T07:15:00Z"),
  endsAt: instant("2026-11-22T08:45:00Z"),
  titleTruncated: false,
} as const;
const LATER = {
  title: "Kvällsöppet med Lina",
  startsAt: instant("2026-11-22T08:45:00Z"),
  endsAt: instant("2026-11-22T09:45:00Z"),
  titleTruncated: false,
} as const;

const hd = channelFixture({ id: "svt1-hd", name: "SVT1 HD", group: "Sverige", number: 1 });
const fhd = channelFixture({ id: "svt1-fhd", name: "SVT1 FHD", group: "Sverige", number: 1 });
const other = channelFixture({ id: "svt24", name: "SVT24", group: "Nyheter", number: 36 });

describe("searchHitOnNow", () => {
  const programmes = guideProgrammesByChannel([
    { channel: hd, programmes: [ON_NOW, LATER], programmesTruncated: false },
    { channel: other, programmes: [LATER], programmesTruncated: false },
  ]);

  it("reads what is on now from any Quality Variant's guide row the Guide has read", () => {
    expect(searchHitOnNow(fhd, [fhd, hd], { programmes, now: NOW })).toBe(ON_NOW);
  });

  it("asks the Channel the hit tunes first, then a sibling that has something on now", () => {
    const own = { ...ON_NOW, title: "Own row" };
    const read = guideProgrammesByChannel([
      // The first variant indexed has rows, but nothing on now.
      { channel: hd, programmes: [LATER], programmesTruncated: false },
      { channel: fhd, programmes: [ON_NOW], programmesTruncated: false },
    ]);

    expect(searchHitOnNow(hd, [hd, fhd], { programmes: read, now: NOW })).toBe(ON_NOW);
    expect(
      searchHitOnNow(fhd, [hd, fhd], {
        programmes: guideProgrammesByChannel([
          { channel: hd, programmes: [ON_NOW], programmesTruncated: false },
          { channel: fhd, programmes: [own], programmesTruncated: false },
        ]),
        now: NOW,
      }),
    ).toBe(own);
  });

  it("says nothing when the row is unread or nothing airs now", () => {
    expect(searchHitOnNow(fhd, [fhd], { programmes, now: NOW })).toBeNull();
    expect(searchHitOnNow(other, [other], { programmes, now: NOW })).toBeNull();
    expect(searchHitOnNow(hd, [hd], undefined)).toBeNull();
  });
});
