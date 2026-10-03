import { ChevronDown, ChevronUp, List } from "lucide-react";
import { useId } from "react";
import type { ChannelId } from "../../client/contracts";
import { familyProgrammes } from "../guide/guide-families";
import { programmeAt } from "../guide/now-next";
import { neighbouringZapStop, type ZapStop } from "./zap";

/** Inputs for the bar under pocket's watch screen. */
export interface ChannelBarProps {
  /** The guide rows around the playing Channel, in Channel Catalog order. */
  readonly stops: readonly ZapStop[];
  /** The Channel the info block describes: a pending zap target, else the playing one. */
  readonly current: ChannelId;
  readonly now: Date;
  /** Moves to the Channel before (`-1`) or after (`1`) the current one. */
  readonly onZap: (direction: -1 | 1) => void;
  readonly onShowGuide: () => void;
}

/** Offers the Channel on either side of the current one, and the way to the guide between them. */
export function ChannelBar({
  stops,
  current,
  now,
  onZap,
  onShowGuide,
}: ChannelBarProps) {
  return (
    <div className="channel-bar" role="group" aria-label="Channels">
      <ChannelBarStop
        direction={-1}
        stop={neighbouringZapStop(stops, current, -1)}
        now={now}
        onZap={onZap}
      />
      <button className="channel-bar__guide" type="button" onClick={onShowGuide}>
        <List aria-hidden="true" />
        Guide
      </button>
      <ChannelBarStop
        direction={1}
        stop={neighbouringZapStop(stops, current, 1)}
        now={now}
        onZap={onZap}
      />
    </div>
  );
}

function ChannelBarStop({
  direction,
  stop,
  now,
  onZap,
}: {
  readonly direction: -1 | 1;
  /** Null at either end of the list, and until the rows around the Channel are known. */
  readonly stop: ZapStop | null;
  readonly now: Date;
  readonly onZap: (direction: -1 | 1) => void;
}) {
  const towards = direction === -1 ? "Previous channel" : "Next channel";
  const Chevron = direction === -1 ? ChevronUp : ChevronDown;
  // The label names the Channel; what it has on is the button's description.
  const onId = useId();
  return (
    <button
      className="channel-bar__stop"
      type="button"
      aria-label={
        stop === null
          ? towards
          : `${towards}, ${stop.family.number} ${stop.family.title}`
      }
      aria-describedby={stop === null ? undefined : onId}
      disabled={stop === null}
      onClick={() => onZap(direction)}
    >
      <Chevron aria-hidden="true" />
      {stop === null ? null : (
        <span className="channel-bar__channel">
          <b>
            <i>{stop.family.number}</i> {stop.family.title}
          </b>
          <span id={onId}>
            {programmeAt(familyProgrammes(stop.family, stop.target), now)
              ?.title ?? "No guide data"}
          </span>
        </span>
      )}
    </button>
  );
}
