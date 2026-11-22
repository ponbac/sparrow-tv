import { channelTitle, qualityLabel } from "../guide/guide-families";
import { clockLabel } from "../guide/guide-window";
import { elapsedFraction, minutesLeft } from "../guide/now-next";
import { QualitySwitch } from "../guide/quality-switch";
import { televisionProgrammes, type TelevisionNow } from "./television-now-context";

/**
 * The fullscreen top scrim's identity block: the Channel Number tile, the
 * Channel's name with its live marker and quality, the Programme and a line
 * about it, and the clock. Drawn inside the player's fullscreen section.
 */
export function TelevisionHead({
  info,
  state,
}: {
  readonly info: TelevisionNow;
  /** The player's state, worded in the live marker when it is not simply playing. */
  readonly state: string | null;
}) {
  const subject = info.subject;
  if (subject === null) return null;
  const { live } = televisionProgrammes(info);
  const quality = subject.channel.variant?.quality;
  return (
    <div className="hosted-player__tv-head" aria-hidden="true">
      <span className="hosted-player__tv-number">{subject.channel.number}</span>
      <div className="hosted-player__tv-identity">
        <p className="hosted-player__tv-channel">
          <b>{channelTitle(subject.channel)}</b>
          <em className="hosted-player__tv-live" data-state={state === null ? "live" : "other"}>
            {state ?? "Live"}
          </em>
          {quality === undefined ? null : (
            <em className="hosted-player__tv-quality">{qualityLabel(quality)}</em>
          )}
        </p>
        <p className="hosted-player__tv-title">
          {live?.title ?? channelTitle(subject.channel)}
        </p>
        {live?.description == null || live.description === "" ? null : (
          <p className="hosted-player__tv-about">{live.description}</p>
        )}
      </div>
      <time className="hosted-player__tv-clock" dateTime={info.now.toISOString()}>
        {clockLabel(info.now)}
      </time>
    </div>
  );
}

/** The bottom scrim's "Next" line and the Programme's progress across the bar. */
export function TelevisionProgress({ info }: { readonly info: TelevisionNow }) {
  const { live, next } = televisionProgrammes(info);
  if (live === null && next === null) return null;
  return (
    <div className="hosted-player__tv-progress" aria-hidden="true">
      {next === null ? null : (
        <p className="hosted-player__tv-next">
          Next <b>{clockLabel(next.startsAt)}</b> {next.title}
        </p>
      )}
      {live === null ? null : (
        <div className="hosted-player__tv-meter">
          <time dateTime={live.startsAt}>{clockLabel(live.startsAt)}</time>
          <span
            style={{ "--progress": `${elapsedFraction(live, info.now) * 100}%` }}
          />
          <time dateTime={live.endsAt}>{clockLabel(live.endsAt)}</time>
          <strong>{minutesLeft(live, info.now)} min left</strong>
        </div>
      )}
    </div>
  );
}

/**
 * The labelled quality pill of a TV control row: "Quality" and the
 * Channel's Quality Variants, the chosen one marked. Each chip tunes its
 * variant. Nothing is drawn for a Channel without variants.
 */
export function TelevisionQuality({ info }: { readonly info: TelevisionNow }) {
  const subject = info.subject;
  if (subject === null || subject.variants.length < 2) return null;
  return (
    <span className="hosted-player__quality">
      <span className="hosted-player__quality-label" aria-hidden="true">
        Quality
      </span>
      <QualitySwitch
        variants={subject.variants}
        currentChannel={subject.channel.id}
        playingChannel={info.playingChannel}
        onPreparePlayback={info.onPreparePlayback}
        onTuneVariant={info.onTuneVariant}
      />
    </span>
  );
}
