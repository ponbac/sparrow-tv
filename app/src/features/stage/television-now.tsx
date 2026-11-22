import { ChannelIcon } from "../guide/channel-icon";
import { channelTitle, qualityLabel } from "../guide/guide-families";
import { clockLabel } from "../guide/guide-window";
import { elapsedFraction, minutesLeft } from "../guide/now-next";
import { QualitySwitch } from "../guide/quality-switch";
import { televisionProgrammes, type TelevisionNow } from "./television-now-context";

/**
 * The fullscreen band under the picture: the Channel's tile (its icon, or
 * its number on the tally red), the Channel's name with its live marker and
 * quality over the Programme, then what is left of it and what follows over
 * its meter, and the clock. Drawn
 * inside the player's fullscreen section; the controls are its second line.
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
  const { live, next } = televisionProgrammes(info);
  const quality = subject.channel.variant?.quality;
  return (
    <div className="hosted-player__tv-head" aria-hidden="true">
      <ChannelIcon
        className="hosted-player__tv-number"
        icon={subject.channel.icon}
        number={subject.channel.number}
      />
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
      </div>
      {live === null && next === null ? (
        <span />
      ) : (
        <div className="hosted-player__tv-progress">
          <p className="hosted-player__tv-next">
            {live === null ? null : (
              <strong>{minutesLeft(live, info.now)} min left</strong>
            )}
            {next === null ? null : (
              <span>
                {live === null ? "" : "· "}Next <b>{clockLabel(next.startsAt)}</b>{" "}
                {next.title}
              </span>
            )}
          </p>
          {live === null ? null : (
            <div className="hosted-player__tv-meter">
              <time dateTime={live.startsAt}>{clockLabel(live.startsAt)}</time>
              <span
                style={{ "--progress": `${elapsedFraction(live, info.now) * 100}%` }}
              />
              <time dateTime={live.endsAt}>{clockLabel(live.endsAt)}</time>
            </div>
          )}
        </div>
      )}
      <time className="hosted-player__tv-clock" dateTime={info.now.toISOString()}>
        {clockLabel(info.now)}
      </time>
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
