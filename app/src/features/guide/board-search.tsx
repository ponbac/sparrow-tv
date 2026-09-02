import { Autocomplete } from "@base-ui/react/autocomplete";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Search, X } from "lucide-react";
import { useMemo, useState } from "react";
import type {
  CatalogGeneration,
  ChannelSummary,
  ProgrammeSearchHit,
  ProgrammeSlot,
  SparrowClient,
} from "../../client/contracts";
import {
  clientErrorFromQuery,
  generationBoundResult,
} from "../../client/query-result";
import { useDebounce } from "../../hooks/useDebounce";
import { BoardSearchDesk } from "./board-search-desk";
import {
  visibleSearchChannels,
  visibleSearchProgrammes,
} from "./board-search-scope";
import {
  canonicalSearchTerm,
  MAX_SEARCH_TERM_BYTES,
  SEARCH_DEBOUNCE_MS,
  searchTermFits,
} from "./board-search-term";
import { clockLabel } from "./guide-window";
import "./board-search.css";

const SEARCH_RESULT_LIMIT = 8;
const SEARCH_FETCH_LIMIT = 40;

type SearchChoice =
  | { readonly _tag: "desk" }
  | { readonly _tag: "channel"; readonly channel: ChannelSummary }
  | { readonly _tag: "programme"; readonly programme: ProgrammeSearchHit };

type SearchPresentation =
  | "invalid"
  | "unavailable"
  | "loading"
  | "generation-mismatch"
  | "error"
  | "hidden"
  | "empty"
  | "ready";

/** Inputs for the asynchronous Channel and Programme board search. */
export interface BoardSearchProps {
  readonly client: Pick<SparrowClient, "search" | "searchChannels">;
  readonly generation: CatalogGeneration | null;
  readonly excludedGroups: ReadonlySet<string>;
  readonly onGenerationMismatch: () => void;
  readonly onPreparePlayback: () => void;
  readonly onTune: (
    channel: ChannelSummary,
    programme: ProgrammeSlot | null,
  ) => void;
}

/** Searches the complete catalog while keeping results inside the guide pane. */
export function BoardSearch({
  client,
  generation,
  excludedGroups,
  onGenerationMismatch,
  onPreparePlayback,
  onTune,
}: BoardSearchProps) {
  const queryClient = useQueryClient();
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const [deskOpen, setDeskOpen] = useState(false);
  const requestTerm = canonicalSearchTerm(query);
  const debouncedQuery = useDebounce(requestTerm, SEARCH_DEBOUNCE_MS);
  const cachedResult = queryClient.getQueryData([
    "catalog",
    "search",
    "board",
    requestTerm,
    generation,
    SEARCH_FETCH_LIMIT,
  ]);
  const searchTerm = cachedResult === undefined ? debouncedQuery : requestTerm;
  const requestValid = searchTermFits(query.trim());
  const queryValid = searchTermFits(searchTerm);
  const searchQuery = useQuery({
    queryKey: [
      "catalog",
      "search",
      "board",
      searchTerm,
      generation,
      SEARCH_FETCH_LIMIT,
    ],
    queryFn: ({ signal }) =>
      generationBoundResult(
        client.search({
          term: searchTerm,
          channelLimit: SEARCH_FETCH_LIMIT,
          programmeLimit: SEARCH_FETCH_LIMIT,
          signal,
        }),
        generation,
      ),
    enabled:
      searchTerm.length > 0 &&
      requestValid &&
      queryValid &&
      generation !== null,
    retry: false,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const result = searchQuery.data?.ok === true ? searchQuery.data.value : null;
  const visibleChannels = useMemo(
    () =>
      result === null
        ? []
        : visibleSearchChannels(result.channels.items, excludedGroups, false),
    [excludedGroups, result],
  );
  const visibleProgrammes = useMemo(
    () =>
      result === null
        ? []
        : visibleSearchProgrammes(
            result.programmes.items,
            excludedGroups,
            false,
          ),
    [excludedGroups, result],
  );
  const hiddenCount =
    result === null
      ? 0
      : result.channels.items.length -
        visibleChannels.length +
        (result.programmes.items.length - visibleProgrammes.length);
  const choices = useMemo<readonly SearchChoice[]>(() => {
    const hits: SearchChoice[] = [
      ...visibleChannels.slice(0, SEARCH_RESULT_LIMIT).map(
        (channel): SearchChoice => ({
          _tag: "channel",
          channel,
        }),
      ),
      ...visibleProgrammes.slice(0, SEARCH_RESULT_LIMIT).map(
        (programme): SearchChoice => ({
          _tag: "programme",
          programme,
        }),
      ),
    ];
    if (requestTerm.length === 0 || !requestValid) {
      return hits;
    }
    return [{ _tag: "desk" }, ...hits];
  }, [requestTerm.length, requestValid, visibleChannels, visibleProgrammes]);
  const error = clientErrorFromQuery(searchQuery.error);
  const generationMismatch = error?._tag === "stale-cursor";
  const presentation = searchPresentation({
    requestValid,
    generationAvailable: generation !== null,
    waitingForDebounce: requestTerm !== searchTerm,
    fetching: searchQuery.isFetching,
    hasResult: result !== null,
    generationMismatch,
    failed: error !== null,
    hasChoices: visibleChannels.length + visibleProgrammes.length > 0,
    hasHidden: hiddenCount > 0,
  });

  const clear = () => {
    setQuery("");
    setOpen(false);
    setDeskOpen(false);
  };
  const openDesk = () => {
    setOpen(false);
    setDeskOpen(true);
  };
  const prepareChoice = () => {
    onPreparePlayback();
  };
  const choose = (choice: SearchChoice) => {
    if (choice._tag === "desk") {
      openDesk();
      return;
    }
    onPreparePlayback();
    if (choice._tag === "channel") {
      onTune(choice.channel, null);
      clear();
      return;
    }

    onTune(choice.programme.channel, choice.programme);
    clear();
  };

  return (
    <>
      <Autocomplete.Root
        items={choices}
        mode="none"
        value={query}
        open={open && requestTerm.length > 0 && !deskOpen}
        onValueChange={(value, details) => {
          if (details.reason === "item-press") {
            return;
          }
          setQuery(value);
          setOpen(value.trim().length > 0);
        }}
        onOpenChange={setOpen}
        itemToStringValue={choiceLabel}
        autoHighlight="always"
        openOnInputClick
        modal={false}
      >
        <Autocomplete.InputGroup className="board-search" data-acceptance-search>
          <Search aria-hidden="true" />
          <Autocomplete.Input
            aria-label="Search Channels and Programmes"
            placeholder="Search the board"
            autoComplete="off"
            spellCheck={false}
          />
          <Autocomplete.Clear aria-label="Clear search">
            <X aria-hidden="true" />
          </Autocomplete.Clear>
        </Autocomplete.InputGroup>

        <Autocomplete.Portal>
          <Autocomplete.Positioner
            className="board-search__positioner"
            align="start"
            sideOffset={7}
          >
            <Autocomplete.Popup className="board-search__popup">
              {presentation === "invalid" ? (
                <p className="board-search__state" role="alert">
                  Keep the search within {MAX_SEARCH_TERM_BYTES} UTF-8 bytes.
                </p>
              ) : presentation === "unavailable" ? (
                <p className="board-search__state" role="status">
                  Search opens after a catalog is ready.
                </p>
              ) : (
                <>
                  <Autocomplete.List className="board-search__results">
                    {choices.map((choice, index) =>
                      choice._tag === "desk" ? (
                        <Autocomplete.Item
                          className="board-search__result board-search__result--desk"
                          key="desk"
                          index={index}
                          value={choice}
                          onClick={() => choose(choice)}
                        >
                          <span>Search</span>
                          <strong>Open full Channel search</strong>
                          <small>Full list</small>
                        </Autocomplete.Item>
                      ) : (
                        <Autocomplete.Item
                          className="board-search__result"
                          key={choiceKey(choice, index)}
                          index={index}
                          value={choice}
                          onMouseEnter={prepareChoice}
                          onFocus={prepareChoice}
                          onClick={() => choose(choice)}
                        >
                          <span>
                            {choice._tag === "channel"
                              ? "Channel"
                              : "Programme"}
                          </span>
                          <strong>{choiceLabel(choice)}</strong>
                          <small>{choiceDetail(choice)}</small>
                        </Autocomplete.Item>
                      ),
                    )}
                  </Autocomplete.List>
                  {presentation === "loading" ? (
                    <p className="board-search__state">Scanning the catalog…</p>
                  ) : presentation === "generation-mismatch" ? (
                    <div className="board-search__state" role="alert">
                      The catalog changed while searching.
                      <button type="button" onClick={onGenerationMismatch}>
                        Rescan
                      </button>
                    </div>
                  ) : presentation === "error" ? (
                    <p className="board-search__state" role="alert">
                      Search is temporarily unavailable.
                    </p>
                  ) : presentation === "hidden" ? (
                    <p className="board-search__state">
                      Matching signals are in excluded groups.
                    </p>
                  ) : presentation === "empty" ? (
                    <p className="board-search__state">No matching signals.</p>
                  ) : null}
                </>
              )}
            </Autocomplete.Popup>
          </Autocomplete.Positioner>
        </Autocomplete.Portal>
      </Autocomplete.Root>
      <BoardSearchDesk
        client={client}
        generation={generation}
        term={query}
        excludedGroups={excludedGroups}
        open={deskOpen}
        onOpenChange={setDeskOpen}
        onTermChange={setQuery}
        onGenerationMismatch={onGenerationMismatch}
        onPreparePlayback={onPreparePlayback}
        onTune={(channel, programme) => {
          onTune(channel, programme);
          clear();
        }}
      />
    </>
  );
}

function searchPresentation({
  requestValid,
  generationAvailable,
  waitingForDebounce,
  fetching,
  hasResult,
  generationMismatch,
  failed,
  hasChoices,
  hasHidden,
}: {
  readonly requestValid: boolean;
  readonly generationAvailable: boolean;
  readonly waitingForDebounce: boolean;
  readonly fetching: boolean;
  readonly hasResult: boolean;
  readonly generationMismatch: boolean;
  readonly failed: boolean;
  readonly hasChoices: boolean;
  readonly hasHidden: boolean;
}): SearchPresentation {
  if (!requestValid) {
    return "invalid";
  }
  if (!generationAvailable) {
    return "unavailable";
  }
  if (waitingForDebounce || (fetching && !hasResult)) {
    return "loading";
  }
  if (generationMismatch) {
    return "generation-mismatch";
  }
  if (failed) {
    return "error";
  }
  if (hasChoices) {
    return "ready";
  }
  return hasHidden ? "hidden" : "empty";
}

function choiceKey(choice: SearchChoice, occurrence: number): string {
  if (choice._tag === "desk") {
    return "desk";
  }
  return choice._tag === "channel"
    ? `channel:${choice.channel.id}:${occurrence}`
    : `programme:${choice.programme.channel.id}:${choice.programme.startsAt}:${choice.programme.endsAt}:${choice.programme.title}:${occurrence}`;
}

function choiceLabel(choice: SearchChoice): string {
  if (choice._tag === "desk") {
    return "Open full Channel search";
  }
  return choice._tag === "channel"
    ? choice.channel.name
    : choice.programme.title;
}

function choiceDetail(choice: SearchChoice): string {
  if (choice._tag === "desk") {
    return "Full list";
  }
  if (choice._tag === "channel") {
    return choice.channel.group === "" ? "Ungrouped" : choice.channel.group;
  }
  return `${clockLabel(choice.programme.startsAt)}–${clockLabel(choice.programme.endsAt)}`;
}
