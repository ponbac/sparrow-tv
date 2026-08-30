import { z } from "zod";

const textEncoder = new TextEncoder();
const withinUtf8ByteLimit = (value: string, limit: number): boolean =>
  textEncoder.encode(value).byteLength <= limit;

const channelIdSchema = z
  .string()
  .min(1)
  .max(1024)
  .brand<"ChannelId">();
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

/** A minimized source failure safe to render or retain in browser state. */
export interface SafeFailure {
  readonly _tag:
    | "source-access"
    | "source-read"
    | "snapshot"
    | "snapshot-recovery"
    | "decoded-limit-exceeded"
    | "invalid-encoding"
    | "invalid-format"
    | "no-playable-channels"
    | "invalid-epg-format"
    | "no-epg-channels";
}

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

/** Expected, browser-safe failures returned by every client operation. */
export type ClientError =
  | {
      readonly _tag: "authentication-required";
    }
  | {
      readonly _tag: "invalid-input";
      readonly field:
        | "query"
        | "route"
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
}

const capabilitiesSchema: z.ZodType<Capabilities> = z.strictObject({
  sourceConfiguration: z.literal("deployment-readonly"),
  playbackTransport: z.literal("same-origin-http"),
  audioTrackSelection: z.literal(false),
  mpvFailover: z.literal(false),
});

const safeFailureSchema: z.ZodType<SafeFailure> = z.strictObject({
  _tag: z.enum([
    "source-access",
    "source-read",
    "snapshot",
    "snapshot-recovery",
    "decoded-limit-exceeded",
    "invalid-encoding",
    "invalid-format",
    "no-playable-channels",
    "invalid-epg-format",
    "no-epg-channels",
  ]),
});

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

const catalogStatusSchema: z.ZodType<CatalogStatus> = z.strictObject({
  generation: catalogGenerationSchema.nullable(),
  configuration: z.strictObject({
    configured: z.boolean(),
    epgConfigured: z.boolean(),
  }),
  m3u: sourceStateSchema,
  epg: sourceStateSchema.nullable(),
});

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

const pageSchema = <Item>(
  itemSchema: z.ZodType<Item>,
): z.ZodType<Page<Item>> =>
  z.strictObject({
    generation: catalogGenerationSchema,
    items: z.array(itemSchema),
    next: pageCursorSchema.nullable(),
  });

type ServerClientError = Exclude<
  ClientError,
  { readonly _tag: "transport" } | { readonly _tag: "cancelled" }
>;

const serverClientErrorSchema: z.ZodType<ServerClientError> = z.discriminatedUnion("_tag", [
  z.strictObject({
    _tag: z.literal("authentication-required"),
  }),
  z.strictObject({
    _tag: z.literal("invalid-input"),
    field: z.enum([
      "query",
      "route",
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
]);

const clientErrorEnvelopeSchema: z.ZodType<{
  readonly error: ServerClientError;
}> =
  z.strictObject({
    error: serverClientErrorSchema,
  });

/** Runtime parsers for every success payload and the shared error envelope value. */
export const clientSchemas = Object.freeze({
  capabilities: capabilitiesSchema,
  status: catalogStatusSchema,
  groupsPage: pageSchema(channelGroupSchema),
  channelsPage: pageSchema(channelSummarySchema),
  channel: channelDetailsSchema,
  errorEnvelope: clientErrorEnvelopeSchema,
});
