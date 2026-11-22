import { skipToken, useQuery } from "@tanstack/react-query";
import { useEffect, useState, useSyncExternalStore } from "react";
import type { PreviewStore } from "./guide-preview-store";
import type {
  CatalogGeneration,
  ChannelId,
  GuideProgramme,
  IsoInstant,
  SparrowClient,
} from "../../client/contracts";
import { generationBoundResult } from "../../client/query-result";
import {
  familyProgrammeSource,
  preferredVariant,
  qualityLabel,
  type GuideFamily,
  type VariantPreferences,
} from "./guide-families";
import { clockLabel } from "./guide-window";
import { elapsedFraction, minutesLeft, programmeAt } from "./now-next";

// The description read waits until focus rests: a run of D-pad presses
// reads nothing for the Channels it passes.
const DESCRIPTION_SETTLE_MS = 300;
const IMMUTABLE_CATALOG_STALE_TIME = Number.POSITIVE_INFINITY;

/** Where the preview reads a Programme's description: the schedule read. */
export interface GuidePreviewSource {
  readonly client: Pick<SparrowClient, "schedule">;
  readonly generation: CatalogGeneration | null;
}

/**
 * The TV band's panel about the Channel focus rests on: its name, quality
 * and group, the Programme in large type with its times and progress, two
 * lines about it, and what follows. With nothing playing it also takes the
 * picture's place with the Channel's number, so the band is never empty.
 */
export function GuidePreview({
  source,
  store,
  families,
  variantPreferences,
  playingChannel,
  now,
}: {
  readonly source: GuidePreviewSource;
  readonly store: PreviewStore;
  readonly families: readonly GuideFamily[];
  readonly variantPreferences: VariantPreferences;
  readonly playingChannel: ChannelId | null;
  readonly now: Date;
}) {
  const focus = useSyncExternalStore(store.subscribe, store.get, store.get);
  const family =
    (focus === null
      ? undefined
      : families.find((candidate) => candidate.number === focus.number)) ??
    families.find((candidate) =>
      candidate.variants.some((variant) => variant.channel.id === playingChannel),
    ) ??
    families[0] ??
    null;
  const preferred =
    family === null ? null : preferredVariant(family, variantPreferences);
  const programmes =
    family === null || preferred === null
      ? []
      : familyProgrammeSource(family, preferred).programmes;
  const chosen =
    focus !== null && focus.startsAt !== null && family?.number === focus.number
      ? (programmes.find((programme) => programme.startsAt === focus.startsAt) ??
        null)
      : null;
  const shown =
    chosen ??
    programmeAt(programmes, now) ??
    programmes.find((programme) => Date.parse(programme.startsAt) > now.getTime()) ??
    null;
  const next =
    shown === null
      ? null
      : (programmes.find(
          (programme) => Date.parse(programme.startsAt) >= Date.parse(shown.endsAt),
        ) ?? null);
  const description = useProgrammeDescription(
    source,
    preferred?.channel.id ?? null,
    shown,
  );
  const idle = playingChannel === null;

  if (family === null || preferred === null) {
    return idle ? (
      <div className="programme-guide__preview" data-idle="true" aria-hidden="true">
        <div className="programme-guide__poster">
          <strong>Sparrow</strong>
          <span>Pick a channel to start watching.</span>
        </div>
      </div>
    ) : null;
  }
  const qualities = family.variants.flatMap((variant) =>
    variant.channel.variant === null ? [] : [variant.channel.variant.quality],
  );
  const current = preferred.channel.variant?.quality ?? null;
  const live =
    shown !== null &&
    Date.parse(shown.startsAt) <= now.getTime() &&
    now.getTime() < Date.parse(shown.endsAt);
  const playing = family.variants.some(
    (variant) => variant.channel.id === playingChannel,
  );

  return (
    // The rows already say all of this to a screen reader: the panel is for the eye.
    <div
      className="programme-guide__preview"
      data-idle={idle}
      data-playing={playing}
      aria-hidden="true"
    >
      {idle ? (
        <div className="programme-guide__poster">
          <span className="programme-guide__poster-number">{family.number}</span>
          <strong>{family.title}</strong>
          <span className="programme-guide__poster-hint">
            <kbd>OK</kbd> Watch
          </span>
        </div>
      ) : null}
      <div className="programme-guide__preview-body">
        <p className="programme-guide__preview-channel">
          <span className="programme-guide__preview-number">{family.number}</span>
          <b>{family.title}</b>
          {qualities.length === 0 ? null : (
            <span className="programme-guide__preview-qualities">
              {qualities.map((quality) => (
                <em key={quality} data-current={quality === current}>
                  {qualityLabel(quality)}
                </em>
              ))}
            </span>
          )}
          {family.group === "" ? null : <span>{family.group}</span>}
        </p>
        <h2 className="programme-guide__preview-title">
          {shown === null ? family.title : programmeTitle(shown)}
        </h2>
        <div className="programme-guide__preview-meter">
          {shown === null ? (
            <span>No guide data. The channel still plays.</span>
          ) : (
            <>
              <time dateTime={shown.startsAt}>{clockLabel(shown.startsAt)}</time>
              <span
                className="programme-guide__preview-progress"
                style={{
                  "--progress": `${live ? elapsedFraction(shown, now) * 100 : 0}%`,
                }}
              />
              <time dateTime={shown.endsAt}>{clockLabel(shown.endsAt)}</time>
              <strong>
                {live
                  ? `${minutesLeft(shown, now)} min left`
                  : Date.parse(shown.startsAt) > now.getTime()
                    ? `In ${startsIn(shown, now)}`
                    : "Ended"}
              </strong>
            </>
          )}
        </div>
        <p className="programme-guide__preview-about">{description ?? ""}</p>
        {next === null ? null : (
          <p className="programme-guide__preview-next">
            <b>{clockLabel(next.startsAt)}</b>
            {programmeTitle(next)}
          </p>
        )}
      </div>
    </div>
  );
}

function programmeTitle(programme: GuideProgramme): string {
  return programme.titleTruncated ? `${programme.title}…` : programme.title;
}

function startsIn(programme: GuideProgramme, now: Date): string {
  const minutes = Math.max(
    1,
    Math.round((Date.parse(programme.startsAt) - now.getTime()) / 60_000),
  );
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours} h` : `${hours} h ${rest} min`;
}

/**
 * The shown Programme's description, once focus has rested on it. Guide rows
 * carry none; the schedule read from the Programme's start does. Null while
 * it is read, and when there is none.
 */
function useProgrammeDescription(
  source: GuidePreviewSource,
  channel: ChannelId | null,
  programme: GuideProgramme | null,
): string | null {
  const from = programme?.startsAt ?? null;
  const wantedKey = channel === null || from === null ? null : `${channel}@${from}`;
  const [settled, setSettled] = useState<{
    readonly channel: ChannelId;
    readonly from: IsoInstant;
    readonly key: string;
  } | null>(null);
  useEffect(() => {
    const timer = setTimeout(
      () =>
        setSettled(
          channel === null || from === null
            ? null
            : { channel, from, key: `${channel}@${from}` },
        ),
      DESCRIPTION_SETTLE_MS,
    );
    return () => clearTimeout(timer);
  }, [channel, from]);
  const { client, generation } = source;
  const read = settled !== null && settled.key === wantedKey ? settled : null;
  const query = useQuery({
    queryKey: ["catalog", "preview", "schedule", generation, read?.channel, read?.from],
    queryFn:
      read === null || generation === null
        ? skipToken
        : ({ signal }) =>
            generationBoundResult(
              client.schedule({
                id: read.channel,
                from: read.from,
                limit: 1,
                signal,
              }),
              generation,
            ),
    retry: false,
    staleTime: IMMUTABLE_CATALOG_STALE_TIME,
  });
  if (read === null || programme === null) return null;
  const item = query.data?.value.items.find(
    (candidate) => candidate.startsAt === programme.startsAt,
  );
  return item?.description ?? null;
}
