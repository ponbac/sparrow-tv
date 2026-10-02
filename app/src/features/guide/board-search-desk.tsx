import { Dialog } from "@base-ui/react/dialog";
import { Search, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type {
  CatalogGeneration,
  ChannelSummary,
  SparrowClient,
} from "../../client/contracts";
import { useDebounce } from "../../hooks/useDebounce";
import { groupDisplayName } from "./board-group-roster";
import { channelTitle, qualityLabel } from "./guide-families";
import {
  shouldAdvancePastExcludedSearchHits,
  visibleSearchChannels,
} from "./board-search-scope";
import {
  canonicalSearchTerm,
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
  readonly onTune: (channel: ChannelSummary) => void;
}

/**
 * Full Channel search over the guide pane. Ranking stays catalog-wide; the
 * desk leaves out hidden Channel Groups until the viewer includes them.
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
  const closedByTune = useRef(false);
  useEffect(() => {
    if (open) closedByTune.current = false;
  }, [open]);
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
          // Stage takes focus on tune; restoring the search input here
          // would reopen Android's keyboard over the new Playback Session.
          finalFocus={() => !closedByTune.current}
        >
          <header className="board-search-desk__header">
            <div>
              <Dialog.Title>Find a channel</Dialog.Title>
              <Dialog.Description>
                Search every channel by name. Hidden groups are left out
                unless you include them.
              </Dialog.Description>
            </div>
            <Dialog.Close
              className="board-search-desk__close"
              aria-label="Close channel search"
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
                placeholder="Search channels"
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                aria-label="Search channels"
                onChange={(event) => onTermChange(event.target.value)}
              />
            </label>
            <p className="board-search-desk__tally">
              <b>{visibleChannels.length}</b> found
              {hiddenCount > 0 ? (
                <>
                  , <b>{hiddenCount}</b> hidden
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
                Include hidden
              </button>
            ) : null}
          </div>

          <div className="board-search-desk__list" role="list">
            {presentation === "invalid" ? (
              <p className="board-search-desk__state" role="alert">
                That search is too long. Shorten it and try again.
              </p>
            ) : presentation === "unavailable" ? (
              <p className="board-search-desk__state" role="status">
                Search is ready once the channels have loaded.
              </p>
            ) : presentation === "idle" ? (
              <p className="board-search-desk__state" role="status">
                Type a channel name to search.
              </p>
            ) : presentation === "loading" ? (
              <p className="board-search-desk__state">Searching…</p>
            ) : presentation === "generation-mismatch" ? (
              <div className="board-search-desk__state" role="alert">
                The channels changed while you searched.
                <button type="button" onClick={onGenerationMismatch}>
                  Search again
                </button>
              </div>
            ) : presentation === "error" ? (
              <div className="board-search-desk__state" role="alert">
                Search is not available right now.
                <button type="button" onClick={retry}>
                  Try again
                </button>
              </div>
            ) : presentation === "hidden" ? (
              <p className="board-search-desk__state" role="status">
                The only matches are in hidden groups. Choose Include hidden to
                see them.
              </p>
            ) : presentation === "empty" ? (
              <p className="board-search-desk__state" role="status">
                No channel matches that search.
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
                      onClick={() => {
                        closedByTune.current = true;
                        onTune(channel);
                      }}
                    >
                      <span className="board-search__number">
                        {channel.number}
                      </span>
                      <strong>{channelTitle(channel)}</strong>
                      {channel.variant === null ? null : (
                        <span className="board-search__quality">
                          {qualityLabel(channel.variant.quality)}
                        </span>
                      )}
                      {excluded ? (
                        <span className="board-search-desk__hidden">Hidden</span>
                      ) : null}
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
                {loadingMore ? "Loading more channels…" : "More channels"}
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
