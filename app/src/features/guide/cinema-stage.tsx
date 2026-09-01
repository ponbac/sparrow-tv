import { Play, Radio } from "lucide-react";
import { useRef, type ReactNode } from "react";
import type { ChannelSummary, ProgrammeSlot } from "../../client/contracts";
import {
  clockLabel,
  isProgrammeLive,
  programmeKey,
  programmeTiming,
  sameProgramme,
} from "./guide-window";
import "./cinema-stage.css";

/** Inputs for the video, Programme metadata, and selected Channel rundown. */
export interface CinemaStageProps {
  readonly programme: ProgrammeSlot | null;
  readonly channel: ChannelSummary | null;
  readonly programmes: readonly ProgrammeSlot[];
  readonly now: Date;
  readonly player: ReactNode;
  readonly playing: boolean;
  readonly onPlay: () => void;
  readonly onSelectProgramme: (programme: ProgrammeSlot) => void;
}

/** Presents live playback above metadata so native Android video never obscures controls. */
export function CinemaStage({
  programme,
  channel,
  programmes,
  now,
  player,
  playing,
  onPlay,
  onSelectProgramme,
}: CinemaStageProps) {
  const monitorRef = useRef<HTMLDivElement>(null);
  const heading = programme?.title ?? channel?.name ?? "Select a signal";
  const beginPlayback = () => {
    onPlay();
    requestAnimationFrame(() => monitorRef.current?.focus());
  };

  return (
    <section className="cinema-stage" aria-labelledby="cinema-stage-heading">
      <div className="cinema-stage__tally" aria-hidden="true" />
      <div className="cinema-stage__monitor" ref={monitorRef} tabIndex={-1}>
        {player ?? <AtmosphericStandby />}
      </div>

      <div className="cinema-stage__metadata">
        <div className="cinema-stage__channel">
          <span>
            {playing
              ? programme !== null && !isProgrammeLive(programme, now)
                ? "Schedule"
                : "Tuned"
              : "Preview"}
          </span>
          {channel === null ? (
            "No Channel selected"
          ) : (
            <>
              {channel.name}
              {channel.group === "" ? null : <em>{channel.group}</em>}
            </>
          )}
        </div>
        <h1 id="cinema-stage-heading">{heading}</h1>
        <p className="cinema-stage__timing">
          {programme === null
            ? channel === null
              ? "Choose a Programme or Channel from the guide"
              : "Live Channel · no matched Programme"
            : programmeTiming(programme, now)}
        </p>
        {!playing && channel !== null ? (
          <button
            className="cinema-stage__play"
            type="button"
            onClick={beginPlayback}
          >
            <Play aria-hidden="true" />
            Play live
          </button>
        ) : null}
      </div>

      {programmes.length > 0 ? (
        <nav className="cinema-stage__rundown" aria-label={`Schedule for ${channel?.name ?? "selected Channel"}`}>
          {programmes.map((item, index) => {
            const selected = sameProgramme(item, programme);
            return (
              <button
                key={programmeKey(item, index)}
                type="button"
                data-live={isProgrammeLive(item, now)}
                data-selected={selected}
                aria-current={selected ? "true" : undefined}
                onClick={() => onSelectProgramme(item)}
              >
                <time dateTime={item.startsAt}>{clockLabel(item.startsAt)}</time>
                <strong>{item.title}</strong>
              </button>
            );
          })}
        </nav>
      ) : null}
    </section>
  );
}

function AtmosphericStandby() {
  return (
    <div className="cinema-stage__standby" role="status">
      <div className="cinema-stage__wash" aria-hidden="true" />
      <Radio aria-hidden="true" />
      <span>Receiver standing by</span>
    </div>
  );
}
