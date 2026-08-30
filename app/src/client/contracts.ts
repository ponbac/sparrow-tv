import { z } from "zod";

const textEncoder = new TextEncoder();
const withinUtf8ByteLimit = (value: string, limit: number): boolean =>
  textEncoder.encode(value).byteLength <= limit;

const channelIdSchema = z
  .string()
  .min(1)
  .max(1024)
  .brand<"ChannelId">();
const sameOriginPlaybackEndpointSchema = z
  .string()
  .max(4096)
  .regex(/^\/api\/v1\/play\/[^/?#]+$/)
  .brand<"SameOriginPlaybackEndpoint">();
const catalogGenerationSchema = z
  .number()
  .int()
  .positive()
  .max(Number.MAX_SAFE_INTEGER)
  .brand<"CatalogGeneration">();
const pageCursorSchema = z
  .string()
  .min(1)
  .max(1024)
  .refine((value) => withinUtf8ByteLimit(value, 1024), {
    message: "Page cursors cannot exceed 1024 UTF-8 bytes.",
  })
  .brand<"PageCursor">();
const timestampSchema = z
  .string()
  .datetime({ offset: true })
  .brand<"IsoInstant">();
const channelGroupNameSchema = z
  .string()
  .max(1024)
  .refine((value) => withinUtf8ByteLimit(value, 1024), {
    message: "Channel Group names cannot exceed 1024 UTF-8 bytes.",
  })
  .refine((value) => !/\p{Cc}/u.test(value), {
    message: "Channel Group names cannot contain control characters.",
  });

/** An opaque, non-empty identifier assigned to a channel by the catalog. */
export type ChannelId = z.output<typeof channelIdSchema>;

/** A Sparrow-owned relative playback route that cannot name a provider. */
export type SameOriginPlaybackEndpoint = z.output<
  typeof sameOriginPlaybackEndpointSchema
>;

/** A positive, JavaScript-safe integer identifying one immutable catalog view. */
export type CatalogGeneration = z.output<typeof catalogGenerationSchema>;

/** An opaque, non-empty continuation token tied to one catalog generation. */
export type PageCursor = z.output<typeof pageCursorSchema>;

/** An RFC 3339 instant carrying an explicit UTC offset. */
export type IsoInstant = z.output<typeof timestampSchema>;

/** Deployment capabilities that determine which client controls may be offered. */
export interface Capabilities {
  readonly sourceConfiguration: "deployment-readonly";
  readonly playbackTransport: "same-origin-http";
  readonly audioTrackSelection: false;
  readonly mpvFailover: false;
}

/** A minimized, closed source failure safe to render or retain in browser state. */
export type SafeFailure =
  | {
      readonly _tag: "source-access";
      readonly source: "m3u" | "epg";
      readonly reason: "unavailable" | "rejected" | "timed-out" | "invalid-response";
      readonly retryAfterSeconds: number | null;
    }
  | {
      readonly _tag: "source-read";
      readonly source: "m3u" | "epg";
      readonly reason: "interrupted" | "invalid-body";
    }
  | {
      readonly _tag: "snapshot";
      readonly source: "m3u" | "epg";
      readonly operation:
        | "scan-candidates"
        | "open-candidate"
        | "adopt-candidate"
        | "revalidate-candidate"
        | "begin-stage"
        | "write-stage"
        | "read-stage"
        | "prepare-activation"
        | "activate"
        | "discard";
      readonly reason: "unavailable" | "capacity" | "corrupt";
    }
  | {
      readonly _tag: "snapshot-recovery";
      readonly source: "m3u" | "epg";
      readonly reason:
        | "missing-active-pointer"
        | "corrupt-active-pointer"
        | "missing-manifest"
        | "corrupt-manifest"
        | "missing-payload"
        | "source-mismatch"
        | "length-mismatch"
        | "checksum-mismatch";
    }
  | {
      readonly _tag: "decoded-limit-exceeded";
      readonly source: "m3u" | "epg";
      readonly limitBytes: number;
    }
  | { readonly _tag: "invalid-encoding"; readonly source: "m3u" | "epg" }
  | {
      readonly _tag: "invalid-format";
      readonly source: "m3u";
      readonly entry: number | null;
      readonly reason:
        | "missing-header"
        | "malformed-metadata"
        | "unterminated-quote"
        | "incomplete-entry"
        | "empty-name"
        | "unexpected-location"
        | "unsupported-playback-source";
    }
  | { readonly _tag: "no-playable-channels"; readonly source: "m3u" }
  | {
      readonly _tag: "invalid-epg-format";
      readonly source: "epg";
      readonly reason: "malformed-xml";
    }
  | { readonly _tag: "no-epg-channels"; readonly source: "epg" };

/** The freshness and refresh lifecycle of one configured source. */
export type SourceState =
  | {
      readonly _tag: "fresh";
      readonly validatedAt: IsoInstant;
    }
  | {
      readonly _tag: "stale";
      readonly validatedAt: IsoInstant;
      readonly nextAttemptAt: IsoInstant | null;
    }
  | {
      readonly _tag: "unavailable";
      readonly failure: SafeFailure | null;
    }
  | {
      readonly _tag: "refreshing";
      readonly validatedAt: IsoInstant | null;
      readonly startedAt: IsoInstant;
    }
  | {
      readonly _tag: "deferred";
      readonly validatedAt: IsoInstant | null;
      readonly deferredAt: IsoInstant;
    }
  | {
      readonly _tag: "failed";
      readonly validatedAt: IsoInstant | null;
      readonly failure: SafeFailure;
      readonly nextAttemptAt: IsoInstant;
    };

/** Browser-safe status for the current catalog and its configured sources. */
export interface CatalogStatus {
  readonly generation: CatalogGeneration | null;
  readonly configuration: {
    readonly configured: boolean;
    readonly epgConfigured: boolean;
  };
  readonly m3u: SourceState;
  readonly epg: SourceState | null;
}

/** The safe result of refreshing one independently configured Source. */
export type RefreshOutcome =
  | { readonly _tag: "not-configured" }
  | { readonly _tag: "updated"; readonly validatedAt: IsoInstant }
  | { readonly _tag: "not-modified"; readonly validatedAt: IsoInstant }
  | {
      readonly _tag: "skipped";
      readonly reason: "fresh" | "backoff";
      readonly nextAttemptAt: IsoInstant;
    }
  | {
      readonly _tag: "failed";
      readonly failure: SafeFailure;
      readonly nextAttemptAt: IsoInstant;
    };

/** Completion record for one coalesced hosted manual refresh. */
export interface RefreshReport {
  readonly trigger: "manual";
  readonly m3u: RefreshOutcome;
  readonly epg: RefreshOutcome | null;
  readonly status: CatalogStatus;
}

/** A closed, browser-safe event emitted by the hosted catalog process. */
export type SparrowEvent =
  | {
      readonly _tag: "catalog-status-changed";
      readonly occurredAt: IsoInstant;
      readonly status: CatalogStatus;
    }
  | {
      readonly _tag: "catalog-published";
      readonly occurredAt: IsoInstant;
      readonly generation: CatalogGeneration;
    }
  | {
      readonly _tag: "refresh-completed";
      readonly occurredAt: IsoInstant;
      readonly source: "m3u" | "epg";
      readonly outcome: RefreshOutcome;
    };

/** A group name and the number of channels in that group. */
export interface ChannelGroup {
  readonly name: string;
  readonly channelCount: number;
}

/** The browser-safe identity and grouping fields for a catalog channel. */
export interface ChannelSummary {
  readonly id: ChannelId;
  readonly name: string;
  readonly group: string;
}

/** Complete channel metadata currently exposed by the browse contract. */
export interface ChannelDetails {
  readonly id: ChannelId;
  readonly name: string;
  readonly group: string;
}

/** Browser-safe Programme metadata associated with one catalog Channel. */
export interface ProgrammeSummary {
  readonly channelId: ChannelId;
  readonly title: string;
  readonly description: string | null;
  readonly startsAt: IsoInstant;
  readonly endsAt: IsoInstant;
}

/** One generation-bound page of immutable catalog values. */
export interface Page<Item> {
  readonly generation: CatalogGeneration;
  readonly items: readonly Item[];
  readonly next: PageCursor | null;
}

/** Cancellation options accepted by non-paginated client reads. */
export interface ClientRequestOptions {
  readonly signal?: AbortSignal;
}

/** Input for reading a page of channel groups. */
export interface ListGroupsInput extends ClientRequestOptions {
  readonly limit: number;
  readonly cursor?: PageCursor;
}

/** Input for reading a page of channels, optionally narrowed to one group. */
export interface ListChannelsInput extends ClientRequestOptions {
  readonly limit: number;
  /** Omit for every group; use an empty string for the ungrouped bucket. */
  readonly group?: string;
  readonly cursor?: PageCursor;
}

/** Input for resolving one channel by its opaque identifier. */
export interface ChannelInput extends ClientRequestOptions {
  readonly id: ChannelId;
}

/** Input for reading one generation-bound page of a Channel's schedule. */
export interface ScheduleInput extends ClientRequestOptions {
  readonly id: ChannelId;
  readonly limit: number;
  readonly cursor?: PageCursor;
  /** Last Programme start from the preceding page; used only for response correlation. */
  readonly afterStartsAt?: IsoInstant;
  /** Earlier submitted cursors; used only to reject malformed response cycles. */
  readonly previousCursors?: readonly PageCursor[];
}

/** Input for independently paginating Channel and Programme search results. */
export interface SearchInput extends ClientRequestOptions {
  readonly term: string;
  readonly channelLimit: number;
  readonly channelCursor?: PageCursor;
  readonly channelPreviousCursors?: readonly PageCursor[];
  readonly programmeLimit: number;
  readonly programmeCursor?: PageCursor;
  readonly programmePreviousCursors?: readonly PageCursor[];
}

/** Input for reading one independently paginated search-result lane. */
export interface SearchPageInput extends ClientRequestOptions {
  readonly term: string;
  readonly limit: number;
  readonly cursor?: PageCursor;
  /** Earlier submitted cursors; used only to reject malformed response cycles. */
  readonly previousCursors?: readonly PageCursor[];
}

/** Input for resolving a hosted playback route from an opaque Channel Identifier. */
export interface StartPlaybackInput extends ClientRequestOptions {
  readonly id: ChannelId;
}

/** The browser-safe transport needed to start one hosted live stream. */
export interface PlaybackDescriptor {
  readonly _tag: "same-origin-http";
  readonly endpoint: SameOriginPlaybackEndpoint;
}

/** Independently paginated Channel and Programme matches from one catalog generation. */
export interface SearchResults {
  readonly generation: CatalogGeneration;
  readonly channels: Page<ChannelSummary>;
  readonly programmes: Page<ProgrammeSummary>;
}

/** Expected, browser-safe failures returned by every client operation. */
export type ClientError =
  | {
      readonly _tag: "authentication-required";
    }
  | {
      readonly _tag: "service-unavailable";
    }
  | {
      readonly _tag: "invalid-input";
      readonly field:
        | "query"
        | "route"
        | "body"
        | "header"
        | "m3u"
        | "epg"
        | "channel-id"
        | "channel-group"
        | "search-term"
        | "page-limit"
        | "page-cursor";
      readonly reason:
        | "required"
        | "too-long"
        | "contains-control-character"
        | "unsupported-location"
        | "out-of-range"
        | "invalid-format"
        | "cursor-query-mismatch"
        | "cursor-position-out-of-range";
    }
  | {
      readonly _tag: "not-configured";
    }
  | {
      readonly _tag: "catalog-unavailable";
      readonly status: CatalogStatus;
    }
  | {
      readonly _tag: "not-found";
      readonly resource: "channel";
    }
  | {
      readonly _tag: "stale-cursor";
      readonly current: CatalogGeneration;
    }
  | {
      readonly _tag: "playback-failed";
      readonly reason: "rejected" | "timed-out" | "unavailable" | "invalid-response";
      readonly retryable: boolean;
    }
  | {
      readonly _tag: "transport";
      readonly retryable: boolean;
      readonly message: string;
    }
  | {
      readonly _tag: "cancelled";
    };

/** A success value or an expected client failure; operations do not reject for these cases. */
export type ClientResult<Value> =
  | { readonly ok: true; readonly value: Value }
  | { readonly ok: false; readonly error: ClientError };

/** Transport-neutral reads needed by the hosted catalog browser. */
export interface SparrowClient {
  /** Reads immutable deployment capabilities. */
  capabilities(
    options?: ClientRequestOptions,
  ): Promise<ClientResult<Capabilities>>;

  /** Reads the catalog and source lifecycle status. */
  status(options?: ClientRequestOptions): Promise<ClientResult<CatalogStatus>>;

  /** Requests one coalesced manual refresh of every configured Source. */
  refresh(options?: ClientRequestOptions): Promise<ClientResult<RefreshReport>>;

  /** Subscribes to strictly parsed hosted catalog events; the returned release is idempotent. */
  subscribe(listener: (event: SparrowEvent) => void): () => void;

  /** Reads a generation-bound page of channel groups. */
  listGroups(
    input: ListGroupsInput,
  ): Promise<ClientResult<Page<ChannelGroup>>>;

  /** Reads a generation-bound page of channels, optionally within one group. */
  listChannels(
    input: ListChannelsInput,
  ): Promise<ClientResult<Page<ChannelSummary>>>;

  /** Resolves browser-safe details for one channel. */
  channel(input: ChannelInput): Promise<ClientResult<ChannelDetails>>;

  /** Reads a generation-bound page of Programmes for one Channel. */
  schedule(
    input: ScheduleInput,
  ): Promise<ClientResult<Page<ProgrammeSummary>>>;

  /** Searches Channels and Programmes with independent continuation tokens. */
  search(input: SearchInput): Promise<ClientResult<SearchResults>>;

  /** Searches only the Channel lane with its own continuation token. */
  searchChannels(
    input: SearchPageInput,
  ): Promise<ClientResult<Page<ChannelSummary>>>;

  /** Searches only the Programme lane with its own continuation token. */
  searchProgrammes(
    input: SearchPageInput,
  ): Promise<ClientResult<Page<ProgrammeSummary>>>;

  /** Resolves a same-origin route; provider access starts only when the player consumes it. */
  startPlayback(
    input: StartPlaybackInput,
  ): Promise<ClientResult<PlaybackDescriptor>>;
}

const capabilitiesSchema: z.ZodType<Capabilities> = z.strictObject({
  sourceConfiguration: z.literal("deployment-readonly"),
  playbackTransport: z.literal("same-origin-http"),
  audioTrackSelection: z.literal(false),
  mpvFailover: z.literal(false),
});

const sourceKindSchema = z.enum(["m3u", "epg"]);
const safeUnsignedIntegerSchema = z
  .number()
  .int()
  .nonnegative()
  .max(Number.MAX_SAFE_INTEGER);
const safeFailureSchema: z.ZodType<SafeFailure> = z.discriminatedUnion("_tag", [
  z.strictObject({
    _tag: z.literal("source-access"),
    source: sourceKindSchema,
    reason: z.enum(["unavailable", "rejected", "timed-out", "invalid-response"]),
    retryAfterSeconds: safeUnsignedIntegerSchema.nullable(),
  }),
  z.strictObject({
    _tag: z.literal("source-read"),
    source: sourceKindSchema,
    reason: z.enum(["interrupted", "invalid-body"]),
  }),
  z.strictObject({
    _tag: z.literal("snapshot"),
    source: sourceKindSchema,
    operation: z.enum([
      "scan-candidates",
      "open-candidate",
      "adopt-candidate",
      "revalidate-candidate",
      "begin-stage",
      "write-stage",
      "read-stage",
      "prepare-activation",
      "activate",
      "discard",
    ]),
    reason: z.enum(["unavailable", "capacity", "corrupt"]),
  }),
  z.strictObject({
    _tag: z.literal("snapshot-recovery"),
    source: sourceKindSchema,
    reason: z.enum([
      "missing-active-pointer",
      "corrupt-active-pointer",
      "missing-manifest",
      "corrupt-manifest",
      "missing-payload",
      "source-mismatch",
      "length-mismatch",
      "checksum-mismatch",
    ]),
  }),
  z.strictObject({
    _tag: z.literal("decoded-limit-exceeded"),
    source: sourceKindSchema,
    limitBytes: safeUnsignedIntegerSchema,
  }),
  z.strictObject({
    _tag: z.literal("invalid-encoding"),
    source: sourceKindSchema,
  }),
  z.strictObject({
    _tag: z.literal("invalid-format"),
    source: z.literal("m3u"),
    entry: safeUnsignedIntegerSchema.nullable(),
    reason: z.enum([
      "missing-header",
      "malformed-metadata",
      "unterminated-quote",
      "incomplete-entry",
      "empty-name",
      "unexpected-location",
      "unsupported-playback-source",
    ]),
  }),
  z.strictObject({
    _tag: z.literal("no-playable-channels"),
    source: z.literal("m3u"),
  }),
  z.strictObject({
    _tag: z.literal("invalid-epg-format"),
    source: z.literal("epg"),
    reason: z.literal("malformed-xml"),
  }),
  z.strictObject({
    _tag: z.literal("no-epg-channels"),
    source: z.literal("epg"),
  }),
]);

const sourceStateSchema: z.ZodType<SourceState> = z.discriminatedUnion("_tag", [
  z.strictObject({
    _tag: z.literal("fresh"),
    validatedAt: timestampSchema,
  }),
  z.strictObject({
    _tag: z.literal("stale"),
    validatedAt: timestampSchema,
    nextAttemptAt: timestampSchema.nullable(),
  }),
  z.strictObject({
    _tag: z.literal("unavailable"),
    failure: safeFailureSchema.nullable(),
  }),
  z.strictObject({
    _tag: z.literal("refreshing"),
    validatedAt: timestampSchema.nullable(),
    startedAt: timestampSchema,
  }),
  z.strictObject({
    _tag: z.literal("deferred"),
    validatedAt: timestampSchema.nullable(),
    deferredAt: timestampSchema,
  }),
  z.strictObject({
    _tag: z.literal("failed"),
    validatedAt: timestampSchema.nullable(),
    failure: safeFailureSchema,
    nextAttemptAt: timestampSchema,
  }),
]);

function sourceStateSchemaFor(
  source: SafeFailure["source"],
): z.ZodType<SourceState> {
  return sourceStateSchema.refine(
    (state) =>
      (state._tag !== "failed" && state._tag !== "unavailable") ||
      state.failure === null ||
      state.failure.source === source,
    {
      message: "A source state failure must identify its containing source.",
      path: ["failure", "source"],
    },
  );
}

const catalogStatusSchema: z.ZodType<CatalogStatus> = z.strictObject({
  generation: catalogGenerationSchema.nullable(),
  configuration: z.strictObject({
    configured: z.boolean(),
    epgConfigured: z.boolean(),
  }),
  m3u: sourceStateSchemaFor("m3u"),
  epg: sourceStateSchemaFor("epg").nullable(),
});

const refreshOutcomeSchema: z.ZodType<RefreshOutcome> = z.discriminatedUnion(
  "_tag",
  [
    z.strictObject({ _tag: z.literal("not-configured") }),
    z.strictObject({
      _tag: z.literal("updated"),
      validatedAt: timestampSchema,
    }),
    z.strictObject({
      _tag: z.literal("not-modified"),
      validatedAt: timestampSchema,
    }),
    z.strictObject({
      _tag: z.literal("skipped"),
      reason: z.enum(["fresh", "backoff"]),
      nextAttemptAt: timestampSchema,
    }),
    z.strictObject({
      _tag: z.literal("failed"),
      failure: safeFailureSchema,
      nextAttemptAt: timestampSchema,
    }),
  ],
);

function refreshOutcomeSchemaFor(
  source: SafeFailure["source"],
): z.ZodType<RefreshOutcome> {
  return refreshOutcomeSchema.refine(
    (outcome) =>
      outcome._tag !== "failed" || outcome.failure.source === source,
    {
      message: "A refresh failure must identify its containing source.",
      path: ["failure", "source"],
    },
  );
}

const refreshReportSchema: z.ZodType<RefreshReport> = z.strictObject({
  trigger: z.literal("manual"),
  m3u: refreshOutcomeSchemaFor("m3u"),
  epg: refreshOutcomeSchemaFor("epg").nullable(),
  status: catalogStatusSchema,
});

const sparrowEventSchema: z.ZodType<SparrowEvent> = z
  .discriminatedUnion("_tag", [
    z.strictObject({
      _tag: z.literal("catalog-status-changed"),
      occurredAt: timestampSchema,
      status: catalogStatusSchema,
    }),
    z.strictObject({
      _tag: z.literal("catalog-published"),
      occurredAt: timestampSchema,
      generation: catalogGenerationSchema,
    }),
    z.strictObject({
      _tag: z.literal("refresh-completed"),
      occurredAt: timestampSchema,
      source: z.enum(["m3u", "epg"]),
      outcome: refreshOutcomeSchema,
    }),
  ])
  .refine(
    (event) =>
      event._tag !== "refresh-completed" ||
      event.outcome._tag !== "failed" ||
      event.outcome.failure.source === event.source,
    {
      message: "A refresh event failure must identify its event source.",
      path: ["outcome", "failure", "source"],
    },
  );

const channelGroupSchema: z.ZodType<ChannelGroup> = z.strictObject({
  name: channelGroupNameSchema,
  channelCount: z.number().int().nonnegative().max(4_294_967_295),
});

const channelSummarySchema: z.ZodType<ChannelSummary> = z.strictObject({
  id: channelIdSchema,
  name: z.string(),
  group: channelGroupNameSchema,
});

const channelDetailsSchema: z.ZodType<ChannelDetails> = z.strictObject({
  id: channelIdSchema,
  name: z.string(),
  group: channelGroupNameSchema,
});

const programmeSummarySchema: z.ZodType<ProgrammeSummary> = z
  .strictObject({
    channelId: channelIdSchema,
    title: z.string().min(1),
    description: z.string().nullable(),
    startsAt: timestampSchema,
    endsAt: timestampSchema,
  })
  .refine(
    (programme) =>
      Date.parse(programme.endsAt) > Date.parse(programme.startsAt),
    { message: "Programme end must follow its start." },
  );

const playbackDescriptorSchema: z.ZodType<PlaybackDescriptor> = z.strictObject({
  _tag: z.literal("same-origin-http"),
  endpoint: sameOriginPlaybackEndpointSchema,
});

const pageSchema = <Item>(
  itemSchema: z.ZodType<Item>,
): z.ZodType<Page<Item>> =>
  z.strictObject({
    generation: catalogGenerationSchema,
    items: z.array(itemSchema).max(100),
    next: pageCursorSchema.nullable(),
  });

const requestedPageSchema = <Item>(
  itemSchema: z.ZodType<Item>,
  requestedLimit: number,
): z.ZodType<Page<Item>> =>
  pageSchema(itemSchema).refine(
    (page) =>
      isPageLimit(requestedLimit) &&
      page.items.length <= requestedLimit &&
      (page.next === null || page.items.length === requestedLimit),
    {
      message:
        "A response page must honor its requested limit and fill every continuing page.",
    },
  );

const schedulePageSchemaFor = (
  input: Pick<
    ScheduleInput,
    "id" | "limit" | "cursor" | "afterStartsAt" | "previousCursors"
  >,
): z.ZodType<Page<ProgrammeSummary>> =>
  requestedPageSchema(programmeSummarySchema, input.limit)
    .refine(
      (page) =>
        isNewCursor(page.next, input.cursor, input.previousCursors),
      { message: "A schedule continuation cannot repeat an earlier cursor." },
    )
    .refine(
      (page) => page.items.every((programme) => programme.channelId === input.id),
      { message: "Every scheduled Programme must belong to the requested Channel." },
    )
    .refine(
      (page) => isNondecreasingByStart(page.items),
      { message: "A schedule page must be ordered by Programme start." },
    )
    .refine(
      (page) =>
        input.afterStartsAt === undefined ||
        page.items[0] === undefined ||
        Date.parse(page.items[0].startsAt) >= Date.parse(input.afterStartsAt),
      { message: "A schedule continuation cannot precede its prior page." },
    );

const searchResultsSchema: z.ZodType<SearchResults> = z
  .strictObject({
    generation: catalogGenerationSchema,
    channels: pageSchema(channelSummarySchema),
    programmes: pageSchema(programmeSummarySchema),
  })
  .refine(
    (results) =>
      results.channels.generation === results.generation &&
      results.programmes.generation === results.generation,
    { message: "Search result pages must share the outer generation." },
  );

const searchResultsSchemaFor = (
  input: Pick<
    SearchInput,
    | "channelLimit"
    | "channelCursor"
    | "channelPreviousCursors"
    | "programmeLimit"
    | "programmeCursor"
    | "programmePreviousCursors"
  >,
): z.ZodType<SearchResults> =>
  z
    .strictObject({
      generation: catalogGenerationSchema,
      channels: requestedPageSchema(channelSummarySchema, input.channelLimit),
      programmes: requestedPageSchema(
        programmeSummarySchema,
        input.programmeLimit,
      ),
    })
    .refine(
      (results) =>
        results.channels.generation === results.generation &&
        results.programmes.generation === results.generation,
      { message: "Search result pages must share the outer generation." },
    )
    .refine(
      (results) =>
        isNewCursor(
          results.channels.next,
          input.channelCursor,
          input.channelPreviousCursors,
        ) &&
        isNewCursor(
          results.programmes.next,
          input.programmeCursor,
          input.programmePreviousCursors,
        ),
      { message: "A search continuation cannot repeat an earlier cursor." },
    );

const searchPageSchemaFor = <Item>(
  itemSchema: z.ZodType<Item>,
  input: Pick<SearchPageInput, "limit" | "cursor" | "previousCursors">,
): z.ZodType<Page<Item>> =>
  requestedPageSchema(itemSchema, input.limit).refine(
    (page) => isNewCursor(page.next, input.cursor, input.previousCursors),
    { message: "A search continuation cannot repeat an earlier cursor." },
  );

const searchChannelsPageSchemaFor = (
  input: Pick<SearchPageInput, "limit" | "cursor" | "previousCursors">,
): z.ZodType<Page<ChannelSummary>> =>
  searchPageSchemaFor(channelSummarySchema, input);

const searchProgrammesPageSchemaFor = (
  input: Pick<SearchPageInput, "limit" | "cursor" | "previousCursors">,
): z.ZodType<Page<ProgrammeSummary>> =>
  searchPageSchemaFor(programmeSummarySchema, input);

function isNewCursor(
  next: PageCursor | null,
  submitted: PageCursor | undefined,
  previous: readonly PageCursor[] | undefined,
): boolean {
  return (
    next === null ||
    (next !== submitted && previous?.includes(next) !== true)
  );
}

function isPageLimit(value: number): boolean {
  return Number.isInteger(value) && value >= 1 && value <= 100;
}

function isNondecreasingByStart(
  programmes: readonly ProgrammeSummary[],
): boolean {
  let previousStart: number | undefined;
  for (const programme of programmes) {
    const currentStart = Date.parse(programme.startsAt);
    if (previousStart !== undefined && currentStart < previousStart) {
      return false;
    }
    previousStart = currentStart;
  }
  return true;
}

type ServerClientError = Exclude<
  ClientError,
  { readonly _tag: "transport" } | { readonly _tag: "cancelled" }
>;

const serverClientErrorSchema: z.ZodType<ServerClientError> = z.discriminatedUnion("_tag", [
  z.strictObject({
    _tag: z.literal("authentication-required"),
  }),
  z.strictObject({
    _tag: z.literal("service-unavailable"),
  }),
  z.strictObject({
    _tag: z.literal("invalid-input"),
    field: z.enum([
      "query",
      "route",
      "body",
      "header",
      "m3u",
      "epg",
      "channel-id",
      "channel-group",
      "search-term",
      "page-limit",
      "page-cursor",
    ]),
    reason: z.enum([
      "required",
      "too-long",
      "contains-control-character",
      "unsupported-location",
      "out-of-range",
      "invalid-format",
      "cursor-query-mismatch",
      "cursor-position-out-of-range",
    ]),
  }),
  z.strictObject({
    _tag: z.literal("not-configured"),
  }),
  z.strictObject({
    _tag: z.literal("catalog-unavailable"),
    status: catalogStatusSchema,
  }),
  z.strictObject({
    _tag: z.literal("not-found"),
    resource: z.literal("channel"),
  }),
  z.strictObject({
    _tag: z.literal("stale-cursor"),
    current: catalogGenerationSchema,
  }),
  z.strictObject({
    _tag: z.literal("playback-failed"),
    reason: z.enum(["rejected", "timed-out", "unavailable", "invalid-response"]),
    retryable: z.boolean(),
  }),
]);

const clientErrorEnvelopeSchema: z.ZodType<{
  readonly error: ServerClientError;
}> =
  z.strictObject({
    error: serverClientErrorSchema,
  });

/** Runtime parsers and request-aware parser factories for hosted protocol payloads. */
export const clientSchemas = Object.freeze({
  capabilities: capabilitiesSchema,
  status: catalogStatusSchema,
  refreshOutcome: refreshOutcomeSchema,
  refreshReport: refreshReportSchema,
  sparrowEvent: sparrowEventSchema,
  groupsPage: pageSchema(channelGroupSchema),
  channelsPage: pageSchema(channelSummarySchema),
  channel: channelDetailsSchema,
  schedulePage: pageSchema(programmeSummarySchema),
  searchResults: searchResultsSchema,
  schedulePageFor: schedulePageSchemaFor,
  searchResultsFor: searchResultsSchemaFor,
  searchChannelsPageFor: searchChannelsPageSchemaFor,
  searchProgrammesPageFor: searchProgrammesPageSchemaFor,
  playbackDescriptor: playbackDescriptorSchema,
  errorEnvelope: clientErrorEnvelopeSchema,
});
