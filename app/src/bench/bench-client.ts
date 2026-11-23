import {
  clientSchemas,
  type Capabilities,
  type CatalogStatus,
  type ChannelDetails,
  type ChannelGroup,
  type ChannelSummary,
  type ClientResult,
  type GuideProgramme,
  type GuideWindow,
  type GuideWindowInput,
  type InstalledPlaybackSession,
  type InstalledPlaybackTransport,
  type InstalledSparrowClient,
  type ListGroupsInput,
  type Page,
  type ProgrammeSummary,
  type RefreshReport,
  type ScheduleInput,
  type SearchInput,
  type SearchPageInput,
  type SearchResults,
} from "../client/contracts";

const GENERATION = 7;
const MINUTE_MS = 60_000;
const GROUP_NAMES = ["News", "Sports", "Movies", "Kids", "Documentary", "Music", "Nordic", "World"];
const CHANNELS_PER_GROUP = 75;
const DURATIONS = [15, 30, 30, 45, 60, 60, 60, 90, 120];
const TITLES = [
  "Morning Report", "Live Football", "Cooking Together", "The Late Film", "Nature Walk",
  "Headlines", "Studio Talk", "Quiz Night", "Weather", "Concert Hall", "Archive Gems",
  "Crime Drama", "Science Hour", "Travel Diaries", "Comedy Club", "Market Watch",
];

interface BenchChannel {
  readonly summary: ChannelSummary;
  readonly programmes: readonly GuideProgramme[];
}

function ok<Value>(value: Value): ClientResult<Value> {
  return { ok: true, value };
}

function seeded(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return state / 2 ** 32;
  };
}

function iso(time: number) {
  return clientSchemas.isoInstant.parse(new Date(time).toISOString());
}

/**
 * A deterministic catalog of a few hundred Channels with a full day of
 * Programmes each, so the TV guide has the same shape on every run.
 */
export function createBenchCatalog(now = Date.now()): readonly BenchChannel[] {
  const dayStart = Math.floor(now / (24 * 60 * MINUTE_MS)) * 24 * 60 * MINUTE_MS - 6 * 60 * MINUTE_MS;
  const dayEnd = dayStart + 36 * 60 * MINUTE_MS;
  const channels: BenchChannel[] = [];
  let number = 1;
  for (const group of GROUP_NAMES) {
    for (let index = 0; index < CHANNELS_PER_GROUP; index++) {
      const random = seeded(number * 7919);
      const programmes: GuideProgramme[] = [];
      let start = dayStart - Math.floor(random() * 4) * 15 * MINUTE_MS;
      while (start < dayEnd && programmes.length < 100) {
        const duration = DURATIONS[Math.floor(random() * DURATIONS.length)] * MINUTE_MS;
        programmes.push({
          title: `${TITLES[Math.floor(random() * TITLES.length)]} ${1 + Math.floor(random() * 40)}`,
          titleTruncated: false,
          startsAt: iso(start),
          endsAt: iso(start + duration),
        });
        start += duration;
      }
      const name = `${group} ${index + 1}`;
      // Every tenth Channel comes as two Quality Variants sharing one row.
      const variants = index % 10 === 3 ? (["hd", "fhd"] as const) : [null];
      for (const quality of variants) {
        channels.push({
          summary: clientSchemas.channel.parse({
            id: `bench-${number}${quality === null ? "" : `-${quality}`}`,
            name: quality === null ? name : `${name} ${quality.toUpperCase()}`,
            group,
            number,
            variant: quality === null ? null : { quality, baseName: name },
            icon: null,
          }),
          programmes,
        });
      }
      number += 1;
    }
  }
  return channels;
}

export class BenchSparrowClient implements InstalledSparrowClient {
  private readonly catalog = createBenchCatalog();

  constructor(private readonly options: { readonly pictureOverlay: boolean }) {}

  capabilities(): Promise<ClientResult<Capabilities>> {
    return Promise.resolve(ok(clientSchemas.capabilities.parse({
      sourceConfiguration: "device-writable",
      playbackTransport: "platform-native",
      audioTrackSelection: true,
      mpvFailover: false,
      pictureOverlay: this.options.pictureOverlay,
    })));
  }

  status(): Promise<ClientResult<CatalogStatus>> {
    return Promise.resolve(ok(this.freshStatus()));
  }

  refresh(): Promise<ClientResult<RefreshReport>> {
    return Promise.resolve(ok(clientSchemas.refreshReport.parse({
      trigger: "manual",
      m3u: { _tag: "not-modified", validatedAt: new Date().toISOString() },
      epg: { _tag: "not-modified", validatedAt: new Date().toISOString() },
      status: this.freshStatus(),
    })));
  }

  subscribe(): () => void {
    return () => undefined;
  }

  listGroups(input: ListGroupsInput): Promise<ClientResult<Page<ChannelGroup>>> {
    const items = GROUP_NAMES.map((name) => ({
      name,
      channelCount: this.catalog.filter((channel) => channel.summary.group === name).length,
    }));
    return Promise.resolve(ok(clientSchemas.groupsPageFor(input).parse({
      generation: GENERATION,
      items: items.slice(0, input.limit),
      next: null,
    })));
  }

  listChannels(): Promise<ClientResult<Page<ChannelSummary>>> {
    return Promise.resolve(ok(clientSchemas.channelsPage.parse({ generation: GENERATION, items: [], next: null })));
  }

  guideWindow(input: GuideWindowInput): Promise<ClientResult<GuideWindow>> {
    const channels = input.group === undefined
      ? this.catalog
      : this.catalog.filter((channel) => channel.summary.group === input.group);
    let start = input.cursor === undefined ? 0 : Number(input.cursor.replace("bench-", ""));
    if (input.around !== undefined) {
      const position = channels.findIndex((channel) => channel.summary.id === input.around);
      start = Math.max(0, position - Math.floor(input.channelLimit / 2));
    }
    const end = Math.min(channels.length, start + input.channelLimit);
    const from = Date.parse(input.startsAt);
    const to = Date.parse(input.endsAt);
    const items = channels.slice(start, end).map((channel) => ({
      channel: channel.summary,
      programmes: channel.programmes.filter((programme) =>
        Date.parse(programme.endsAt) > from && Date.parse(programme.startsAt) < to),
      programmesTruncated: false,
    }));
    return Promise.resolve(ok(clientSchemas.guideWindowFor(input).parse({
      generation: GENERATION,
      items,
      next: end < channels.length ? `bench-${end}` : null,
    })));
  }

  channel(input: { readonly id: string }): Promise<ClientResult<ChannelDetails>> {
    const channel = this.catalog.find((candidate) => candidate.summary.id === input.id);
    return Promise.resolve(channel === undefined
      ? { ok: false, error: { _tag: "not-found", resource: "channel" } }
      : ok(channel.summary));
  }

  schedule(input: ScheduleInput): Promise<ClientResult<Page<ProgrammeSummary>>> {
    const channel = this.catalog.find((candidate) => candidate.summary.id === input.id);
    const from = input.from === undefined ? null : Date.parse(input.from);
    const items = from === null || channel === undefined
      ? []
      : channel.programmes
        .filter((programme) => Date.parse(programme.endsAt) > from)
        .slice(0, input.limit)
        .map((programme) => ({
          channelId: input.id,
          title: programme.title,
          description: `About ${programme.title}.`,
          startsAt: programme.startsAt,
          endsAt: programme.endsAt,
        }));
    return Promise.resolve(ok(clientSchemas.schedulePageFor(input).parse({
      generation: GENERATION,
      items,
      next: null,
    })));
  }

  search(input: SearchInput): Promise<ClientResult<SearchResults>> {
    const term = input.term.toLowerCase();
    const channels = this.catalog
      .filter((channel) => channel.summary.name.toLowerCase().includes(term))
      .slice(0, input.channelLimit)
      .map((channel) => channel.summary);
    return Promise.resolve(ok(clientSchemas.searchResultsFor(input).parse({
      generation: GENERATION,
      channels: { generation: GENERATION, items: channels, next: null },
      programmes: { generation: GENERATION, items: [], next: null },
    })));
  }

  async searchChannels(input: SearchPageInput): Promise<ClientResult<Page<ChannelSummary>>> {
    const result = await this.search({ term: input.term, channelLimit: input.limit, programmeLimit: 1 });
    return result.ok ? ok(result.value.channels) : result;
  }

  searchProgrammes(): Promise<ClientResult<Page<ProgrammeSummary>>> {
    return Promise.resolve(ok(clientSchemas.schedulePage.parse({ generation: GENERATION, items: [], next: null })));
  }

  startPlayback(): never {
    throw new Error("The bench plays through its installed engine.");
  }

  replaceSourceConfiguration(): Promise<ClientResult<CatalogStatus>> {
    return this.status();
  }

  createPlaybackSession(): InstalledPlaybackSession {
    const transport: InstalledPlaybackTransport = {
      _tag: "tauri-native-stream",
      streamHandle: clientSchemas.nativeStreamHandle.parse(`stream1_${"b".repeat(16)}`),
      presentation: "webview-mse",
      tracks: [],
      selection: { _tag: "none" },
    };
    const session: InstalledPlaybackSession = {
      start: async () => ok(transport),
      reopen: async () => ok(transport),
      restart: async () => ok(transport),
      read: async () => ok(new ArrayBuffer(0)),
      startAndroidPresentation: async () => ({
        ok: false,
        error: { _tag: "transport", retryable: false, message: "not on the bench" },
      } as const),
      controlMpv: async () => ok(undefined),
      suspend: async () => ok(undefined),
      setActivity: async () => ok(undefined),
      stop: async () => ok(undefined),
    };
    return session;
  }

  readPlayback(): Promise<ClientResult<ArrayBuffer>> {
    return Promise.resolve(ok(new ArrayBuffer(0)));
  }

  stopPlayback(): Promise<ClientResult<void>> {
    return Promise.resolve(ok(undefined));
  }

  private freshStatus(): CatalogStatus {
    return clientSchemas.status.parse({
      generation: GENERATION,
      configuration: { configured: true, epgConfigured: true },
      m3u: { _tag: "fresh", validatedAt: new Date().toISOString() },
      epg: { _tag: "fresh", validatedAt: new Date().toISOString() },
    });
  }
}
