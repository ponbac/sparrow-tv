import {
  keepPreviousData,
  queryOptions,
  skipToken,
  useQueries,
  useQuery,
  type UseQueryResult,
} from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useState } from "react";
import type {
  CatalogGeneration,
  ChannelId,
  GuideProgramme,
  IsoInstant,
  ProgrammeSlot,
  SparrowClient,
} from "../../client/contracts";
import {
  clientErrorFromQuery,
  generationBoundResult,
} from "../../client/query-result";
import {
  familyProgrammes,
  guideFamilies,
  type GuideFamily,
} from "../guide/guide-families";
import {
  extensionAnchors,
  extensionWith,
  mergedNeighbourhood,
  type AroundRead,
  type NeighbourhoodExtension,
  type ZapNeighbourhood,
} from "./zap-neighbourhood";

const SCHEDULE_LIMIT = 8;
// A further read around a row at the end of the neighbourhood holds as many
// rows as one read may: half of them lie beyond that row.
const EXTENSION_LIMIT = 100;
const IMMUTABLE_CATALOG_STALE_TIME = Number.POSITIVE_INFINITY;
const NO_PROGRAMMES: readonly NowPlayingProgramme[] = [];

/**
 * How far the reads about the playing Channel have got. "programmes": no
 * Programme is known yet and a read is under way. "descriptions": the
 * Programmes known so far come from guide rows, which carry no description,
 * and the schedule read that does is under way. "done": nothing more is
 * coming.
 */
export type NowPlayingReading = "programmes" | "descriptions" | "done";

/** A Programme as the info block shows it. */
export interface NowPlayingProgramme extends ProgrammeSlot {
  /** Only the schedule read carries one; guide rows have none. */
  readonly description: string | null;
}

/** What to read about the playing Channel, and for which guide window. */
export interface NowPlayingInput {
  readonly client: Pick<SparrowClient, "schedule" | "guideWindow">;
  /** The playing Channel; null reads nothing. */
  readonly channel: ChannelId | null;
  /** The catalog generation both reads must belong to; null reads nothing. */
  readonly generation: CatalogGeneration | null;
  readonly startsAt: IsoInstant;
  readonly endsAt: IsoInstant;
  /** How many guide rows to read around the playing Channel. */
  readonly channelLimit: number;
  /** Called once when a read answers from another catalog generation. */
  readonly onGenerationMismatch: () => void;
}

/** The playing Channel's Programmes and the guide rows around it. */
export interface NowPlayingRead {
  /**
   * The guide rows around the playing Channel, with any read further on.
   * Null unless they belong to the current generation and hold that Channel.
   */
  readonly neighbourhood: ZapNeighbourhood | null;
  /** The playing Channel's guide row in the neighbourhood. */
  readonly family: GuideFamily | null;
  /**
   * The playing Channel's Programmes from the guide window's start on, in
   * start order: its schedule, else what its guide row shows. Empty when
   * neither read has any, and when both fail.
   */
  readonly programmes: readonly NowPlayingProgramme[];
  readonly reading: NowPlayingReading;
  /**
   * Reads the guide rows beyond one end of the neighbourhood: before its
   * first row (`-1`) or after its last (`1`). Asking again for the same end
   * reads nothing twice, and one neighbourhood grows only so far.
   */
  readonly extend: (direction: -1 | 1) => void;
}

/**
 * Reads the playing Channel's schedule and the guide rows around it, and
 * further rows on either side when asked. A failed read is never surfaced:
 * the Channel then simply has no Programme data, or no further rows.
 */
export function useNowPlaying({
  client,
  channel,
  generation,
  startsAt,
  endsAt,
  channelLimit,
  onGenerationMismatch,
}: NowPlayingInput): NowPlayingRead {
  const scheduleQuery = useQuery({
    queryKey: [
      "catalog",
      "now-playing",
      "schedule",
      generation,
      channel,
      startsAt,
    ],
    queryFn:
      channel === null || generation === null
        ? skipToken
        : ({ signal }) =>
            generationBoundResult(
              client.schedule({
                id: channel,
                from: startsAt,
                limit: SCHEDULE_LIMIT,
                signal,
              }),
              generation,
            ),
    // Keeps the Programme through a guide window roll, but never shows
    // another Channel's or another catalog generation's.
    placeholderData: (previous) =>
      previous?.value.generation === generation &&
      previous.value.items[0]?.channelId === channel
        ? previous
        : undefined,
    retry: false,
    staleTime: IMMUTABLE_CATALOG_STALE_TIME,
  });
  const aroundRead = (around: ChannelId | null, limit: number) =>
    aroundReadOptions({ client, generation, around, startsAt, endsAt, limit });
  const neighbourhoodQuery = useQuery({
    ...aroundRead(channel, channelLimit),
    placeholderData: keepPreviousData,
  });
  // The first read shown may still be the previous Channel's: the further
  // reads go with the read they extend, so a zap past its rows keeps them
  // until the new Channel's own rows arrive.
  const first = neighbourhoodQuery.data;
  const [extension, setExtension] = useState<NeighbourhoodExtension | null>(
    null,
  );
  const further = useQueries({
    queries: (first === undefined
      ? []
      : extensionAnchors(extension, first.around)
    ).map((anchor) => aroundRead(anchor, EXTENSION_LIMIT)),
    combine: answeredReads,
  });

  const generationMismatch = [scheduleQuery.error, neighbourhoodQuery.error].some(
    (error) => clientErrorFromQuery(error)?._tag === "stale-cursor",
  );
  useEffect(() => {
    if (generationMismatch) {
      onGenerationMismatch();
    }
  }, [generationMismatch, onGenerationMismatch]);

  // The rows shown while the next read is under way may be around another
  // Channel or from the previous generation.
  const neighbourhood = useMemo(() => {
    if (
      channel === null ||
      first === undefined ||
      first.page.generation !== generation
    ) {
      return null;
    }
    const merged = mergedNeighbourhood(
      first,
      further.filter((read) => read.page.generation === generation),
    );
    return merged.rows.some((row) => row.channel.id === channel)
      ? merged
      : null;
  }, [channel, first, further, generation]);
  const extend = useCallback(
    (direction: -1 | 1) => {
      const edge =
        direction === -1
          ? neighbourhood?.rows[0]
          : neighbourhood?.rows.at(-1);
      if (first === undefined || edge === undefined) {
        return;
      }
      setExtension((current) =>
        extensionWith(current, first.around, edge.channel.id),
      );
    },
    [first, neighbourhood],
  );
  const family = useMemo(
    () =>
      neighbourhood === null
        ? null
        : (guideFamilies(neighbourhood.rows).find((candidate) =>
            candidate.variants.some((row) => row.channel.id === channel),
          ) ?? null),
    [channel, neighbourhood],
  );
  const schedule = scheduleQuery.data?.value.items;
  const programmes = useMemo(() => {
    const playingRow = family?.variants.find(
      (row) => row.channel.id === channel,
    );
    const rowProgrammes =
      family === null || playingRow === undefined
        ? NO_PROGRAMMES
        : familyProgrammes(family, playingRow).map(guideRowProgramme);
    const scheduleEnd = schedule?.at(-1)?.endsAt;
    if (schedule === undefined || scheduleEnd === undefined) {
      return rowProgrammes;
    }
    // The schedule read is one short page. Where Programmes are brief it can
    // end before now, so the guide row carries on from its last Programme.
    const scheduleEndsAt = Date.parse(scheduleEnd);
    return [
      ...schedule,
      ...rowProgrammes.filter(
        (programme) => Date.parse(programme.startsAt) >= scheduleEndsAt,
      ),
    ];
  }, [channel, family, schedule]);

  return {
    neighbourhood,
    family,
    programmes,
    reading:
      programmes.length === 0 &&
      (scheduleQuery.isFetching || neighbourhoodQuery.isFetching)
        ? "programmes"
        : scheduleQuery.data === undefined && scheduleQuery.isFetching
          ? "descriptions"
          : "done",
    extend,
  };
}

/** One guide read placed around a Channel; the same read is never made twice. */
function aroundReadOptions({
  client,
  generation,
  around,
  startsAt,
  endsAt,
  limit,
}: {
  readonly client: Pick<SparrowClient, "guideWindow">;
  readonly generation: CatalogGeneration | null;
  readonly around: ChannelId | null;
  readonly startsAt: IsoInstant;
  readonly endsAt: IsoInstant;
  readonly limit: number;
}) {
  return queryOptions({
    queryKey: [
      "catalog",
      "now-playing",
      "neighbourhood",
      generation,
      around,
      startsAt,
      endsAt,
      limit,
    ],
    queryFn:
      around === null || generation === null
        ? skipToken
        : async ({ signal }): Promise<AroundRead> => {
            const result = await generationBoundResult(
              client.guideWindow({
                around,
                startsAt,
                endsAt,
                channelLimit: limit,
                signal,
              }),
              generation,
            );
            return { around, limit, page: result.value };
          },
    retry: false,
    staleTime: IMMUTABLE_CATALOG_STALE_TIME,
  });
}

/** The reads that have answered, in the order they were asked. */
function answeredReads(
  results: readonly UseQueryResult<AroundRead>[],
): readonly AroundRead[] {
  return results.flatMap((result) =>
    result.data === undefined ? [] : [result.data],
  );
}

/** A guide row's Programme as the info block shows it: without a description. */
export function guideRowProgramme(
  programme: GuideProgramme,
): NowPlayingProgramme {
  return {
    title: programme.titleTruncated ? `${programme.title}…` : programme.title,
    startsAt: programme.startsAt,
    endsAt: programme.endsAt,
    description: null,
  };
}
