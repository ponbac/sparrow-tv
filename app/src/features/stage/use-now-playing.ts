import { keepPreviousData, skipToken, useQuery } from "@tanstack/react-query";
import { useEffect, useMemo } from "react";
import type {
  CatalogGeneration,
  ChannelId,
  GuideProgramme,
  GuideWindowChannel,
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

const SCHEDULE_LIMIT = 8;
const IMMUTABLE_CATALOG_STALE_TIME = Number.POSITIVE_INFINITY;
const NO_PROGRAMMES: readonly NowPlayingProgramme[] = [];

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
   * The guide rows around the playing Channel in Channel Catalog order. Null
   * unless they belong to the current generation and hold that Channel.
   */
  readonly rows: readonly GuideWindowChannel[] | null;
  /** The playing Channel's guide row among `rows`. */
  readonly family: GuideFamily | null;
  /**
   * The playing Channel's Programmes from the guide window's start on, in
   * start order: its schedule, else what its guide row shows. Empty when
   * neither read has any, and when both fail.
   */
  readonly programmes: readonly NowPlayingProgramme[];
  /** No Programme is known yet and a read is still under way. */
  readonly loading: boolean;
}

/**
 * Reads the playing Channel's schedule and the guide rows around it. A failed
 * read is never surfaced: the Channel then simply has no Programme data.
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
  const neighbourhoodQuery = useQuery({
    queryKey: [
      "catalog",
      "now-playing",
      "neighbourhood",
      generation,
      channel,
      startsAt,
      endsAt,
      channelLimit,
    ],
    queryFn:
      channel === null || generation === null
        ? skipToken
        : ({ signal }) =>
            generationBoundResult(
              client.guideWindow({
                around: channel,
                startsAt,
                endsAt,
                channelLimit,
                signal,
              }),
              generation,
            ),
    placeholderData: keepPreviousData,
    retry: false,
    staleTime: IMMUTABLE_CATALOG_STALE_TIME,
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
  const neighbourhood = neighbourhoodQuery.data?.value;
  const rows =
    channel !== null &&
    neighbourhood !== undefined &&
    neighbourhood.generation === generation &&
    neighbourhood.items.some((row) => row.channel.id === channel)
      ? neighbourhood.items
      : null;
  const family = useMemo(
    () =>
      rows === null
        ? null
        : (guideFamilies(rows).find((candidate) =>
            candidate.variants.some((row) => row.channel.id === channel),
          ) ?? null),
    [channel, rows],
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
    rows,
    family,
    programmes,
    loading:
      programmes.length === 0 &&
      (scheduleQuery.isFetching || neighbourhoodQuery.isFetching),
  };
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
