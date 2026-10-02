import { useEffect, useMemo } from "react";
import type { ChannelId } from "../../client/contracts";
import type { VariantPreferences } from "../guide/guide-families";
import { zapSidesToRead, zapStops, type ZapStop } from "./zap";
import type { ZapNeighbourhood } from "./zap-neighbourhood";

const NO_STOPS: readonly ZapStop[] = [];
const NO_SIDES: readonly (-1 | 1)[] = [];

/** What decides the guide rows a zap moves through. */
export interface ZapStopsInput {
  /** The guide rows around the playing Channel; null until they are known. */
  readonly neighbourhood: ZapNeighbourhood | null;
  readonly playing: ChannelId | null;
  /** The Channel a zap would move from: its pending target, else the playing one. */
  readonly shown: ChannelId | null;
  readonly excludedGroups: ReadonlySet<string>;
  readonly preferences: VariantPreferences;
  /** Reads the rows beyond one end of the neighbourhood. */
  readonly onExtend: (direction: -1 | 1) => void;
}

/**
 * The guide rows a zap moves through; empty until the rows around the
 * playing Channel are known. Where the shown Channel has no stop beside it
 * and the Channel Catalog goes on, it asks for the rows beyond, so a zap
 * runs past the rows first read and past hidden Channel Groups.
 */
export function useZapStops({
  neighbourhood,
  playing,
  shown,
  excludedGroups,
  preferences,
  onExtend,
}: ZapStopsInput): readonly ZapStop[] {
  const stops = useMemo(
    () =>
      playing === null || neighbourhood === null
        ? NO_STOPS
        : zapStops(neighbourhood, playing, excludedGroups, preferences),
    [excludedGroups, neighbourhood, playing, preferences],
  );
  const sides = useMemo(
    () =>
      shown === null || neighbourhood === null
        ? NO_SIDES
        : zapSidesToRead(stops, neighbourhood, shown),
    [neighbourhood, shown, stops],
  );
  // Each read that lands changes the neighbourhood, and so asks again while
  // the side still has no stop.
  useEffect(() => {
    for (const side of sides) {
      onExtend(side);
    }
  }, [onExtend, sides]);
  return stops;
}
