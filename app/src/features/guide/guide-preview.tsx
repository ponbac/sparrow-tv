import { skipToken, useQuery } from "@tanstack/react-query";
import { useEffect, useState, useSyncExternalStore } from "react";
import type { PreviewStore } from "./guide-preview-store";
import {
  clientSchemas,
  type CatalogGeneration,
  type ChannelId,
  type GuideProgramme,
  type IsoInstant,
  type PageCursor,
  type ProgrammeSlot,
  type SparrowClient,
} from "../../client/contracts";
import { generationBoundResult } from "../../client/query-result";
import {
  familyProgrammeSource,
  preferredVariant,
  qualityLabel,
  type GuideFamily,
  type VariantPreferences,
} from "./guide-families";
import { ChannelIcon } from "./channel-icon";
import { clockLabel, durationLabel, programmeKey, programmeKeys } from "./guide-window";
import { elapsedFraction, minutesLeft, programmeAt } from "./now-next";

// The description read waits until focus rests: a run of D-pad presses
// reads nothing for the Channels it passes.
const DESCRIPTION_SETTLE_MS = 300;
const IMMUTABLE_CATALOG_STALE_TIME = Number.POSITIVE_INFINITY;
// One page of the schedule from the shown Programme's start, or the guide
// window's when it began before; more follow while Programmes that overlap
// that instant fill the page.
const DESCRIPTION_READ_LIMIT = 4;
// What the panel lists after the shown Programme, from the rows it holds.
const LATER_PROGRAMMES = 3;

/** Where the preview reads a Programme's description: the schedule read. */
export interface GuidePreviewSource {
  readonly client: Pick<SparrowClient, "schedule">;
  readonly generation: CatalogGeneration | null;
}

/**
 * The TV band's panel about the Channel focus rests on: its name, quality
 * and group, the Programme in large type with its times and progress, two
 * lines about it, and the next three Programmes with their times. With
 * nothing playing a compact plate with the Channel's number takes the
 * picture's place, so the band is never empty.
 */
export function GuidePreview({
  source: descriptions,
  store,
  families,
  variantPreferences,
  playingChannel,
  now,
  windowStart,
}: {
  readonly source: GuidePreviewSource;
  readonly store: PreviewStore;
  readonly families: readonly GuideFamily[];
  readonly variantPreferences: VariantPreferences;
  readonly playingChannel: ChannelId | null;
  readonly now: Date;
  /** Where the guide window starts: every Programme shown ends after it. */
  readonly windowStart: Date;
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
  // The row whose Programmes the panel shows: the preferred variant's, or a
  // sibling's when the preferred one has none. Its description is read from
  // that same Channel.
  const source =
    family === null || preferred === null
      ? null
      : familyProgrammeSource(family, preferred);
  const programmes = source?.programmes ?? [];
  // The focused cell's own Programme, by the key the Timeline gave its cell:
  // two Programmes that start together are told apart.
  const chosenIndex =
    focus !== null && focus.programme !== null && family?.number === focus.number
      ? programmeKeys(programmes).indexOf(focus.programme)
      : -1;
  const chosen = programmes[chosenIndex] ?? null;
  const shown =
    chosen ??
    programmeAt(programmes, now) ??
    programmes.find((programme) => Date.parse(programme.startsAt) > now.getTime()) ??
    null;
  const later =
    shown === null
      ? []
      : programmes
          .filter(
            (programme) => Date.parse(programme.startsAt) >= Date.parse(shown.endsAt),
          )
          .slice(0, LATER_PROGRAMMES);
  const description = useProgrammeDescription(
    descriptions,
    source?.channel.id ?? null,
    shown,
    shown === null ? 0 : slotOrdinal(programmes, shown),
    windowStart,
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
  // Every Quality Variant of a row carries the same Channel Icon.
  const icon = family.variants[0].channel.icon;
  const live =
    shown !== null &&
    Date.parse(shown.startsAt) <= now.getTime() &&
    now.getTime() < Date.parse(shown.endsAt);
  const playing = family.variants.some(
    (variant) => variant.channel.id === playingChannel,
  );

  return (
    // The rows already say the Channel, the Programme and its times to a
    // screen reader, so those parts are for the eye. What the rows do not
    // say, the description and what follows, is a named region that is
    // read when reached, never announced as it changes.
    <div
      className="programme-guide__preview"
      data-idle={idle}
      data-playing={playing}
    >
      {idle ? (
        <div className="programme-guide__poster" aria-hidden="true">
          <ChannelIcon
            className="programme-guide__poster-icon"
            icon={icon}
            number={family.number}
          />
          <strong>{family.title}</strong>
          <span className="programme-guide__poster-hint">
            <kbd>OK</kbd> Watch
          </span>
        </div>
      ) : null}
      <div className="programme-guide__preview-body">
        <div className="programme-guide__preview-head" aria-hidden="true">
          <ChannelIcon
            className="programme-guide__preview-icon"
            icon={icon}
            number={family.number}
          />
          <div className="programme-guide__preview-heading">
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
          </div>
        </div>
        <div
          className="programme-guide__preview-meter"
          aria-hidden="true"
          data-when={
            shown === null
              ? undefined
              : live
                ? "live"
                : Date.parse(shown.startsAt) > now.getTime()
                  ? "later"
                  : "ended"
          }
        >
          {shown === null ? (
            <span>No guide data. The channel still plays.</span>
          ) : live ? (
            <>
              <time dateTime={shown.startsAt}>{clockLabel(shown.startsAt)}</time>
              <span
                className="programme-guide__preview-progress"
                style={{ "--progress": `${elapsedFraction(shown, now) * 100}%` }}
              />
              <time dateTime={shown.endsAt}>{clockLabel(shown.endsAt)}</time>
              <strong>{`${minutesLeft(shown, now)} min left`}</strong>
            </>
          ) : Date.parse(shown.startsAt) > now.getTime() ? (
            // Not on yet: when it starts and how soon, with nothing to fill.
            <>
              <span>
                Starts{" "}
                <time dateTime={shown.startsAt}>{clockLabel(shown.startsAt)}</time>
              </span>
              <span aria-hidden="true">·</span>
              <strong>in {startsIn(shown, now)}</strong>
              <span className="programme-guide__preview-length">
                {durationLabel(shown)}
              </span>
            </>
          ) : (
            <>
              <span>
                Ended{" "}
                <time dateTime={shown.endsAt}>{clockLabel(shown.endsAt)}</time>
              </span>
              <span className="programme-guide__preview-length">
                {durationLabel(shown)}
              </span>
            </>
          )}
        </div>
        <div
          className="programme-guide__preview-details"
          role="region"
          aria-label={`About ${shown === null ? family.title : programmeTitle(shown)}`}
        >
          <p className="programme-guide__preview-about">{description ?? ""}</p>
          {later.length === 0 ? null : (
            <ol className="programme-guide__preview-later" aria-label="Later">
              {later.map((programme) => (
                <li key={programme.startsAt}>
                  <time dateTime={programme.startsAt}>
                    {clockLabel(programme.startsAt)}
                  </time>
                  <b>{programmeTitle(programme)}</b>
                  <span>{durationLabel(programme)}</span>
                </li>
              ))}
            </ol>
          )}
        </div>
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
 * How many Programmes before `shown` in `programmes` have its exact slot.
 * The guide row and the schedule both list a Channel's Programmes in
 * catalog order, and each holds every Programme of that slot, so the count
 * finds `shown` in the schedule. Titles cannot: the row cuts long ones short.
 */
function slotOrdinal(
  programmes: readonly GuideProgramme[],
  shown: GuideProgramme,
): number {
  const index = programmes.indexOf(shown);
  return programmes
    .slice(0, Math.max(0, index))
    .filter((programme) => sameSlot(programme, shown)).length;
}

function sameSlot(a: ProgrammeSlot, b: ProgrammeSlot): boolean {
  return a.startsAt === b.startsAt && a.endsAt === b.endsAt;
}

/**
 * The shown Programme's description, once focus has rested on it. Guide rows
 * carry none; the schedule read from the Programme's start does. The read
 * goes on page by page until it finds the Programme (the `ordinal`-th of its
 * slot), passes its start, or runs out. Null while it is read, and when there
 * is none.
 */
function useProgrammeDescription(
  source: GuidePreviewSource,
  channel: ChannelId | null,
  programme: GuideProgramme | null,
  ordinal: number,
  windowStart: Date,
): string | null {
  const wantedKey =
    channel === null || programme === null
      ? null
      : `${channel}@${programmeKey(programme, ordinal)}`;
  // The key focus last rested on long enough to read.
  const [settledKey, setSettledKey] = useState<string | null>(null);
  useEffect(() => {
    const timer = setTimeout(() => setSettledKey(wantedKey), DESCRIPTION_SETTLE_MS);
    return () => clearTimeout(timer);
  }, [wantedKey]);
  const { client, generation } = source;
  const settled = wantedKey !== null && settledKey === wantedKey;
  const query = useQuery({
    queryKey: ["catalog", "preview", "description", generation, settled ? wantedKey : null],
    queryFn:
      !settled || channel === null || programme === null || generation === null
        ? skipToken
        : ({ signal }) =>
            readDescription(
              client,
              generation,
              channel,
              programme,
              ordinal,
              windowStart,
              signal,
            ),
    retry: false,
    staleTime: IMMUTABLE_CATALOG_STALE_TIME,
  });
  if (!settled) return null;
  return query.data ?? null;
}

async function readDescription(
  client: GuidePreviewSource["client"],
  generation: CatalogGeneration,
  channel: ChannelId,
  programme: GuideProgramme,
  ordinal: number,
  windowStart: Date,
  signal: AbortSignal,
): Promise<string | null> {
  const start = Date.parse(programme.startsAt);
  // Read from the later of the Programme's start and the window's: the
  // Programme is on at both, and a Programme that began long before the
  // window is not preceded by every one still on at its start.
  const from =
    windowStart.getTime() > start
      ? clientSchemas.isoInstant.parse(windowStart.toISOString())
      : programme.startsAt;
  let seen = 0;
  let cursor: PageCursor | undefined;
  let afterStartsAt: IsoInstant | undefined;
  const previousCursors: PageCursor[] = [];
  for (;;) {
    const page = (
      await generationBoundResult(
        client.schedule({
          id: channel,
          from,
          // An earlier Programme still on at `from` may come first; the
          // shown one is matched by its slot and its place in it.
          limit: DESCRIPTION_READ_LIMIT,
          ...(cursor === undefined ? {} : { cursor, afterStartsAt, previousCursors: [...previousCursors] }),
          signal,
        }),
        generation,
      )
    ).value;
    for (const item of page.items) {
      if (Date.parse(item.startsAt) > start) return null;
      if (!sameSlot(item, programme)) continue;
      if (seen === ordinal) return item.description;
      seen += 1;
    }
    signal.throwIfAborted();
    if (page.next === null) return null;
    if (cursor !== undefined) previousCursors.push(cursor);
    cursor = page.next;
    afterStartsAt = page.items.at(-1)?.startsAt ?? afterStartsAt;
  }
}
