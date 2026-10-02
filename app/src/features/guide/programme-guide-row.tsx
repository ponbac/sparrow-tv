import { Tooltip } from "@base-ui/react/tooltip";
import { memo } from "react";
import type {
  ChannelId,
  ChannelSummary,
  GuideWindowChannel,
} from "../../client/contracts";
import { familyProgrammeSource, type GuideFamily } from "./guide-families";
import {
  clockLabel,
  programmeKey,
  programmeLayout,
  type ClockWindow,
} from "./guide-window";
import { QualitySwitch } from "./quality-switch";

/** Isolates tune updates to the old and new timetable rows. */
export const ProgrammeGuideRow = memo(function ProgrammeGuideRow({
  family,
  preferred,
  window,
  now,
  playingChannel,
  channelNameTooltip,
  onPreparePlayback,
  onTune,
  onTuneVariant,
}: {
  readonly family: GuideFamily;
  /** The variant the row and its Programme cells tune. */
  readonly preferred: GuideWindowChannel;
  readonly window: ClockWindow;
  readonly now: Date;
  /** The playing Channel when it is one of this row's variants, else null. */
  readonly playingChannel: ChannelId | null;
  readonly channelNameTooltip: Tooltip.Handle<string>;
  readonly onPreparePlayback: () => void;
  readonly onTune: (channel: ChannelSummary) => void;
  readonly onTuneVariant: (channel: ChannelSummary) => void;
}) {
  const playing = playingChannel !== null;
  const switchable = family.variants.length > 1;
  const source = familyProgrammeSource(family, preferred);
  return (
    <div className="programme-guide__row" data-playing={playing}>
      <div className="programme-guide__channel-cell">
        <span className="programme-guide__number" aria-hidden="true">
          {family.number}
        </span>
        <Tooltip.Trigger
          className="programme-guide__channel"
          // Each Channel carries the mark once: a row with a quality switch
          // marks its chips instead.
          data-acceptance-channel={switchable ? undefined : true}
          handle={channelNameTooltip}
          payload={family.title}
          type="button"
          aria-label={`Tune ${family.title}`}
          aria-pressed={playing}
          onMouseEnter={onPreparePlayback}
          onFocus={onPreparePlayback}
          onClick={() => onTune(preferred.channel)}
        >
          <strong>{family.title}</strong>
        </Tooltip.Trigger>
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
      </div>
      <div className="programme-guide__track">
        {source.programmes.map((programme, programmeIndex) => {
          const layout = programmeLayout(programme, window, now);
          if (layout === null) {
            return null;
          }
          const key = programmeKey(programme, programmeIndex);
          const times = `${clockLabel(programme.startsAt)} to ${clockLabel(programme.endsAt)}`;
          return (
            <button
              key={key}
              className="programme-guide__programme"
              data-live={layout.live}
              type="button"
              title={`${programme.title}${programme.titleTruncated ? "…" : ""}`}
              aria-label={`${programme.title}${programme.titleTruncated ? ", title truncated" : ""}, ${times}, ${family.title}`}
              style={{
                left: `${layout.leftPercent}%`,
                width: `${layout.widthPercent}%`,
              }}
              onMouseEnter={onPreparePlayback}
              onFocus={onPreparePlayback}
              onClick={() => onTune(preferred.channel)}
            >
              {layout.live ? (
                <span
                  className="programme-guide__elapsed"
                  style={{ width: `${layout.elapsedPercent}%` }}
                  aria-hidden="true"
                />
              ) : null}
              <span className="programme-guide__programme-copy">
                <b>
                  {programme.title}
                  {programme.titleTruncated ? "…" : null}
                </b>
                {layout.timesFit ? <small>{times}</small> : null}
              </span>
            </button>
          );
        })}
        {source.programmes.length === 0 ? (
          <span className="programme-guide__no-programmes">
            No guide data. The channel still plays.
          </span>
        ) : source.programmesTruncated ? (
          <span className="programme-guide__truncated">
            More overlapping programmes not shown
          </span>
        ) : null}
      </div>
    </div>
  );
});
