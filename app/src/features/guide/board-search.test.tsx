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
  type SearchInput,
  type SparrowClient,
} from "../../client/contracts";
import { BoardSearch } from "./board-search";

afterEach(cleanup);

const CHANNEL = clientSchemas.channel.parse({
  id: "world-news",
  name: "World News",
  group: "News",
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

describe("BoardSearch", () => {
  it("debounces the trimmed request term without leaving padded input scanning", async () => {
    const search = vi.fn(async (input: SearchInput) => {
      void input;
      return success(CHANNEL_RESULTS);
    });
    const client = searchClient(search);
    renderSearch(client, CHANNEL_RESULTS.generation);

    await userEvent
      .setup()
      .type(
        screen.getByRole("combobox", {
          name: "Search Channels and Programmes",
        }),
        "  news  ",
      );

    expect(await screen.findByText("World News")).toBeVisible();
    expect(search).toHaveBeenCalledTimes(1);
    expect(search.mock.calls[0]?.[0]).toMatchObject({ term: "news" });
    expect(screen.queryByText("Scanning the catalog…")).not.toBeInTheDocument();
  });

  it("returns focus to the input when the clear control disappears", async () => {
    const client = searchClient(async () => success(CHANNEL_RESULTS));
    const view = renderSearch(client, CHANNEL_RESULTS.generation);
    const user = userEvent.setup();
    const input = screen.getByRole("combobox", {
      name: "Search Channels and Programmes",
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
        name: "Search Channels and Programmes",
      }),
      { target: { value: "x".repeat(257) } },
    );

    expect(
      await screen.findByText("Keep the search within 256 UTF-8 bytes."),
    ).toBeVisible();
    expect(screen.queryByText("Scanning the catalog…")).not.toBeInTheDocument();
    expect(search).not.toHaveBeenCalled();
  });

  it("explains when catalog search is not ready", async () => {
    const search = vi.fn(async () => success(CHANNEL_RESULTS));
    renderSearch(searchClient(search), null);

    fireEvent.change(
      screen.getByRole("combobox", {
        name: "Search Channels and Programmes",
      }),
      { target: { value: "news" } },
    );

    expect(
      await screen.findByText("Search opens after a catalog is ready."),
    ).toBeVisible();
    expect(search).not.toHaveBeenCalled();
  });

  it("does not offer results from a different catalog generation", async () => {
    const client = searchClient(async () => success(REPLACEMENT_RESULTS));
    renderSearch(client, CHANNEL_RESULTS.generation);

    await userEvent.setup().type(
      screen.getByRole("combobox", {
        name: "Search Channels and Programmes",
      }),
      "news",
    );

    expect(
      await screen.findByText("The catalog changed while searching."),
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
        name: "Search Channels and Programmes",
      }),
      "news",
    );
    await user.click(await screen.findByRole("button", { name: "Rescan" }));

    expect(onGenerationMismatch).toHaveBeenCalledTimes(1);
    expect(search).toHaveBeenCalledTimes(1);
  });

  it("tunes a Channel result without an extra detail lookup", async () => {
    const client = searchClient(async () => success(CHANNEL_RESULTS));
    const onTune = vi.fn();
    renderSearch(client, CHANNEL_RESULTS.generation, onTune);
    const user = userEvent.setup();

    await user.type(
      screen.getByRole("combobox", { name: "Search Channels and Programmes" }),
      "news",
    );
    await user.click(await screen.findByRole("option", { name: /World News/ }));

    expect(onTune).toHaveBeenCalledWith(CHANNEL, null);
  });

  it("tunes the highlighted result from the keyboard", async () => {
    const client = searchClient(async () => success(CHANNEL_RESULTS));
    const onTune = vi.fn();
    renderSearch(client, CHANNEL_RESULTS.generation, onTune);
    const user = userEvent.setup();

    await user.type(
      screen.getByRole("combobox", { name: "Search Channels and Programmes" }),
      "news",
    );
    expect(await screen.findByText("World News")).toBeVisible();
    await user.keyboard("{Enter}");

    expect(onTune).toHaveBeenCalledWith(CHANNEL, null);
  });

  it("tunes a Programme directly from its generation-bound search hit", async () => {
    const client = searchClient(async () => success(PROGRAMME_RESULTS));
    const onTune = vi.fn();
    renderSearch(client, PROGRAMME_RESULTS.generation, onTune);

    await userEvent
      .setup()
      .type(
        screen.getByRole("combobox", {
          name: "Search Channels and Programmes",
        }),
        "studio",
      );
    await userEvent
      .setup()
      .click(await screen.findByRole("option", { name: /Evening Studio/ }));

    expect(onTune).toHaveBeenCalledWith(CHANNEL, PROGRAMME_HIT);
  });
});

function searchClient(
  search: Pick<SparrowClient, "search">["search"],
): Pick<SparrowClient, "search"> {
  return { search };
}

function renderSearch(
  client: Pick<SparrowClient, "search">,
  generation: CatalogGeneration | null,
  onTune = vi.fn(),
  onGenerationMismatch = vi.fn(),
) {
  return render(searchTree(client, generation, onTune, onGenerationMismatch));
}

function searchTree(
  client: Pick<SparrowClient, "search">,
  generation: CatalogGeneration | null,
  onTune: () => void,
  onGenerationMismatch: () => void,
): ReactElement {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return (
    <QueryClientProvider client={queryClient}>
      <BoardSearch
        client={client}
        generation={generation}
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
