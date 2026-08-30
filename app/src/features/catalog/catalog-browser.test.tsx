import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import {
  clientSchemas,
  type Capabilities,
  type CatalogStatus,
  type ChannelDetails,
  type ChannelGroup,
  type ChannelInput,
  type ChannelSummary,
  type ClientError,
  type ClientRequestOptions,
  type ClientResult,
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
  type SparrowClient,
  type SparrowEvent,
  type StartPlaybackInput,
} from "../../client/contracts";
import { CatalogBrowser } from "./catalog-browser";
import type { HostedPlaybackEngine } from "../playback/mpegts-engine";

afterEach(cleanup);

const CAPABILITIES = clientSchemas.capabilities.parse({
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

const NEXT_GENERATION_STATUS = clientSchemas.status.parse({
  generation: 8,
  configuration: { configured: true, epgConfigured: true },
  m3u: { _tag: "fresh", validatedAt: "2026-08-30T11:00:00Z" },
  epg: { _tag: "fresh", validatedAt: "2026-08-30T11:00:01Z" },
});

const DEFAULT_REFRESH_REPORT = clientSchemas.refreshReport.parse({
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
});

const STALE_STATUS = clientSchemas.status.parse({
  generation: 7,
  configuration: { configured: true, epgConfigured: true },
  m3u: {
    _tag: "stale",
    validatedAt: "2026-08-30T04:00:00Z",
    nextAttemptAt: "2026-08-30T10:01:00Z",
  },
  epg: { _tag: "fresh", validatedAt: "2026-08-30T10:00:01Z" },
});

const UNAVAILABLE_STATUS = clientSchemas.status.parse({
  generation: null,
  configuration: { configured: true, epgConfigured: false },
  m3u: {
    _tag: "unavailable",
    failure: { _tag: "source-read", source: "m3u", reason: "interrupted" },
  },
  epg: null,
});

const GROUPS_PAGE = clientSchemas.groupsPage.parse({
  generation: 7,
  items: [
    { name: "News & Current", channelCount: 1 },
    { name: "Cinema", channelCount: 1 },
    { name: "", channelCount: 1 },
  ],
  next: null,
});

const CHANNELS_PAGE = clientSchemas.channelsPage.parse({
  generation: 7,
  items: [
    { id: "world-news", name: "World News", group: "News & Current" },
    { id: "cinema-one", name: "Cinema One", group: "Cinema" },
    { id: "free-channel", name: "Free Channel", group: "" },
  ],
  next: null,
});

const NEWS_CHANNELS_PAGE = clientSchemas.channelsPage.parse({
  generation: 7,
  items: [
    { id: "world-news", name: "World News", group: "News & Current" },
  ],
  next: null,
});

const UNGROUPED_CHANNELS_PAGE = clientSchemas.channelsPage.parse({
  generation: 7,
  items: [{ id: "free-channel", name: "Free Channel", group: "" }],
  next: null,
});

const CHANNEL_DETAILS = clientSchemas.channel.parse({
  id: "world-news",
  name: "World News",
  group: "News & Current",
});

const NEXT_GENERATION_CHANNEL_DETAILS = clientSchemas.channel.parse({
  id: "world-news",
  name: "World News Reloaded",
  group: "News & Current",
});

const EMPTY_GROUPS_PAGE = clientSchemas.groupsPage.parse({
  generation: 7,
  items: [],
  next: null,
});

const EMPTY_CHANNELS_PAGE = clientSchemas.channelsPage.parse({
  generation: 7,
  items: [],
  next: null,
});

const FIRST_CHANNELS_PAGE = clientSchemas.channelsPage.parse({
  generation: 7,
  items: [{ id: "first-channel", name: "First Channel", group: "News" }],
  next: "channels-page-two",
});

const SECOND_CHANNELS_PAGE = clientSchemas.channelsPage.parse({
  generation: 7,
  items: [{ id: "second-channel", name: "Second Channel", group: "News" }],
  next: null,
});

const SECOND_CONTINUING_CHANNELS_PAGE = clientSchemas.channelsPage.parse({
  ...SECOND_CHANNELS_PAGE,
  next: "channels-page-three",
});

const NEXT_GENERATION_FIRST_CHANNELS_PAGE = clientSchemas.channelsPage.parse({
  generation: 8,
  items: [{ id: "replacement", name: "Replacement Channel", group: "News" }],
  next: FIRST_CHANNELS_PAGE.next,
});

const NEXT_GENERATION_CHANNELS_PAGE = clientSchemas.channelsPage.parse({
  generation: 8,
  items: [{ id: "replacement", name: "Replacement Channel", group: "News" }],
  next: null,
});

const EMPTY_SCHEDULE_PAGE = clientSchemas.schedulePage.parse({
  generation: 7,
  items: [],
  next: null,
});

const EMPTY_SEARCH_RESULTS = clientSchemas.searchResults.parse({
  generation: 7,
  channels: EMPTY_CHANNELS_PAGE,
  programmes: EMPTY_SCHEDULE_PAGE,
});

interface FakeBehavior {
  readonly capabilities?: (
    options: ClientRequestOptions | undefined,
  ) => Promise<ClientResult<Capabilities>>;
  readonly status?: (
    options: ClientRequestOptions | undefined,
  ) => Promise<ClientResult<CatalogStatus>>;
  readonly refresh?: (
    options: ClientRequestOptions | undefined,
  ) => Promise<ClientResult<RefreshReport>>;
  readonly groups?: (
    input: ListGroupsInput,
  ) => Promise<ClientResult<Page<ChannelGroup>>>;
  readonly channels?: (
    input: ListChannelsInput,
  ) => Promise<ClientResult<Page<ChannelSummary>>>;
  readonly channel?: (
    input: ChannelInput,
  ) => Promise<ClientResult<ChannelDetails>>;
  readonly schedule?: (
    input: ScheduleInput,
  ) => Promise<ClientResult<Page<ProgrammeSummary>>>;
  readonly search?: (
    input: SearchInput,
  ) => Promise<ClientResult<SearchResults>>;
}

class FakeSparrowClient implements SparrowClient {
  readonly capabilityInputs: (ClientRequestOptions | undefined)[] = [];
  readonly statusInputs: (ClientRequestOptions | undefined)[] = [];
  readonly groupInputs: ListGroupsInput[] = [];
  readonly channelListInputs: ListChannelsInput[] = [];
  readonly channelInputs: ChannelInput[] = [];
  readonly scheduleInputs: ScheduleInput[] = [];
  readonly searchInputs: SearchInput[] = [];
  readonly channelSearchInputs: SearchPageInput[] = [];
  readonly programmeSearchInputs: SearchPageInput[] = [];
  readonly playbackInputs: StartPlaybackInput[] = [];
  readonly refreshInputs: (ClientRequestOptions | undefined)[] = [];
  readonly eventListeners = new Set<(event: SparrowEvent) => void>();

  constructor(private readonly behavior: FakeBehavior = {}) {}

  capabilities(
    options?: ClientRequestOptions,
  ): Promise<ClientResult<Capabilities>> {
    this.capabilityInputs.push(options);
    return (
      this.behavior.capabilities?.(options) ?? Promise.resolve(success(CAPABILITIES))
    );
  }

  status(
    options?: ClientRequestOptions,
  ): Promise<ClientResult<CatalogStatus>> {
    this.statusInputs.push(options);
    return this.behavior.status?.(options) ?? Promise.resolve(success(FRESH_STATUS));
  }

  refresh(
    options?: ClientRequestOptions,
  ): Promise<ClientResult<RefreshReport>> {
    this.refreshInputs.push(options);
    return (
      this.behavior.refresh?.(options) ??
      Promise.resolve(success(DEFAULT_REFRESH_REPORT))
    );
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
    return this.behavior.groups?.(input) ?? Promise.resolve(success(GROUPS_PAGE));
  }

  listChannels(
    input: ListChannelsInput,
  ): Promise<ClientResult<Page<ChannelSummary>>> {
    this.channelListInputs.push(input);
    return (
      this.behavior.channels?.(input) ?? Promise.resolve(success(CHANNELS_PAGE))
    );
  }

  channel(input: ChannelInput): Promise<ClientResult<ChannelDetails>> {
    this.channelInputs.push(input);
    return (
      this.behavior.channel?.(input) ?? Promise.resolve(success(CHANNEL_DETAILS))
    );
  }

  schedule(input: ScheduleInput): Promise<ClientResult<Page<ProgrammeSummary>>> {
    this.scheduleInputs.push(input);
    return (
      this.behavior.schedule?.(input) ??
      Promise.resolve(success(EMPTY_SCHEDULE_PAGE))
    );
  }

  search(input: SearchInput): Promise<ClientResult<SearchResults>> {
    this.searchInputs.push(input);
    return (
      this.behavior.search?.(input) ?? Promise.resolve(success(EMPTY_SEARCH_RESULTS))
    );
  }

  searchChannels(
    input: SearchPageInput,
  ): Promise<ClientResult<Page<ChannelSummary>>> {
    this.channelSearchInputs.push(input);
    return Promise.resolve(success(EMPTY_CHANNELS_PAGE));
  }

  searchProgrammes(
    input: SearchPageInput,
  ): Promise<ClientResult<Page<ProgrammeSummary>>> {
    this.programmeSearchInputs.push(input);
    return Promise.resolve(success(EMPTY_SCHEDULE_PAGE));
  }

  startPlayback(
    input: StartPlaybackInput,
  ): Promise<ClientResult<PlaybackDescriptor>> {
    this.playbackInputs.push(input);
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

describe("CatalogBrowser", () => {
  it("renders one deterministic loading state until the initial catalog reads settle", async () => {
    const capabilities = deferred<ClientResult<Capabilities>>();
    const status = deferred<ClientResult<CatalogStatus>>();
    const groups = deferred<ClientResult<Page<ChannelGroup>>>();
    const channels = deferred<ClientResult<Page<ChannelSummary>>>();
    const client = new FakeSparrowClient({
      capabilities: () => capabilities.promise,
      status: () => status.promise,
      groups: () => groups.promise,
      channels: () => channels.promise,
    });

    renderBrowser(client);

    expect(
      screen.getByRole("heading", { name: "Tuning catalog" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "All frequencies" }),
    ).not.toBeInTheDocument();

    await act(async () => {
      capabilities.resolve(success(CAPABILITIES));
      status.resolve(success(FRESH_STATUS));
      groups.resolve(success(GROUPS_PAGE));
      channels.resolve(success(CHANNELS_PAGE));
      await Promise.all([
        capabilities.promise,
        status.promise,
        groups.promise,
        channels.promise,
      ]);
    });

    expect(
      await screen.findByRole("heading", { name: "All frequencies" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "Tuning catalog" }),
    ).not.toBeInTheDocument();
  });

  it("selects an exact source group and resolves the chosen Channel details", async () => {
    const client = new FakeSparrowClient({
      channels: (input) =>
        Promise.resolve(
          success(
            input.group === "News & Current"
              ? NEWS_CHANNELS_PAGE
              : CHANNELS_PAGE,
          ),
        ),
    });
    const user = userEvent.setup();
    renderBrowser(client);

    await screen.findByLabelText("3 channels loaded");
    const groupRail = screen.getByRole("navigation", {
      name: "Channel groups",
    });
    await user.click(
      within(groupRail).getByRole("button", { name: /News & Current/ }),
    );

    expect(
      await screen.findByRole("heading", { name: "News & Current" }),
    ).toBeVisible();
    expect(await screen.findByLabelText("1 channels loaded")).toBeVisible();
    const groupRequest = requireMatch(
      client.channelListInputs,
      (input) => input.group === "News & Current",
      "expected the selected source group to be queried",
    );
    expect(groupRequest.group).toBe("News & Current");

    await user.click(screen.getByRole("button", { name: /World News/ }));

    expect(
      await screen.findByRole("heading", { level: 3, name: "World News" }),
    ).toBeVisible();
    expect(screen.getByText("Catalog locked")).toBeVisible();
    expect(screen.getByLabelText("Selected channel")).toHaveAttribute(
      "aria-live",
      "polite",
    );
    expect(screen.getByLabelText("Selected channel")).toHaveAttribute(
      "aria-atomic",
      "true",
    );
    const detailRequest = requireFirst(
      client.channelInputs,
      "expected a Channel detail request",
    );
    expect(detailRequest.id).toBe(CHANNEL_DETAILS.id);
    expect(await screen.findByText("ON AIR")).toBeVisible();
    const playbackRequest = requireFirst(
      client.playbackInputs,
      "expected a hosted playback descriptor request",
    );
    expect(playbackRequest.id).toBe(CHANNEL_DETAILS.id);
  });

  it("distinguishes an omitted all-groups filter from the empty Ungrouped name", async () => {
    const client = new FakeSparrowClient({
      channels: (input) =>
        Promise.resolve(
          success(
            input.group === "" ? UNGROUPED_CHANNELS_PAGE : CHANNELS_PAGE,
          ),
        ),
    });
    const user = userEvent.setup();
    renderBrowser(client);

    await screen.findByLabelText("3 channels loaded");
    const initialRequest = requireFirst(
      client.channelListInputs,
      "expected the initial all-groups request",
    );
    expect("group" in initialRequest).toBe(false);

    const groupRail = screen.getByRole("navigation", {
      name: "Channel groups",
    });
    await user.click(
      within(groupRail).getByRole("button", { name: /Ungrouped/ }),
    );

    expect(
      await screen.findByRole("heading", { name: "Ungrouped" }),
    ).toBeVisible();
    expect(await screen.findByText("Free Channel")).toBeVisible();
    const ungroupedRequest = requireMatch(
      client.channelListInputs,
      (input) => input.group === "",
      "expected an explicit empty group filter",
    );
    expect(ungroupedRequest.group).toBe("");
  });

  it("retries failed selected Channel details without restarting its schedule", async () => {
    let detailAttempts = 0;
    const client = new FakeSparrowClient({
      channel: () => {
        detailAttempts += 1;
        return Promise.resolve(
          detailAttempts === 1
            ? failure({ _tag: "not-found", resource: "channel" })
            : success(CHANNEL_DETAILS),
        );
      },
    });
    const user = userEvent.setup();
    renderBrowser(client);

    await user.click(await screen.findByRole("button", { name: /World News/ }));
    const schedule = screen.getByRole("complementary", {
      name: "Programme schedule",
    });
    expect(
      await within(schedule).findByText("That Channel left the catalog"),
    ).toBeVisible();

    await user.click(within(schedule).getByRole("button", { name: "Try again" }));

    expect(await within(schedule).findByText("World News")).toBeVisible();
    expect(client.channelInputs).toHaveLength(2);
    expect(client.scheduleInputs).toHaveLength(1);
  });

  it("refetches selected Channel details for a newly published generation", async () => {
    let currentGeneration = 7;
    const client = new FakeSparrowClient({
      channel: () =>
        Promise.resolve(
          success(
            currentGeneration === 7
              ? CHANNEL_DETAILS
              : NEXT_GENERATION_CHANNEL_DETAILS,
          ),
        ),
    });
    const user = userEvent.setup();
    renderBrowser(client);

    await user.click(await screen.findByRole("button", { name: /World News/ }));
    const schedule = screen.getByRole("complementary", {
      name: "Programme schedule",
    });
    expect(await within(schedule).findByText("World News")).toBeVisible();

    currentGeneration = 8;
    await act(async () => {
      client.emit(
        clientSchemas.sparrowEvent.parse({
          _tag: "catalog-status-changed",
          occurredAt: "2026-08-30T11:00:02Z",
          status: NEXT_GENERATION_STATUS,
        }),
      );
    });

    expect(
      await within(schedule).findByText("World News Reloaded"),
    ).toBeVisible();
    expect(client.channelInputs).toHaveLength(2);
  });

  it("retains earlier Channels while a requested page is loading and appends it", async () => {
    const nextPage = deferred<ClientResult<Page<ChannelSummary>>>();
    const client = new FakeSparrowClient({
      channels: (input) =>
        input.cursor === FIRST_CHANNELS_PAGE.next
          ? nextPage.promise
          : Promise.resolve(success(FIRST_CHANNELS_PAGE)),
    });
    const user = userEvent.setup();
    renderBrowser(client);

    expect(await screen.findByText("First Channel")).toBeVisible();
    await user.click(
      screen.getByRole("button", { name: "Receive next 24 channels" }),
    );

    expect(
      await screen.findByRole("button", { name: /Receiving next block/ }),
    ).toBeDisabled();
    expect(screen.getByText("First Channel")).toBeVisible();

    await act(async () => {
      nextPage.resolve(success(SECOND_CHANNELS_PAGE));
      await nextPage.promise;
    });

    expect(await screen.findByText("Second Channel")).toBeVisible();
    expect(screen.getByText("First Channel")).toBeVisible();
    expect(screen.getByLabelText("2 channels loaded")).toBeVisible();
  });

  it("renders an explicit empty catalog without inventing source groups", async () => {
    const client = new FakeSparrowClient({
      groups: () => Promise.resolve(success(EMPTY_GROUPS_PAGE)),
      channels: () => Promise.resolve(success(EMPTY_CHANNELS_PAGE)),
    });
    renderBrowser(client);

    expect(
      await screen.findByRole("heading", { name: "All frequencies" }),
    ).toBeVisible();
    expect(screen.getByText("No source-defined groups.")).toBeVisible();
    expect(
      screen.getByRole("heading", {
        level: 3,
        name: "No Channels on this frequency",
      }),
    ).toBeVisible();
    expect(
      screen.getByText("The current catalog contains no browseable Channels."),
    ).toBeVisible();
  });

  it.each([
    {
      name: "authentication-required",
      title: "Access credential required",
      behavior: {
        capabilities: () =>
          Promise.resolve(
            failure({
              _tag: "authentication-required",
            }),
          ),
      },
    },
    {
      name: "catalog-unavailable",
      title: "The Channel Catalog is unavailable",
      behavior: {
        channels: () =>
          Promise.resolve(
            failure({
              _tag: "catalog-unavailable",
              status: UNAVAILABLE_STATUS,
            }),
          ),
      },
    },
  ])("renders the typed $name recovery state", async ({ behavior, title }) => {
    renderBrowser(new FakeSparrowClient(behavior));

    const alert = await screen.findByRole("alert");
    expect(within(alert).getByRole("heading", { name: title })).toBeVisible();
    expect(
      within(alert).getByRole("button", { name: "Check again" }),
    ).toBeEnabled();
  });

  it("retries a cached capability failure when the user checks again", async () => {
    const attempts = { count: 0 };
    const client = new FakeSparrowClient({
      capabilities: () => {
        attempts.count += 1;
        return Promise.resolve(
          attempts.count === 1
            ? failure({ _tag: "authentication-required" })
            : success(CAPABILITIES),
        );
      },
    });
    const user = userEvent.setup();
    renderBrowser(client);

    const alert = await screen.findByRole("alert");
    await user.click(within(alert).getByRole("button", { name: "Check again" }));

    await waitFor(() => expect(client.capabilityInputs).toHaveLength(2));
    expect(await screen.findByText("SAME ORIGIN")).toBeVisible();
  });

  it("reports a stale cursor while retaining the already loaded Channel page", async () => {
    const client = new FakeSparrowClient({
      channels: (input) =>
        Promise.resolve(
          input.cursor === FIRST_CHANNELS_PAGE.next
            ? failure({
                _tag: "stale-cursor",
                current: SECOND_CHANNELS_PAGE.generation,
              })
            : success(FIRST_CHANNELS_PAGE),
        ),
    });
    const user = userEvent.setup();
    renderBrowser(client);

    expect(await screen.findByText("First Channel")).toBeVisible();
    await user.click(
      screen.getByRole("button", { name: "Receive next 24 channels" }),
    );

    expect(
      await screen.findByRole("heading", {
        name: "A newer catalog is on air",
      }),
    ).toBeVisible();
    expect(screen.getByText("First Channel")).toBeVisible();
    expect(
      screen.getByText(/Reload to continue from its first page/),
    ).toBeVisible();
  });

  it("retains every loaded Channel page when a publication refetch later fails", async () => {
    let published = false;
    let currentStatus = FRESH_STATUS;
    const client = new FakeSparrowClient({
      status: () => Promise.resolve(success(currentStatus)),
      channels: (input) => {
        if (!published) {
          return Promise.resolve(
            success(
              input.cursor === FIRST_CHANNELS_PAGE.next
                ? SECOND_CONTINUING_CHANNELS_PAGE
                : FIRST_CHANNELS_PAGE,
            ),
          );
        }
        return Promise.resolve(
          input.cursor === FIRST_CHANNELS_PAGE.next
            ? failure({
                _tag: "transport",
                retryable: true,
                message: "A safe catalog refetch failed.",
              })
            : success(NEXT_GENERATION_FIRST_CHANNELS_PAGE),
        );
      },
    });
    const user = userEvent.setup();
    renderBrowser(client);

    await user.click(
      await screen.findByRole("button", {
        name: "Receive next 24 channels",
      }),
    );
    expect(await screen.findByText("Second Channel")).toBeVisible();

    published = true;
    currentStatus = NEXT_GENERATION_STATUS;
    await act(async () => {
      client.emit(
        clientSchemas.sparrowEvent.parse({
          _tag: "catalog-published",
          occurredAt: "2026-08-30T11:00:02Z",
          generation: 8,
        }),
      );
    });

    expect(
      await screen.findByRole("heading", {
        name: "The hosted desk did not answer",
      }),
    ).toBeVisible();
    expect(screen.getByText("First Channel")).toBeVisible();
    expect(screen.getByText("Second Channel")).toBeVisible();
    expect(screen.queryByText("Replacement Channel")).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Updating catalog generation…" }),
    ).toBeDisabled();
    const retainedBanner = screen.getByText("RECORDED SIGNAL").closest("aside");
    expect(retainedBanner).not.toBeNull();
    if (retainedBanner === null) {
      throw new Error("expected a retained catalog banner");
    }
    expect(within(retainedBanner).getByText(/generation 7/)).toBeVisible();
    expect(within(retainedBanner).getByText(/generation is 8/)).toBeVisible();
    expect(
      client.channelListInputs.filter(
        (input) => input.cursor === FIRST_CHANNELS_PAGE.next,
      ),
    ).toHaveLength(2);
  });

  it("rechecks status after a failed manual refresh flight", async () => {
    const refresh = deferred<ClientResult<RefreshReport>>();
    let currentStatus = FRESH_STATUS;
    const client = new FakeSparrowClient({
      status: () => Promise.resolve(success(currentStatus)),
      refresh: () => refresh.promise,
    });
    const user = userEvent.setup();
    renderBrowser(client);

    await user.click(
      await screen.findByRole("button", { name: "Refresh sources" }),
    );
    expect(
      screen.getByRole("button", { name: "Refresh in progress" }),
    ).toBeDisabled();

    currentStatus = NEXT_GENERATION_STATUS;
    await act(async () => {
      refresh.resolve(
        failure({
          _tag: "transport",
          retryable: true,
          message: "Detached request status is unknown.",
        }),
      );
      await refresh.promise;
    });

    expect(
      await screen.findByText("Refresh result was not received"),
    ).toBeVisible();
    await waitFor(() => expect(client.statusInputs).toHaveLength(2));
    expect(await screen.findByText("CATALOG / 8")).toBeVisible();
    await waitFor(() => expect(client.channelListInputs.length).toBeGreaterThan(1));
  });

  it("resynchronizes a missed publication from a refresh-completed hint", async () => {
    let currentStatus = FRESH_STATUS;
    const client = new FakeSparrowClient({
      status: () => Promise.resolve(success(currentStatus)),
      channels: () =>
        Promise.resolve(
          success(
            currentStatus.generation === NEXT_GENERATION_STATUS.generation
              ? NEXT_GENERATION_CHANNELS_PAGE
              : CHANNELS_PAGE,
          ),
        ),
    });
    renderBrowser(client);
    expect(await screen.findByText("World News")).toBeVisible();

    currentStatus = NEXT_GENERATION_STATUS;
    await act(async () => {
      client.emit(
        clientSchemas.sparrowEvent.parse({
          _tag: "refresh-completed",
          occurredAt: "2026-08-30T11:00:03Z",
          source: "m3u",
          outcome: {
            _tag: "updated",
            validatedAt: "2026-08-30T11:00:00Z",
          },
        }),
      );
    });

    expect(await screen.findByText("Replacement Channel")).toBeVisible();
    expect(screen.queryByText("World News")).not.toBeInTheDocument();
    expect(client.statusInputs).toHaveLength(2);
    expect(client.channelListInputs).toHaveLength(2);
  });

  it("owns exactly one event subscription after StrictMode replay and releases it", async () => {
    const client = new FakeSparrowClient();
    const queryClient = new QueryClient({
      defaultOptions: {
        queries: { retry: false, refetchOnWindowFocus: false },
      },
    });
    const view = render(
      <StrictMode>
        <QueryClientProvider client={queryClient}>
          <CatalogBrowser client={client} playbackEngine={TEST_PLAYBACK_ENGINE} />
        </QueryClientProvider>
      </StrictMode>,
    );

    await screen.findByText("World News");
    expect(client.eventListeners.size).toBe(1);
    view.unmount();
    expect(client.eventListeners.size).toBe(0);
  });

  it("warns about a retained stale source without hiding usable Channels", async () => {
    const client = new FakeSparrowClient({
      status: () => Promise.resolve(success(STALE_STATUS)),
    });
    renderBrowser(client);

    expect(await screen.findByText("World News")).toBeVisible();
    const retainedBanner = screen.getByRole("status");
    expect(within(retainedBanner).getByText("RECORDED SIGNAL")).toBeVisible();
    expect(within(retainedBanner).getByText(/generation 7/)).toBeVisible();
    expect(screen.getByText("RECORDED / STALE")).toBeVisible();
  });
});

function renderBrowser(client: SparrowClient): QueryClient {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        refetchOnWindowFocus: false,
      },
    },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <CatalogBrowser client={client} playbackEngine={TEST_PLAYBACK_ENGINE} />
    </QueryClientProvider>,
  );
  return queryClient;
}

const TEST_PLAYBACK_ENGINE: HostedPlaybackEngine = {
  start: ({ video }) => {
    video.dispatchEvent(new Event("playing"));
    return { stop: () => undefined };
  },
};

function success<Value>(
  value: Value,
): { readonly ok: true; readonly value: Value } {
  return { ok: true, value };
}

function failure(
  error: ClientError,
): { readonly ok: false; readonly error: ClientError } {
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

function requireFirst<Value>(
  values: readonly Value[],
  message: string,
): Value {
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
