import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import {
  clientSchemas,
  type ClientResult,
  type SearchResults,
} from "../../client/contracts";
import {
  CINEMA_CHANNEL,
  EMPTY_CHANNELS,
  EMPTY_PROGRAMMES,
  FAILED_GUIDE_STATUS,
  FRESH_STATUS,
  MORNING_NEWS,
  MORNING_PROGRAMME_PAGE,
  STALE_GUIDE_STATUS,
  WORLD_CHANNEL,
  WORLD_CHANNEL_PAGE,
  FakeSparrowClient,
  channelPage,
  continuingChannelPage,
  continuingProgrammePage,
  deferred,
  failure,
  renderConsole,
  requireInput,
  searchResults,
  statusAtGeneration,
  submitSearch,
  success,
} from "./search-console.test-support";

afterEach(cleanup);

describe("SearchConsole search results", () => {
  it("paginates Channel and Programme results on independent cursor tracks", async () => {
    const firstChannels = continuingChannelPage(WORLD_CHANNEL, "channel-next");
    const secondChannels = clientSchemas.channelsPage.parse({
      generation: 7,
      items: [
        CINEMA_CHANNEL,
        ...Array.from({ length: 11 }, (_, index) => ({
          id: `cinema-filler-${index}`,
          name: `Cinema filler ${index}`,
          group: "Fixture",
        })),
      ],
      next: "channel-last",
    });
    const finalChannels = clientSchemas.channelsPage.parse({
      generation: 7,
      items: [
        { id: "final-channel", name: "Final Channel", group: "Fixture" },
      ],
      next: null,
    });
    const firstProgrammes = continuingProgrammePage(
      MORNING_NEWS,
      "programme-next",
    );
    const laterProgramme = clientSchemas.schedulePage.parse({
      generation: 7,
      items: [
        {
          channelId: "world-news",
          title: "Evening News",
          description: null,
          startsAt: "2026-08-30T18:00:00Z",
          endsAt: "2026-08-30T19:00:00Z",
        },
      ],
      next: null,
    });
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(success(searchResults(firstChannels, firstProgrammes))),
      searchChannels: (input) =>
        Promise.resolve(
          success(
            input.cursor === secondChannels.next
              ? finalChannels
              : secondChannels,
          ),
        ),
      searchProgrammes: () => Promise.resolve(success(laterProgramme)),
    });
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);
    await submitSearch(user, "news");

    await user.click(
      await screen.findByRole("button", { name: "More Channels +" }),
    );
    expect(await screen.findByText("Cinema One")).toBeVisible();
    expect(screen.getByText("Morning News")).toBeVisible();
    await user.click(
      screen.getByRole("button", { name: "More Channels +" }),
    );
    expect(await screen.findByText("Final Channel")).toBeVisible();

    await user.click(
      screen.getByRole("button", { name: "More Programmes +" }),
    );
    expect(await screen.findByText("Evening News")).toBeVisible();
    expect(screen.getByText("World News")).toBeVisible();

    const channelPageInput = requireInput(
      client.channelSearchInputs,
      (input) => input.cursor === firstChannels.next,
      "expected a Channel continuation request",
    );
    expect(channelPageInput.limit).toBe(12);
    expect(channelPageInput.previousCursors).toEqual([]);
    const finalChannelPageInput = requireInput(
      client.channelSearchInputs,
      (input) => input.cursor === secondChannels.next,
      "expected the final Channel continuation request",
    );
    expect(finalChannelPageInput.previousCursors).toEqual([
      firstChannels.next,
    ]);
    const programmePageInput = requireInput(
      client.programmeSearchInputs,
      (input) => input.cursor === firstProgrammes.next,
      "expected a Programme continuation request",
    );
    expect(programmePageInput.limit).toBe(10);
    expect(programmePageInput.previousCursors).toEqual([]);
  });

  it("rejects a search term that exceeds the UTF-8 contract before transport", async () => {
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);

    await submitSearch(user, "🛰️".repeat(40));

    expect(
      screen.getByText("Keep the search term within 256 UTF-8 bytes."),
    ).toBeVisible();
    expect(client.searchInputs).toHaveLength(0);
  });

  it("rejects a successful cross-generation search page before rendering it", async () => {
    const firstChannels = continuingChannelPage(WORLD_CHANNEL, "channel-next");
    const newerChannels = clientSchemas.channelsPage.parse({
      generation: 8,
      items: [CINEMA_CHANNEL],
      next: null,
    });
    let generationCrossed = false;
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(
          success(
            searchResults(
              generationCrossed ? newerChannels : firstChannels,
              EMPTY_PROGRAMMES,
            ),
          ),
        ),
      searchChannels: () => {
        generationCrossed = true;
        return Promise.resolve(success(newerChannels));
      },
    });
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);
    await submitSearch(user, "news");
    await user.click(
      await screen.findByRole("button", { name: "More Channels +" }),
    );

    expect(
      await screen.findByText(/catalog changed during pagination/),
    ).toBeVisible();
    expect(screen.getByText("World News")).toBeVisible();
    expect(screen.queryByText("Cinema One")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Restart scan" }));
    expect(await screen.findByText("Cinema One")).toBeVisible();
  });

  it("refetches a submitted search when status publishes a new generation", async () => {
    let generation = 7;
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(
          success(
            searchResults(
              channelPage(
                generation === 7 ? WORLD_CHANNEL : CINEMA_CHANNEL,
                generation,
              ),
              EMPTY_PROGRAMMES,
            ),
          ),
        ),
    });
    const user = userEvent.setup();
    const rendered = renderConsole(client, FRESH_STATUS);
    await submitSearch(user, "news");
    expect(await screen.findByText("World News")).toBeVisible();

    generation = 8;
    rendered.rerenderStatus(statusAtGeneration(8));
    expect(await screen.findByText("Cinema One")).toBeVisible();
    expect(client.searchInputs).toHaveLength(2);
  });

  it("retains a submitted search and disables its old cursor when publication refetch fails", async () => {
    const firstChannels = continuingChannelPage(WORLD_CHANNEL, "channel-next");
    let published = false;
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(
          published
            ? failure({
                _tag: "transport",
                retryable: true,
                message: "Safe refetch failure.",
              })
            : success(searchResults(firstChannels, EMPTY_PROGRAMMES)),
        ),
    });
    const user = userEvent.setup();
    const rendered = renderConsole(client, FRESH_STATUS);
    await submitSearch(user, "news");
    expect(await screen.findByText("World News")).toBeVisible();
    expect(screen.getByRole("button", { name: "More Channels +" })).toBeEnabled();

    published = true;
    rendered.rerenderStatus(statusAtGeneration(8));

    expect(
      await screen.findByText(/catalog changed during pagination/),
    ).toBeVisible();
    expect(screen.getByText("World News")).toBeVisible();
    expect(
      screen.getByText(/Earlier results remain visible/),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "More Channels +" }),
    ).not.toBeInTheDocument();
    expect(client.searchInputs).toHaveLength(2);
  });

  it("renders an explicit no-results state for both live result lanes", async () => {
    const client = new FakeSparrowClient();
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);

    await submitSearch(user, "not-on-air");

    expect(await screen.findByText("not-on-air", { selector: "q" })).toBeVisible();
    expect(screen.getByText(/No Programmes match.*not-on-air/)).toBeVisible();
  });

  it("labels retained stale Guide results without hiding them", async () => {
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(
          success(searchResults(WORLD_CHANNEL_PAGE, MORNING_PROGRAMME_PAGE)),
        ),
    });
    const user = userEvent.setup();
    renderConsole(client, STALE_GUIDE_STATUS);

    expect(screen.getByText("GUIDE RECORDED")).toBeVisible();
    await submitSearch(user, "news");
    expect(await screen.findByText("Morning News")).toBeVisible();
    expect(screen.getByText("World News")).toBeVisible();
  });

  it("explains a failed Guide while retaining usable Channel search", async () => {
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(
          success(searchResults(WORLD_CHANNEL_PAGE, EMPTY_PROGRAMMES)),
        ),
    });
    const user = userEvent.setup();
    renderConsole(client, FAILED_GUIDE_STATUS);

    expect(screen.getByText("GUIDE FAILED")).toBeVisible();
    await submitSearch(user, "news");
    expect(await screen.findByText("World News")).toBeVisible();
    expect(
      screen.getByText(/Programme search has no validated Guide snapshot/),
    ).toBeVisible();
  });

  it("retains an earlier page after stale-cursor failure and restarts at generation one", async () => {
    const firstChannels = continuingChannelPage(WORLD_CHANNEL, "channel-next");
    const currentChannels = clientSchemas.channelsPage.parse({
      generation: 8,
      items: [CINEMA_CHANNEL],
      next: null,
    });
    let cursorRejected = false;
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(
          success(
            searchResults(
              cursorRejected ? currentChannels : firstChannels,
              EMPTY_PROGRAMMES,
            ),
          ),
        ),
      searchChannels: () => {
        cursorRejected = true;
        return Promise.resolve(
          failure({
            _tag: "stale-cursor",
            current: currentChannels.generation,
          }),
        );
      },
    });
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);
    await submitSearch(user, "news");

    await user.click(
      await screen.findByRole("button", { name: "More Channels +" }),
    );
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/Earlier results remain visible/)).toBeVisible();
    expect(screen.getByText("World News")).toBeVisible();

    await user.click(
      within(alert).getByRole("button", {
        name: "Restart on current catalog",
      }),
    );
    expect(await screen.findByText("Cinema One")).toBeVisible();
    expect(screen.queryByText("World News")).not.toBeInTheDocument();
    expect(client.searchInputs).toHaveLength(2);
    expect(client.channelSearchInputs).toHaveLength(1);
  });

  it("renders one typed authentication failure for a failed initial search", async () => {
    const client = new FakeSparrowClient({
      search: () => Promise.resolve(failure({ _tag: "authentication-required" })),
    });
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);
    await submitSearch(user, "news");

    expect(await screen.findByText("Access credential required")).toBeVisible();
    expect(screen.getAllByRole("alert")).toHaveLength(1);
  });

  it("retains results after a typed transport failure without rendering its cause", async () => {
    const privateMarker = "provider-secret.invalid";
    const firstChannels = continuingChannelPage(WORLD_CHANNEL, "channel-next");
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(success(searchResults(firstChannels, EMPTY_PROGRAMMES))),
      searchChannels: () =>
        Promise.resolve(
          failure({
            _tag: "transport",
            retryable: true,
            message: privateMarker,
          }),
        ),
    });
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);
    await submitSearch(user, "news");
    await user.click(
      await screen.findByRole("button", { name: "More Channels +" }),
    );

    expect(await screen.findByText("The hosted desk did not answer")).toBeVisible();
    expect(screen.getByText(/Earlier results remain visible/)).toBeVisible();
    expect(screen.getByText("World News")).toBeVisible();
    expect(screen.queryByText(privateMarker)).not.toBeInTheDocument();
  });

  it("shows stable lane loading states while a search is pending", async () => {
    const pending = deferred<ClientResult<SearchResults>>();
    const client = new FakeSparrowClient({ search: () => pending.promise });
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);
    await submitSearch(user, "news");

    expect(await screen.findByText("Searching Channels…")).toBeVisible();
    expect(screen.getByText("Searching Programmes…")).toBeVisible();

    pending.resolve(success(searchResults(EMPTY_CHANNELS, EMPTY_PROGRAMMES)));
    await waitFor(() =>
      expect(screen.queryByText("Searching Channels…")).not.toBeInTheDocument(),
    );
  });
});
