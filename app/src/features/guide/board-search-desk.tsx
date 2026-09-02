import { Dialog } from "@base-ui/react/dialog";
import { Search, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type {
  CatalogGeneration,
  ChannelSummary,
  ProgrammeSlot,
  SparrowClient,
} from "../../client/contracts";
import { useDebounce } from "../../hooks/useDebounce";
import { groupDisplayName } from "./board-group-roster";
import {
  shouldAdvancePastExcludedSearchHits,
  visibleSearchChannels,
} from "./board-search-scope";
import {
  canonicalSearchTerm,
  MAX_SEARCH_TERM_BYTES,
  SEARCH_DEBOUNCE_MS,
  searchTermFits,
} from "./board-search-term";
import { useBoardChannelSearch } from "./use-board-channel-search";

/** Inputs for the dedicated Channel search desk over the guide pane. */
export interface BoardSearchDeskProps {
  readonly client: Pick<SparrowClient, "searchChannels">;
  readonly generation: CatalogGeneration | null;
  readonly term: string;
  readonly excludedGroups: ReadonlySet<string>;
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly onTermChange: (term: string) => void;
  readonly onGenerationMismatch: () => void;
  readonly onPreparePlayback: () => void;
  readonly onTune: (
    channel: ChannelSummary,
    programme: ProgrammeSlot | null,
  ) => void;
}

/**
 * Full Channel search over the guide pane. Ranking stays catalog-wide; the
 * desk hides excluded Channel Groups until the operator includes them.
 */
export function BoardSearchDesk({
  client,
  generation,
  term,
  excludedGroups,
  open,
  onOpenChange,
  onTermChange,
  onGenerationMismatch,
  onPreparePlayback,
  onTune,
}: BoardSearchDeskProps) {
  const [includeExcluded, setIncludeExcluded] = useState(false);
  const termInput = useRef<HTMLInputElement>(null);
  const requestTerm = canonicalSearchTerm(term);
  const requestValid = searchTermFits(term);
  const debouncedTerm = useDebounce(requestTerm, SEARCH_DEBOUNCE_MS);
  const queryValid = searchTermFits(debouncedTerm);
  const searchTerm = requestValid ? debouncedTerm : requestTerm;
  const generationAvailable = generation !== null;
  const searchEnabled =
    open &&
    searchTerm.length > 0 &&
    requestValid &&
    queryValid &&
    generationAvailable;
  const {
    channels,
    loading,
    error,
    hasMore,
    loadingMore,
    retry,
    loadMore,
  } = useBoardChannelSearch({
    client,
    term: searchTerm,
    generation,
    enabled: searchEnabled,
  });
  const visibleChannels = visibleSearchChannels(
    channels,
    excludedGroups,
    includeExcluded,
  );
  const hiddenCount = Math.max(0, channels.length - visibleChannels.length);
  const generationMismatch = error?._tag === "stale-cursor";
  const waitingForDebounce = requestTerm !== searchTerm;
  const shouldAdvance =
    error === null &&
    shouldAdvancePastExcludedSearchHits({
      includeExcluded,
      excludedCount: excludedGroups.size,
      receivedCount: channels.length,
      visibleCount: visibleChannels.length,
      hasMore,
      loading: loading || loadingMore,
    });
  const presentation = deskPresentation({
    requestValid,
    generationAvailable,
    termReady: searchTerm.length > 0 || requestTerm.length > 0,
    waitingForDebounce,
    loading:
      loading ||
      (visibleChannels.length === 0 && (loadingMore || shouldAdvance)),
    generationMismatch,
    failed: error !== null,
    hasVisible: visibleChannels.length > 0,
    hasHidden: hiddenCount > 0,
  });

  useEffect(() => {
    if (!shouldAdvance) {
      return;
    }
    loadMore();
  }, [loadMore, shouldAdvance]);

  return (
    <Dialog.Root
      open={open}
      onOpenChange={(nextOpen) => {
        if (!nextOpen) {
          setIncludeExcluded(false);
        }
        onOpenChange(nextOpen);
      }}
    >
      <Dialog.Portal>
        <Dialog.Backdrop className="board-search-desk__backdrop" />
        <Dialog.Popup
          className="board-search-desk__popup"
          initialFocus={termInput}
        >
          <header className="board-search-desk__header">
            <div>
              <p>Board search</p>
              <Dialog.Title>Find a Channel</Dialog.Title>
              <Dialog.Description>
                Search Channels still on this desk. Include excluded groups
                when you need a dump you already hid.
              </Dialog.Description>
            </div>
            <Dialog.Close
              className="board-search-desk__close"
              aria-label="Close Channel search"
            >
              <X aria-hidden="true" />
            </Dialog.Close>
          </header>

          <div className="board-search-desk__toolbar">
            <label
              className="board-search-desk__search"
              htmlFor="board-search-desk-term"
            >
              <Search aria-hidden="true" />
              <input
                ref={termInput}
                id="board-search-desk-term"
                type="search"
                value={term}
                placeholder="Search Channels"
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                aria-label="Search Channels on the board"
                onChange={(event) => onTermChange(event.target.value)}
              />
            </label>
            <p className="board-search-desk__tally">
              <b>{visibleChannels.length}</b> on the board
              {hiddenCount > 0 ? (
                <>
                  <i aria-hidden="true" />
                  <b>{hiddenCount}</b> excluded
                </>
              ) : null}
            </p>
            {excludedGroups.size > 0 ? (
              <button
                className="board-search-desk__include"
                type="button"
                aria-pressed={includeExcluded}
                onClick={() => setIncludeExcluded((current) => !current)}
              >
                Include excluded
              </button>
            ) : null}
          </div>

          <div className="board-search-desk__list" role="list">
            {presentation === "invalid" ? (
              <p className="board-search-desk__state" role="alert">
                Keep the search within {MAX_SEARCH_TERM_BYTES} UTF-8 bytes.
              </p>
            ) : presentation === "unavailable" ? (
              <p className="board-search-desk__state" role="status">
                Search opens after a catalog is ready.
              </p>
            ) : presentation === "idle" ? (
              <p className="board-search-desk__state" role="status">
                Type a Channel name to scan the board.
              </p>
            ) : presentation === "loading" ? (
              <p className="board-search-desk__state">Scanning the catalog…</p>
            ) : presentation === "generation-mismatch" ? (
              <div className="board-search-desk__state" role="alert">
                The catalog changed while searching.
                <button type="button" onClick={onGenerationMismatch}>
                  Rescan
                </button>
              </div>
            ) : presentation === "error" ? (
              <div className="board-search-desk__state" role="alert">
                Search is temporarily unavailable.
                <button type="button" onClick={retry}>
                  Retry
                </button>
              </div>
            ) : presentation === "hidden" ? (
              <p className="board-search-desk__state" role="status">
                Matching Channels are in excluded groups.
              </p>
            ) : presentation === "empty" ? (
              <p className="board-search-desk__state" role="status">
                No matching Channels.
              </p>
            ) : (
              visibleChannels.map((channel) => {
                const excluded = excludedGroups.has(channel.group);
                return (
                  <div
                    className="board-search-desk__row"
                    data-excluded={excluded}
                    key={channel.id}
                    role="listitem"
                  >
                    <Dialog.Close
                      className="board-search-desk__pick"
                      type="button"
                      aria-label={`Tune ${channel.name}`}
                      onMouseEnter={onPreparePlayback}
                      onFocus={onPreparePlayback}
                      onClick={() => onTune(channel, null)}
                    >
                      <span>{excluded ? "Excluded" : "Channel"}</span>
                      <strong>{channel.name}</strong>
                      <small>{groupDisplayName(channel.group)}</small>
                    </Dialog.Close>
                  </div>
                );
              })
            )}
            {presentation === "ready" && hasMore ? (
              <button
                className="board-search-desk__more"
                type="button"
                disabled={loadingMore}
                onClick={loadMore}
              >
                {loadingMore ? "Opening more Channels…" : "More Channels"}
              </button>
            ) : null}
          </div>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

type DeskPresentation =
  | "invalid"
  | "unavailable"
  | "idle"
  | "loading"
  | "generation-mismatch"
  | "error"
  | "hidden"
  | "empty"
  | "ready";

function deskPresentation({
  requestValid,
  generationAvailable,
  termReady,
  waitingForDebounce,
  loading,
  generationMismatch,
  failed,
  hasVisible,
  hasHidden,
}: {
  readonly requestValid: boolean;
  readonly generationAvailable: boolean;
  readonly termReady: boolean;
  readonly waitingForDebounce: boolean;
  readonly loading: boolean;
  readonly generationMismatch: boolean;
  readonly failed: boolean;
  readonly hasVisible: boolean;
  readonly hasHidden: boolean;
}): DeskPresentation {
  if (!requestValid) {
    return "invalid";
  }
  if (!generationAvailable) {
    return "unavailable";
  }
  if (!termReady) {
    return "idle";
  }
  if (waitingForDebounce || (loading && !hasVisible)) {
    return "loading";
  }
  if (generationMismatch) {
    return "generation-mismatch";
  }
  if (failed) {
    return "error";
  }
  if (hasVisible) {
    return "ready";
  }
  return hasHidden ? "hidden" : "empty";
}
