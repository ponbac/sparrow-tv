import type {
  ChannelQuality,
  ChannelSummary,
  GuideProgramme,
  GuideWindowChannel,
} from "../../client/contracts";

const QUALITY_LABELS: Record<ChannelQuality, string> = {
  sd: "SD",
  hd: "HD",
  fhd: "FHD",
  uhd: "UHD",
};

const QUALITY_RANKS: Record<ChannelQuality, number> = {
  sd: 0,
  hd: 1,
  fhd: 2,
  uhd: 3,
};

/** The viewer's chosen picture quality per guide row, keyed by `familyKey`. */
export type VariantPreferences = ReadonlyMap<string, ChannelQuality>;

/** One guide row: a Channel, or the adjacent Quality Variants of one. */
export interface GuideFamily {
  /** The Channel Number every variant shares. */
  readonly number: number;
  /** The first variant's name without its picture-quality token. */
  readonly title: string;
  readonly group: string;
  /** The row's Channels in Channel Catalog order; never empty. */
  readonly variants: readonly [GuideWindowChannel, ...GuideWindowChannel[]];
}

/** The name a Channel goes by on its guide row: without the quality token. */
export function channelTitle(channel: ChannelSummary): string {
  return channel.variant?.baseName ?? channel.name;
}

/** The tag shown for a picture quality, such as "FHD". */
export function qualityLabel(quality: ChannelQuality): string {
  return QUALITY_LABELS[quality];
}

/**
 * Folds guide rows into families. Adjacent rows with the same Channel Number
 * become one family; the input order is kept.
 */
export function guideFamilies(
  rows: readonly GuideWindowChannel[],
): readonly GuideFamily[] {
  const families: GuideFamily[] = [];
  for (const row of rows) {
    const last = families[families.length - 1];
    if (last !== undefined && last.number === row.channel.number) {
      families[families.length - 1] = {
        ...last,
        variants: [...last.variants, row],
      };
      continue;
    }
    families.push({
      number: row.channel.number,
      title: channelTitle(row.channel),
      group: row.channel.group,
      variants: [row],
    });
  }
  return families;
}

/**
 * The variant a family plays when the viewer chooses its row: the stored
 * quality when the family has it, otherwise the highest quality.
 */
export function preferredVariant(
  family: GuideFamily,
  preferences: VariantPreferences,
): GuideWindowChannel {
  const stored = preferences.get(familyKey(family));
  let best = family.variants[0];
  for (const variant of family.variants) {
    const quality = variant.channel.variant?.quality;
    if (quality !== undefined && quality === stored) {
      return variant;
    }
    if (qualityRank(variant) > qualityRank(best)) {
      best = variant;
    }
  }
  return best;
}

/**
 * The variant whose Programmes stand for the family: the preferred one, or
 * the first variant with any when the preferred one has no guide data.
 */
export function familyProgrammeSource(
  family: GuideFamily,
  preferred: GuideWindowChannel,
): GuideWindowChannel {
  if (preferred.programmes.length > 0) {
    return preferred;
  }
  return (
    family.variants.find((variant) => variant.programmes.length > 0) ??
    preferred
  );
}

/** The Programmes shown for a family; see `familyProgrammeSource`. */
export function familyProgrammes(
  family: GuideFamily,
  preferred: GuideWindowChannel,
): readonly GuideProgramme[] {
  return familyProgrammeSource(family, preferred).programmes;
}

/**
 * Identifies a family across catalog generations by its Channel Group and
 * title, folded as core folds base names when it forms the family. Channel
 * Numbers move when the M3U Source reorders, so they are not part of the key.
 */
export function familyKey(family: Pick<GuideFamily, "group" | "title">): string {
  const title = family.title
    .normalize("NFKC")
    .toLowerCase()
    .replace(/\s+/g, " ")
    .trim();
  return JSON.stringify([family.group, title]);
}

function qualityRank(row: GuideWindowChannel): number {
  const quality = row.channel.variant?.quality;
  return quality === undefined ? -1 : QUALITY_RANKS[quality];
}
