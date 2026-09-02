import { z } from "zod";

import type {
  ChannelSummary,
  GuideProgramme,
  GuideWindow,
  GuideWindowChannel,
  GuideWindowInput,
  Page,
  PageCursor,
} from "./contracts";

const NANOSECONDS_PER_MILLISECOND = 1_000_000n;
const MAX_GUIDE_WINDOW_NANOSECONDS = 24n * 60n * 60n * 1_000_000_000n;
const MAX_GUIDE_PROGRAMMES_PER_CHANNEL = 100;
const MAX_COMPACT_PROGRAMME_TITLE_BYTES = 256;
const MIN_TRUNCATED_PROGRAMME_TITLE_BYTES =
  MAX_COMPACT_PROGRAMME_TITLE_BYTES - 3;
const MAX_INSTANT_CHARACTERS = 64;
const ISO_INSTANT_PARTS =
  /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d+))?(Z|[+-]\d{2}:\d{2})$/u;

/** Runtime parser for an RFC 3339 instant carrying an explicit UTC offset. */
export const isoInstantSchema = z
  .string()
  .max(MAX_INSTANT_CHARACTERS)
  .datetime({ offset: true })
  .brand<"IsoInstant">();

/** An RFC 3339 instant carrying an explicit UTC offset. */
export type IsoInstant = z.output<typeof isoInstantSchema>;

type GuideWindowCorrelationInput = Pick<
  GuideWindowInput,
  | "startsAt"
  | "endsAt"
  | "channelLimit"
  | "group"
  | "cursor"
  | "previousCursors"
>;

interface GuideContractDependencies {
  readonly channelSummarySchema: z.ZodType<ChannelSummary>;
  readonly pageSchema: <Item>(
    itemSchema: z.ZodType<Item>,
  ) => z.ZodType<Page<Item>>;
  readonly requestedPageSchema: <Item>(
    itemSchema: z.ZodType<Item>,
    requestedLimit: number,
  ) => z.ZodType<Page<Item>>;
  readonly withinUtf8ByteLimit: (value: string, limit: number) => boolean;
  readonly isNewCursor: (
    next: PageCursor | null,
    submitted: PageCursor | undefined,
    previous: readonly PageCursor[] | undefined,
  ) => boolean;
}

interface GuideContractSchemas {
  readonly guideWindow: z.ZodType<GuideWindow>;
  readonly guideWindowFor: (
    input: GuideWindowCorrelationInput,
  ) => z.ZodType<GuideWindow>;
}

/** Returns whether one parsed instant is strictly earlier than another. */
export function isInstantBefore(
  left: IsoInstant,
  right: IsoInstant,
): boolean {
  const leftNanoseconds = instantNanoseconds(left);
  const rightNanoseconds = instantNanoseconds(right);
  return (
    leftNanoseconds !== null &&
    rightNanoseconds !== null &&
    leftNanoseconds < rightNanoseconds
  );
}

/** Returns whether Programme starts are parseable and ordered nondecreasingly. */
export function areProgrammeStartsNondecreasing(
  programmes: readonly { readonly startsAt: IsoInstant }[],
): boolean {
  let previousStart: bigint | undefined;
  for (const programme of programmes) {
    const currentStart = instantNanoseconds(programme.startsAt);
    if (currentStart === null) {
      return false;
    }
    if (previousStart !== undefined && currentStart < previousStart) {
      return false;
    }
    previousStart = currentStart;
  }
  return true;
}

/** Builds the shared bounded Programme slot parser used by guide and search projections. */
export function createBoundedProgrammeSlotSchema(
  withinUtf8ByteLimit: (value: string, limit: number) => boolean,
) {
  return z
    .strictObject({
      title: z
        .string()
        .min(1)
        .max(MAX_COMPACT_PROGRAMME_TITLE_BYTES)
        .refine(
          (value) =>
            withinUtf8ByteLimit(value, MAX_COMPACT_PROGRAMME_TITLE_BYTES),
          { message: "A compact Programme title exceeds its UTF-8 byte bound." },
        ),
      titleTruncated: z.boolean(),
      startsAt: isoInstantSchema,
      endsAt: isoInstantSchema,
    })
    .refine(
      (programme) =>
        isInstantBefore(programme.startsAt, programme.endsAt),
      { message: "Programme end must follow its start." },
    )
    .refine(
      (programme) =>
        !programme.titleTruncated ||
        !withinUtf8ByteLimit(
          programme.title,
          MIN_TRUNCATED_PROGRAMME_TITLE_BYTES - 1,
        ),
      {
        message: "A truncated Programme title must fill its byte bound.",
        path: ["titleTruncated"],
      },
    );
}

/**
 * Builds the bounded guide response parsers from the shared catalog primitives.
 * The returned request-aware parser also enforces cursor and window correlation.
 */
export function createGuideContractSchemas(
  dependencies: GuideContractDependencies,
): GuideContractSchemas {
  const guideProgrammeSchema: z.ZodType<GuideProgramme> =
    createBoundedProgrammeSlotSchema(dependencies.withinUtf8ByteLimit);

  const guideWindowChannelSchema: z.ZodType<GuideWindowChannel> = z
    .strictObject({
      channel: dependencies.channelSummarySchema,
      programmes: z
        .array(guideProgrammeSchema)
        .max(MAX_GUIDE_PROGRAMMES_PER_CHANNEL),
      programmesTruncated: z.boolean(),
    })
    .superRefine((row, context) => {
      if (!areProgrammeStartsNondecreasing(row.programmes)) {
        context.addIssue({
          code: z.ZodIssueCode.custom,
          message: "Guide Programmes must be ordered by start.",
          path: ["programmes"],
        });
      }
      if (
        row.programmesTruncated &&
        row.programmes.length !== MAX_GUIDE_PROGRAMMES_PER_CHANNEL
      ) {
        context.addIssue({
          code: z.ZodIssueCode.custom,
          message: "A truncated guide row must fill the Programme cap.",
          path: ["programmesTruncated"],
        });
      }
    });

  const guideWindow = dependencies.pageSchema(guideWindowChannelSchema);
  const guideWindowFor = (
    input: GuideWindowCorrelationInput,
  ): z.ZodType<GuideWindow> =>
    dependencies
      .requestedPageSchema(guideWindowChannelSchema, input.channelLimit)
      .refine(
        (page) =>
          dependencies.isNewCursor(
            page.next,
            input.cursor,
            input.previousCursors,
          ),
        { message: "A guide continuation cannot repeat an earlier cursor." },
      )
      .refine(
        (page) =>
          input.group === undefined ||
          page.items.every((row) => row.channel.group === input.group),
        { message: "Every guide Channel must belong to the requested group." },
      )
      .refine(
        (page) =>
          new Set(page.items.map((row) => row.channel.id)).size ===
          page.items.length,
        { message: "A guide page cannot repeat a Channel." },
      )
      .refine(
        (page) =>
          isBoundedGuideWindow(input.startsAt, input.endsAt) &&
          page.items.every((row) =>
            row.programmes.every(
              (programme) =>
                isInstantBefore(programme.startsAt, input.endsAt) &&
                isInstantBefore(input.startsAt, programme.endsAt),
            ),
          ),
        { message: "Guide Programmes must overlap the requested UTC window." },
      );

  return Object.freeze({ guideWindow, guideWindowFor });
}

function isBoundedGuideWindow(
  startsAt: IsoInstant,
  endsAt: IsoInstant,
): boolean {
  const start = instantNanoseconds(startsAt);
  const end = instantNanoseconds(endsAt);
  if (start === null || end === null) {
    return false;
  }
  const duration = end - start;
  return duration > 0n && duration <= MAX_GUIDE_WINDOW_NANOSECONDS;
}

function instantNanoseconds(value: IsoInstant): bigint | null {
  const parts = ISO_INSTANT_PARTS.exec(value);
  if (parts === null) {
    return null;
  }
  const wholeSeconds = parts[1];
  const fraction = parts[2] ?? "";
  const offset = parts[3];
  if (wholeSeconds === undefined || offset === undefined) {
    return null;
  }
  const milliseconds = Date.parse(`${wholeSeconds}${offset}`);
  if (!Number.isSafeInteger(milliseconds)) {
    return null;
  }
  const normalizedFraction = fraction.slice(0, 9).padEnd(9, "0");
  const nanoseconds = BigInt(normalizedFraction);
  return BigInt(milliseconds) * NANOSECONDS_PER_MILLISECOND + nanoseconds;
}
