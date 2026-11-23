import { Autocomplete } from "@base-ui/react/autocomplete";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { RotateCcw, Search, X } from "lucide-react";
import { useCallback, useInsertionEffect, useMemo, useRef, useState } from "react";
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
  foldSearchVariants,
  visibleSearchChannels,
  visibleSearchProgrammes,
} from "./board-search-scope";
import {
  canonicalSearchTerm,
  SEARCH_DEBOUNCE_MS,
  searchTermFits,
} from "./board-search-term";
import { useTelevisionInputEdit } from "./dialog-navigation";
import { revealInScrollContainer } from "../stage/stage-dom";
import { ChannelIcon } from "./channel-icon";
import { SearchQualities } from "./search-qualities";
import type { SearchOnNow } from "./search-on-now";
import {
  channelTitle,
  qualityLabel,
  type VariantPreferences,
} from "./guide-families";
import { focusGuideFallback } from "./guide-navigation";
import { clockLabel } from "./guide-window";
import "./board-search.css";

const SEARCH_RESULT_LIMIT = 8;
const SEARCH_FETCH_LIMIT = 40;
const DESK_CHOICE_LABEL = "Open full channel search";
const RETRY_CHOICE_LABEL = "Search again";

type SearchChoice =
  | { readonly _tag: "retry" }
  | { readonly _tag: "desk" }
  | {
      readonly _tag: "channel";
      /** The Channel the result tunes. */
      readonly channel: ChannelSummary;
      /**
       * On the TV, the Quality Variants of its guide row that matched,
       * folded into this one result; elsewhere the Channel alone.
       */
      readonly variants: readonly ChannelSummary[];
    }
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
  /** Uses press-Center-to-edit on Android TV so browsing across search keeps the keyboard closed. */
  readonly television?: boolean;
  readonly client: Pick<SparrowClient, "search" | "searchChannels">;
  readonly generation: CatalogGeneration | null;
  readonly excludedGroups: ReadonlySet<string>;
  /**
   * The viewer's chosen quality per guide row. The TV folds a guide row's
   * Quality Variants into one result that tunes the chosen or best one.
   */
  readonly variantPreferences?: VariantPreferences;
  readonly onGenerationMismatch: () => void;
  readonly onPreparePlayback: () => void;
  readonly onTune: (channel: ChannelSummary) => void;
  /**
   * The Programmes the Guide has already read, so the TV's Find a channel
   * sheet can say what is on for a hit without a read of its own.
   */
  readonly onNow?: SearchOnNow;
}

const NO_PREFERENCES: VariantPreferences = new Map();

/** Searches the complete catalog while keeping results inside the guide pane. */
export function BoardSearch({
  television = false,
  client,
  generation,
  excludedGroups,
  variantPreferences = NO_PREFERENCES,
  onGenerationMismatch,
  onPreparePlayback,
  onTune,
  onNow,
}: BoardSearchProps) {
  const queryClient = useQueryClient();
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const searchEdit = useTelevisionInputEdit(television);
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
  const error = clientErrorFromQuery(searchQuery.error);
  const generationMismatch = error?._tag === "stale-cursor";
  const choices = useMemo<readonly SearchChoice[]>(() => {
    const channelRows = television
      ? foldSearchVariants(visibleChannels, variantPreferences)
      : visibleChannels.map((channel) => ({ channel, variants: [channel] }));
    const hits: SearchChoice[] = [
      ...channelRows.slice(0, SEARCH_RESULT_LIMIT).map(
        (row): SearchChoice => ({
          _tag: "channel",
          channel: row.channel,
          variants: row.variants,
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
    if (generationMismatch) {
      return [{ _tag: "retry" }, { _tag: "desk" }];
    }
    return [{ _tag: "desk" }, ...hits];
  }, [
    generationMismatch,
    requestTerm.length,
    requestValid,
    television,
    variantPreferences,
    visibleChannels,
    visibleProgrammes,
  ]);
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
    if (choice._tag === "retry") {
      onGenerationMismatch();
      return;
    }
    if (choice._tag === "desk") {
      openDesk();
      return;
    }
    onPreparePlayback();
    onTune(choiceChannel(choice));
    clear();
  };
  const popupOpen = open && requestTerm.length > 0 && !deskOpen;
  // The TV dims the Guide under open results by a root attribute, not by
  // `html:has(input[aria-expanded])`, which restyles the page on every move.
  // Set before layout effects, as the input's own attribute was.
  useInsertionEffect(() => {
    if (!popupOpen) return;
    document.documentElement.setAttribute("data-search-expanded", "");
    return () => document.documentElement.removeAttribute("data-search-expanded");
  }, [popupOpen]);

  return (
    <>
      <Autocomplete.Root
        items={choices}
        mode="none"
        value={query}
        open={popupOpen}
        onValueChange={(value, details) => {
          if (details.reason === "item-press") {
            return;
          }
          setQuery(value);
          setOpen(value.trim().length > 0);
        }}
        onOpenChange={(nextOpen, details) => {
          if (
            !nextOpen &&
            details.reason === "item-press" &&
            generationMismatch
          ) {
            return;
          }
          setOpen(nextOpen);
        }}
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
            ref={inputRef}
            aria-label="Search channels and programmes"
            placeholder="Search channels and programmes"
            autoComplete="off"
            spellCheck={false}
            readOnly={searchEdit.readOnly}
            onClick={searchEdit.startEditing}
            onBlur={searchEdit.stopEditing}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) {
                event.preventBaseUIHandler();
                return;
              }
              if (
                television &&
                popupOpen &&
                event.key === "ArrowDown" &&
                !event.altKey &&
                !event.ctrlKey &&
                !event.metaKey
              ) {
                // On Android TV WebView, pressing Center (KEYCODE_DPAD_CENTER)
                // on a focused editable <input> re-opens the soft keyboard
                // instead of firing Enter. Move real DOM focus into the option
                // list so D-pad Center activates suggestions directly.
                const firstOption = listRef.current?.querySelector<HTMLElement>(
                  ".board-search__result",
                );
                if (firstOption != null) {
                  event.preventDefault();
                  event.stopPropagation();
                  event.preventBaseUIHandler();
                  searchEdit.stopEditing();
                  firstOption.focus({ preventScroll: true });
                  revealInScrollContainer(listRef.current, firstOption);
                  return;
                }
              }
              if (searchEdit.handleKeyDown(event, { popupOpen })) {
                event.preventBaseUIHandler();
                return;
              }
              if (television && event.key === "Escape" && !popupOpen) {
                event.preventDefault();
                event.stopPropagation();
                searchEdit.stopEditing();
                focusGuideFallback(
                  event.currentTarget.closest<HTMLElement>(".programme-guide"),
                  { includeNotice: true },
                );
              }
            }}
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
            <Autocomplete.Popup
              className="board-search__popup"
              onFocus={(event) => {
                if (television) {
                  event.preventBaseUIHandler();
                }
              }}
            >
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
                  <Autocomplete.List
                    ref={listRef}
                    className="board-search__results"
                    onKeyDown={(event) => {
                      if (
                        !television ||
                        event.defaultPrevented ||
                        event.nativeEvent.isComposing ||
                        event.altKey ||
                        event.ctrlKey ||
                        event.metaKey ||
                        !(event.target instanceof HTMLElement)
                      ) {
                        return;
                      }
                      const list = listRef.current;
                      const item = event.target.closest<HTMLElement>(
                        ".board-search__result",
                      );
                      if (list === null || item === null || !list.contains(item)) {
                        return;
                      }
                      if (event.key === "Escape") {
                        event.preventDefault();
                        event.stopPropagation();
                        setOpen(false);
                        searchEdit.stopEditing();
                        inputRef.current?.focus({ preventScroll: true });
                        return;
                      }
                      if (
                        event.key === "ArrowLeft" ||
                        event.key === "ArrowRight"
                      ) {
                        event.preventDefault();
                        event.stopPropagation();
                        return;
                      }
                      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                        event.preventDefault();
                        event.stopPropagation();
                        const items = Array.from(
                          list.querySelectorAll<HTMLElement>(
                            ".board-search__result",
                          ),
                        );
                        const idx = items.indexOf(item);
                        if (idx === -1) return;
                        if (event.key === "ArrowUp" && idx === 0) {
                          searchEdit.stopEditing();
                          inputRef.current?.focus({ preventScroll: true });
                          return;
                        }
                        const next =
                          items[event.key === "ArrowDown" ? idx + 1 : idx - 1];
                        if (next !== undefined) {
                          next.focus({ preventScroll: true });
                          revealInScrollContainer(list, next);
                        }
                        return;
                      }
                      if (
                        (event.key === "Enter" || event.key === " ") &&
                        !event.repeat
                      ) {
                        event.preventDefault();
                        event.stopPropagation();
                        const items = Array.from(
                          list.querySelectorAll<HTMLElement>(
                            ".board-search__result",
                          ),
                        );
                        const idx = items.indexOf(item);
                        const choice = idx >= 0 ? choices[idx] : undefined;
                        if (choice !== undefined) {
                          choose(choice);
                        }
                      }
                    }}
                  >
                    {choices.map((choice, index) =>
                      choice._tag === "retry" ? (
                        <Autocomplete.Item
                          className="board-search__result board-search__result--desk"
                          key="retry"
                          index={index}
                          value={choice}
                          tabIndex={television ? -1 : undefined}
                          onClick={() => choose(choice)}
                        >
                          <span className="board-search__number">
                            <RotateCcw aria-hidden="true" />
                          </span>
                          <span className="board-search__name">
                            <strong>{RETRY_CHOICE_LABEL}</strong>
                          </span>
                          <small>Reload channels</small>
                        </Autocomplete.Item>
                      ) : choice._tag === "desk" ? (
                        <Autocomplete.Item
                          className="board-search__result board-search__result--desk"
                          key="desk"
                          index={index}
                          value={choice}
                          tabIndex={television ? -1 : undefined}
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
                          tabIndex={television ? -1 : undefined}
                          onMouseEnter={prepareChoice}
                          onFocus={prepareChoice}
                          onClick={() => choose(choice)}
                        >
                          {television ? (
                            <ChannelIcon
                              className="board-search__icon"
                              icon={choiceChannel(choice).icon}
                              number={choiceChannel(choice).number}
                            />
                          ) : (
                            <span className="board-search__number">
                              {choiceChannel(choice).number}
                            </span>
                          )}
                          {choice._tag === "channel" && television ? (
                            <span className="board-search__name">
                              <span className="board-search__channel-number">
                                {choice.channel.number}
                              </span>
                              <strong>{channelTitle(choice.channel)}</strong>
                              <SearchQualities
                                variants={choice.variants}
                                current={choice.channel}
                              />
                            </span>
                          ) : choice._tag === "channel" ? (
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
                    <p className="board-search__state" role="alert">
                      The channels changed while you searched.
                    </p>
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
        television={television}
        client={client}
        generation={generation}
        term={query}
        excludedGroups={excludedGroups}
        variantPreferences={variantPreferences}
        onNow={onNow}
        open={deskOpen}
        onOpenChange={setDeskOpen}
        onTermChange={setQuery}
        onGenerationMismatch={onGenerationMismatch}
        onPreparePlayback={onPreparePlayback}
        returnFocusElement={() => inputRef.current}
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
  if (choice._tag === "retry") {
    return "retry";
  }
  if (choice._tag === "desk") {
    return "desk";
  }
  return choice._tag === "channel"
    ? `channel:${choice.channel.id}:${occurrence}`
    : `programme:${choice.programme.channel.id}:${choice.programme.startsAt}:${choice.programme.endsAt}:${choice.programme.title}:${occurrence}`;
}

function choiceLabel(choice: SearchChoice): string {
  if (choice._tag === "retry") {
    return RETRY_CHOICE_LABEL;
  }
  if (choice._tag === "desk") {
    return DESK_CHOICE_LABEL;
  }
  return choice._tag === "channel"
    ? choice.channel.name
    : choice.programme.title;
}

/** The Channel a hit tunes: itself, or the one airing the Programme. */
function choiceChannel(
  choice: Exclude<SearchChoice, { readonly _tag: "desk" } | { readonly _tag: "retry" }>,
): ChannelSummary {
  return choice._tag === "channel" ? choice.channel : choice.programme.channel;
}

function choiceDetail(
  choice: Exclude<SearchChoice, { readonly _tag: "desk" } | { readonly _tag: "retry" }>,
): string {
  if (choice._tag === "channel") {
    return groupDisplayName(choice.channel.group);
  }
  return `${choice.programme.channel.name}, ${clockLabel(choice.programme.startsAt)} to ${clockLabel(choice.programme.endsAt)}`;
}
