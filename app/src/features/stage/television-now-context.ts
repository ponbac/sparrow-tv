import { createContext, useContext } from "react";
import type { ChannelId, ChannelSummary } from "../../client/contracts";
import { programmeAt } from "../guide/now-next";
import type { NowPlayingSubject } from "./now-playing";

/**
 * What a TV player draws about the Channel it plays: the identity and
 * Programme lines of the fullscreen scrims, and the quality pill in its
 * control row. Only the TV shell provides it; elsewhere the player draws
 * none of this.
 */
export interface TelevisionNow {
  readonly subject: NowPlayingSubject | null;
  readonly playingChannel: ChannelId | null;
  readonly now: Date;
  readonly onPreparePlayback: () => void;
  readonly onTuneVariant: (channel: ChannelSummary) => void;
}

const TelevisionNowContext = createContext<TelevisionNow | null>(null);

/** Supplies the playing Channel's lines to a TV player below it. */
export const TelevisionNowProvider = TelevisionNowContext.Provider;

/** Reads the TV shell's lines about the playing Channel, or null outside one. */
export function useTelevisionNow(): TelevisionNow | null {
  return useContext(TelevisionNowContext);
}

/** The live Programme and the one after it, as the scrims show them. */
export function televisionProgrammes(info: TelevisionNow) {
  const programmes = info.subject?.programmes ?? [];
  const live = programmeAt(programmes, info.now);
  const next =
    programmes.find(
      (programme) => Date.parse(programme.startsAt) > info.now.getTime(),
    ) ?? null;
  return { live, next };
}
