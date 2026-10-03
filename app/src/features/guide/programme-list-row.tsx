import { memo, useId } from "react";
import type {
  ChannelId,
  ChannelSummary,
  GuideWindowChannel,
} from "../../client/contracts";
import {
  familyProgrammeSource,
  qualityLabel,
  type GuideFamily,
} from "./guide-families";
import { listRowLines } from "./list-row-lines";
import { QualitySwitch } from "./quality-switch";

/**
 * One row of pocket's now-and-next list: what a Channel has on, and what
 * follows. Isolates tune updates to the old and new rows.
 */
export const ProgrammeListRow = memo(function ProgrammeListRow({
  family,
  preferred,
  now,
  at,
  pending,
  playingChannel,
  onPreparePlayback,
  onTune,
  onTuneVariant,
}: {
  readonly family: GuideFamily;
  /** The variant the row tunes. */
  readonly preferred: GuideWindowChannel;
  readonly now: Date;
  /** The time the viewer chose to look at, or null for now. */
  readonly at: Date | null;
  /** The rows are being replaced, so what is missing may yet arrive. */
  readonly pending: boolean;
  /** The playing Channel when it is one of this row's variants, else null. */
  readonly playingChannel: ChannelId | null;
  readonly onPreparePlayback: () => void;
  readonly onTune: (channel: ChannelSummary) => void;
  readonly onTuneVariant: (channel: ChannelSummary) => void;
}) {
  const playing = playingChannel !== null;
  const switchable = family.variants.length > 1;
  const source = familyProgrammeSource(family, preferred);
  const lines = listRowLines({
    programmes: source.programmes,
    truncated: source.programmesTruncated,
    now,
    at,
    pending,
  });
  const loneQuality = switchable
    ? null
    : (family.variants[0].channel.variant?.quality ?? null);
  const live = lines._tag === "programme" ? lines.live : null;
  // The button's name is its label alone, so its lines are its description:
  // a screen reader says the Channel, then what it has on.
  const lineId = useId();
  const describedBy = [
    lines._tag === "programme" ? `${lineId}-on` : null,
    lines.after === null ? null : `${lineId}-after`,
    live === null ? null : `${lineId}-left`,
  ].filter((id) => id !== null);
  return (
    <div
      className="programme-guide__row"
      data-playing={playing}
      data-body="list"
      // Titled by its Channel: there is no Programme to name.
      data-bare={lines._tag === "channel"}
      // The first line keeps clear of this many quality chips.
      style={switchable ? { "--variants": family.variants.length } : undefined}
    >
      <span className="programme-guide__number" aria-hidden="true">
        {family.number}
      </span>
      <button
        className="programme-guide__channel"
        // Each Channel carries the mark once: a row with a quality switch
        // marks its chips instead.
        data-acceptance-channel={switchable ? undefined : true}
        type="button"
        aria-label={`Tune ${family.title}`}
        aria-describedby={
          describedBy.length === 0 ? undefined : describedBy.join(" ")
        }
        aria-pressed={playing}
        onMouseEnter={onPreparePlayback}
        onFocus={onPreparePlayback}
        onClick={() => onTune(preferred.channel)}
      >
        <span className="programme-guide__name">
          {/* A row titled by its Channel does not name it twice. */}
          {lines._tag === "programme" ? family.title : null}
          {loneQuality === null ? null : (
            <small>{qualityLabel(loneQuality)}</small>
          )}
        </span>
        <strong className="programme-guide__on" id={`${lineId}-on`}>
          {lines._tag === "programme" ? lines.title : family.title}
        </strong>
        {lines.after === null ? null : (
          <span className="programme-guide__after" id={`${lineId}-after`}>
            {lines.after}
          </span>
        )}
        {live === null ? null : (
          <span className="programme-guide__left" id={`${lineId}-left`}>
            {live.minutesLeft} min left
          </span>
        )}
      </button>
      {switchable ? (
        <QualitySwitch
          className="programme-guide__variants"
          variants={family.variants.map((variant) => variant.channel)}
          currentChannel={playingChannel ?? preferred.channel.id}
          playingChannel={playingChannel}
          acceptanceChannel
          onPreparePlayback={onPreparePlayback}
          onTuneVariant={onTuneVariant}
        />
      ) : null}
      {live === null ? null : (
        <span
          className="programme-guide__elapsed"
          style={{ "--progress": `${live.elapsed * 100}%` }}
          aria-hidden="true"
        />
      )}
    </div>
  );
});
