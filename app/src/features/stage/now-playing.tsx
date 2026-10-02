import type { ChannelId, ChannelSummary } from "../../client/contracts";
import { channelTitle } from "../guide/guide-families";
import { clockLabel, programmeKey } from "../guide/guide-window";
import { elapsedFraction, minutesLeft, programmeAt } from "../guide/now-next";
import { QualitySwitch } from "../guide/quality-switch";
import type { StagePicture } from "./stage-chrome";
import type {
  NowPlayingProgramme,
  NowPlayingReading,
} from "./use-now-playing";
import type { StageLayout } from "./use-stage-layout";

// Pocket lists what follows on the Channel down the watch screen; Theater
// has one line for it.
const UPCOMING_LIMIT: Readonly<Record<StageLayout, number>> = {
  theater: 3,
  pocket: 4,
};

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
  /**
   * How far the reads have got. Until "done", a missing Programme or
   * description may yet arrive.
   */
  readonly reading: NowPlayingReading;
}

/** Inputs for the block of information about what is on now. */
export interface NowPlayingProps {
  /** Null while nothing is playing. */
  readonly subject: NowPlayingSubject | null;
  readonly playingChannel: ChannelId | null;
  readonly now: Date;
  /**
   * Theater has a search key to point at while nothing is playing. Pocket
   * lists one more upcoming Programme and words the player's state itself.
   */
  readonly layout: StageLayout;
  /** What the player says of its picture; null while there is no player. */
  readonly picture: Pick<StagePicture, "state" | "status"> | null;
  /** Receives the element the player puts its controls in. */
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
  picture,
  controlsRef,
  onPreparePlayback,
  onTuneVariant,
}: NowPlayingProps) {
  const programmes = subject?.programmes ?? [];
  // "Now" is only ever a Programme airing at this instant: the schedule read
  // starts at the guide window, so its first item may already have ended.
  const live = programmeAt(programmes, now);
  const upcoming = programmes
    .filter((programme) => Date.parse(programme.startsAt) > now.getTime())
    .slice(0, UPCOMING_LIMIT[layout]);
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
            {/* Nothing is drawn over pocket's picture, and native video
                covers the player's own overlay, so the state is worded here.
                The player's heading is what announces it. */}
            {layout === "pocket" &&
            picture !== null &&
            picture.state !== "playing" ? (
              <span className="now-playing__state" data-state={picture.state}>
                {picture.status}
              </span>
            ) : null}
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
              "Choose a channel below."
            )
          ) : live !== null ? (
            <Meter programme={live} now={now} />
          ) : subject.reading === "programmes" ? null : (
            "Live channel, no guide data"
          )}
        </div>
        {/* The part of pocket's watch screen that scrolls. Pocket keeps it
            out of sight until the description has had its chance to arrive,
            so what follows appears once, in its place, and never jumps. */}
        <div
          className="now-playing__later"
          data-settled={subject === null || subject.reading === "done"}
        >
          {live === null ||
          live.description === null ||
          live.description === "" ? null : (
            <p className="now-playing__about">{live.description}</p>
          )}
          {upcoming.length === 0 ? null : (
            <ol className="now-playing__next" aria-label="Up next">
              {upcoming.map((programme, index) => (
                <li key={programmeKey(programme, index)}>
                  {index === 0 ? (
                    <span className="now-playing__next-lead">Next at </span>
                  ) : null}
                  <time dateTime={programme.startsAt}>
                    {clockLabel(programme.startsAt)}
                  </time>{" "}
                  <b>{programme.title}</b>
                </li>
              ))}
            </ol>
          )}
        </div>
      </div>
      {/* The player's controls land here, except while its own section is fullscreen. */}
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
  return (
    <>
      <time dateTime={programme.startsAt}>{clockLabel(programme.startsAt)}</time>
      <span
        className="now-playing__progress"
        style={{ "--progress": `${elapsedFraction(programme, now) * 100}%` }}
        aria-hidden="true"
      />
      <time dateTime={programme.endsAt}>{clockLabel(programme.endsAt)}</time>
      <strong>{minutesLeft(programme, now)} min left</strong>
    </>
  );
}
