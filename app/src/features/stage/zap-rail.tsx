import type { ChannelId, ChannelSummary } from "../../client/contracts";
import { familyProgrammes, qualityLabel } from "../guide/guide-families";
import { liveProgramme } from "../guide/guide-window";
import type { ZapStop } from "./zap";

/** Inputs for the row of nearby Channels under the lower third. */
export interface ZapRailProps {
  /** The stops to offer, in Channel Catalog order. */
  readonly stops: readonly ZapStop[];
  /** The Channel the info block describes: a pending zap target, else the playing one. */
  readonly current: ChannelId;
  readonly now: Date;
  readonly onPreparePlayback: () => void;
  readonly onTune: (channel: ChannelSummary) => void;
}

/** Offers the Channels on either side of the current one, each with what it shows now. */
export function ZapRail({
  stops,
  current,
  now,
  onPreparePlayback,
  onTune,
}: ZapRailProps) {
  return (
    <nav className="zap-rail" aria-label="Nearby channels">
      {stops.map(({ family, target }) => {
        const live = liveProgramme(familyProgrammes(family, target), now);
        const quality = target.channel.variant?.quality;
        return (
          <button
            key={family.number}
            type="button"
            aria-current={
              family.variants.some((variant) => variant.channel.id === current)
                ? "true"
                : undefined
            }
            onMouseEnter={onPreparePlayback}
            onFocus={onPreparePlayback}
            onClick={() => onTune(target.channel)}
          >
            <span className="zap-rail__channel">
              <b>{family.number}</b>
              <span>{family.title}</span>
              {quality === undefined ? null : <em>{qualityLabel(quality)}</em>}
            </span>
            <strong>{live?.title ?? "No guide data"}</strong>
            <span
              className="zap-rail__progress"
              style={{
                "--progress": `${live === null ? 0 : elapsedPercent(live, now)}%`,
              }}
              aria-hidden="true"
            />
          </button>
        );
      })}
    </nav>
  );
}

function elapsedPercent(
  programme: { readonly startsAt: string; readonly endsAt: string },
  now: Date,
): number {
  const startsAt = Date.parse(programme.startsAt);
  const endsAt = Date.parse(programme.endsAt);
  const elapsed = (now.getTime() - startsAt) / (endsAt - startsAt);
  return Math.min(Math.max(elapsed, 0), 1) * 100;
}
