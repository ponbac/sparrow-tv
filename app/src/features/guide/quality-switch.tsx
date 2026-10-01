import type { ChannelId, ChannelSummary } from "../../client/contracts";
import { qualityLabel } from "./guide-families";
import "./quality-switch.css";

/** Inputs for the picture-quality chips of one guide row. */
export interface QualitySwitchProps {
  readonly className?: string;
  /** The row's Quality Variants in Channel Catalog order. */
  readonly variants: readonly ChannelSummary[];
  /** The variant shown as chosen: the playing one, else the preferred one. */
  readonly currentChannel: ChannelId;
  readonly playingChannel: ChannelId | null;
  /**
   * Marks each chip as its Channel's acceptance button. Only the guide row
   * sets this; a Channel must carry the mark exactly once.
   */
  readonly acceptanceChannel?: boolean;
  readonly onPreparePlayback: () => void;
  readonly onTuneVariant: (channel: ChannelSummary) => void;
}

/** Offers the Quality Variants of one guide row as chips that tune them. */
export function QualitySwitch({
  className,
  variants,
  currentChannel,
  playingChannel,
  acceptanceChannel = false,
  onPreparePlayback,
  onTuneVariant,
}: QualitySwitchProps) {
  return (
    <span
      className={
        className === undefined
          ? "quality-switch"
          : `quality-switch ${className}`
      }
      role="group"
      aria-label="Picture quality"
    >
      {variants.map((channel) =>
        channel.variant === null ? null : (
          <button
            key={channel.id}
            type="button"
            data-acceptance-channel={acceptanceChannel ? true : undefined}
            data-current={channel.id === currentChannel}
            aria-label={`Tune ${channel.name}`}
            aria-pressed={channel.id === playingChannel}
            onMouseEnter={onPreparePlayback}
            onFocus={onPreparePlayback}
            onClick={() => onTuneVariant(channel)}
          >
            {qualityLabel(channel.variant.quality)}
          </button>
        ),
      )}
    </span>
  );
}
