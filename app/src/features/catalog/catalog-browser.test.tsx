import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
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
  type SearchResults,
  type SourceConfigurationInput,
  type SparrowEvent,
  type StartPlaybackInput,
} from "../../client/contracts";
import type { InstalledPlaybackEngine } from "../playback/installed-playback-engine";
import type { HostedPlaybackEngine } from "../playback/mpegts-engine";
import { CatalogBrowser } from "./catalog-browser";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const HOSTED_CAPABILITIES = clientSchemas.capabilities.parse({
  sourceConfiguration: "deployment-readonly",
  playbackTransport: "same-origin-http",
  audioTrackSelection: false,
  mpvFailover: false,
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

const WORLD_NEWS = clientSchemas.channel.parse({
  id: "world-news",
  name: "World News",
  group: "News",
});

const CINEMA_ONE = clientSchemas.channel.parse({
  id: "cinema-one",
  name: "Cinema One",
  group: "Cinema",
});

const EMPTY_SCHEDULE = clientSchemas.schedulePage.parse({
  generation: 7,
  items: [],
  next: null,
});

const EMPTY_SEARCH_RESULTS = clientSchemas.searchResults.parse({
  generation: 7,
  channels: { generation: 7, items: [], next: null },
  programmes: { generation: 7, items: [], next: null },
});

interface FakeBehavior {
  readonly status?: (
    options: ClientRequestOptions | undefined,
  ) => Promise<ClientResult<CatalogStatus>>;
  readonly groups?: (
    input: ListGroupsInput,
  ) => Promise<ClientResult<Page<ChannelGroup>>>;
  readonly guide?: (
    input: GuideWindowInput,
  ) => Promise<ClientResult<GuideWindow>>;
  readonly search?: (
    input: SearchInput,
  ) => Promise<ClientResult<SearchResults>>;
  readonly replaceConfiguration?: (
    input: SourceConfigurationInput,
  ) => Promise<ClientResult<CatalogStatus>>;
}

class FakeSparrowClient implements InstalledSparrowClient {
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
    return Promise.resolve(success(HOSTED_CAPABILITIES));
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
      Promise.resolve(success(defaultGuidePage(input)))
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
    return Promise.resolve(success(EMPTY_SCHEDULE));
  }

  search(input: SearchInput): Promise<ClientResult<SearchResults>> {
    this.searchInputs.push(input);
    return (
      this.behavior.search?.(input) ??
      Promise.resolve(success(EMPTY_SEARCH_RESULTS))
    );
  }

  searchChannels(): Promise<ClientResult<Page<ChannelSummary>>> {
    return Promise.resolve(
      success({ generation: 7 as CatalogGeneration, items: [], next: null }),
    );
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
    const transport: InstalledPlaybackTransport = {
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

describe("CatalogBrowser Split Stage", () => {
  it("shows the one initial status loader before mounting the Split Stage", async () => {
    const status = deferred<ClientResult<CatalogStatus>>();
    const client = new FakeSparrowClient({ status: () => status.promise });

    renderHostedBrowser(client);

    expect(
      screen.getByRole("heading", { name: "Tuning catalog" }),
    ).toBeVisible();
    expect(screen.queryByLabelText("Programme guide")).not.toBeInTheDocument();

    await act(async () => {
      status.resolve(success(FRESH_STATUS));
      await status.promise;
    });

    expect(await screen.findByLabelText("Programme guide")).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "Tuning catalog" }),
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
      screen.getByRole("heading", { level: 1, name: "Published Feature" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Tune World News" }),
    ).not.toBeInTheDocument();
    expect(client.guideInputs).toHaveLength(initialGuideRequests + 1);
    expect(client.groupInputs).toHaveLength(initialGroupRequests + 1);
    expect(
      screen.queryByText(
        "Guide refresh failed; the visible window is retained.",
      ),
    ).not.toBeInTheDocument();
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
    expect(
      screen.getByRole("heading", { level: 1, name: "Published Bulletin" }),
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

    await user.type(
      await screen.findByRole("combobox", {
        name: "Search Channels and Programmes",
      }),
      "world",
    );
    expect(
      await screen.findByText("The catalog changed while searching."),
    ).toBeVisible();
    expect(client.searchInputs).toHaveLength(1);
    statusGeneration = publishedGeneration;

    await user.click(screen.getByRole("button", { name: "Rescan" }));

    expect(
      await screen.findByRole("option", { name: /World News/ }),
    ).toBeVisible();
    expect(client.statusInputs).toHaveLength(2);
    expect(client.searchInputs).toHaveLength(2);
  });

  it("builds rows and Programme cells from one bounded guide-window read", async () => {
    const client = new FakeSparrowClient();

    renderHostedBrowser(client);

    const guide = await screen.findByLabelText("Programme guide");
    expect(
      within(guide).getByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    expect(
      within(guide).getByRole("button", { name: /Live Bulletin,/ }),
    ).toBeVisible();
    expect(
      within(guide).getByRole("button", { name: /Future Bulletin,/ }),
    ).toBeVisible();
    expect(
      screen.getByRole("heading", { level: 1, name: "Live Bulletin" }),
    ).toBeVisible();

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

  it("keeps an explicitly selected future Programme aligned with its playing Channel", async () => {
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(
      await screen.findByRole("button", { name: /Future Bulletin,/ }),
    );

    expect(
      await screen.findByRole("heading", { level: 1, name: "Future Bulletin" }),
    ).toBeVisible();
    expect(screen.getByText("Schedule")).toBeVisible();
    expect(
      screen.getByRole("heading", { level: 2, name: "World News" }),
    ).toBeVisible();
    expect(client.playbackInputs).toHaveLength(1);
    expect(client.playbackInputs[0]?.id).toBe(WORLD_NEWS.id);
  });

  it.each([
    {
      name: "unconfigured",
      status: NOT_CONFIGURED_STATUS,
      title: "Patch a feed to this receiver",
      detail: "Open Feeds to configure the installed catalog before browsing.",
    },
    {
      name: "configured without a generation",
      status: CONFIGURED_WITHOUT_GENERATION_STATUS,
      title: "Waiting for the first catalog",
      detail:
        "The configured feeds have not published a validated snapshot yet.",
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

    await screen.findByText("Patch a feed to this receiver");
    await user.click(screen.getByRole("button", { name: "Feeds" }));
    const m3u = await screen.findByLabelText("Required / Channel source");
    const epg = screen.getByLabelText("Optional / Guide source");
    const privateM3u = "https://viewer:secret@provider.invalid/list.m3u";
    const privateEpg = "https://viewer:secret@provider.invalid/guide.xml";

    expect(m3u).toHaveAttribute("autocomplete", "off");
    await user.type(m3u, privateM3u);
    await user.type(epg, privateEpg);
    await user.click(
      screen.getByRole("button", { name: "Build local catalog" }),
    );

    expect(
      await screen.findByText(
        "Configuration saved. Safe catalog status will update as the local build completes.",
      ),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Close Feeds" }));
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

  it("keeps hosted Feeds read-only and free of source-location controls", async () => {
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderHostedBrowser(client);

    await user.click(await screen.findByRole("button", { name: "Feeds" }));

    const dialog = await screen.findByRole("dialog", {
      name: "Feeds & signal health",
    });
    expect(
      within(dialog).getByText(/deployment-managed sources/),
    ).toBeVisible();
    expect(
      within(dialog).queryByLabelText("Required / Channel source"),
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
      "Retained catalog · a fresh source check is pending",
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
    expect(
      screen.getByText("Guide refresh failed; the visible window is retained."),
    ).toBeVisible();
    expect(client.statusInputs).toHaveLength(1);
    expect(client.groupInputs).toHaveLength(1);
    expect(client.guideInputs).toHaveLength(1);

    await user.click(screen.getByRole("button", { name: "Retry" }));

    await waitFor(() => expect(client.statusInputs).toHaveLength(2));
    await waitFor(() =>
      expect(
        screen.queryByText(
          "Guide refresh failed; the visible window is retained.",
        ),
      ).not.toBeInTheDocument(),
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
      await screen.findByRole("button", { name: "More Channels" }),
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
    expect(
      screen.getByText("Guide refresh failed; the visible window is retained."),
    ).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Retry" }));

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
    expect(
      screen.queryByText(
        "Guide refresh failed; the visible window is retained.",
      ),
    ).not.toBeInTheDocument();
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
      await screen.findByRole("button", { name: "More Channels" }),
    );
    await waitFor(() => expect(client.guideInputs).toHaveLength(2));

    expect(
      screen.getByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Tune Cinema One" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Replacement Feature")).not.toBeInTheDocument();
    expect(
      await screen.findByText(
        "Guide refresh failed; the visible window is retained.",
      ),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "More Channels" })).toBeEnabled();
  });
});

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
  );
}

function renderBrowser(browser: ReactElement): QueryClient {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false },
    },
  });
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

function defaultGuidePage(input: GuideWindowInput): GuideWindow {
  const newsProgrammes = programmesFor(input, [
    ["Live Bulletin", 0, 60],
    ["Future Bulletin", 60, 120],
  ]);
  return guidePage(input, {
    rows: [
      {
        channel: WORLD_NEWS,
        programmes: newsProgrammes,
        programmesTruncated: false,
      },
      guideRow(CINEMA_ONE, input, "Feature Presentation"),
    ],
  });
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
  input: GuideWindowInput,
  programmes: readonly (readonly [
    title: string,
    startsAfterMinutes: number,
    endsAfterMinutes: number,
  ])[],
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
      const channel = clientSchemas.channel.parse({
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
