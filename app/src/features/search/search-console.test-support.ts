import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement, useState } from "react";
import {
  clientSchemas,
  type Capabilities,
  type CatalogStatus,
  type ChannelDetails,
  type ChannelGroup,
  type ChannelId,
  type ChannelInput,
  type ChannelSummary,
  type ClientError,
  type ClientResult,
  type Page,
  type PlaybackDescriptor,
  type ProgrammeSummary,
  type RefreshReport,
  type ScheduleInput,
  type SearchInput,
  type SearchPageInput,
  type SearchResults,
  type SparrowClient,
  type StartPlaybackInput,
} from "../../client/contracts";
import { SearchConsole } from "./search-console";

const CAPABILITIES = clientSchemas.capabilities.parse({
  sourceConfiguration: "deployment-readonly",
  playbackTransport: "same-origin-http",
  audioTrackSelection: false,
  mpvFailover: false,
});

/** A fresh catalog status shared by search and schedule behavior tests. */
export const FRESH_STATUS = clientSchemas.status.parse({
  generation: 7,
  configuration: { configured: true, epgConfigured: true },
  m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
  epg: { _tag: "fresh", validatedAt: "2026-08-30T10:00:01Z" },
});

/** A channel-only catalog status with no configured Guide source. */
export const NO_GUIDE_STATUS = clientSchemas.status.parse({
  generation: 7,
  configuration: { configured: true, epgConfigured: false },
  m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
  epg: null,
});

/** A status retaining an older validated Guide snapshot. */
export const STALE_GUIDE_STATUS = clientSchemas.status.parse({
  generation: 7,
  configuration: { configured: true, epgConfigured: true },
  m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
  epg: {
    _tag: "stale",
    validatedAt: "2026-08-30T04:00:00Z",
    nextAttemptAt: "2026-08-30T10:10:00Z",
  },
});

/** A status whose configured Guide has never produced a valid snapshot. */
export const FAILED_GUIDE_STATUS = clientSchemas.status.parse({
  generation: 7,
  configuration: { configured: true, epgConfigured: true },
  m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
  epg: {
    _tag: "failed",
    validatedAt: null,
    failure: {
      _tag: "invalid-epg-format",
      source: "epg",
      reason: "malformed-xml",
    },
    nextAttemptAt: "2026-08-30T10:10:00Z",
  },
});

/** Primary Channel fixture used throughout the hosted search tests. */
export const WORLD_CHANNEL = clientSchemas.channel.parse({
  id: "world-news",
  name: "World News",
  group: "News & Current",
});

/** Secondary Channel fixture used to prove page and generation replacement. */
export const CINEMA_CHANNEL = clientSchemas.channel.parse({
  id: "cinema-one",
  name: "Cinema One",
  group: "Cinema",
});

/** Primary Programme fixture with a safe synopsis and valid RFC 3339 times. */
export const MORNING_NEWS = programmeFixture(
  "Morning News",
  "2026-08-30T08:00:00Z",
  "2026-08-30T09:00:00Z",
  "A safe fixture rundown.",
);

/** Empty generation-seven Channel page. */
export const EMPTY_CHANNELS = clientSchemas.channelsPage.parse({
  generation: 7,
  items: [],
  next: null,
});

/** Empty generation-seven Programme page. */
export const EMPTY_PROGRAMMES = clientSchemas.schedulePage.parse({
  generation: 7,
  items: [],
  next: null,
});

/** Single-page World News Channel result. */
export const WORLD_CHANNEL_PAGE = clientSchemas.channelsPage.parse({
  generation: 7,
  items: [WORLD_CHANNEL],
  next: null,
});

/** Single-page Morning News Programme result. */
export const MORNING_PROGRAMME_PAGE = clientSchemas.schedulePage.parse({
  generation: 7,
  items: [MORNING_NEWS],
  next: null,
});

/** Configurable seams exposed by the contract-aware fake hosted client. */
export interface FakeBehavior {
  readonly search?: (input: SearchInput) => Promise<ClientResult<SearchResults>>;
  readonly searchChannels?: (
    input: SearchPageInput,
  ) => Promise<ClientResult<Page<ChannelSummary>>>;
  readonly searchProgrammes?: (
    input: SearchPageInput,
  ) => Promise<ClientResult<Page<ProgrammeSummary>>>;
  readonly schedule?: (
    input: ScheduleInput,
  ) => Promise<ClientResult<Page<ProgrammeSummary>>>;
}

/**
 * Records hosted client inputs and parses every successful fake response through
 * the same request-aware schemas used by the HTTP adapter.
 */
export class FakeSparrowClient implements SparrowClient {
  readonly searchInputs: SearchInput[] = [];
  readonly channelSearchInputs: SearchPageInput[] = [];
  readonly programmeSearchInputs: SearchPageInput[] = [];
  readonly scheduleInputs: ScheduleInput[] = [];

  constructor(private readonly behavior: FakeBehavior = {}) {}

  capabilities(): Promise<ClientResult<Capabilities>> {
    return Promise.resolve(success(CAPABILITIES));
  }

  status(): Promise<ClientResult<CatalogStatus>> {
    return Promise.resolve(success(FRESH_STATUS));
  }

  refresh(): Promise<ClientResult<RefreshReport>> {
    return Promise.resolve(
      success(
        clientSchemas.refreshReport.parse({
          trigger: "manual",
          m3u: {
            _tag: "not-modified",
            validatedAt: "2026-08-30T10:00:00Z",
          },
          epg: {
            _tag: "not-modified",
            validatedAt: "2026-08-30T10:00:01Z",
          },
          status: FRESH_STATUS,
        }),
      ),
    );
  }

  subscribe(): () => void {
    return () => undefined;
  }

  listGroups(): Promise<ClientResult<Page<ChannelGroup>>> {
    return Promise.resolve(
      success(
        clientSchemas.groupsPage.parse({ generation: 7, items: [], next: null }),
      ),
    );
  }

  listChannels(): Promise<ClientResult<Page<ChannelSummary>>> {
    return Promise.resolve(success(EMPTY_CHANNELS));
  }

  channel(input: ChannelInput): Promise<ClientResult<ChannelDetails>> {
    return Promise.resolve(channelDetailsResult(input.id));
  }

  async schedule(
    input: ScheduleInput,
  ): Promise<ClientResult<Page<ProgrammeSummary>>> {
    this.scheduleInputs.push(input);
    const result = await (
      this.behavior.schedule?.(input) ?? Promise.resolve(success(EMPTY_PROGRAMMES))
    );
    if (!result.ok) {
      return result;
    }
    const parsed = clientSchemas.schedulePageFor(input).safeParse(result.value);
    if (!parsed.success) {
      throw new Error("the fake returned a schedule page rejected by the HTTP contract");
    }
    return success(parsed.data);
  }

  async search(input: SearchInput): Promise<ClientResult<SearchResults>> {
    this.searchInputs.push(input);
    const result = await (
      this.behavior.search?.(input) ??
      Promise.resolve(success(searchResults(EMPTY_CHANNELS, EMPTY_PROGRAMMES)))
    );
    if (!result.ok) {
      return result;
    }
    const parsed = clientSchemas.searchResultsFor(input).safeParse(result.value);
    if (!parsed.success) {
      throw new Error("the fake returned search results rejected by the HTTP contract");
    }
    return success(parsed.data);
  }

  async searchChannels(
    input: SearchPageInput,
  ): Promise<ClientResult<Page<ChannelSummary>>> {
    this.channelSearchInputs.push(input);
    const result = await (
      this.behavior.searchChannels?.(input) ??
      Promise.resolve(success(EMPTY_CHANNELS))
    );
    if (!result.ok) {
      return result;
    }
    const parsed = clientSchemas.searchChannelsPageFor(input).safeParse(result.value);
    if (!parsed.success) {
      throw new Error(
        "the fake returned a Channel search page rejected by the HTTP contract",
      );
    }
    return success(parsed.data);
  }

  async searchProgrammes(
    input: SearchPageInput,
  ): Promise<ClientResult<Page<ProgrammeSummary>>> {
    this.programmeSearchInputs.push(input);
    const result = await (
      this.behavior.searchProgrammes?.(input) ??
      Promise.resolve(success(EMPTY_PROGRAMMES))
    );
    if (!result.ok) {
      return result;
    }
    const parsed = clientSchemas.searchProgrammesPageFor(input).safeParse(
      result.value,
    );
    if (!parsed.success) {
      throw new Error(
        "the fake returned a Programme search page rejected by the HTTP contract",
      );
    }
    return success(parsed.data);
  }

  startPlayback(
    input: StartPlaybackInput,
  ): Promise<ClientResult<PlaybackDescriptor>> {
    return Promise.resolve(
      success(
        clientSchemas.playbackDescriptor.parse({
          _tag: "same-origin-http",
          endpoint: `/api/v1/play/${encodeURIComponent(input.id)}`,
        }),
      ),
    );
  }
}

/** Render handle for publishing a new catalog status into the existing harness. */
export interface RenderedConsole {
  readonly rerenderStatus: (status: CatalogStatus) => void;
}

/** Renders SearchConsole with a fresh QueryClient and stateful Channel selection. */
export function renderConsole(
  client: SparrowClient,
  status: CatalogStatus,
): RenderedConsole {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false },
    },
  });
  const tree = (currentStatus: CatalogStatus) =>
    createElement(
      QueryClientProvider,
      { client: queryClient },
      createElement(SearchHarness, { client, status: currentStatus }),
    );
  const rendered = render(tree(status));
  return {
    rerenderStatus: (nextStatus) => {
      rendered.rerender(tree(nextStatus));
      queryClient
        .invalidateQueries({ queryKey: ["catalog"], refetchType: "active" })
        .catch(() => undefined);
    },
  };
}

/** Enters and submits one hosted catalog search term. */
export async function submitSearch(
  user: ReturnType<typeof userEvent.setup>,
  term: string,
): Promise<void> {
  await user.type(
    screen.getByRole("searchbox", { name: "Channel or Programme" }),
    term,
  );
  await user.click(screen.getByRole("button", { name: "Scan index" }));
}

/** Builds a contract-valid result combining independent result lanes. */
export function searchResults(
  channels: Page<ChannelSummary>,
  programmes: Page<ProgrammeSummary>,
): SearchResults {
  const generation =
    channels.items.length > 0 || channels.next !== null
      ? channels.generation
      : programmes.generation;
  return clientSchemas.searchResults.parse({
    generation,
    channels: { ...channels, generation },
    programmes: { ...programmes, generation },
  });
}

/** Builds a full first Channel page carrying a continuation cursor. */
export function continuingChannelPage(
  first: ChannelSummary,
  next: string,
  generation = 7,
): Page<ChannelSummary> {
  return clientSchemas.channelsPage.parse({
    generation,
    items: [
      first,
      ...Array.from({ length: 11 }, (_, index) => ({
        id: `channel-filler-${index}`,
        name: `Channel filler ${index}`,
        group: "Fixture",
      })),
    ],
    next,
  });
}

/** Builds a full first Programme page carrying a continuation cursor. */
export function continuingProgrammePage(
  first: ProgrammeSummary,
  next: string,
  generation = 7,
): Page<ProgrammeSummary> {
  return clientSchemas.schedulePage.parse({
    generation,
    items: [
      first,
      ...Array.from({ length: 9 }, (_, index) => {
        const startsAt = new Date(Date.parse(first.endsAt) + index * 3_600_000);
        const endsAt = new Date(startsAt.getTime() + 3_600_000);
        return {
          channelId: first.channelId,
          title: `Programme filler ${index}`,
          description: null,
          startsAt: startsAt.toISOString(),
          endsAt: endsAt.toISOString(),
        };
      }),
    ],
    next,
  });
}

/** Builds an eight-item first schedule page carrying a continuation cursor. */
export function continuingSchedulePage(
  first: ProgrammeSummary,
  next: string,
  generation = 7,
): Page<ProgrammeSummary> {
  const continuing = continuingProgrammePage(first, next, generation);
  return clientSchemas.schedulePage.parse({
    ...continuing,
    items: continuing.items.slice(0, 8),
  });
}

/** Builds one parsed Programme fixture for a World News schedule. */
export function programmeFixture(
  title: string,
  startsAt: string,
  endsAt: string,
  description: string | null = null,
): ProgrammeSummary {
  const page = clientSchemas.schedulePage.parse({
    generation: 7,
    items: [
      {
        channelId: WORLD_CHANNEL.id,
        title,
        description,
        startsAt,
        endsAt,
      },
    ],
    next: null,
  });
  const programme = page.items[0];
  if (programme === undefined) {
    throw new Error("Programme fixture is missing");
  }
  return programme;
}

/** Wraps one Programme in a terminal page at the requested generation. */
export function programmePage(
  programme: ProgrammeSummary,
  generation: number,
): Page<ProgrammeSummary> {
  return clientSchemas.schedulePage.parse({
    generation,
    items: [programme],
    next: null,
  });
}

/** Wraps one Channel in a terminal page at the requested generation. */
export function channelPage(
  channel: ChannelSummary,
  generation: number,
): Page<ChannelSummary> {
  return clientSchemas.channelsPage.parse({
    generation,
    items: [channel],
    next: null,
  });
}

/** Builds a fresh status at a specific positive catalog generation. */
export function statusAtGeneration(generation: number): CatalogStatus {
  return clientSchemas.status.parse({
    generation,
    configuration: { configured: true, epgConfigured: true },
    m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
    epg: { _tag: "fresh", validatedAt: "2026-08-30T10:00:01Z" },
  });
}

/** Produces a successful typed client result. */
export function success<Value>(
  value: Value,
): { readonly ok: true; readonly value: Value } {
  return { ok: true, value };
}

/** Produces an expected typed client failure. */
export function failure(
  error: ClientError,
): { readonly ok: false; readonly error: ClientError } {
  return { ok: false, error };
}

/** Returns the required recorded input or fails with a focused fixture error. */
export function requireInput<Value>(
  values: readonly Value[],
  predicate: (value: Value) => boolean,
  message: string,
): Value {
  const value = values.find(predicate);
  if (value === undefined) {
    throw new Error(message);
  }
  return value;
}

/** A manually settled Promise used to assert stable pending UI. */
export interface Deferred<Value> {
  readonly promise: Promise<Value>;
  readonly resolve: (value: Value) => void;
}

/** Creates a Promise whose successful settlement is owned by the test. */
export function deferred<Value>(): Deferred<Value> {
  let settle: ((value: Value) => void) | undefined;
  const promise = new Promise<Value>((resolve) => {
    settle = resolve;
  });
  return {
    promise,
    resolve: (value) => {
      if (settle === undefined) {
        throw new Error("deferred promise was not initialized");
      }
      settle(value);
    },
  };
}

function SearchHarness({
  client,
  status,
}: {
  readonly client: SparrowClient;
  readonly status: CatalogStatus;
}) {
  const [selectedChannel, setSelectedChannel] = useState<ChannelId | null>(null);
  return createElement(SearchConsole, {
    client,
    status,
    selectedChannel,
    selectedDetails:
      selectedChannel === null
        ? undefined
        : channelDetailsResult(selectedChannel),
    selectedLoading: false,
    onSelectChannel: setSelectedChannel,
    onRetrySelectedDetails: () => undefined,
  });
}

function channelDetailsResult(id: ChannelId): ClientResult<ChannelDetails> {
  if (id === WORLD_CHANNEL.id) {
    return success(WORLD_CHANNEL);
  }
  if (id === CINEMA_CHANNEL.id) {
    return success(CINEMA_CHANNEL);
  }
  return failure({ _tag: "not-found", resource: "channel" });
}
