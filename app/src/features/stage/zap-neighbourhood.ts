import type {
  ChannelId,
  GuideWindowChannel,
  Page,
} from "../../client/contracts";

// Further reads one first read may grow by: each holds up to a hundred rows,
// so a zap can skip that many rows of hidden Channel Groups several times
// over before the bar gives up on that side.
export const MAX_NEIGHBOURHOOD_EXTENSIONS = 8;

/** The guide rows read around the playing Channel, and how far they reach. */
export interface ZapNeighbourhood {
  /** Adjacent rows in Channel Catalog order. */
  readonly rows: readonly GuideWindowChannel[];
  /** No row of the Channel Catalog comes before the first. */
  readonly startReached: boolean;
  /** No row of the Channel Catalog comes after the last. */
  readonly endReached: boolean;
}

/** One guide read placed around a Channel, as it was asked for and answered. */
export interface AroundRead {
  readonly around: ChannelId;
  /** The most rows the read asked for. */
  readonly limit: number;
  readonly page: Page<GuideWindowChannel>;
}

/** The further reads asked for to extend the rows of one first read. */
export interface NeighbourhoodExtension {
  /** The Channel the first read is placed around. */
  readonly base: ChannelId;
  /** The rows the further reads are placed around, in the order asked. */
  readonly anchors: readonly ChannelId[];
}

/**
 * The rows of one read placed around a Channel. Core starts such a page half
 * its limit before the Channel, or at the start of the Channel Catalog when
 * the Channel sits nearer to it than that; a page with no `next` ends with
 * the Channel Catalog.
 */
export function neighbourhoodOf(read: AroundRead): ZapNeighbourhood {
  return {
    rows: read.page.items,
    startReached: readStartsTheCatalog(read),
    endReached: read.page.next === null,
  };
}

/**
 * Adds the rows a further read holds beyond either end of the neighbourhood.
 * The read must overlap an end to add anything there; the same
 * neighbourhood comes back when it adds nothing.
 */
export function extendNeighbourhood(
  neighbourhood: ZapNeighbourhood,
  read: AroundRead,
): ZapNeighbourhood {
  const first = neighbourhood.rows[0];
  const last = neighbourhood.rows.at(-1);
  if (first === undefined || last === undefined) {
    return neighbourhood;
  }
  const items = read.page.items;
  const firstAt = items.findIndex((row) => row.channel.id === first.channel.id);
  const lastAt = items.findIndex((row) => row.channel.id === last.channel.id);
  const before = firstAt > 0 ? items.slice(0, firstAt) : [];
  const after = lastAt === -1 ? [] : items.slice(lastAt + 1);
  const startReached =
    neighbourhood.startReached || (firstAt !== -1 && readStartsTheCatalog(read));
  const endReached =
    neighbourhood.endReached || (lastAt !== -1 && read.page.next === null);
  if (
    before.length === 0 &&
    after.length === 0 &&
    startReached === neighbourhood.startReached &&
    endReached === neighbourhood.endReached
  ) {
    return neighbourhood;
  }
  return {
    rows: [...before, ...neighbourhood.rows, ...after],
    startReached,
    endReached,
  };
}

/** The neighbourhood of a first read with every further read added in turn. */
export function mergedNeighbourhood(
  first: AroundRead,
  further: readonly AroundRead[],
): ZapNeighbourhood {
  return further.reduce(extendNeighbourhood, neighbourhoodOf(first));
}

/**
 * Asks for one more read placed around `anchor` to extend the first read
 * placed around `base`. A new base starts over. The same extension comes
 * back when the anchor was asked for already or the limit is reached.
 */
export function extensionWith(
  current: NeighbourhoodExtension | null,
  base: ChannelId,
  anchor: ChannelId,
): NeighbourhoodExtension | null {
  const anchors = extensionAnchors(current, base);
  if (
    anchors.includes(anchor) ||
    anchors.length >= MAX_NEIGHBOURHOOD_EXTENSIONS
  ) {
    return current;
  }
  return { base, anchors: [...anchors, anchor] };
}

/** The anchors asked for around the first read placed around `base`. */
export function extensionAnchors(
  extension: NeighbourhoodExtension | null,
  base: ChannelId,
): readonly ChannelId[] {
  return extension !== null && extension.base === base
    ? extension.anchors
    : [];
}

function readStartsTheCatalog(read: AroundRead): boolean {
  const position = read.page.items.findIndex(
    (row) => row.channel.id === read.around,
  );
  return position !== -1 && position < Math.floor(read.limit / 2);
}
