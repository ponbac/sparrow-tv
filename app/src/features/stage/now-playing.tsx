import type { ChannelId, ChannelSummary } from "../../client/contracts";
import { channelTitle } from "../guide/guide-families";
import { clockLabel, liveProgramme, programmeKey } from "../guide/guide-window";
import { QualitySwitch } from "../guide/quality-switch";
import type { NowPlayingProgramme } from "./use-now-playing";
import type { StageLayout } from "./use-stage-layout";

const UPCOMING_LIMIT = 3;

/** The Channel the info block describes, with what is known about it. */
export interface NowPlayingSubject {
  readonly channel: ChannelSummary;
  /**
   * The Quality Variants of the Channel's guide row in Channel Catalog order.
   * Fewer than two shows no quality switch.
   */
  readonly variants: readonly ChannelSummary[];
  /** The Channel's Programmes in start order; ended ones are ignored. */
  readonly programmes: readonly NowPlayingProgramme[];
  /** The Programmes are still being read, so their absence means nothing yet. */
  readonly loading: boolean;
}

/** Inputs for the block of information about what is on now. */
export interface NowPlayingProps {
  /** Null while nothing is playing. */
  readonly subject: NowPlayingSubject | null;
  readonly playingChannel: ChannelId | null;
  readonly now: Date;
  /** Theater has a search key to point at while nothing is playing. */
  readonly layout: StageLayout;
  /** Receives the element the Theater layout puts the player's controls in. */
  readonly controlsRef: (element: HTMLDivElement | null) => void;
  readonly onPreparePlayback: () => void;
  readonly onTuneVariant: (channel: ChannelSummary) => void;
}

/** Describes what is on now on a Channel, and what follows. */
export function NowPlaying({
  subject,
  playingChannel,
  now,
  layout,
  controlsRef,
  onPreparePlayback,
  onTuneVariant,
}: NowPlayingProps) {
  const programmes = subject?.programmes ?? [];
  // "Now" is only ever a Programme airing at this instant: the schedule read
  // starts at the guide window, so its first item may already have ended.
  const live = liveProgramme(programmes, now);
  const upcoming = programmes
    .filter((programme) => Date.parse(programme.startsAt) > now.getTime())
    .slice(0, UPCOMING_LIMIT);
  return (
    <div className="now-playing">
      {subject === null ? null : (
        <div className="now-playing__number">{subject.channel.number}</div>
      )}
      <div className="now-playing__body">
        {subject === null ? null : (
          <div className="now-playing__channel">
            <strong>{channelTitle(subject.channel)}</strong>
            {subject.channel.group === "" ? null : (
              <span className="now-playing__group">
                {subject.channel.group}
              </span>
            )}
            {subject.variants.length > 1 ? (
              <QualitySwitch
                variants={subject.variants}
                currentChannel={subject.channel.id}
                playingChannel={playingChannel}
                onPreparePlayback={onPreparePlayback}
                onTuneVariant={onTuneVariant}
              />
            ) : null}
          </div>
        )}
        <h1 id="stage-heading">
          {subject === null
            ? "Pick a channel"
            : (live?.title ?? channelTitle(subject.channel))}
        </h1>
        {/* Always present, so the block keeps its height while reads settle. */}
        <div className="now-playing__meter">
          {subject === null ? (
            layout === "theater" ? (
              "Choose a programme below, or press / to search."
            ) : (
              "Choose a programme below."
            )
          ) : live !== null ? (
            <Meter programme={live} now={now} />
          ) : subject.loading ? null : (
            "Live channel, no guide data"
          )}
        </div>
        {live === null ||
        live.description === null ||
        live.description === "" ? null : (
          <p className="now-playing__about">{live.description}</p>
        )}
        {upcoming.length === 0 ? null : (
          <ol className="now-playing__next" aria-label="Up next">
            {upcoming.map((programme, index) => (
              <li key={programmeKey(programme, index)}>
                {index === 0 ? "Next at " : null}
                <time dateTime={programme.startsAt}>
                  {clockLabel(programme.startsAt)}
                </time>{" "}
                <b>{programme.title}</b>
              </li>
            ))}
          </ol>
        )}
      </div>
      {/* The player's controls land here in the Theater layout. */}
      <div className="now-playing__controls" ref={controlsRef} />
    </div>
  );
}

function Meter({
  programme,
  now,
}: {
  readonly programme: NowPlayingProgramme;
  readonly now: Date;
}) {
  const startsAt = Date.parse(programme.startsAt);
  const endsAt = Date.parse(programme.endsAt);
  const elapsed = (now.getTime() - startsAt) / (endsAt - startsAt);
  const minutesLeft = Math.max(1, Math.ceil((endsAt - now.getTime()) / 60_000));
  return (
    <>
      <time dateTime={programme.startsAt}>{clockLabel(programme.startsAt)}</time>
      <span
        className="now-playing__progress"
        style={{ "--progress": `${Math.min(Math.max(elapsed, 0), 1) * 100}%` }}
        aria-hidden="true"
      />
      <time dateTime={programme.endsAt}>{clockLabel(programme.endsAt)}</time>
      <strong>{minutesLeft} min left</strong>
    </>
  );
}
