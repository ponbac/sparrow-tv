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
import { clockLabel } from "./guide-window";
import "./board-search.css";

const MAX_SEARCH_TERM_BYTES = 256;
const SEARCH_RESULT_LIMIT = 8;
const SEARCH_DEBOUNCE_MS = 90;
const textEncoder = new TextEncoder();

type SearchChoice =
  | { readonly _tag: "channel"; readonly channel: ChannelSummary }
  | { readonly _tag: "programme"; readonly programme: ProgrammeSearchHit };

type SearchPresentation =
  | "invalid"
  | "unavailable"
  | "loading"
  | "generation-mismatch"
  | "error"
  | "empty"
  | "ready";

/** Inputs for the asynchronous Channel and Programme board search. */
export interface BoardSearchProps {
  readonly client: Pick<SparrowClient, "search">;
  readonly generation: CatalogGeneration | null;
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
  onGenerationMismatch,
  onPreparePlayback,
  onTune,
}: BoardSearchProps) {
  const queryClient = useQueryClient();
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const requestTerm = canonicalSearchTerm(query);
  const debouncedQuery = useDebounce(requestTerm, SEARCH_DEBOUNCE_MS);
  const cachedResult = queryClient.getQueryData([
    "catalog",
    "search",
    "board",
    requestTerm,
    generation,
  ]);
  const searchTerm = cachedResult === undefined ? debouncedQuery : requestTerm;
  const requestValid = searchTermFits(query.trim());
  const queryValid = searchTermFits(searchTerm);
  const searchQuery = useQuery({
    queryKey: ["catalog", "search", "board", searchTerm, generation],
    queryFn: ({ signal }) =>
      generationBoundResult(
        client.search({
          term: searchTerm,
          channelLimit: SEARCH_RESULT_LIMIT,
          programmeLimit: SEARCH_RESULT_LIMIT,
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
  const choices = useMemo<readonly SearchChoice[]>(
    () =>
      result === null
        ? []
        : [
            ...result.channels.items.map((channel): SearchChoice => ({
              _tag: "channel",
              channel,
            })),
            ...result.programmes.items.map((programme): SearchChoice => ({
              _tag: "programme",
              programme,
            })),
          ],
    [result],
  );
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
    hasChoices: choices.length > 0,
  });

  const clear = () => {
    setQuery("");
    setOpen(false);
  };
  const prepareChoice = () => {
    onPreparePlayback();
  };
  const choose = (choice: SearchChoice) => {
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
    <Autocomplete.Root
      items={choices}
      mode="none"
      value={query}
      open={open && requestTerm.length > 0}
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
            ) : presentation === "loading" ? (
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
            ) : presentation === "empty" ? (
              <p className="board-search__state">No matching signals.</p>
            ) : (
              <Autocomplete.List className="board-search__results">
                {choices.map((choice, index) => (
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
                      {choice._tag === "channel" ? "Channel" : "Programme"}
                    </span>
                    <strong>{choiceLabel(choice)}</strong>
                    <small>{choiceDetail(choice)}</small>
                  </Autocomplete.Item>
                ))}
              </Autocomplete.List>
            )}
          </Autocomplete.Popup>
        </Autocomplete.Positioner>
      </Autocomplete.Portal>
    </Autocomplete.Root>
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
}: {
  readonly requestValid: boolean;
  readonly generationAvailable: boolean;
  readonly waitingForDebounce: boolean;
  readonly fetching: boolean;
  readonly hasResult: boolean;
  readonly generationMismatch: boolean;
  readonly failed: boolean;
  readonly hasChoices: boolean;
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
  return hasChoices ? "ready" : "empty";
}

function searchTermFits(value: string): boolean {
  const canonical = canonicalSearchTerm(value);
  return (
    textEncoder.encode(value).byteLength <= MAX_SEARCH_TERM_BYTES &&
    textEncoder.encode(canonical).byteLength <= MAX_SEARCH_TERM_BYTES
  );
}

function canonicalSearchTerm(value: string): string {
  return value.normalize("NFKC").toLowerCase().trim().replace(/\s+/gu, " ");
}

function choiceKey(choice: SearchChoice, occurrence: number): string {
  return choice._tag === "channel"
    ? `channel:${choice.channel.id}:${occurrence}`
    : `programme:${choice.programme.channel.id}:${choice.programme.startsAt}:${choice.programme.endsAt}:${choice.programme.title}:${occurrence}`;
}

function choiceLabel(choice: SearchChoice): string {
  return choice._tag === "channel"
    ? choice.channel.name
    : choice.programme.title;
}

function choiceDetail(choice: SearchChoice): string {
  if (choice._tag === "channel") {
    return choice.channel.group === "" ? "Ungrouped" : choice.channel.group;
  }
  return `${clockLabel(choice.programme.startsAt)}–${clockLabel(choice.programme.endsAt)}`;
}
