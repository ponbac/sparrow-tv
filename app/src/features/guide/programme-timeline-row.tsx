import { memo } from "react";
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
import { clockLabel, programmeKey, type ClockWindow } from "./guide-window";
import { QualitySwitch } from "./quality-switch";

const MINUTE_MS = 60_000;

/**
 * One row of the TV timeline: the Channel's number and name, then its
 * Programmes along the time axis. Each Programme is a button that tunes the
 * row's Channel; the Channel's own button keeps the row's name and mark, as
 * in every other guide row. Where each cell lies is given in minutes from
 * the window's start, and CSS places it against the minute the timeline
 * shows first, so browsing along time changes one property, not the rows.
 */
export const ProgrammeTimelineRow = memo(function ProgrammeTimelineRow({
  family,
  preferred,
  window,
  now,
  playingChannel,
  lastPlayed = false,
  onPreparePlayback,
  onTune,
  onTuneVariant,
}: {
  readonly family: GuideFamily;
  /** The variant the row and its cells tune. */
  readonly preferred: GuideWindowChannel;
  readonly window: ClockWindow;
  readonly now: Date;
  /** The playing Channel when it is one of this row's variants, else null. */
  readonly playingChannel: ChannelId | null;
  /** True when this row's family was the last played Channel after Stop. */
  readonly lastPlayed?: boolean;
  readonly onPreparePlayback: () => void;
  readonly onTune: (channel: ChannelSummary) => void;
  readonly onTuneVariant: (channel: ChannelSummary) => void;
}) {
  const playing = playingChannel !== null;
  const switchable = family.variants.length > 1;
  const source = familyProgrammeSource(family, preferred);
  const windowStart = window.startsAt.getTime();
  const windowEnd = window.endsAt.getTime();
  const nowTime = now.getTime();
  const loneQuality = switchable
    ? null
    : (family.variants[0].channel.variant?.quality ?? null);
  const cells = source.programmes.flatMap((programme, index) => {
    const start = Date.parse(programme.startsAt);
    const end = Date.parse(programme.endsAt);
    if (end <= windowStart || start >= windowEnd || end <= start) return [];
    const live = start <= nowTime && nowTime < end;
    return [{ programme, index, start, end, live }];
  });
  return (
    <div
      className="programme-guide__row"
      data-body="timeline"
      data-number={family.number}
      data-playing={playing}
      data-last-played={lastPlayed ? "true" : undefined}
    >
      <div className="programme-guide__channel-cell">
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
          aria-pressed={playing}
          onMouseEnter={onPreparePlayback}
          onFocus={onPreparePlayback}
          onClick={() => onTune(preferred.channel)}
        >
          <strong>{family.title}</strong>
          {loneQuality === null ? null : <small>{qualityLabel(loneQuality)}</small>}
        </button>
        {switchable ? (
          <QualitySwitch
            className="programme-guide__variants"
            variants={family.variants.map((variant) => variant.channel)}
            currentChannel={playingChannel ?? preferred.channel.id}
            playingChannel={playingChannel}
            acceptanceChannel
            focusable={false}
            onPreparePlayback={onPreparePlayback}
            onTuneVariant={onTuneVariant}
          />
        ) : null}
      </div>
      <div className="programme-guide__track">
        {cells.map(({ programme, index, start, end, live }) => {
          const title = `${programme.title}${programme.titleTruncated ? "…" : ""}`;
          const times = `${clockLabel(programme.startsAt)} to ${clockLabel(programme.endsAt)}`;
          return (
            <button
              key={programmeKey(programme, index)}
              className="programme-guide__programme"
              data-live={live}
              data-starts-at={programme.startsAt}
              data-start={(start - windowStart) / MINUTE_MS}
              data-end={(end - windowStart) / MINUTE_MS}
              type="button"
              aria-label={`${title}, ${times}, ${family.title}`}
              style={{
                "--start": (start - windowStart) / MINUTE_MS,
                "--end": (end - windowStart) / MINUTE_MS,
                ...(live
                  ? { "--elapsed": `${((nowTime - start) / (end - start)) * 100}%` }
                  : {}),
              }}
              onMouseEnter={onPreparePlayback}
              onFocus={onPreparePlayback}
              onClick={() => onTune(preferred.channel)}
            >
              <small>{clockLabel(programme.startsAt)}</small>
              <b>{title}</b>
            </button>
          );
        })}
        {cells.length === 0 ? (
          <span className="programme-guide__no-programmes">
            No guide data. The channel still plays.
          </span>
        ) : null}
      </div>
    </div>
  );
});
