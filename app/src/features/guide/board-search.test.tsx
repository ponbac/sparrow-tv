import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clientSchemas,
  type CatalogGeneration,
  type ClientResult,
  type Page,
  type SearchInput,
  type SearchPageInput,
  type SparrowClient,
} from "../../client/contracts";
import { channelFixture } from "../../test/channel-fixture";
import { BoardSearch } from "./board-search";

afterEach(cleanup);

const CHANNEL = channelFixture({
  id: "world-news",
  name: "World News",
  group: "News",
});

const CINEMA = channelFixture({
  id: "cinema-one",
  name: "Cinema One",
  group: "Cinema",
});

const PROGRAMME_HIT = {
  channel: CHANNEL,
  title: "Evening Studio",
  titleTruncated: false,
  startsAt: "2026-09-01T20:30:00Z",
  endsAt: "2026-09-01T21:00:00Z",
} as const;

const CHANNEL_RESULTS = clientSchemas.searchResults.parse({
  generation: 7,
  channels: { generation: 7, items: [CHANNEL], next: null },
  programmes: { generation: 7, items: [], next: null },
});

const MIXED_CHANNEL_RESULTS = clientSchemas.searchResults.parse({
  generation: 7,
  channels: { generation: 7, items: [CHANNEL, CINEMA], next: null },
  programmes: { generation: 7, items: [], next: null },
});

const PROGRAMME_RESULTS = clientSchemas.searchResults.parse({
  generation: 7,
  channels: { generation: 7, items: [], next: null },
  programmes: { generation: 7, items: [PROGRAMME_HIT], next: null },
});

const REPLACEMENT_RESULTS = clientSchemas.searchResults.parse({
  generation: 8,
  channels: { generation: 8, items: [CHANNEL], next: null },
  programmes: { generation: 8, items: [], next: null },
});

const CHANNEL_PAGE = {
  generation: CHANNEL_RESULTS.generation,
  items: [CHANNEL],
  next: null,
} as const satisfies Page<typeof CHANNEL>;

describe("BoardSearch", () => {
  it("debounces the trimmed request term without leaving padded input scanning", async () => {
    const search = vi.fn(async (input: SearchInput) => {
      void input;
      return success(CHANNEL_RESULTS);
    });
    renderSearch(searchClient(search), CHANNEL_RESULTS.generation);

    fireEvent.change(
      screen.getByRole("combobox", {
        name: "Search channels and programmes",
      }),
      { target: { value: "  news  " } },
    );

    expect(search).not.toHaveBeenCalled();

    expect(await screen.findByText("World News")).toBeVisible();
    expect(search).toHaveBeenCalledTimes(1);
    expect(search.mock.calls[0]?.[0]).toMatchObject({ term: "news" });
    expect(screen.queryByText("Searching…")).not.toBeInTheDocument();
  });

  it("returns focus to the input when the clear control disappears", async () => {
    const client = searchClient(async () => success(CHANNEL_RESULTS));
    const view = renderSearch(client, CHANNEL_RESULTS.generation);
    const user = userEvent.setup();
    const input = screen.getByRole("combobox", {
      name: "Search channels and programmes",
    });

    await user.type(input, "news");
    const clearButton = view.container.querySelector<HTMLButtonElement>(
      'button[aria-label="Clear search"]',
    );
    if (clearButton === null) {
      throw new Error("expected the search clear control");
    }
    await user.click(clearButton);

    await waitFor(() => expect(input).toHaveFocus());
    expect(input).toHaveValue("");
  });

  it("reports an oversized term without starting a disabled query", async () => {
    const search = vi.fn(async () => success(CHANNEL_RESULTS));
    renderSearch(searchClient(search), CHANNEL_RESULTS.generation);

    fireEvent.change(
      screen.getByRole("combobox", {
        name: "Search channels and programmes",
      }),
      { target: { value: "x".repeat(257) } },
    );

    expect(
      await screen.findByText("That search is too long. Shorten it and try again."),
    ).toBeVisible();
    expect(screen.queryByText("Searching…")).not.toBeInTheDocument();
    expect(search).not.toHaveBeenCalled();
  });

  it("explains when catalog search is not ready", async () => {
    const search = vi.fn(async () => success(CHANNEL_RESULTS));
    renderSearch(searchClient(search), null);

    fireEvent.change(
      screen.getByRole("combobox", {
        name: "Search channels and programmes",
      }),
      { target: { value: "news" } },
    );

    expect(
      await screen.findByText("Search is ready once the channels have loaded."),
    ).toBeVisible();
    expect(search).not.toHaveBeenCalled();
  });

  it("does not offer results from a different catalog generation", async () => {
    const client = searchClient(async () => success(REPLACEMENT_RESULTS));
    renderSearch(client, CHANNEL_RESULTS.generation);

    await userEvent.setup().type(
      screen.getByRole("combobox", {
        name: "Search channels and programmes",
      }),
      "news",
    );

    expect(
      await screen.findByText("The channels changed while you searched."),
    ).toBeVisible();
    expect(screen.queryByText("World News")).not.toBeInTheDocument();
  });

  it("delegates a generation mismatch to catalog reconciliation", async () => {
    const search = vi.fn(async () => success(REPLACEMENT_RESULTS));
    const onGenerationMismatch = vi.fn();
    renderSearch(
      searchClient(search),
      CHANNEL_RESULTS.generation,
      vi.fn(),
      onGenerationMismatch,
    );
    const user = userEvent.setup();

    await user.type(
      screen.getByRole("combobox", {
        name: "Search channels and programmes",
      }),
      "news",
    );
    await user.click(await screen.findByRole("button", { name: "Search again" }));

    expect(onGenerationMismatch).toHaveBeenCalledTimes(1);
    expect(search).toHaveBeenCalledTimes(1);
  });

  it("tunes a Channel result without an extra detail lookup", async () => {
    const client = searchClient(async () => success(CHANNEL_RESULTS));
    const onTune = vi.fn();
    renderSearch(client, CHANNEL_RESULTS.generation, onTune);
    const user = userEvent.setup();

    await user.type(
      screen.getByRole("combobox", { name: "Search channels and programmes" }),
      "news",
    );
    await user.click(await screen.findByRole("option", { name: /World News/ }));

    expect(onTune).toHaveBeenCalledWith(CHANNEL);
  });

  it("shows a Quality Variant hit by Channel Number, base name and quality", async () => {
    const variant = channelFixture({
      id: "svt1-fhd",
      name: "SVT1 FHD",
      group: "Sweden",
      number: 101,
      variant: { quality: "fhd", baseName: "SVT1" },
    });
    const page = { generation: CHANNEL_RESULTS.generation, items: [variant], next: null };
    renderSearch(
      searchClient(
        async () =>
          success(
            clientSchemas.searchResults.parse({
              generation: 7,
              channels: page,
              programmes: { generation: 7, items: [], next: null },
            }),
          ),
        async () => success(page),
      ),
      CHANNEL_RESULTS.generation,
    );
    const user = userEvent.setup();

    await user.type(
      screen.getByRole("combobox", { name: "Search channels and programmes" }),
      "svt",
    );
    expect(
      await screen.findByRole("option", { name: /SVT1/ }),
    ).toHaveTextContent(/^101SVT1FHDSweden$/u);

    await user.keyboard("{Enter}");
    expect(
      await screen.findByRole("button", { name: "Tune SVT1 FHD" }),
    ).toHaveTextContent(/^101SVT1FHDSweden$/u);
  });

  it("opens the Channel search desk from Enter and the first dropdown option", async () => {
    const searchChannels = vi.fn(async (input: SearchPageInput) => {
      void input;
      return success(CHANNEL_PAGE);
    });
    renderSearch(
      searchClient(async () => success(CHANNEL_RESULTS), searchChannels),
      CHANNEL_RESULTS.generation,
    );
    const user = userEvent.setup();

    await user.type(
      screen.getByRole("combobox", { name: "Search channels and programmes" }),
      "news",
    );
    expect(
      await screen.findByRole("option", { name: /Open full channel search/ }),
    ).toBeVisible();
    await user.keyboard("{Enter}");

    expect(
      await screen.findByRole("heading", { name: "Find a channel" }),
    ).toBeVisible();
    expect(screen.getByRole("searchbox", { name: "Search channels" })).toHaveValue(
      "news",
    );
    expect(
      await screen.findByRole("button", { name: "Tune World News" }),
    ).toBeVisible();
    expect(searchChannels).toHaveBeenCalledWith(
      expect.objectContaining({ term: "news" }),
    );
  });

  it("tunes the first Channel hit from the keyboard after skipping the desk option", async () => {
    const client = searchClient(async () => success(CHANNEL_RESULTS));
    const onTune = vi.fn();
    renderSearch(client, CHANNEL_RESULTS.generation, onTune);
    const user = userEvent.setup();

    await user.type(
      screen.getByRole("combobox", { name: "Search channels and programmes" }),
      "news",
    );
    expect(await screen.findByText("World News")).toBeVisible();
    await user.keyboard("{ArrowDown}{Enter}");

    expect(onTune).toHaveBeenCalledWith(CHANNEL);
  });

  it("tunes the Channel of a Programme search hit", async () => {
    const client = searchClient(async () => success(PROGRAMME_RESULTS));
    const onTune = vi.fn();
    renderSearch(client, PROGRAMME_RESULTS.generation, onTune);

    await userEvent
      .setup()
      .type(
        screen.getByRole("combobox", {
          name: "Search channels and programmes",
        }),
        "studio",
      );
    await userEvent
      .setup()
      .click(await screen.findByRole("option", { name: /Evening Studio/ }));

    expect(onTune).toHaveBeenCalledWith(CHANNEL);
  });

  it("keeps dropdown hits inside non-excluded groups", async () => {
    const client = searchClient(async () => success(MIXED_CHANNEL_RESULTS));
    renderSearch(
      client,
      MIXED_CHANNEL_RESULTS.generation,
      vi.fn(),
      vi.fn(),
      new Set(["News"]),
    );

    await userEvent
      .setup()
      .type(
        screen.getByRole("combobox", {
          name: "Search channels and programmes",
        }),
        "one",
      );

    expect(await screen.findByText("Cinema One")).toBeVisible();
    expect(screen.queryByText("World News")).not.toBeInTheDocument();
  });

  it("can include excluded groups from the Channel search desk", async () => {
    const onTune = vi.fn();
    renderSearch(
      searchClient(async () => success(CHANNEL_RESULTS)),
      CHANNEL_RESULTS.generation,
      onTune,
      vi.fn(),
      new Set(["News"]),
    );
    const user = userEvent.setup();

    await user.type(
      screen.getByRole("combobox", { name: "Search channels and programmes" }),
      "news",
    );
    expect(
      await screen.findByText("The only matches are in hidden groups."),
    ).toBeVisible();
    expect(screen.queryByText("World News")).not.toBeInTheDocument();

    await user.click(
      await screen.findByRole("option", { name: /Open full channel search/ }),
    );
    expect(
      await screen.findByText(
        "The only matches are in hidden groups. Choose Include hidden to see them.",
      ),
    ).toBeVisible();

    await user.click(
      screen.getByRole("button", { name: "Include hidden" }),
    );
    await user.click(
      await screen.findByRole("button", { name: "Tune World News" }),
    );

    expect(onTune).toHaveBeenCalledWith(CHANNEL);
  });
});

function searchClient(
  search: Pick<SparrowClient, "search">["search"],
  searchChannels: Pick<SparrowClient, "searchChannels">["searchChannels"] = async () =>
    success(CHANNEL_PAGE),
): Pick<SparrowClient, "search" | "searchChannels"> {
  return { search, searchChannels };
}

function renderSearch(
  client: Pick<SparrowClient, "search" | "searchChannels">,
  generation: CatalogGeneration | null,
  onTune = vi.fn(),
  onGenerationMismatch = vi.fn(),
  excludedGroups: ReadonlySet<string> = new Set(),
) {
  return render(
    searchTree(
      client,
      generation,
      onTune,
      onGenerationMismatch,
      excludedGroups,
    ),
  );
}

function searchTree(
  client: Pick<SparrowClient, "search" | "searchChannels">,
  generation: CatalogGeneration | null,
  onTune: () => void,
  onGenerationMismatch: () => void,
  excludedGroups: ReadonlySet<string>,
): ReactElement {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return (
    <QueryClientProvider client={queryClient}>
      <BoardSearch
        client={client}
        generation={generation}
        excludedGroups={excludedGroups}
        onGenerationMismatch={onGenerationMismatch}
        onPreparePlayback={() => undefined}
        onTune={onTune}
      />
    </QueryClientProvider>
  );
}

function success<Value>(value: Value): ClientResult<Value> {
  return { ok: true, value };
}
