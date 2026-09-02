import { Tooltip } from "@base-ui/react/tooltip";
import { memo } from "react";
import type {
  ChannelSummary,
  GuideProgramme,
  GuideWindowChannel,
  ProgrammeSlot,
} from "../../client/contracts";
import {
  clockLabel,
  programmeAt,
  programmeKey,
  programmeLayout,
  sameProgramme,
  type ClockWindow,
} from "./guide-window";

/** Isolates selection updates to the old and new timetable rows. */
export const ProgrammeGuideRow = memo(function ProgrammeGuideRow({
  row,
  rowIndex,
  window,
  now,
  selected,
  selectedProgramme,
  playing,
  channelNameTooltip,
  onPreparePlayback,
  onTune,
}: {
  readonly row: GuideWindowChannel;
  readonly rowIndex: number;
  readonly window: ClockWindow;
  readonly now: Date;
  readonly selected: boolean;
  readonly selectedProgramme: ProgrammeSlot | null;
  readonly playing: boolean;
  readonly channelNameTooltip: Tooltip.Handle<string>;
  readonly onPreparePlayback: () => void;
  readonly onTune: (
    channel: ChannelSummary,
    programme: GuideProgramme | null,
  ) => void;
}) {
  return (
    <div
      className="programme-guide__row"
      data-playing={playing}
      data-selected={selected}
    >
      <Tooltip.Trigger
        className="programme-guide__channel"
        data-acceptance-channel
        handle={channelNameTooltip}
        payload={row.channel.name}
        type="button"
        aria-label={`Tune ${row.channel.name}`}
        aria-pressed={playing}
        onMouseEnter={onPreparePlayback}
        onFocus={onPreparePlayback}
        onClick={() => onTune(row.channel, programmeAt(row.programmes, now))}
      >
        <span aria-hidden="true">
          {String(rowIndex + 1).padStart(2, "0")}
        </span>
        <strong>{row.channel.name}</strong>
      </Tooltip.Trigger>
      <div className="programme-guide__track">
        {row.programmes.map((programme, programmeIndex) => {
          const layout = programmeLayout(programme, window, now);
          if (layout === null) {
            return null;
          }
          const key = programmeKey(programme, programmeIndex);
          const isSelected = sameProgramme(programme, selectedProgramme);
          return (
            <button
              key={key}
              className="programme-guide__programme"
              data-live={layout.live}
              data-selected={isSelected}
              type="button"
              title={`${programme.title}${programme.titleTruncated ? "…" : ""}`}
              aria-pressed={isSelected}
              aria-label={`${programme.title}${programme.titleTruncated ? ", title truncated" : ""}, ${clockLabel(programme.startsAt)} to ${clockLabel(programme.endsAt)}, ${row.channel.name}`}
              style={{
                left: `${layout.leftPercent}%`,
                width: `${layout.widthPercent}%`,
              }}
              onMouseEnter={onPreparePlayback}
              onFocus={onPreparePlayback}
              onClick={() => onTune(row.channel, programme)}
            >
              {layout.live ? (
                <span
                  className="programme-guide__elapsed"
                  style={{ width: `${layout.elapsedPercent}%` }}
                  aria-hidden="true"
                />
              ) : null}
              <span className="programme-guide__programme-copy">
                {layout.live ? <small>Now</small> : null}
                <b>
                  {programme.title}
                  {programme.titleTruncated ? "…" : null}
                </b>
              </span>
            </button>
          );
        })}
        {row.programmes.length === 0 ? (
          <span className="programme-guide__no-programmes">
            No matched Guide data
          </span>
        ) : row.programmesTruncated ? (
          <span className="programme-guide__truncated">
            Additional overlapping entries omitted
          </span>
        ) : null}
      </div>
    </div>
  );
});
