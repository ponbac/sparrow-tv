import { Autocomplete } from "@base-ui/react/autocomplete";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Search, X } from "lucide-react";
import { useCallback, useMemo, useState } from "react";
import type {
  CatalogGeneration,
  ChannelSummary,
  ProgrammeSearchHit,
  SparrowClient,
} from "../../client/contracts";
import {
  clientErrorFromQuery,
  generationBoundResult,
} from "../../client/query-result";
import { useDebounce } from "../../hooks/useDebounce";
import { groupDisplayName } from "./board-group-roster";
import { BoardSearchDesk } from "./board-search-desk";
import {
  visibleSearchChannels,
  visibleSearchProgrammes,
} from "./board-search-scope";
import {
  canonicalSearchTerm,
  SEARCH_DEBOUNCE_MS,
  searchTermFits,
} from "./board-search-term";
import { channelTitle, qualityLabel } from "./guide-families";
import { clockLabel } from "./guide-window";
import "./board-search.css";

const SEARCH_RESULT_LIMIT = 8;
const SEARCH_FETCH_LIMIT = 40;
const DESK_CHOICE_LABEL = "Open full channel search";

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
  readonly onTune: (channel: ChannelSummary) => void;
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
  const [searchBoundary, setSearchBoundary] = useState<HTMLElement | null>(null);
  const bindSearchBoundary = useCallback((element: HTMLElement | null) => {
    // In the pocket layout the results must stay inside the guide pane,
    // below the native picture.
    setSearchBoundary(
      element?.closest<HTMLElement>(".programme-guide") ??
        element?.closest<HTMLElement>(".shell") ??
        null,
    );
  }, []);
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
    onTune(choiceChannel(choice));
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
        <Autocomplete.InputGroup
          ref={bindSearchBoundary}
          className="board-search"
          data-acceptance-search
        >
          <Search aria-hidden="true" />
          <Autocomplete.Input
            aria-label="Search channels and programmes"
            placeholder="Search channels and programmes"
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
            collisionBoundary={searchBoundary ?? undefined}
          >
            <Autocomplete.Popup className="board-search__popup">
              {presentation === "invalid" ? (
                <p className="board-search__state" role="alert">
                  That search is too long. Shorten it and try again.
                </p>
              ) : presentation === "unavailable" ? (
                <p className="board-search__state" role="status">
                  Search is ready once the channels have loaded.
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
                          <span className="board-search__number">
                            <Search aria-hidden="true" />
                          </span>
                          <span className="board-search__name">
                            <strong>{DESK_CHOICE_LABEL}</strong>
                          </span>
                          <small>All matches</small>
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
                          <span className="board-search__number">
                            {choiceChannel(choice).number}
                          </span>
                          {choice._tag === "channel" ? (
                            <span className="board-search__name">
                              <strong>{channelTitle(choice.channel)}</strong>
                              {choice.channel.variant === null ? null : (
                                <span className="board-search__quality">
                                  {qualityLabel(choice.channel.variant.quality)}
                                </span>
                              )}
                            </span>
                          ) : (
                            <span className="board-search__name">
                              <strong>{choice.programme.title}</strong>
                            </span>
                          )}
                          <small>{choiceDetail(choice)}</small>
                        </Autocomplete.Item>
                      ),
                    )}
                  </Autocomplete.List>
                  {presentation === "loading" ? (
                    <p className="board-search__state">Searching…</p>
                  ) : presentation === "generation-mismatch" ? (
                    <div className="board-search__state" role="alert">
                      The channels changed while you searched.
                      <button type="button" onClick={onGenerationMismatch}>
                        Search again
                      </button>
                    </div>
                  ) : presentation === "error" ? (
                    <p className="board-search__state" role="alert">
                      Search is not available right now. Try again in a moment.
                    </p>
                  ) : presentation === "hidden" ? (
                    <p className="board-search__state">
                      The only matches are in hidden groups.
                    </p>
                  ) : presentation === "empty" ? (
                    <p className="board-search__state">
                      Nothing matches that search.
                    </p>
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
        onTune={(channel) => {
          onTune(channel);
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
    return DESK_CHOICE_LABEL;
  }
  return choice._tag === "channel"
    ? choice.channel.name
    : choice.programme.title;
}

/** The Channel a hit tunes: itself, or the one airing the Programme. */
function choiceChannel(
  choice: Exclude<SearchChoice, { readonly _tag: "desk" }>,
): ChannelSummary {
  return choice._tag === "channel" ? choice.channel : choice.programme.channel;
}

function choiceDetail(
  choice: Exclude<SearchChoice, { readonly _tag: "desk" }>,
): string {
  if (choice._tag === "channel") {
    return groupDisplayName(choice.channel.group);
  }
  return `${choice.programme.channel.name}, ${clockLabel(choice.programme.startsAt)} to ${clockLabel(choice.programme.endsAt)}`;
}
