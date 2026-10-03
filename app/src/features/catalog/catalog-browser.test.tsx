import { type QueryClient, QueryClientProvider, onlineManager } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createSparrowQueryClient } from "../../client/query-client";
import { type ReactElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clientSchemas,
  type Capabilities,
  type CatalogGeneration,
  type CatalogStatus,
  type ChannelDetails,
  type ChannelGroup,
  type ChannelInput,
  type ChannelSummary,
  type ClientError,
  type ClientRequestOptions,
  type ClientResult,
  type GuideProgramme,
  type GuideWindow,
  type GuideWindowChannel,
  type GuideWindowInput,
  type InstalledPlaybackSession,
  type InstalledPlaybackTransport,
  type InstalledSparrowClient,
  type ListChannelsInput,
  type ListGroupsInput,
  type Page,
  type PlaybackDescriptor,
  type ProgrammeSummary,
  type RefreshReport,
  type ScheduleInput,
  type SearchInput,
  type SearchPageInput,
  type SearchResults,
  type SourceConfigurationInput,
  type SparrowEvent,
  type StartPlaybackInput,
} from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import { stubViewport } from "../../test/theater-viewport";
import type { InstalledPlaybackEngine } from "../playback/installed-playback-engine";
import type { HostedPlaybackEngine } from "../playback/mpegts-engine";
import { BOARD_GROUP_EXCLUSIONS_STORAGE_KEY } from "../guide/board-group-roster";
import { CatalogBrowser } from "./catalog-browser";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  localStorage.clear();
});

const HOSTED_CAPABILITIES = clientSchemas.capabilities.parse({
  sourceConfiguration: "deployment-readonly",
  playbackTransport: "same-origin-http",
  audioTrackSelection: false,
  mpvFailover: false,
  pictureOverlay: true,
});

const FRESH_STATUS = clientSchemas.status.parse({
  generation: 7,
  configuration: { configured: true, epgConfigured: true },
  m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
  epg: { _tag: "fresh", validatedAt: "2026-08-30T10:00:01Z" },
});

const NOT_CONFIGURED_STATUS = clientSchemas.status.parse({
  generation: null,
  configuration: { configured: false, epgConfigured: false },
  m3u: { _tag: "unavailable", failure: null },
  epg: null,
});

const CONFIGURED_WITHOUT_GENERATION_STATUS = clientSchemas.status.parse({
  generation: null,
  configuration: { configured: true, epgConfigured: false },
  m3u: { _tag: "unavailable", failure: null },
  epg: null,
});

const RETAINED_STATUS = clientSchemas.status.parse({
  generation: 7,
  configuration: { configured: true, epgConfigured: true },
  m3u: {
    _tag: "stale",
    validatedAt: "2026-08-30T10:00:00Z",
    nextAttemptAt: "2026-08-30T10:10:00Z",
  },
  epg: { _tag: "fresh", validatedAt: "2026-08-30T10:00:01Z" },
});

const DEFAULT_REFRESH_REPORT = clientSchemas.refreshReport.parse({
  trigger: "manual",
  m3u: { _tag: "not-modified", validatedAt: "2026-08-30T10:00:00Z" },
  epg: { _tag: "not-modified", validatedAt: "2026-08-30T10:00:01Z" },
  status: FRESH_STATUS,
});

const GROUPS_PAGE = clientSchemas.groupsPage.parse({
  generation: 7,
  items: [
    { name: "News", channelCount: 1 },
    { name: "Cinema", channelCount: 1 },
  ],
  next: null,
});

const CONTINUING_GROUPS_PAGE = clientSchemas.groupsPageFor({}).parse({
  generation: 7,
  items: Array.from({ length: 100 }, (_, index) => ({
    name: `Group ${index + 1}`,
    channelCount: 1,
  })),
  next: "groups-next",
});

const WORLD_NEWS = channelFixture({
  id: "world-news",
  name: "World News",
  group: "News",
});

const CINEMA_ONE = channelFixture({
  id: "cinema-one",
  name: "Cinema One",
  group: "Cinema",
});

// Two Quality Variants of one Channel: they share a Channel Number and a row.
const SVT1_SD = channelFixture({
  id: "svt1-sd",
  name: "SVT1 SD",
  group: "Sweden",
  number: 901,
  variant: { quality: "sd", baseName: "SVT1" },
});

const SVT1_HD = channelFixture({
  id: "svt1-hd",
  name: "SVT1 HD",
  group: "Sweden",
  number: 901,
  variant: { quality: "hd", baseName: "SVT1" },
});

const EMPTY_SCHEDULE = clientSchemas.schedulePage.parse({
  generation: 7,
  items: [],
  next: null,
});

const GUIDE_UPDATE_FAILED =
  "The guide could not update. These are the last loaded channels.";

const EMPTY_SEARCH_RESULTS = clientSchemas.searchResults.parse({
  generation: 7,
  channels: { generation: 7, items: [], next: null },
  programmes: { generation: 7, items: [], next: null },
});

interface FakeBehavior {
  /** Whether the device lets the page draw over the picture. */
  readonly pictureOverlay?: boolean;
  /** What a Playback Session opens; the default plays in the page. */
  readonly transport?: InstalledPlaybackTransport;
  readonly status?: (
    options: ClientRequestOptions | undefined,
  ) => Promise<ClientResult<CatalogStatus>>;
  readonly groups?: (
    input: ListGroupsInput,
  ) => Promise<ClientResult<Page<ChannelGroup>>>;
  readonly guide?: (
    input: GuideWindowInput,
  ) => Promise<ClientResult<GuideWindow>>;
  readonly schedule?: (
    input: ScheduleInput,
  ) => Promise<ClientResult<Page<ProgrammeSummary>>>;
  readonly search?: (
    input: SearchInput,
  ) => Promise<ClientResult<SearchResults>>;
  readonly replaceConfiguration?: (
    input: SourceConfigurationInput,
  ) => Promise<ClientResult<CatalogStatus>>;
}

class FakeSparrowClient implements InstalledSparrowClient {
  installedSessionCount = 0;
  readonly statusInputs: (ClientRequestOptions | undefined)[] = [];
  readonly groupInputs: ListGroupsInput[] = [];
  readonly guideInputs: GuideWindowInput[] = [];
  readonly channelListInputs: ListChannelsInput[] = [];
  readonly channelInputs: ChannelInput[] = [];
  readonly scheduleInputs: ScheduleInput[] = [];
  readonly searchInputs: SearchInput[] = [];
  readonly playbackInputs: StartPlaybackInput[] = [];
  readonly configurationInputs: SourceConfigurationInput[] = [];
  readonly eventListeners = new Set<(event: SparrowEvent) => void>();

  constructor(private readonly behavior: FakeBehavior = {}) {}

  capabilities(): Promise<ClientResult<Capabilities>> {
    return Promise.resolve(
      success({
        ...HOSTED_CAPABILITIES,
        pictureOverlay: this.behavior.pictureOverlay ?? true,
      }),
    );
  }

  status(options?: ClientRequestOptions): Promise<ClientResult<CatalogStatus>> {
    this.statusInputs.push(options);
    return (
      this.behavior.status?.(options) ?? Promise.resolve(success(FRESH_STATUS))
    );
  }

  refresh(): Promise<ClientResult<RefreshReport>> {
    return Promise.resolve(success(DEFAULT_REFRESH_REPORT));
  }

  subscribe(listener: (event: SparrowEvent) => void): () => void {
    this.eventListeners.add(listener);
    return () => this.eventListeners.delete(listener);
  }

  emit(event: SparrowEvent): void {
    for (const listener of this.eventListeners) {
      listener(event);
    }
  }

  listGroups(
    input: ListGroupsInput,
  ): Promise<ClientResult<Page<ChannelGroup>>> {
    this.groupInputs.push(input);
    return (
      this.behavior.groups?.(input) ?? Promise.resolve(success(GROUPS_PAGE))
    );
  }

  listChannels(
    input: ListChannelsInput,
  ): Promise<ClientResult<Page<ChannelSummary>>> {
    this.channelListInputs.push(input);
    return Promise.resolve(
      success({ generation: 7 as CatalogGeneration, items: [], next: null }),
    );
  }

  guideWindow(input: GuideWindowInput): Promise<ClientResult<GuideWindow>> {
    this.guideInputs.push(input);
    return (
      this.behavior.guide?.(input) ??
      Promise.resolve(defaultGuideResult(input))
    );
  }

  channel(input: ChannelInput): Promise<ClientResult<ChannelDetails>> {
    this.channelInputs.push(input);
    return Promise.resolve(
      success(input.id === CINEMA_ONE.id ? CINEMA_ONE : WORLD_NEWS),
    );
  }

  schedule(
    input: ScheduleInput,
  ): Promise<ClientResult<Page<ProgrammeSummary>>> {
    this.scheduleInputs.push(input);
    return (
      this.behavior.schedule?.(input) ??
      Promise.resolve(success(defaultSchedulePage(input)))
    );
  }

  search(input: SearchInput): Promise<ClientResult<SearchResults>> {
    this.searchInputs.push(input);
    return (
      this.behavior.search?.(input) ??
      Promise.resolve(success(EMPTY_SEARCH_RESULTS))
    );
  }

  searchChannels(
    input: SearchPageInput,
  ): Promise<ClientResult<Page<ChannelSummary>>> {
    return this.search({
      term: input.term,
      channelLimit: input.limit,
      ...(input.cursor === undefined ? {} : { channelCursor: input.cursor }),
      programmeLimit: 1,
    }).then((result) => {
      if (!result.ok) {
        return result;
      }
      return { ok: true, value: result.value.channels };
    });
  }

  searchProgrammes(): Promise<ClientResult<Page<ProgrammeSummary>>> {
    return Promise.resolve(success(EMPTY_SCHEDULE));
  }

  startPlayback(
    input: StartPlaybackInput,
  ): Promise<ClientResult<PlaybackDescriptor>> {
    this.playbackInputs.push(input);
    return Promise.resolve(
      success(
        clientSchemas.hostedPlaybackDescriptor.parse({
          _tag: "same-origin-http",
          endpoint: `/api/v1/play/${encodeURIComponent(input.id)}`,
        }),
      ),
    );
  }

  replaceSourceConfiguration(
    input: SourceConfigurationInput,
  ): Promise<ClientResult<CatalogStatus>> {
    this.configurationInputs.push(input);
    return (
      this.behavior.replaceConfiguration?.(input) ??
      Promise.resolve(success(FRESH_STATUS))
    );
  }

  createPlaybackSession(): InstalledPlaybackSession {
    this.installedSessionCount += 1;
    const transport: InstalledPlaybackTransport = this.behavior.transport ?? {
      _tag: "tauri-native-stream",
      streamHandle: clientSchemas.nativeStreamHandle.parse(
        `stream1_${"b".repeat(16)}`,
      ),
      presentation: "webview-mse",
      tracks: [],
      selection: { _tag: "none" },
    };
    return {
      start: async () => success(transport),
      reopen: async () => success(transport),
      restart: async () => success(transport),
      read: async () => success(new ArrayBuffer(0)),
      startAndroidPresentation: async () =>
        failure({
          _tag: "transport",
          retryable: false,
          message: "not used in this test",
        }),
      controlMpv: async () => success(undefined),
      suspend: async () => success(undefined),
      setActivity: async () => success(undefined),
      stop: async () => success(undefined),
    };
  }

  readPlayback(): Promise<ClientResult<ArrayBuffer>> {
    return Promise.resolve(success(new ArrayBuffer(0)));
  }

  stopPlayback(): Promise<ClientResult<void>> {
    return Promise.resolve(success(undefined));
  }
}

describe("CatalogBrowser shell", () => {
  it("opens the installed saved guide while the device is offline", async () => {
    onlineManager.setOnline(false);
    try {
      renderInstalledBrowser(new FakeSparrowClient());
      expect(await screen.findByLabelText("Programme guide")).toBeVisible();
      expect(screen.queryByRole("heading", { name: "Opening your channels" })).not.toBeInTheDocument();
    } finally {
      onlineManager.setOnline(true);
    }
  });

  it("shows the one initial status loader before mounting the shell", async () => {
    const status = deferred<ClientResult<CatalogStatus>>();
    const client = new FakeSparrowClient({ status: () => status.promise });

    renderHostedBrowser(client);

    expect(
      screen.getByRole("heading", { name: "Opening your channels" }),
    ).toBeVisible();
    expect(screen.queryByLabelText("Programme guide")).not.toBeInTheDocument();

    await act(async () => {
      status.resolve(success(FRESH_STATUS));
      await status.promise;
    });

    expect(await screen.findByLabelText("Programme guide")).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "Opening your channels" }),
    ).not.toBeInTheDocument();
  });

  it("replaces a parallel bootstrap guide from an older generation", async () => {
    const status = deferred<ClientResult<CatalogStatus>>();
    let guideRequest = 0;
    const client = new FakeSparrowClient({
      status: () => status.promise,
      guide: async (input) => {
        guideRequest += 1;
        return success(
          guidePage(input, {
            generation: guideRequest === 1 ? 8 : 7,
            rows: [
              guideRow(
                guideRequest === 1 ? CINEMA_ONE : WORLD_NEWS,
                input,
                guideRequest === 1 ? "Stale Feature" : "Live Bulletin",
              ),
            ],
          }),
        );
      },
    });
    renderHostedBrowser(client);

    await waitFor(() => expect(client.guideInputs).toHaveLength(1));
    await act(async () => {
      status.resolve(success(FRESH_STATUS));
      await status.promise;
    });

    await waitFor(() => expect(client.guideInputs).toHaveLength(2));
    expect(
      await screen.findByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    expect(screen.queryByText("Stale Feature")).not.toBeInTheDocument();
  });

  it("promotes matching bootstrap pages without repeating their requests", async () => {
    const status = deferred<ClientResult<CatalogStatus>>();
    const client = new FakeSparrowClient({ status: () => status.promise });
    renderHostedBrowser(client);

    await waitFor(() => {
      expect(client.groupInputs).toHaveLength(1);
      expect(client.guideInputs).toHaveLength(1);
    });
    await act(async () => Promise.resolve());

    await act(async () => {
      status.resolve(success(FRESH_STATUS));
      await status.promise;
    });

    expect(
      await screen.findByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    expect(client.groupInputs).toHaveLength(1);
    expect(client.guideInputs).toHaveLength(1);
  });

  it("keeps faster status from replacing pending bootstrap requests", async () => {
    const groups = deferred<ClientResult<Page<ChannelGroup>>>();
    const guide = deferred<ClientResult<GuideWindow>>();
    const client = new FakeSparrowClient({
      groups: () => groups.promise,
      guide: () => guide.promise,
    });
    renderHostedBrowser(client);

    await waitFor(() => {
      expect(client.statusInputs).toHaveLength(1);
      expect(client.groupInputs).toHaveLength(1);
      expect(client.guideInputs).toHaveLength(1);
    });
    await act(async () => Promise.resolve());
    expect(client.groupInputs).toHaveLength(1);
    expect(client.guideInputs).toHaveLength(1);

    const guideInput = requireFirst(
      client.guideInputs,
      "expected the pending bootstrap guide request",
    );
    await act(async () => {
      groups.resolve(success(GROUPS_PAGE));
      guide.resolve(success(defaultGuidePage(guideInput)));
      await Promise.all([groups.promise, guide.promise]);
    });

    expect(
      await screen.findByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    expect(client.groupInputs).toHaveLength(1);
    expect(client.guideInputs).toHaveLength(1);
  });

  it("opens a published generation without an old-closure refetch or manual retry", async () => {
    let generation = 7;
    const client = new FakeSparrowClient({
      status: async () =>
        success(
          clientSchemas.status.parse({
            ...FRESH_STATUS,
            generation,
          }),
        ),
      groups: async (input) =>
        success(
          clientSchemas.groupsPageFor(input).parse({
            generation,
            items: [
              {
                name: generation === 7 ? "News" : "Cinema",
                channelCount: 1,
              },
            ],
            next: null,
          }),
        ),
      guide: async (input) =>
        success(
          guidePage(input, {
            generation,
            rows: [
              guideRow(
                generation === 7 ? WORLD_NEWS : CINEMA_ONE,
                input,
                generation === 7 ? "Live Bulletin" : "Published Feature",
              ),
            ],
          }),
        ),
    });
    renderHostedBrowser(client);

    expect(
      await screen.findByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    const initialGuideRequests = client.guideInputs.length;
    const initialGroupRequests = client.groupInputs.length;
    generation = 8;

    act(() => {
      client.emit(
        clientSchemas.sparrowEvent.parse({
          _tag: "catalog-published",
          occurredAt: "2026-09-01T20:00:00Z",
          generation,
        }),
      );
    });

    expect(
      await screen.findByRole("button", { name: "Tune Cinema One" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Tune World News" }),
    ).not.toBeInTheDocument();
    expect(client.guideInputs).toHaveLength(initialGuideRequests + 1);
    expect(client.groupInputs).toHaveLength(initialGroupRequests + 1);
    expect(screen.queryByText(GUIDE_UPDATE_FAILED)).not.toBeInTheDocument();
  });

  it("reconciles status before retrying a missed guide generation", async () => {
    let statusGeneration = 7;
    const publishedGeneration = 8;
    const client = new FakeSparrowClient({
      status: async () =>
        success(
          clientSchemas.status.parse({
            ...FRESH_STATUS,
            generation: statusGeneration,
          }),
        ),
      groups: async (input) =>
        success(
          clientSchemas.groupsPageFor(input).parse({
            generation: publishedGeneration,
            items: [{ name: "News", channelCount: 1 }],
            next: null,
          }),
        ),
      guide: async (input) =>
        success(
          guidePage(input, {
            generation: publishedGeneration,
            rows: [guideRow(WORLD_NEWS, input, "Published Bulletin")],
          }),
        ),
    });
    renderInstalledBrowser(client);

    expect(
      await screen.findByRole("button", { name: "Try again" }),
    ).toBeVisible();
    expect(client.statusInputs).toHaveLength(1);
    expect(client.groupInputs).toHaveLength(1);
    expect(client.guideInputs).toHaveLength(1);
    statusGeneration = publishedGeneration;

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "Try again" }));

    expect(
      await screen.findByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    expect(client.statusInputs).toHaveLength(2);
    expect(client.groupInputs).toHaveLength(2);
    expect(client.guideInputs).toHaveLength(2);
  });

  it("reconciles status before repeating a mismatched board search", async () => {
    let statusGeneration = 7;
    const publishedGeneration = 8;
    const client = new FakeSparrowClient({
      status: async () =>
        success(
          clientSchemas.status.parse({
            ...FRESH_STATUS,
            generation: statusGeneration,
          }),
        ),
      groups: async (input) =>
        success(
          clientSchemas.groupsPageFor(input).parse({
            generation: statusGeneration,
            items: [{ name: "News", channelCount: 1 }],
            next: null,
          }),
        ),
      guide: async (input) =>
        success(
          guidePage(input, {
            generation: statusGeneration,
            rows: [guideRow(WORLD_NEWS, input, "Live Bulletin")],
          }),
        ),
      search: async () =>
        success(
          clientSchemas.searchResults.parse({
            generation: publishedGeneration,
            channels: {
              generation: publishedGeneration,
              items: [WORLD_NEWS],
              next: null,
            },
            programmes: {
              generation: publishedGeneration,
              items: [],
              next: null,
            },
          }),
        ),
    });
    renderInstalledBrowser(client);
    const user = userEvent.setup();

    await user.click(
      await screen.findByRole("combobox", {
        name: "Search channels and programmes",
      }),
    );
    // This case owns generation reconciliation, not inter-keystroke debounce.
    await user.paste("world");
    expect(
      await screen.findByText("The channels changed while you searched."),
    ).toBeVisible();
    expect(client.searchInputs).toHaveLength(1);
    statusGeneration = publishedGeneration;

    await user.click(screen.getByRole("button", { name: "Search again" }));

    expect(
      await screen.findByRole("option", { name: /World News/ }),
    ).toBeVisible();
    expect(client.statusInputs).toHaveLength(2);
    expect(client.searchInputs).toHaveLength(2);
  });

  it("builds now-and-next rows from one bounded guide-window read", async () => {
    const client = new FakeSparrowClient();

    renderHostedBrowser(client);

    const guide = await screen.findByLabelText("Programme guide");
    const worldNews = within(guide).getByRole("button", {
      name: "Tune World News",
    });
    expect(worldNews).toBeVisible();
    // What is on, then what follows with its start time.
    expect(worldNews).toHaveTextContent(
      /^World NewsLive Bulletin\d\d:\d\d Future Bulletin\d+ min left$/u,
    );
    // The list has no Programme cells: the timeline is Theater's.
    expect(
      within(guide).queryByRole("button", { name: /Bulletin,/ }),
    ).not.toBeInTheDocument();

    const input = requireFirst(
      client.guideInputs,
      "expected a guide-window request",
    );
    expect(input.channelLimit).toBe(40);
    expect("group" in input).toBe(false);
    expect(Date.parse(input.endsAt) - Date.parse(input.startsAt)).toBe(
      3 * 60 * 60 * 1_000,
    );
    expect(client.channelListInputs).toHaveLength(0);
    expect(client.scheduleInputs).toHaveLength(0);
  });

  it.each(["suggestion", "desk"] as const)(
    "moves focus out of %s search when a Channel starts playing",
    async (surface) => {
      const client = new FakeSparrowClient({
        search: async () => success(clientSchemas.searchResults.parse({
          generation: 7,
          channels: { generation: 7, items: [WORLD_NEWS], next: null },
          programmes: { generation: 7, items: [], next: null },
        })),
      });
      const user = userEvent.setup();
      renderHostedBrowser(client);
      const search = await screen.findByRole("combobox", {
        name: "Search channels and programmes",
      });
      await user.click(search);
      await user.paste("world");
      if (surface === "desk") {
        await user.click(await screen.findByRole("option", { name: /Open full channel search/ }));
        await user.click(await screen.findByRole("button", { name: "Tune World News" }));
      } else {
        await user.click(await screen.findByRole("option", { name: /World News/ }));
      }
      await waitFor(() => expect(client.playbackInputs).toHaveLength(1));
      await waitFor(() => expect(search).not.toHaveFocus());
      await waitFor(() => expect(
        screen.getByRole("heading", { level: 1 })
          .closest("section")?.contains(document.activeElement),
      ).toBe(true));
    },
  );

  it("filters guide rows without unmounting or restarting active playback", async () => {
    const client = new FakeSparrowClient({
      guide: async (input) =>
        success(
          guidePage(input, {
            rows:
              input.group === "Cinema"
                ? [guideRow(CINEMA_ONE, input, "Feature Presentation")]
                : [guideRow(WORLD_NEWS, input, "Live Bulletin")],
          }),
        ),
    });
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    await waitFor(() => expect(client.playbackInputs).toHaveLength(1));
    expect(client.playbackInputs[0]?.id).toBe(WORLD_NEWS.id);

    await user.click(screen.getByRole("radio", { name: /Cinema/ }));

    expect(
      await screen.findByRole("button", { name: "Tune Cinema One" }),
    ).toBeVisible();
    expect(
      screen.getByRole("heading", { level: 2, name: "World News" }),
    ).toBeVisible();
    expect(client.playbackInputs).toHaveLength(1);
    expect(
      requireMatch(
        client.guideInputs,
        (input) => input.group === "Cinema",
        "expected the selected group to reach guideWindow",
      ).group,
    ).toBe("Cinema");
  });

  it("excludes a Channel Group from the board and All-window rows", async () => {
    const client = new FakeSparrowClient({
      guide: async (input) =>
        success(
          guidePage(input, {
            rows: [guideRow(WORLD_NEWS, input, "Live Bulletin"), guideRow(CINEMA_ONE, input, "Feature Presentation")],
          }),
        ),
    });
    const user = userEvent.setup();
    renderHostedBrowser(client);

    expect(
      await screen.findByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Choose groups" }));
    await user.click(
      await screen.findByRole("button", { name: "Hide News" }),
    );
    await user.click(
      screen.getByRole("button", { name: "Close channel groups" }),
    );

    const lane = screen.getByRole("radiogroup", { name: "Channel groups" });
    expect(within(lane).queryByRole("radio", { name: /News/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Tune World News" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Tune Cinema One" })).toBeVisible();
    expect(localStorage.getItem(BOARD_GROUP_EXCLUSIONS_STORAGE_KEY)).toContain(
      "News",
    );
  });

  it("keeps playback mounted when the clock opens the next guide window", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-01T20:29:45.000Z"));
    const client = new FakeSparrowClient();
    renderHostedBrowser(client);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    fireEvent.click(screen.getByRole("button", { name: "Tune World News" }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(client.playbackInputs).toHaveLength(1);
    const firstWindow = requireFirst(
      client.guideInputs,
      "expected the initial guide window",
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });

    expect(client.guideInputs.length).toBeGreaterThan(1);
    expect(client.guideInputs.at(-1)?.startsAt).not.toBe(firstWindow.startsAt);
    expect(client.playbackInputs).toHaveLength(1);
    expect(
      screen.getByRole("heading", { level: 2, name: "World News" }),
    ).toBeVisible();
  });

  it("choosing a future cell of the timeline tunes the Channel and the stage shows the live Programme", async () => {
    stubViewport(true);
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(
      await screen.findByRole("button", { name: /Future Bulletin,/ }),
    );

    expect(
      await screen.findByRole("heading", { level: 1, name: "Live Bulletin" }),
    ).toBeVisible();
    expect(
      screen.getByRole("heading", { level: 2, name: "World News" }),
    ).toBeVisible();
    expect(client.playbackInputs).toHaveLength(1);
    expect(client.playbackInputs[0]?.id).toBe(WORLD_NEWS.id);
  });

  it("starts with nothing playing and returns there after Stop", async () => {
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await screen.findByRole("button", { name: "Tune Cinema One" });
    const heading = screen.getByRole("heading", { level: 1 });
    expect(heading).toHaveTextContent("Pick a channel");
    expect(screen.getByText("Choose a channel below.")).toBeVisible();
    expect(screen.getByText("Nothing playing")).toBeVisible();
    expect(client.playbackInputs).toHaveLength(0);

    await user.click(screen.getByRole("button", { name: "Tune Cinema One" }));

    await waitFor(() =>
      expect(heading).toHaveTextContent("Feature Presentation"),
    );
    const stage = within(requireStage(heading));
    expect(stage.getByText("Cinema")).toBeVisible();
    expect(stage.getByText(String(CINEMA_ONE.number))).toBeVisible();
    expect(stage.getByText(/^\d+ min left$/u)).toBeVisible();
    expect(screen.queryByText("Nothing playing")).not.toBeInTheDocument();

    await user.click(
      await screen.findByRole("button", { name: "Stop stream" }),
    );

    await waitFor(() => expect(heading).toHaveTextContent("Pick a channel"));
    expect(screen.getByText("Nothing playing")).toBeVisible();
  });

  it("titles the stage with the Channel when the guide has no Programme for it", async () => {
    const client = new FakeSparrowClient({
      guide: async (input) =>
        success(
          guidePage(input, {
            rows: [
              { channel: CINEMA_ONE, programmes: [], programmesTruncated: false },
            ],
          }),
        ),
      schedule: async () => success(EMPTY_SCHEDULE),
    });
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(
      await screen.findByRole("button", { name: "Tune Cinema One" }),
    );

    expect(
      await screen.findByRole("heading", { level: 1, name: "Cinema One" }),
    ).toBeVisible();
    expect(screen.getByText("Live channel, no guide data")).toBeVisible();
  });

  it("marks one button per Channel in catalog order, before and after a tune", async () => {
    const client = new FakeSparrowClient({ guide: variantGuide });
    const user = userEvent.setup();
    renderHostedBrowser(client);
    await screen.findByRole("button", { name: "Tune SVT1" });

    // Android acceptance counts these, clicks the first two and reads back
    // which one is pressed.
    expect(acceptanceChannels()).toEqual([
      ["Tune SVT1 SD", "false"],
      ["Tune SVT1 HD", "false"],
      ["Tune World News", "false"],
      ["Tune Cinema One", "false"],
    ]);

    await user.click(screen.getByRole("button", { name: "Tune SVT1 HD" }));

    await waitFor(() => expect(client.playbackInputs).toHaveLength(1));
    expect(client.playbackInputs[0]?.id).toBe(SVT1_HD.id);
    // The info block now offers the same qualities; its chips carry no mark.
    await waitFor(() =>
      expect(
        screen.getAllByRole("group", { name: "Picture quality" }),
      ).toHaveLength(2),
    );
    expect(acceptanceChannels()).toEqual([
      ["Tune SVT1 SD", "false"],
      ["Tune SVT1 HD", "true"],
      ["Tune World News", "false"],
      ["Tune Cinema One", "false"],
    ]);
  });

  it("plays a row's best quality until the viewer picks another, and remembers the pick", async () => {
    const client = new FakeSparrowClient({ guide: variantGuide });
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(await screen.findByRole("button", { name: "Tune SVT1" }));
    await waitFor(() => expect(client.playbackInputs).toHaveLength(1));
    expect(client.playbackInputs[0]?.id).toBe(SVT1_HD.id);

    await user.click(
      within(screen.getByLabelText("Programme guide")).getByRole("button", {
        name: "Tune SVT1 SD",
      }),
    );
    await waitFor(() => expect(client.playbackInputs).toHaveLength(2));
    expect(client.playbackInputs[1]?.id).toBe(SVT1_SD.id);

    cleanup();
    const nextVisit = new FakeSparrowClient({ guide: variantGuide });
    renderHostedBrowser(nextVisit);
    await user.click(await screen.findByRole("button", { name: "Tune SVT1" }));
    await waitFor(() => expect(nextVisit.playbackInputs).toHaveLength(1));
    expect(nextVisit.playbackInputs[0]?.id).toBe(SVT1_SD.id);
  });

  it("switches picture quality from the info block and remembers the pick", async () => {
    const client = new FakeSparrowClient({ guide: variantGuide });
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(await screen.findByRole("button", { name: "Tune SVT1" }));
    const stage = within(
      requireStage(screen.getByRole("heading", { level: 1 })),
    );
    const standard = await stage.findByRole("button", { name: "Tune SVT1 SD" });
    expect(stage.getByRole("button", { name: "Tune SVT1 HD" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    await user.click(standard);

    await waitFor(() => expect(client.playbackInputs).toHaveLength(2));
    expect(client.playbackInputs[1]?.id).toBe(SVT1_SD.id);
    await waitFor(() =>
      expect(
        stage.getByRole("button", { name: "Tune SVT1 SD" }),
      ).toHaveAttribute("aria-pressed", "true"),
    );

    cleanup();
    const nextVisit = new FakeSparrowClient({ guide: variantGuide });
    renderHostedBrowser(nextVisit);
    await user.click(await screen.findByRole("button", { name: "Tune SVT1" }));
    await waitFor(() => expect(nextVisit.playbackInputs).toHaveLength(1));
    expect(nextVisit.playbackInputs[0]?.id).toBe(SVT1_SD.id);
  });

  it("drops the previous catalog's rows around the playing Channel while the new ones load", async () => {
    let generation = 7;
    const published = deferred<ClientResult<GuideWindow>>();
    const client = new FakeSparrowClient({
      status: async () =>
        success(clientSchemas.status.parse({ ...FRESH_STATUS, generation })),
      groups: async (input) => success(newsGroupsPage(input, generation)),
      guide: async (input) => {
        // Only the new generation's read around the playing Channel is held.
        if (generation === 8 && input.around !== undefined) {
          return published.promise;
        }
        return success(
          guidePage(input, {
            generation,
            rows: [
              guideRow(SVT1_SD, input, "Rapport"),
              guideRow(SVT1_HD, input, "Rapport"),
            ],
          }),
        );
      },
    });
    const user = userEvent.setup();
    renderHostedBrowser(client);
    await user.click(await screen.findByRole("button", { name: "Tune SVT1" }));
    const stage = within(
      requireStage(screen.getByRole("heading", { level: 1 })),
    );
    await stage.findByRole("group", { name: "Picture quality" });
    generation = 8;

    act(() => client.emit(catalogPublished(generation)));

    // The board has the new catalog; the info block must not keep offering
    // the old one's Quality Variants in the meantime.
    await waitFor(() => expect(neighbourhoodInputs(client)).toHaveLength(2));
    await waitFor(() =>
      expect(
        stage.queryByRole("group", { name: "Picture quality" }),
      ).not.toBeInTheDocument(),
    );
    expect(client.playbackInputs).toHaveLength(1);

    const held = requireMatch(
      neighbourhoodInputs(client).slice(1),
      () => true,
      "expected the new generation's read around the playing Channel",
    );
    await act(async () => {
      published.resolve(
        success(
          guidePage(held, {
            generation,
            rows: [
              guideRow(SVT1_SD, held, "Rapport"),
              guideRow(SVT1_HD, held, "Rapport"),
            ],
          }),
        ),
      );
      await published.promise;
    });

    expect(
      await stage.findByRole("group", { name: "Picture quality" }),
    ).toBeVisible();
  });

  it("reads the playing Channel's schedule and the rows around it once per tune", async () => {
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );

    const heading = await screen.findByRole("heading", {
      level: 1,
      name: "Live Bulletin",
    });
    expect(
      within(requireStage(heading)).getByRole("list", { name: "Up next" }),
    ).toHaveTextContent(/^Next at \d\d:\d\d Future Bulletin$/u);
    const board = requireFirst(
      client.guideInputs,
      "expected the board's guide-window request",
    );
    expect(
      client.scheduleInputs.map(({ id, from, limit }) => ({ id, from, limit })),
    ).toEqual([{ id: WORLD_NEWS.id, from: board.startsAt, limit: 8 }]);
    expect(
      neighbourhoodInputs(client).map(
        ({ around, channelLimit, startsAt, endsAt }) => ({
          around,
          channelLimit,
          startsAt,
          endsAt,
        }),
      ),
    ).toEqual([
      {
        around: WORLD_NEWS.id,
        channelLimit: 21,
        startsAt: board.startsAt,
        endsAt: board.endsAt,
      },
    ]);
  });

  it("re-reads the playing Channel's schedule and neighbourhood when a catalog is published", async () => {
    let generation = 7;
    const bulletin = () =>
      generation === 7 ? "Live Bulletin" : "Published Bulletin";
    const client = new FakeSparrowClient({
      status: async () =>
        success(clientSchemas.status.parse({ ...FRESH_STATUS, generation })),
      groups: async (input) => success(newsGroupsPage(input, generation)),
      guide: async (input) =>
        success(
          guidePage(input, {
            generation,
            rows: [guideRow(WORLD_NEWS, input, bulletin())],
          }),
        ),
      schedule: async (input) =>
        success(schedulePage(input, [[bulletin(), 0, 180]], generation)),
    });
    const user = userEvent.setup();
    renderHostedBrowser(client);
    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    const heading = await screen.findByRole("heading", {
      level: 1,
      name: "Live Bulletin",
    });
    expect(client.scheduleInputs).toHaveLength(1);
    expect(neighbourhoodInputs(client)).toHaveLength(1);
    generation = 8;

    act(() => client.emit(catalogPublished(generation)));

    await waitFor(() => expect(heading).toHaveTextContent("Published Bulletin"));
    expect(client.scheduleInputs).toHaveLength(2);
    expect(neighbourhoodInputs(client)).toHaveLength(2);
    expect(client.playbackInputs).toHaveLength(1);
  });

  it("keeps playing a Channel the published catalog dropped, without guide data or an alert", async () => {
    let generation = 7;
    const dropped = failure({ _tag: "not-found", resource: "channel" });
    const client = new FakeSparrowClient({
      status: async () =>
        success(clientSchemas.status.parse({ ...FRESH_STATUS, generation })),
      groups: async (input) => success(newsGroupsPage(input, generation)),
      guide: async (input) => {
        if (generation === 7) {
          return defaultGuideResult(input);
        }
        return input.around === undefined
          ? success(
              guidePage(input, {
                generation,
                rows: [guideRow(CINEMA_ONE, input, "Published Feature")],
              }),
            )
          : dropped;
      },
      schedule: async (input) =>
        generation === 7 ? success(defaultSchedulePage(input)) : dropped,
    });
    const user = userEvent.setup();
    renderHostedBrowser(client);
    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    const heading = await screen.findByRole("heading", {
      level: 1,
      name: "Live Bulletin",
    });
    generation = 8;

    act(() => client.emit(catalogPublished(generation)));

    await waitFor(() => expect(heading).toHaveTextContent("World News"));
    expect(await screen.findByText("Live channel, no guide data")).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Tune World News" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(client.playbackInputs).toHaveLength(1);
    expect(
      screen.getByRole("heading", { level: 2, name: "World News" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Stop stream" })).toBeVisible();
  });

  it("reports source freshness inside the guide, Channels first", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-08-30T10:04:30.000Z"));
    renderHostedBrowser(
      new FakeSparrowClient({ status: async () => success(RETAINED_STATUS) }),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    // The Android acceptance probe reads the first marked readout's state.
    const readouts = Array.from(
      screen
        .getByLabelText("Programme guide")
        .querySelectorAll("[data-acceptance-status]"),
    );
    expect(
      readouts.map((readout) => [
        readout.textContent,
        readout.getAttribute("data-state"),
      ]),
    ).toEqual([
      ["Channels updated 4 min ago", "stale"],
      ["Guide updated 4 min ago", "fresh"],
    ]);
  });

  it.each([
    {
      name: "unconfigured",
      status: NOT_CONFIGURED_STATUS,
      title: "Add your sources",
      detail: "Open Sources to set up this device before browsing.",
    },
    {
      name: "configured without a generation",
      status: CONFIGURED_WITHOUT_GENERATION_STATUS,
      title: "Waiting for the first catalog",
      detail: "The sources have not loaded yet.",
    },
  ])(
    "keeps installed browse off when $name",
    async ({ status, title, detail }) => {
      const client = new FakeSparrowClient({
        status: async () => success(status),
      });

      renderInstalledBrowser(client);

      expect(await screen.findByText(title)).toBeVisible();
      expect(screen.getByText(detail)).toBeVisible();
      expect(client.groupInputs).toHaveLength(0);
      expect(client.guideInputs).toHaveLength(0);
      expect(client.channelListInputs).toHaveLength(0);
      expect(client.scheduleInputs).toHaveLength(0);
    },
  );

  it("applies private installed configuration, clears its fields, and begins browsing", async () => {
    const client = new FakeSparrowClient({
      status: async () => success(NOT_CONFIGURED_STATUS),
    });
    const user = userEvent.setup();
    renderInstalledBrowser(client);

    await screen.findByText("Add your sources");
    await user.click(screen.getByRole("button", { name: "Sources" }));
    const m3u = await screen.findByLabelText("Channel source (required)");
    const epg = screen.getByLabelText("Guide source (optional)");
    const privateM3u = "https://viewer:secret@provider.invalid/list.m3u";
    const privateEpg = "https://viewer:secret@provider.invalid/guide.xml";

    expect(m3u).toHaveAttribute("autocomplete", "off");
    await user.type(m3u, privateM3u);
    await user.type(epg, privateEpg);
    await user.click(
      screen.getByRole("button", { name: "Save sources" }),
    );

    expect(
      await screen.findByText(
        "Sources saved. The source status updates as the catalog is built.",
      ),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Close sources" }));
    expect(
      await screen.findByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    expect(client.configurationInputs).toHaveLength(1);
    expect(client.configurationInputs[0]).toMatchObject({
      m3uLocation: privateM3u,
      epgLocation: privateEpg,
    });
    expect(m3u).toHaveValue("");
    expect(epg).toHaveValue("");
    expect(document.body).not.toHaveTextContent(privateM3u);
    expect(document.body).not.toHaveTextContent(privateEpg);
    expect(client.groupInputs).toHaveLength(1);
    expect(client.guideInputs).toHaveLength(1);
  });

  it("keeps hosted Sources read-only and free of source-location controls", async () => {
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(await screen.findByRole("button", { name: "Sources" }));

    const dialog = await screen.findByRole("dialog", {
      name: "Sources",
    });
    expect(
      within(dialog).getByText(/sources are set on the server/),
    ).toBeVisible();
    expect(
      within(dialog).queryByLabelText("Channel source (required)"),
    ).not.toBeInTheDocument();
    expect(
      within(dialog).getByRole("region", {
        name: "Safe source diagnostics",
      }),
    ).not.toHaveTextContent("http");
    expect(client.configurationInputs).toHaveLength(0);
  });

  it("marks a retained catalog without hiding its usable guide", async () => {
    const client = new FakeSparrowClient({
      status: async () => success(RETAINED_STATUS),
    });
    renderHostedBrowser(client);

    const retained = await screen.findByText(
      "Showing the saved catalog. A fresh source check is pending.",
    );
    expect(retained.closest("aside")).not.toBeNull();
    expect(
      screen.getByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
  });

  it("retries only status when the visible failure belongs to status", async () => {
    let statusAttempts = 0;
    const client = new FakeSparrowClient({
      status: async () => {
        statusAttempts += 1;
        return statusAttempts === 1
          ? failure({ _tag: "service-unavailable" })
          : success(FRESH_STATUS);
      },
    });
    const user = userEvent.setup();
    renderHostedBrowser(client);

    expect(
      await screen.findByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    expect(screen.getByText(GUIDE_UPDATE_FAILED)).toBeVisible();
    expect(client.statusInputs).toHaveLength(1);
    expect(client.groupInputs).toHaveLength(1);
    expect(client.guideInputs).toHaveLength(1);

    await user.click(screen.getByRole("button", { name: "Try again" }));

    await waitFor(() => expect(client.statusInputs).toHaveLength(2));
    await waitFor(() =>
      expect(screen.queryByText(GUIDE_UPDATE_FAILED)).not.toBeInTheDocument(),
    );
    expect(client.groupInputs).toHaveLength(1);
    expect(client.guideInputs).toHaveLength(1);
  });

  it("retries every failing guide query without refetching status", async () => {
    let groupAttempts = 0;
    let guideAttempts = 0;
    const client = new FakeSparrowClient({
      groups: async () => {
        groupAttempts += 1;
        return groupAttempts === 1
          ? failure({ _tag: "service-unavailable" })
          : success(GROUPS_PAGE);
      },
      guide: async (input) => {
        guideAttempts += 1;
        return guideAttempts === 1
          ? failure({ _tag: "service-unavailable" })
          : success(defaultGuidePage(input));
      },
    });
    const user = userEvent.setup();
    renderInstalledBrowser(client);

    expect(
      await screen.findByRole("button", { name: "Try again" }),
    ).toBeVisible();
    expect(client.statusInputs).toHaveLength(1);
    expect(client.groupInputs).toHaveLength(1);
    expect(client.guideInputs).toHaveLength(1);

    await user.click(screen.getByRole("button", { name: "Try again" }));

    expect(
      await screen.findByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    expect(client.statusInputs).toHaveLength(1);
    expect(client.groupInputs).toHaveLength(2);
    expect(client.guideInputs).toHaveLength(2);
  });

  it("appends a guide continuation with its correlated cursor history", async () => {
    const client = new FakeSparrowClient({
      guide: async (input) =>
        success(
          input.cursor === undefined
            ? guidePage(input, {
                rows: [guideRow(WORLD_NEWS, input, "Live Bulletin")],
                next: "guide-next",
              })
            : guidePage(input, {
                rows: [guideRow(CINEMA_ONE, input, "Feature Presentation")],
              }),
        ),
    });
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(
      await screen.findByRole("button", { name: "More channels" }),
    );

    expect(
      await screen.findByRole("button", { name: "Tune Cinema One" }),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    const continuation = requireMatch(
      client.guideInputs,
      (input) => input.cursor !== undefined,
      "expected a guide continuation",
    );
    expect(continuation.cursor).toBe("guide-next");
    expect(continuation.previousCursors).toEqual([]);
  });

  it("surfaces a failed group continuation and resumes pagination on retry", async () => {
    let continuationAttempts = 0;
    const client = new FakeSparrowClient({
      groups: async (input) => {
        if (input.cursor === undefined) {
          return success(CONTINUING_GROUPS_PAGE);
        }
        continuationAttempts += 1;
        return continuationAttempts === 1
          ? failure({ _tag: "service-unavailable" })
          : success(
              clientSchemas.groupsPageFor(input).parse({
                generation: 7,
                items: [{ name: "Recovered", channelCount: 1 }],
                next: null,
              }),
            );
      },
    });
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await waitFor(() => expect(client.groupInputs).toHaveLength(2));
    await act(async () => Promise.resolve());

    expect(client.groupInputs).toHaveLength(2);
    expect(client.groupInputs[1]).toMatchObject({
      cursor: "groups-next",
      previousCursors: [],
    });
    expect(screen.getByText(GUIDE_UPDATE_FAILED)).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Try again" }));

    expect(
      await screen.findByRole("radio", { name: /Recovered/ }),
    ).toBeVisible();
    await waitFor(() => expect(client.groupInputs).toHaveLength(4));
    expect(client.groupInputs[2]?.cursor).toBeUndefined();
    expect(client.groupInputs[3]).toMatchObject({
      cursor: "groups-next",
      previousCursors: [],
    });
    expect(client.guideInputs).toHaveLength(1);
    expect(screen.queryByText(GUIDE_UPDATE_FAILED)).not.toBeInTheDocument();
  });

  it("rejects rows from a mismatched continuation generation", async () => {
    const client = new FakeSparrowClient({
      guide: async (input) =>
        success(
          input.cursor === undefined
            ? guidePage(input, {
                rows: [guideRow(WORLD_NEWS, input, "Live Bulletin")],
                next: "guide-next",
              })
            : guidePage(input, {
                generation: 8,
                rows: [guideRow(CINEMA_ONE, input, "Replacement Feature")],
              }),
        ),
    });
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(
      await screen.findByRole("button", { name: "More channels" }),
    );
    await waitFor(() => expect(client.guideInputs).toHaveLength(2));

    expect(
      screen.getByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Tune Cinema One" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Replacement Feature")).not.toBeInTheDocument();
    expect(await screen.findByText(GUIDE_UPDATE_FAILED)).toBeVisible();
    expect(screen.getByRole("button", { name: "More channels" })).toBeEnabled();
  });

  it("Agent Control stop clears tune intent before the player commits", async () => {
    const client = new FakeSparrowClient({ search: async () => success(clientSchemas.searchResults.parse({
      generation: 7,
      channels: { generation: 7, items: [WORLD_NEWS], next: null },
      programmes: { generation: 7, items: [], next: null },
    })) });
    renderInstalledBrowser(client);
    await screen.findByRole("button", { name: "Tune World News" });
    const { dispatchAgentControl } = await import("../agent-control/agent-control-binding");
    await act(async () => {
      expect(await dispatchAgentControl({ _tag: "tune", term: "World News" })).toEqual({ ok: true, result: { _tag: "tuned", name: "World News" } });
      expect(await dispatchAgentControl({ _tag: "stop" })).toEqual({ ok: true, result: { _tag: "stopped" } });
    });
    expect(client.installedSessionCount).toBe(0);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Pick a channel");
    // The tune test below pins both strings while a Channel plays.
    expect(screen.queryByText("Loading the player…")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Stop stream" })).not.toBeInTheDocument();
  });

  it("tunes a Channel through Agent Control", async () => {
    const client = new FakeSparrowClient({
      search: async () =>
        success(
          clientSchemas.searchResults.parse({
            generation: 7,
            channels: {
              generation: 7,
              items: [WORLD_NEWS],
              next: null,
            },
            programmes: { generation: 7, items: [], next: null },
          }),
        ),
    });
    renderInstalledBrowser(client);
    await screen.findByRole("button", { name: "Tune World News" });

    const { dispatchAgentControl } = await import(
      "../agent-control/agent-control-binding"
    );
    await expect(
      dispatchAgentControl({ _tag: "tune", term: "World News" }),
    ).resolves.toEqual({
      ok: true,
      result: { _tag: "tuned", name: "World News" },
    });
    expect(
      await screen.findByRole("heading", { level: 2, name: "World News" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Stop stream" })).toBeVisible();
  });
});

describe("CatalogBrowser Theater layout", () => {
  it("opens on the guide, shows the full picture once a Channel is tuned, and returns after Stop", async () => {
    stubViewport(true);
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await screen.findByRole("button", { name: "Tune World News" });
    const shell = requireShell();
    expect(shell).toHaveAttribute("data-layout", "theater");
    expect(shell).toHaveAttribute("data-mode", "guide");
    expect(shell).toHaveAttribute("data-chrome", "shown");
    // With nothing to watch the guide cannot be closed.
    const guideToggle = screen.getByRole("button", { name: "Guide" });
    expect(guideToggle).toBeDisabled();
    expect(guideToggle).toHaveAttribute("aria-pressed", "true");
    expect(
      screen.getByText("Choose a programme below, or press / to search."),
    ).toBeVisible();
    const search = screen.getByRole("combobox", {
      name: "Search channels and programmes",
    });
    expect(requireMasthead()).toContainElement(search);
    expect(screen.getByLabelText("Programme guide")).not.toContainElement(
      search,
    );

    await user.click(screen.getByRole("button", { name: "Tune World News" }));

    expect(shell).toHaveAttribute("data-mode", "watch");
    expect(guideToggle).toBeEnabled();
    expect(guideToggle).toHaveAttribute("aria-pressed", "false");
    const controls = await screen.findByRole("group", {
      name: "Playback controls",
    });
    await waitFor(() => expect(requireControlsSlot()).toContainElement(controls));
    expect(controls).toHaveAttribute("data-variant", "compact");
    expect(
      screen.getByRole("region", { name: "World News" }),
    ).not.toContainElement(controls);
    await waitFor(() =>
      expect(
        neighbourhoodInputs(client).map(({ channelLimit }) => channelLimit),
      ).toEqual([61]),
    );

    await user.click(
      within(controls).getByRole("button", { name: "Stop stream" }),
    );

    expect(shell).toHaveAttribute("data-mode", "guide");
    expect(guideToggle).toBeDisabled();
    expect(screen.getByText("Nothing playing")).toBeVisible();
  });

  it("opens and closes the guide over a playing Channel, and tuning from it returns to the picture", async () => {
    stubViewport(true);
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    const shell = requireShell();
    const video = await screen.findByLabelText("World News live video");
    const guideToggle = screen.getByRole("button", { name: "Guide" });

    await user.click(guideToggle);
    expect(shell).toHaveAttribute("data-mode", "guide");
    expect(guideToggle).toHaveAttribute("aria-pressed", "true");

    await user.click(guideToggle);
    expect(shell).toHaveAttribute("data-mode", "watch");
    expect(guideToggle).toHaveAttribute("aria-pressed", "false");
    // Docking is the shell's own business: the player never noticed.
    expect(screen.getByLabelText("World News live video")).toBe(video);
    expect(client.playbackInputs).toHaveLength(1);

    await user.click(guideToggle);
    await user.click(screen.getByRole("button", { name: "Tune Cinema One" }));
    expect(shell).toHaveAttribute("data-mode", "watch");
    expect(
      await screen.findByLabelText("Cinema One live video"),
    ).toBeInTheDocument();
  });

  it("makes the whole window fullscreen, so the chrome stays over the picture", async () => {
    stubViewport(true);
    const user = userEvent.setup();
    renderHostedBrowser(new FakeSparrowClient());
    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    const fullScreen = await screen.findByRole("button", {
      name: "Full screen",
    });

    const root = stubRequestFullscreen(document.documentElement);
    try {
      await user.click(fullScreen);
      expect(root.request).toHaveBeenCalledTimes(1);
    } finally {
      root.restore();
    }
  });

  it("keeps the picture playing when the window grows into the Theater layout", async () => {
    const viewport = stubViewport(false);
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    const shell = requireShell();
    const video = await screen.findByLabelText("World News live video");
    const controls = screen.getByRole("group", { name: "Playback controls" });
    expect(shell).toHaveAttribute("data-layout", "pocket");
    await waitFor(() => expect(requireControlsSlot()).toContainElement(controls));
    expect(controls).toHaveAttribute("data-variant", "compact");
    expect(screen.getByLabelText("Programme guide")).toContainElement(
      screen.getByRole("combobox", { name: "Search channels and programmes" }),
    );
    // Pocket's way to the guide is in its channel bar, not the masthead.
    expect(requireMasthead()).not.toContainElement(
      screen.getByRole("button", { name: "Guide" }),
    );

    act(() => viewport.resize(true));

    expect(shell).toHaveAttribute("data-layout", "theater");
    expect(shell).toHaveAttribute("data-mode", "watch");
    expect(screen.getByLabelText("World News live video")).toBe(video);
    expect(client.playbackInputs).toHaveLength(1);
    // Both layouts keep the controls in the info block: they never remount.
    expect(screen.getByRole("group", { name: "Playback controls" })).toBe(
      controls,
    );
    expect(requireControlsSlot()).toContainElement(controls);
    expect(requireMasthead()).toContainElement(
      screen.getByRole("combobox", { name: "Search channels and programmes" }),
    );
    expect(requireMasthead()).toContainElement(
      screen.getByRole("button", { name: "Guide" }),
    );
  });

  it("stays pocket where the picture may not be covered: controls under it, and inside the player while that is fullscreen", async () => {
    stubViewport(true);
    const client = new FakeSparrowClient({ pictureOverlay: false });
    const user = userEvent.setup();
    renderInstalledBrowser(client);

    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );

    // By the time the player shows its controls the device has answered.
    const video = await screen.findByLabelText("World News live video");
    const player = screen.getByRole("region", { name: "World News" });
    const controls = () =>
      screen.getByRole("group", { name: "Playback controls" });
    expect(requireShell()).toHaveAttribute("data-layout", "pocket");
    // Under the picture, clear of it: icon controls in the info block, with
    // a More menu that opens away from the picture.
    await waitFor(() =>
      expect(requireControlsSlot()).toContainElement(controls()),
    );
    expect(controls()).toHaveAttribute("data-variant", "compact");
    expect(
      screen.queryByRole("button", { name: "Copy diagnostics" }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "More" }));
    expect(await screen.findByRole("menu")).toHaveAttribute(
      "data-side",
      "bottom",
    );
    expect(
      screen.getAllByRole("menuitem").map((item) => item.textContent),
    ).toEqual(["Restart", "Copy diagnostics"]);
    await user.keyboard("{Escape}");

    // Full screen takes the player alone, so its controls move inside it as
    // the bar, with the More menu's actions as plain buttons.
    const root = stubRequestFullscreen(document.documentElement);
    const section = stubRequestFullscreen(player, { enters: true });
    try {
      await user.click(screen.getByRole("button", { name: "Full screen" }));
      await waitFor(() => expect(section.request).toHaveBeenCalledTimes(1));
      expect(root.request).not.toHaveBeenCalled();
      await waitFor(() =>
        expect(controls()).toHaveAttribute("data-variant", "bar"),
      );
      expect(player).toContainElement(controls());
      expect(requireControlsSlot()).toBeEmptyDOMElement();
      expect(
        within(controls()).getByRole("button", { name: "Restart" }),
      ).toBeInTheDocument();
      expect(
        within(controls()).getByRole("button", { name: "Copy diagnostics" }),
      ).toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: "More" }),
      ).not.toBeInTheDocument();
      expect(requireShell()).toHaveAttribute("data-layout", "pocket");
      expect(screen.getByLabelText("World News live video")).toBe(video);

      act(() => section.exit());

      expect(controls()).toHaveAttribute("data-variant", "compact");
      expect(requireControlsSlot()).toContainElement(controls());
      expect(screen.getByLabelText("World News live video")).toBe(video);
      expect(client.installedSessionCount).toBe(1);
    } finally {
      root.restore();
      section.restore();
    }
  });

  it("moves an installed device that allows it into the Theater layout", async () => {
    stubViewport(true);
    renderInstalledBrowser(new FakeSparrowClient());

    await screen.findByRole("button", { name: "Tune World News" });

    await waitFor(() =>
      expect(requireShell()).toHaveAttribute("data-layout", "theater"),
    );
  });

  it("keeps the guide open while the picture plays in the mpv window", async () => {
    stubViewport(true);
    const client = new FakeSparrowClient({ transport: { _tag: "linux-mpv" } });
    const user = userEvent.setup();
    renderInstalledBrowser(client);
    await waitFor(() =>
      expect(requireShell()).toHaveAttribute("data-layout", "theater"),
    );

    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );

    expect(await screen.findByText("Playing in mpv")).toBeVisible();
    expect(requireShell()).toHaveAttribute("data-mode", "guide");
    expect(screen.getByRole("button", { name: "Guide" })).toBeDisabled();
  });

  it("opens and closes the guide with G, returns to the picture with Esc, and goes to search with /", async () => {
    const user = userEvent.setup();
    const { shell } = await watchWorldNews(
      renderHostedBrowser,
      new FakeSparrowClient(),
      user,
    );
    const search = screen.getByRole("combobox", {
      name: "Search channels and programmes",
    });

    await user.keyboard("g");
    expect(shell).toHaveAttribute("data-mode", "guide");
    await user.keyboard("g");
    expect(shell).toHaveAttribute("data-mode", "watch");
    await user.keyboard("g");
    await user.keyboard("{Escape}");
    expect(shell).toHaveAttribute("data-mode", "watch");

    await user.keyboard("/");
    expect(search).toHaveFocus();
    expect(search).toHaveValue("");
    // Esc in the empty field gives the keys back to the picture.
    await user.keyboard("{Escape}");
    expect(requireMonitor()).toHaveFocus();

    // A letter typed into the field is a search, not a key of the stage.
    await user.keyboard("/");
    await user.keyboard("g");
    expect(search).toHaveValue("g");
    expect(shell).toHaveAttribute("data-mode", "watch");
  });

  it("keeps the guide open on G and Esc while nothing is playing", async () => {
    stubViewport(true);
    const user = userEvent.setup();
    renderHostedBrowser(new FakeSparrowClient());
    await screen.findByRole("button", { name: "Tune World News" });

    await user.keyboard("g");
    await user.keyboard("{Escape}");

    expect(requireShell()).toHaveAttribute("data-mode", "guide");
  });

  it("offers the nearby Channels over the full picture only, and tunes the one chosen", async () => {
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    const { shell } = await watchWorldNews(renderHostedBrowser, client, user);

    const rail = screen.getByRole("navigation", { name: "Nearby channels" });
    expect(
      within(rail)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual([
      `${WORLD_NEWS.number}World NewsLive Bulletin`,
      `${CINEMA_ONE.number}Cinema OneFeature Presentation`,
    ]);
    expect(within(rail).getByRole("button", { current: true })).toHaveTextContent(
      "World News",
    );
    // Only the guide's own buttons are acceptance-marked Channels.
    expect(rail.querySelector("[data-acceptance-channel]")).toBeNull();
    expect(screen.getByText("Change channel")).toBeInTheDocument();

    await user.keyboard("g");
    expect(
      screen.queryByRole("navigation", { name: "Nearby channels" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Change channel")).not.toBeInTheDocument();

    await user.keyboard("g");
    await user.click(
      within(
        screen.getByRole("navigation", { name: "Nearby channels" }),
      ).getByRole("button", { name: /Cinema One/u }),
    );

    expect(
      await screen.findByLabelText("Cinema One live video"),
    ).toBeInTheDocument();
    expect(shell).toHaveAttribute("data-mode", "watch");
    expect(
      within(
        screen.getByRole("navigation", { name: "Nearby channels" }),
      ).getByRole("button", { current: true }),
    ).toHaveTextContent("Cinema One");
  });

  it("has no row of nearby Channels and no stage keys in the pocket layout", async () => {
    const user = userEvent.setup();
    renderHostedBrowser(new FakeSparrowClient());
    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    await screen.findByLabelText("World News live video");
    await waitFor(() =>
      expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
        "Live Bulletin",
      ),
    );

    // Pocket changes Channel from its bar, which is not that landmark.
    expect(screen.getByRole("group", { name: "Channels" })).toBeInTheDocument();
    expect(
      screen.queryByRole("navigation", { name: "Nearby channels" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Change channel")).not.toBeInTheDocument();
    // The stage keys belong to the Theater layout.
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      "Live Bulletin",
    );
  });

  it("shows the next Channel at once on an arrow press and tunes it when no other press follows", async () => {
    const client = new FakeSparrowClient();
    const { shell, heading } = await watchWorldNews(
      renderHostedBrowser,
      client,
    );
    const rail = screen.getByRole("navigation", { name: "Nearby channels" });
    expect(heading).toHaveTextContent("Live Bulletin");

    fireEvent.keyDown(document.body, { key: "ArrowDown" });

    // The info block and the rail move at once; the player has not yet.
    expect(heading).toHaveTextContent("Feature Presentation");
    expect(within(rail).getByRole("button", { current: true })).toHaveTextContent(
      "Cinema One",
    );
    expect(screen.getByLabelText("World News live video")).toBeInTheDocument();
    expect(client.playbackInputs).toHaveLength(1);

    expect(
      await screen.findByLabelText("Cinema One live video"),
    ).toBeInTheDocument();
    expect(client.playbackInputs.map(({ id }) => id)).toEqual([
      WORLD_NEWS.id,
      CINEMA_ONE.id,
    ]);
    expect(heading).toHaveTextContent("Feature Presentation");
    expect(shell).toHaveAttribute("data-mode", "watch");
  });

  it("tunes nothing when the arrows end on the playing Channel or run past the list", async () => {
    const client = new FakeSparrowClient();
    const { heading } = await watchWorldNews(renderHostedBrowser, client);
    vi.useFakeTimers();

    fireEvent.keyDown(document.body, { key: "ArrowDown" });
    expect(heading).toHaveTextContent("Feature Presentation");
    // Past the last Channel there is nowhere to go; the target stays.
    fireEvent.keyDown(document.body, { key: "ArrowDown" });
    expect(heading).toHaveTextContent("Feature Presentation");
    fireEvent.keyDown(document.body, { key: "ArrowUp" });
    expect(heading).toHaveTextContent("Live Bulletin");
    fireEvent.keyDown(document.body, { key: "ArrowUp" });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });

    expect(heading).toHaveTextContent("Live Bulletin");
    expect(screen.getByLabelText("World News live video")).toBeInTheDocument();
    expect(client.playbackInputs).toHaveLength(1);
  });

  it("tunes a zap target 350 ms after the last arrow press, and changes nothing but the Channel", async () => {
    const client = new FakeSparrowClient();
    const { shell } = await watchWorldNews(renderHostedBrowser, client);
    vi.useFakeTimers();
    const wait = (milliseconds: number) =>
      act(async () => {
        await vi.advanceTimersByTimeAsync(milliseconds);
      });

    fireEvent.keyDown(document.body, { key: "ArrowDown" });
    await wait(300);
    // Further presses start the wait again.
    fireEvent.keyDown(document.body, { key: "ArrowUp" });
    fireEvent.keyDown(document.body, { key: "ArrowDown" });
    // Opening the guide meanwhile is the viewer's choice; the zap keeps it.
    fireEvent.keyDown(document.body, { key: "g" });
    await wait(349);
    expect(screen.getByLabelText("World News live video")).toBeInTheDocument();
    expect(client.playbackInputs).toHaveLength(1);

    await wait(1);

    expect(screen.getByLabelText("Cinema One live video")).toBeInTheDocument();
    expect(shell).toHaveAttribute("data-mode", "guide");
  });

  it("keeps the Channel the viewer chooses while a zap is pending", async () => {
    const client = new FakeSparrowClient();
    const { heading } = await watchWorldNews(renderHostedBrowser, client);
    vi.useFakeTimers();

    fireEvent.keyDown(document.body, { key: "ArrowDown" });
    expect(heading).toHaveTextContent("Feature Presentation");
    fireEvent.click(
      within(
        screen.getByRole("navigation", { name: "Nearby channels" }),
      ).getByRole("button", { name: /World News/u }),
    );
    expect(heading).toHaveTextContent("Live Bulletin");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });

    expect(heading).toHaveTextContent("Live Bulletin");
    expect(screen.getByLabelText("World News live video")).toBeInTheDocument();
    expect(client.playbackInputs).toHaveLength(1);
  });

  it("starts nothing when Stop is pressed while a zap is pending", async () => {
    const client = new FakeSparrowClient();
    const { shell, heading } = await watchWorldNews(
      renderHostedBrowser,
      client,
    );
    vi.useFakeTimers();

    fireEvent.keyDown(document.body, { key: "ArrowDown" });
    expect(heading).toHaveTextContent("Feature Presentation");
    fireEvent.click(screen.getByRole("button", { name: "Stop stream" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });

    expect(heading).toHaveTextContent("Pick a channel");
    expect(shell).toHaveAttribute("data-mode", "guide");
    expect(screen.getByText("Nothing playing")).toBeVisible();
    expect(client.playbackInputs).toHaveLength(1);
  });

  it("starts no Playback Session when Agent Control stops while a zap is pending", async () => {
    const client = new FakeSparrowClient();
    const { heading } = await watchWorldNews(renderInstalledBrowser, client);
    expect(client.installedSessionCount).toBe(1);
    const { dispatchAgentControl } = await import(
      "../agent-control/agent-control-binding"
    );
    vi.useFakeTimers();

    fireEvent.keyDown(document.body, { key: "ArrowDown" });
    expect(heading).toHaveTextContent("Feature Presentation");
    await act(async () => {
      expect(await dispatchAgentControl({ _tag: "stop" })).toEqual({
        ok: true,
        result: { _tag: "stopped" },
      });
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });

    expect(client.installedSessionCount).toBe(1);
    expect(heading).toHaveTextContent("Pick a channel");
    expect(screen.getByText("Nothing playing")).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Stop stream" }),
    ).not.toBeInTheDocument();
  });

  it("hides the chrome over a playing picture after three idle seconds, and never while the guide is open", async () => {
    // The first tune loads the player from disk, which fake timers cannot hurry.
    await import("../playback/hosted-player");
    vi.useFakeTimers();
    stubViewport(true);
    renderHostedBrowser(new FakeSparrowClient());
    const settle = (milliseconds: number) =>
      act(async () => {
        await vi.advanceTimersByTimeAsync(milliseconds);
      });
    await settle(0);
    const shell = requireShell();

    await settle(5_000);
    expect(shell).toHaveAttribute("data-chrome", "shown");

    fireEvent.click(screen.getByRole("button", { name: "Tune World News" }));
    // Tuning moves focus to the picture a frame later; that is the last input.
    await settle(100);
    expect(requireMonitor()).toHaveFocus();
    expect(shell).toHaveAttribute("data-chrome", "shown");
    await settle(2_800);
    expect(shell).toHaveAttribute("data-chrome", "shown");
    await settle(200);
    expect(shell).toHaveAttribute("data-chrome", "hidden");

    fireEvent.pointerMove(document.body, { screenX: 300, screenY: 200 });
    expect(shell).toHaveAttribute("data-chrome", "shown");
    await settle(3_000);
    expect(shell).toHaveAttribute("data-chrome", "hidden");

    // G is input too: it brings the chrome back and opens the guide.
    fireEvent.keyDown(document.body, { key: "g" });
    expect(shell).toHaveAttribute("data-mode", "guide");
    await settle(10_000);
    expect(shell).toHaveAttribute("data-chrome", "shown");
  });
});

describe("CatalogBrowser pocket layout", () => {
  it("has no picture until a Channel is tuned, docks it for the guide, and returns by the band, the picture or a tune", async () => {
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await screen.findByRole("button", { name: "Tune World News" });
    const shell = requireShell();
    expect(shell).toHaveAttribute("data-layout", "pocket");
    expect(shell).toHaveAttribute("data-mode", "guide");
    expect(shell).toHaveAttribute("data-playing", "false");
    expect(shell).toHaveAttribute("data-external", "false");
    expect(
      screen.queryByRole("group", { name: "Channels" }),
    ).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Tune World News" }));

    expect(shell).toHaveAttribute("data-mode", "watch");
    expect(shell).toHaveAttribute("data-playing", "true");
    expect(shell).toHaveAttribute("data-dock", "false");
    const video = await screen.findByLabelText("World News live video");
    // Watch mode hides the guide with CSS alone: the acceptance scripts still
    // find its search field, its groups and every Channel button.
    const guide = screen.getByLabelText("Programme guide");
    expect(guide).not.toHaveAttribute("hidden");
    expect(guide).toContainElement(
      screen.getByRole("combobox", { name: "Search channels and programmes" }),
    );
    expect(guide.querySelectorAll("[data-acceptance-group]")).not.toHaveLength(0);
    expect(acceptanceChannels()).toEqual([
      ["Tune World News", "true"],
      ["Tune Cinema One", "false"],
    ]);

    const bar = within(screen.getByRole("group", { name: "Channels" }));
    await user.click(bar.getByRole("button", { name: "Guide" }));
    expect(shell).toHaveAttribute("data-mode", "guide");
    expect(shell).toHaveAttribute("data-dock", "true");
    // Guide mode hides the controls the same way.
    expect(requireControlsSlot()).toContainElement(
      screen.getByRole("group", { name: "Playback controls" }),
    );

    await user.click(screen.getByRole("button", { name: "Back to the picture" }));
    expect(shell).toHaveAttribute("data-mode", "watch");
    expect(shell).toHaveAttribute("data-dock", "false");

    await user.click(bar.getByRole("button", { name: "Guide" }));
    await user.click(video);
    expect(shell).toHaveAttribute("data-mode", "watch");
    expect(shell).toHaveAttribute("data-dock", "false");
    // Docking is the shell's own business: the player never noticed.
    expect(screen.getByLabelText("World News live video")).toBe(video);
    expect(client.playbackInputs).toHaveLength(1);

    await user.click(bar.getByRole("button", { name: "Guide" }));
    await user.click(screen.getByRole("button", { name: "Tune Cinema One" }));
    expect(shell).toHaveAttribute("data-mode", "watch");
    expect(shell).toHaveAttribute("data-dock", "false");
    expect(
      await screen.findByLabelText("Cinema One live video"),
    ).toBeInTheDocument();
  });

  it("opens the guide from the masthead's search button, ready to type in", async () => {
    const user = userEvent.setup();
    renderHostedBrowser(new FakeSparrowClient());
    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    await screen.findByLabelText("World News live video");
    // Tuning rests focus on the picture a frame later.
    await waitFor(() => expect(requireMonitor()).toHaveFocus());

    await user.click(
      within(requireMasthead()).getByRole("button", { name: "Search" }),
    );

    expect(requireShell()).toHaveAttribute("data-mode", "guide");
    await waitFor(() =>
      expect(
        screen.getByRole("combobox", { name: "Search channels and programmes" }),
      ).toHaveFocus(),
    );
  });

  it("takes a watched picture fullscreen when the phone is turned on its side", async () => {
    const client = new FakeSparrowClient({ pictureOverlay: false });
    const user = userEvent.setup();
    const orientation = Object.assign(new EventTarget(), {
      type: "portrait-primary",
    });
    Object.defineProperty(window.screen, "orientation", {
      configurable: true,
      value: orientation,
    });
    const turn = (type: OrientationType) => {
      orientation.type = type;
      act(() => {
        orientation.dispatchEvent(new Event("change"));
      });
    };
    // A phone: a small window, held in the hand.
    vi.stubGlobal("matchMedia", (media: string) => ({
      matches: media === "(pointer: coarse)",
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    }));
    renderInstalledBrowser(client);
    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    const video = await screen.findByLabelText("World News live video");
    const bar = within(await screen.findByRole("group", { name: "Channels" }));
    await screen.findByRole("button", { name: "Pause" });
    const player = video.closest<HTMLElement>(".hosted-player");
    if (player === null) {
      throw new Error("expected the player section");
    }
    const section = stubRequestFullscreen(player, { enters: true });
    const exit = vi.fn(async () => section.exit());
    Object.defineProperty(document, "exitFullscreen", {
      configurable: true,
      value: exit,
    });
    try {
      // Browsing the guide, a turn only turns the layout.
      await user.click(bar.getByRole("button", { name: "Guide" }));
      turn("landscape-primary");
      expect(section.request).not.toHaveBeenCalled();
      turn("portrait-primary");

      // Watching, it takes the picture fullscreen, and upright ends that.
      await user.click(screen.getByRole("button", { name: "Back to the picture" }));
      expect(requireShell()).toHaveAttribute("data-mode", "watch");
      turn("landscape-primary");
      expect(section.request).toHaveBeenCalledTimes(1);
      // The player hears of the fullscreen it asked for before the next turn.
      await act(async () => {});
      turn("portrait-primary");
      expect(exit).toHaveBeenCalledTimes(1);

      // A paused picture stays where it is.
      await user.click(screen.getByRole("button", { name: "Pause" }));
      await screen.findByRole("button", { name: "Resume" });
      turn("landscape-primary");
      expect(section.request).toHaveBeenCalledTimes(1);
    } finally {
      section.restore();
      vi.unstubAllGlobals();
      Reflect.deleteProperty(document, "exitFullscreen");
      Reflect.deleteProperty(window.screen, "orientation");
    }
  });

  it("keeps the picture's box while native video cannot follow it", async () => {
    const client = new FakeSparrowClient({ pictureOverlay: false });
    const user = userEvent.setup();
    renderInstalledBrowser(client);
    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    const shell = requireShell();
    const video = await screen.findByLabelText("World News live video");
    const stage = within(requireStage(screen.getByRole("heading", { level: 1 })));
    const bar = within(await screen.findByRole("group", { name: "Channels" }));
    await screen.findByRole("button", { name: "Pause" });
    expect(shell).toHaveAttribute("data-dock", "false");

    // Paused, the native picture stays where it is: opening the guide must
    // leave its box full width.
    await user.click(screen.getByRole("button", { name: "Pause" }));
    await screen.findByRole("button", { name: "Resume" });
    expect(stage.getByText("Paused", { selector: ".now-playing__state" })).toBeInTheDocument();
    await user.click(bar.getByRole("button", { name: "Guide" }));
    expect(shell).toHaveAttribute("data-mode", "guide");
    expect(shell).toHaveAttribute("data-dock", "false");

    // Live again, the picture follows its box into the band. Resume is a
    // control of the info block: pressing it is not a tap on the picture.
    await user.click(screen.getByRole("button", { name: "Resume" }));
    await screen.findByRole("button", { name: "Pause" });
    expect(shell).toHaveAttribute("data-mode", "guide");
    expect(shell).toHaveAttribute("data-dock", "true");
    // The fixture's transport carries no Audio Track, which is all the state
    // line has left to say of a live picture.
    expect(
      stage.queryByText("Paused", { selector: ".now-playing__state" }),
    ).not.toBeInTheDocument();
    expect(
      stage.getByText("No sound", { selector: ".now-playing__state" }),
    ).toHaveAttribute("data-state", "silent");

    // And the other way: paused in the band, it stays band-sized in watch mode.
    await user.click(screen.getByRole("button", { name: "Pause" }));
    await screen.findByRole("button", { name: "Resume" });
    await user.click(screen.getByRole("button", { name: "Back to the picture" }));
    expect(shell).toHaveAttribute("data-mode", "watch");
    expect(shell).toHaveAttribute("data-dock", "true");
    await user.click(screen.getByRole("button", { name: "Resume" }));
    await screen.findByRole("button", { name: "Pause" });
    expect(shell).toHaveAttribute("data-dock", "false");

    expect(screen.getByLabelText("World News live video")).toBe(video);
    expect(client.installedSessionCount).toBe(1);
  });

  it("holds the picture's box at its size while native video cannot follow it, and lets go when it can", async () => {
    const client = new FakeSparrowClient({ pictureOverlay: false });
    const user = userEvent.setup();
    renderInstalledBrowser(client);
    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    const shell = requireShell();
    const video = await screen.findByLabelText("World News live video");
    const bar = within(await screen.findByRole("group", { name: "Channels" }));
    await screen.findByRole("button", { name: "Pause" });
    layOutMonitor(360, 202.5);
    // Live, the picture follows its box: nothing is held.
    expect(pinnedPictureSize()).toBeNull();

    await user.click(screen.getByRole("button", { name: "Pause" }));
    await screen.findByRole("button", { name: "Resume" });
    expect(pinnedPictureSize()).toEqual({ width: "360px", height: "202.5px" });

    // The soft keyboard opens over the guide: the window is lower, and the
    // page would lay the box out lower with it. The size taken when the
    // picture stopped following is the one that is kept.
    layOutMonitor(360, 156);
    await user.click(bar.getByRole("button", { name: "Guide" }));
    expect(shell).toHaveAttribute("data-dock", "false");
    expect(pinnedPictureSize()).toEqual({ width: "360px", height: "202.5px" });

    // Live again, the box is the layout's to size.
    await user.click(screen.getByRole("button", { name: "Resume" }));
    await screen.findByRole("button", { name: "Pause" });
    expect(shell).toHaveAttribute("data-dock", "true");
    expect(pinnedPictureSize()).toBeNull();

    // Paused in the band, the band's size is held, until the player goes.
    layOutMonitor(150, 84);
    await user.click(screen.getByRole("button", { name: "Pause" }));
    await screen.findByRole("button", { name: "Resume" });
    expect(pinnedPictureSize()).toEqual({ width: "150px", height: "84px" });
    expect(screen.getByLabelText("World News live video")).toBe(video);
    await user.click(screen.getByRole("button", { name: "Stop stream" }));
    await waitFor(() => expect(shell).toHaveAttribute("data-playing", "false"));
    await waitFor(() => expect(pinnedPictureSize()).toBeNull());
  });

  it("docks the picture at once where the page draws it, whatever its state", async () => {
    const user = userEvent.setup();
    renderInstalledBrowser(new FakeSparrowClient());
    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    await screen.findByRole("button", { name: "Pause" });
    layOutMonitor(360, 202.5);
    await user.click(screen.getByRole("button", { name: "Pause" }));
    await screen.findByRole("button", { name: "Resume" });
    // The page moves the picture with its box: there is nothing to hold.
    expect(pinnedPictureSize()).toBeNull();

    await user.click(
      within(screen.getByRole("group", { name: "Channels" })).getByRole(
        "button",
        { name: "Guide" },
      ),
    );

    expect(requireShell()).toHaveAttribute("data-layout", "pocket");
    expect(requireShell()).toHaveAttribute("data-dock", "true");
  });

  it("holds the guide open and marks the shell while mpv has the picture", async () => {
    const user = userEvent.setup();
    renderInstalledBrowser(
      new FakeSparrowClient({ transport: { _tag: "linux-mpv" } }),
    );
    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );

    expect(await screen.findByText("Playing in mpv")).toBeVisible();
    const shell = requireShell();
    expect(shell).toHaveAttribute("data-layout", "pocket");
    expect(shell).toHaveAttribute("data-external", "true");
    expect(shell).toHaveAttribute("data-mode", "guide");
    expect(shell).toHaveAttribute("data-dock", "true");

    // There is no picture in the page to return to.
    await user.click(screen.getByRole("button", { name: "Back to the picture" }));
    expect(shell).toHaveAttribute("data-mode", "guide");
  });

  it("names the Channel on either side in its bar, shows the next one at once and tunes it after the commit delay", async () => {
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderHostedBrowser(client);
    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );
    await screen.findByLabelText("World News live video");
    const shell = requireShell();
    const heading = screen.getByRole("heading", { level: 1 });
    const bar = within(screen.getByRole("group", { name: "Channels" }));
    const next = await bar.findByRole("button", {
      name: `Next channel, ${CINEMA_ONE.number} Cinema One`,
    });
    expect(next).toHaveTextContent("Feature Presentation");
    expect(bar.getByRole("button", { name: "Previous channel" })).toBeDisabled();
    // The Linux probe tunes the first button named "Tune …": none is here.
    expect(
      bar.getAllByRole("button").map((button) => button.getAttribute("aria-label")),
    ).not.toContainEqual(expect.stringMatching(/^Tune /u));

    await user.click(next);

    // The info block and the bar move at once; the player has not yet.
    expect(heading).toHaveTextContent("Feature Presentation");
    expect(
      bar.getByRole("button", {
        name: `Previous channel, ${WORLD_NEWS.number} World News`,
      }),
    ).toHaveTextContent("Live Bulletin");
    expect(bar.getByRole("button", { name: "Next channel" })).toBeDisabled();
    expect(screen.getByLabelText("World News live video")).toBeInTheDocument();
    expect(client.playbackInputs).toHaveLength(1);

    expect(
      await screen.findByLabelText("Cinema One live video"),
    ).toBeInTheDocument();
    expect(client.playbackInputs.map(({ id }) => id)).toEqual([
      WORLD_NEWS.id,
      CINEMA_ONE.id,
    ]);
    expect(shell).toHaveAttribute("data-mode", "watch");
  });

  it("moves focus with the mode, and leaves it on the bar's button through a zap", async () => {
    const surf = surfChannels(5);
    const [, , third, fourth, fifth] = surf;
    const user = userEvent.setup();
    renderHostedBrowser(new FakeSparrowClient({ guide: catalogGuide(surf) }));
    await user.click(
      await screen.findByRole("button", { name: `Tune ${third.name}` }),
    );
    await screen.findByLabelText(`${third.name} live video`);
    await waitFor(() => expect(requireMonitor()).toHaveFocus());
    const shell = requireShell();
    const bar = within(screen.getByRole("group", { name: "Channels" }));

    // The bar leaves with the watch screen: focus goes to the way back.
    bar.getByRole("button", { name: "Guide" }).focus();
    await user.keyboard("{Enter}");
    expect(shell).toHaveAttribute("data-mode", "guide");
    const back = screen.getByRole("button", { name: "Back to the picture" });
    await waitFor(() => expect(back).toHaveFocus());

    // And that button leaves with the guide: focus goes to the picture.
    await user.keyboard("{Enter}");
    expect(shell).toHaveAttribute("data-mode", "watch");
    expect(requireMonitor()).toHaveFocus();

    // A zap from the bar tunes without taking focus off its button.
    const next = await bar.findByRole("button", {
      name: `Next channel, ${fourth.number} ${fourth.name}`,
    });
    next.focus();
    await user.keyboard("{Enter}");
    await screen.findByLabelText(`${fourth.name} live video`);
    await nextFrame();
    expect(next).toHaveFocus();
    expect(next).toHaveAccessibleName(
      `Next channel, ${fifth.number} ${fifth.name}`,
    );
  });

  it("zaps on past the rows first read around the Channel, and past a hidden Channel Group", async () => {
    // Twelve Channels to watch, twenty-four of a hidden group, four more.
    const surf = surfChannels(40, (index) =>
      index >= 12 && index < 36 ? "Hidden" : "Open",
    );
    const playing = surf[11];
    const beyond = surf[36];
    localStorage.setItem(
      BOARD_GROUP_EXCLUSIONS_STORAGE_KEY,
      JSON.stringify({ excluded: ["Hidden"] }),
    );
    const client = new FakeSparrowClient({ guide: catalogGuide(surf) });
    const user = userEvent.setup();
    renderHostedBrowser(client);
    await user.click(
      await screen.findByRole("button", { name: `Tune ${playing.name}` }),
    );
    const bar = within(await screen.findByRole("group", { name: "Channels" }));

    // The ten rows read after the Channel are all hidden. The bar does not
    // end there: it reads on until it finds a Channel to offer.
    const next = await bar.findByRole("button", {
      name: `Next channel, ${beyond.number} ${beyond.name}`,
    });
    expect(
      neighbourhoodInputs(client).map(({ around, channelLimit }) => ({
        around,
        channelLimit,
      })),
    ).toEqual([
      { around: playing.id, channelLimit: 21 },
      { around: surf[21].id, channelLimit: 100 },
    ]);

    await user.click(next);
    expect(
      await screen.findByLabelText(`${beyond.name} live video`),
    ).toBeInTheDocument();
    // And back across the hidden group.
    expect(
      await bar.findByRole("button", {
        name: `Previous channel, ${playing.number} ${playing.name}`,
      }),
    ).toBeEnabled();
  });

  it("keeps the pages the viewer has loaded when the guide window changes", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 1, 20, 10));
    // Forty-five Channels: a full first page and five more.
    const surf = surfChannels(45);
    const last = surf[44];
    const client = new FakeSparrowClient({ guide: catalogGuide(surf) });
    renderHostedBrowser(client);
    await settleRequests();
    const guide = within(screen.getByLabelText("Programme guide"));
    fireEvent.click(guide.getByRole("button", { name: "More channels" }));
    await settleRequests();
    expect(
      guide.getByRole("button", { name: `Tune ${last.name}` }),
    ).toBeInTheDocument();
    expect(guideWindowHours(client)).toEqual([3, 3]);

    // Opening the times asks for a longer window. Both pages are read for it
    // before the rows change, so the second page never leaves the list.
    fireEvent.click(guide.getByRole("button", { name: "Now", expanded: false }));
    expect(
      guide.getByRole("button", { name: `Tune ${last.name}` }),
    ).toBeInTheDocument();
    await settleRequests();

    expect(guideWindowHours(client)).toEqual([3, 3, 8, 8]);
    expect(
      guide.getByRole("button", { name: `Tune ${last.name}` }),
    ).toBeInTheDocument();
    expect(
      guide.queryByRole("button", { name: "More channels" }),
    ).not.toBeInTheDocument();
    expect(acceptanceChannels()).toHaveLength(45);
  });

  it("reads the pages a cached guide window lacks before returning to it", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 1, 20, 10));
    const viewport = stubViewport(false);
    const surf = surfChannels(45);
    const last = surf[44];
    const client = new FakeSparrowClient({ guide: catalogGuide(surf) });
    renderHostedBrowser(client);
    await settleRequests();
    const guide = within(screen.getByLabelText("Programme guide"));
    // The times are opened first and more Channels loaded after: the usual
    // window stays in the cache with its first page alone.
    fireEvent.click(guide.getByRole("button", { name: "Now", expanded: false }));
    await settleRequests();
    fireEvent.click(guide.getByRole("button", { name: "More channels" }));
    await settleRequests();
    expect(guideWindowHours(client)).toEqual([3, 8, 8]);
    expect(acceptanceChannels()).toHaveLength(45);

    // Theater reads the usual window. What the cache holds of it is a page
    // short, so the second page is read before the rows change.
    act(() => viewport.resize(true));
    expect(requireShell()).toHaveAttribute("data-layout", "theater");
    expect(
      guide.getByRole("button", { name: `Tune ${last.name}` }),
    ).toBeInTheDocument();
    await settleRequests();

    expect(guideWindowHours(client)).toEqual([3, 8, 8, 3]);
    expect(
      guide.getByRole("button", { name: `Tune ${last.name}` }),
    ).toBeInTheDocument();
    expect(acceptanceChannels()).toHaveLength(45);

    // And back: both windows now hold both pages, and nothing is asked for.
    act(() => viewport.resize(false));
    await settleRequests();
    expect(guideWindowHours(client)).toEqual([3, 8, 8, 3]);
    expect(acceptanceChannels()).toHaveLength(45);
  });

  it("reads a longer guide window once the times are opened, and shows a chosen time without another read", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 1, 20, 10));
    const client = new FakeSparrowClient();
    renderHostedBrowser(client);
    await settleRequests();
    const guide = within(screen.getByLabelText("Programme guide"));
    // Until the viewer asks for later times the guide reads what Theater's
    // timeline shows.
    expect(guideWindowHours(client)).toEqual([3]);
    expect(listRowLines("World News")).toEqual([
      "World News",
      "Live Bulletin",
      "21:00 Future Bulletin",
      "50 min left",
    ]);

    fireEvent.click(guide.getByRole("button", { name: "Now", expanded: false }));
    await settleRequests();

    // One longer read covers every time on offer.
    expect(guideWindowHours(client)).toEqual([3, 8]);
    const times = within(guide.getByRole("group", { name: "Time" }));
    expect(
      times.getAllByRole("button").map((button) => button.textContent),
    ).toEqual(["Now", "21:00", "22:00", "23:00", "00:00", "01:00", "02:00"]);

    fireEvent.click(times.getByRole("button", { name: "21:00" }));
    expect(listRowLines("World News")).toEqual([
      "World News",
      "Future Bulletin",
      "21:00 to 22:00",
    ]);
    expect(listRowLines("Cinema One")).toEqual([
      "Cinema One",
      "Feature Presentation",
      "20:00 to 23:00",
    ]);
    fireEvent.click(times.getByRole("button", { name: "22:00" }));
    expect(listRowLines("World News")).toEqual([
      "World News",
      "Nothing listed at 22:00",
    ]);

    // The chip keeps the chosen time while the row is closed, and opening the
    // row again asks for nothing: the window stays wide.
    const chip = guide.getByRole("button", { name: "22:00", expanded: true });
    expect(chip).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(chip);
    expect(
      guide.queryByRole("group", { name: "Time" }),
    ).not.toBeInTheDocument();
    expect(chip).toHaveTextContent("22:00");
    fireEvent.click(chip);
    await settleRequests();
    expect(guide.getByRole("group", { name: "Time" })).toBeInTheDocument();
    expect(guideWindowHours(client)).toEqual([3, 8]);

    // A row still tunes its Channel now, whatever time the list shows.
    fireEvent.click(guide.getByRole("button", { name: "Tune World News" }));
    await settleRequests();
    expect(client.playbackInputs.map(({ id }) => id)).toEqual([WORLD_NEWS.id]);
  });

  it("returns to now once the chosen time has passed", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 1, 20, 59, 20));
    renderHostedBrowser(new FakeSparrowClient());
    await settleRequests();
    const guide = within(screen.getByLabelText("Programme guide"));
    fireEvent.click(guide.getByRole("button", { name: "Now", expanded: false }));
    await settleRequests();
    const times = within(guide.getByRole("group", { name: "Time" }));

    fireEvent.click(times.getByRole("button", { name: "21:00" }));
    expect(
      guide.getByRole("button", { name: "21:00", expanded: true }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(listRowLines("World News")).toEqual([
      "World News",
      "Live Bulletin",
      "20:30 to 21:30",
    ]);

    // Two ticks of the guide clock: it is past nine, and the next window.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    await settleRequests();

    expect(
      guide.getByRole("button", { name: "Now", expanded: true }),
    ).toHaveAttribute("aria-pressed", "false");
    expect(times.getByRole("button", { name: "Now" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(
      times.queryByRole("button", { name: "21:00" }),
    ).not.toBeInTheDocument();
    expect(listRowLines("World News")).toEqual([
      "World News",
      "Live Bulletin",
      "22:00 Future Bulletin",
      "60 min left",
    ]);
  });
});

/** Waits until the frame callbacks already asked for have run. */
function nextFrame(): Promise<void> {
  return act(
    () =>
      new Promise<void>((resolve) => {
        requestAnimationFrame(() => resolve());
      }),
  );
}

/** Lets the reads a fake-timer test has started answer and render. */
function settleRequests(): Promise<void> {
  return act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}

/** The spans, in hours, of the guide reads from the top of the catalog. */
function guideWindowHours(client: FakeSparrowClient): readonly number[] {
  return client.guideInputs
    .filter((input) => input.around === undefined)
    .map(
      (input) =>
        (Date.parse(input.endsAt) - Date.parse(input.startsAt)) / 3_600_000,
    );
}

/** The lines of a Channel's row in pocket's list, in the order they are read. */
function listRowLines(channel: string): readonly (string | null)[] {
  return Array.from(
    screen.getByRole("button", { name: `Tune ${channel}` }).children,
    (line) => line.textContent,
  ).filter((line) => line !== "");
}

/**
 * Tunes World News in the Theater layout and waits until it plays with the
 * Channels around it on offer.
 */
async function watchWorldNews(
  renderShell: (client: InstalledSparrowClient) => QueryClient,
  client: InstalledSparrowClient,
  user = userEvent.setup(),
): Promise<{ readonly shell: HTMLElement; readonly heading: HTMLElement }> {
  stubViewport(true);
  renderShell(client);
  await user.click(
    await screen.findByRole("button", { name: "Tune World News" }),
  );
  await screen.findByLabelText("World News live video");
  // An installed device is pocket until it has said the picture may be covered.
  await screen.findByRole("navigation", { name: "Nearby channels" });
  return {
    shell: requireShell(),
    heading: screen.getByRole("heading", { level: 1 }),
  };
}

function requireMonitor(): HTMLElement {
  const monitor = document.querySelector<HTMLElement>(".stage__monitor");
  if (monitor === null) {
    throw new Error("expected the picture box");
  }
  return monitor;
}

/** Gives the picture box the size a browser would have laid it out at. */
function layOutMonitor(width: number, height: number): void {
  requireMonitor().getBoundingClientRect = () =>
    DOMRect.fromRect({ x: 0, y: 38, width, height });
}

/** The size the shell holds the picture's box at, or null while it holds none. */
function pinnedPictureSize(): {
  readonly width: string;
  readonly height: string;
} | null {
  const root = document.documentElement.style;
  const width = root.getPropertyValue("--pocket-pinned-w");
  const height = root.getPropertyValue("--pocket-pinned-h");
  return width === "" && height === "" ? null : { width, height };
}

function requireShell(): HTMLElement {
  const shell = document.querySelector<HTMLElement>(
    "[data-acceptance-catalog-shell]",
  );
  if (shell === null) {
    throw new Error("expected the shell");
  }
  return shell;
}

/**
 * jsdom has no fullscreen: gives one element a request that always succeeds.
 * With `enters`, the request also makes the element the document's fullscreen
 * element, as a browser would, until `exit` or `restore`.
 */
function stubRequestFullscreen(
  element: HTMLElement,
  { enters = false }: { readonly enters?: boolean } = {},
): {
  readonly request: ReturnType<typeof vi.fn>;
  /** Leaves fullscreen as the browser's own exit would. */
  exit(): void;
  restore(): void;
} {
  const setFullscreenElement = (value: HTMLElement | null) => {
    Object.defineProperty(document, "fullscreenElement", {
      configurable: true,
      value,
    });
    document.dispatchEvent(new Event("fullscreenchange"));
  };
  const request = vi.fn(() => {
    if (enters) {
      setFullscreenElement(element);
    }
    return Promise.resolve();
  });
  Object.defineProperty(element, "requestFullscreen", {
    configurable: true,
    value: request,
  });
  return {
    request,
    exit: () => setFullscreenElement(null),
    restore: () => {
      Reflect.deleteProperty(element, "requestFullscreen");
      Reflect.deleteProperty(document, "fullscreenElement");
    },
  };
}

function requireMasthead(): HTMLElement {
  const masthead = document.querySelector<HTMLElement>(".shell__masthead");
  if (masthead === null) {
    throw new Error("expected the masthead");
  }
  return masthead;
}

/** Where the player's controls go while its own section is not fullscreen. */
function requireControlsSlot(): HTMLElement {
  const slot = document.querySelector<HTMLElement>(".now-playing__controls");
  if (slot === null) {
    throw new Error("expected the info block's controls slot");
  }
  return slot;
}

function renderHostedBrowser(client: InstalledSparrowClient): QueryClient {
  return renderBrowser(
    <CatalogBrowser client={client} playbackEngine={TEST_PLAYBACK_ENGINE} />,
  );
}

function renderInstalledBrowser(client: InstalledSparrowClient): QueryClient {
  return renderBrowser(
    <CatalogBrowser
      client={client}
      runtime="installed"
      playbackEngine={TEST_INSTALLED_PLAYBACK_ENGINE}
    />,
    "installed",
  );
}

function renderBrowser(browser: ReactElement, runtime: "hosted" | "installed" = "hosted"): QueryClient {
  const queryClient = createSparrowQueryClient(runtime);
  render(
    <QueryClientProvider client={queryClient}>{browser}</QueryClientProvider>,
  );
  return queryClient;
}

const TEST_PLAYBACK_ENGINE: HostedPlaybackEngine = {
  start: ({ video }) => {
    video.dispatchEvent(new Event("playing"));
    return { stop: () => undefined };
  },
};

const TEST_INSTALLED_PLAYBACK_ENGINE: InstalledPlaybackEngine = {
  start: ({ onPlaying }) => {
    onPlaying();
    return { stop: () => undefined };
  },
};

type ProgrammeOffsets = readonly [
  title: string,
  startsAfterMinutes: number,
  endsAfterMinutes: number,
];

/** The default catalog in Channel Catalog order, with each Channel's Programmes. */
const DEFAULT_CATALOG: readonly (readonly [
  ChannelSummary,
  readonly ProgrammeOffsets[],
])[] = [
  [
    WORLD_NEWS,
    [
      ["Live Bulletin", 0, 60],
      ["Future Bulletin", 60, 120],
    ],
  ],
  [CINEMA_ONE, [["Feature Presentation", 0, 180]]],
];

function defaultGuideRows(
  input: GuideWindowInput,
): readonly GuideWindowChannel[] {
  return DEFAULT_CATALOG.map(([channel, programmes]) => ({
    channel,
    programmes: programmesFor(input, programmes),
    programmesTruncated: false,
  }));
}

function defaultGuidePage(input: GuideWindowInput): GuideWindow {
  return guidePage(input, { rows: defaultGuideRows(input) });
}

/**
 * Answers a guide read over the default catalog as core does: the ordinary
 * first page, or the page placed around one of its Channels.
 */
function defaultGuideResult(
  input: GuideWindowInput,
): ClientResult<GuideWindow> {
  if (input.around === undefined) {
    return success(defaultGuidePage(input));
  }
  const rows = defaultGuideRows(input);
  const position = rows.findIndex((row) => row.channel.id === input.around);
  if (position === -1) {
    return failure({ _tag: "not-found", resource: "channel" });
  }
  const start = Math.max(0, position - Math.floor(input.channelLimit / 2));
  const end = start + input.channelLimit;
  return success(
    guidePage(input, {
      rows: rows.slice(start, end),
      ...(end < rows.length ? { next: "guide-after-around" } : {}),
    }),
  );
}

/** A run of Channels to move through, each in the group `groupOf` names. */
function surfChannels(
  count: number,
  groupOf: (index: number) => string = () => "Open",
): readonly ChannelSummary[] {
  return Array.from({ length: count }, (_, index) =>
    channelFixture({
      id: `surf-${index + 1}`,
      name: `Surf ${index + 1}`,
      group: groupOf(index),
    }),
  );
}

/**
 * Answers guide reads over a catalog of Channels as core does: a page from
 * its start or from a cursor, or the page placed around one of its Channels.
 */
function catalogGuide(
  channels: readonly ChannelSummary[],
): (input: GuideWindowInput) => Promise<ClientResult<GuideWindow>> {
  return (input) => {
    const rows = channels.map((channel) =>
      guideRow(channel, input, `${channel.name} live`),
    );
    const position = rows.findIndex((row) => row.channel.id === input.around);
    if (input.around !== undefined && position === -1) {
      return Promise.resolve(
        failure({ _tag: "not-found", resource: "channel" }),
      );
    }
    const start =
      input.around !== undefined
        ? Math.max(0, position - Math.floor(input.channelLimit / 2))
        : input.cursor === undefined
          ? 0
          : Number(input.cursor.replace("surf-from-", ""));
    const end = start + input.channelLimit;
    return Promise.resolve(
      success(
        guidePage(input, {
          rows: rows.slice(start, end),
          ...(end < rows.length ? { next: `surf-from-${end}` } : {}),
        }),
      ),
    );
  };
}

/**
 * Answers a schedule read from an instant with the default catalog's
 * Programmes for that Channel, each described and laid out from that instant
 * on. A read of the whole schedule stays empty.
 */
function defaultSchedulePage(input: ScheduleInput): Page<ProgrammeSummary> {
  return schedulePage(
    input,
    DEFAULT_CATALOG.find(([channel]) => channel.id === input.id)?.[1] ?? [],
  );
}

/**
 * A schedule page of described Programmes laid out from the read's instant
 * on; empty for a read of the whole schedule.
 */
function schedulePage(
  input: ScheduleInput,
  programmes: readonly ProgrammeOffsets[],
  generation = 7,
): Page<ProgrammeSummary> {
  const from = input.from;
  return clientSchemas.schedulePageFor(input).parse({
    generation,
    items:
      from === undefined
        ? []
        : programmesFor({ startsAt: from }, programmes)
            .slice(0, input.limit)
            .map((programme) => ({
              channelId: input.id,
              title: programme.title,
              description: `About ${programme.title}.`,
              startsAt: programme.startsAt,
              endsAt: programme.endsAt,
            })),
    next: null,
  });
}

/** The guide reads placed around a Channel: the playing Channel's neighbourhood. */
function neighbourhoodInputs(
  client: FakeSparrowClient,
): readonly GuideWindowInput[] {
  return client.guideInputs.filter((input) => input.around !== undefined);
}

function newsGroupsPage(
  input: ListGroupsInput,
  generation: number,
): Page<ChannelGroup> {
  return clientSchemas.groupsPageFor(input).parse({
    generation,
    items: [{ name: "News", channelCount: 1 }],
    next: null,
  });
}

function catalogPublished(generation: number): SparrowEvent {
  return clientSchemas.sparrowEvent.parse({
    _tag: "catalog-published",
    occurredAt: "2026-09-01T20:00:00Z",
    generation,
  });
}

/** A guide page whose first row folds two Quality Variants. */
async function variantGuide(
  input: GuideWindowInput,
): Promise<ClientResult<GuideWindow>> {
  return success(
    guidePage(input, {
      rows: [
        guideRow(SVT1_SD, input, "Rapport"),
        guideRow(SVT1_HD, input, "Rapport"),
        ...defaultGuideRows(input),
      ],
    }),
  );
}

/** The accessible name and pressed state of every acceptance-marked Channel button. */
function acceptanceChannels(): readonly (readonly [string | null, string | null])[] {
  return Array.from(
    document.querySelectorAll("button[data-acceptance-channel]"),
    (button) => [
      button.getAttribute("aria-label"),
      button.getAttribute("aria-pressed"),
    ],
  );
}

function guideRow(
  channel: ChannelSummary,
  input: GuideWindowInput,
  title: string,
): GuideWindowChannel {
  return {
    channel,
    programmes: programmesFor(input, [[title, 0, 180]]),
    programmesTruncated: false,
  };
}

function programmesFor(
  input: Pick<GuideWindowInput, "startsAt">,
  programmes: readonly ProgrammeOffsets[],
): readonly GuideProgramme[] {
  const windowStart = Date.parse(input.startsAt);
  return programmes.map(([title, startsAfterMinutes, endsAfterMinutes]) => ({
    title,
    titleTruncated: false,
    startsAt: clientSchemas.isoInstant.parse(
      new Date(windowStart + startsAfterMinutes * 60_000).toISOString(),
    ),
    endsAt: clientSchemas.isoInstant.parse(
      new Date(windowStart + endsAfterMinutes * 60_000).toISOString(),
    ),
  }));
}

function guidePage(
  input: GuideWindowInput,
  options: {
    readonly rows: readonly GuideWindowChannel[];
    readonly generation?: number;
    readonly next?: string;
  },
): GuideWindow {
  const items =
    options.next === undefined
      ? options.rows
      : fillContinuingGuidePage(input, options.rows);
  return clientSchemas.guideWindowFor(input).parse({
    generation: options.generation ?? 7,
    items: items.map((row) => ({
      channel: row.channel,
      programmes: row.programmes.map((programme) => ({
        title: programme.title,
        titleTruncated: programme.titleTruncated,
        startsAt: programme.startsAt,
        endsAt: programme.endsAt,
      })),
      programmesTruncated: row.programmesTruncated,
    })),
    next: options.next ?? null,
  });
}

function fillContinuingGuidePage(
  input: GuideWindowInput,
  rows: readonly GuideWindowChannel[],
): readonly GuideWindowChannel[] {
  const fillerCount = input.channelLimit - rows.length;
  return [
    ...rows,
    ...Array.from({ length: fillerCount }, (_, index) => {
      const channel = channelFixture({
        id: `guide-filler-${index}`,
        name: `Guide filler ${index + 1}`,
        group: input.group ?? "Auxiliary",
      });
      return guideRow(channel, input, `Filler Programme ${index + 1}`);
    }),
  ];
}

function success<Value>(value: Value): {
  readonly ok: true;
  readonly value: Value;
} {
  return { ok: true, value };
}

function failure(error: ClientError): {
  readonly ok: false;
  readonly error: ClientError;
} {
  return { ok: false, error };
}

interface Deferred<Value> {
  readonly promise: Promise<Value>;
  readonly resolve: (value: Value) => void;
}

function deferred<Value>(): Deferred<Value> {
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

function requireStage(heading: HTMLElement): HTMLElement {
  const stage = heading.closest("section");
  if (stage === null) {
    throw new Error("expected the stage heading inside the stage section");
  }
  return stage;
}

function requireFirst<Value>(values: readonly Value[], message: string): Value {
  const value = values[0];
  if (value === undefined) {
    throw new Error(message);
  }
  return value;
}

function requireMatch<Value>(
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
