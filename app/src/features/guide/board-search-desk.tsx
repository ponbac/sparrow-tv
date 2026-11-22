import { Dialog } from "@base-ui/react/dialog";
import { Search, X } from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FocusEvent,
} from "react";
import type {
  CatalogGeneration,
  ChannelSummary,
  SparrowClient,
} from "../../client/contracts";
import { useDebounce } from "../../hooks/useDebounce";
import { groupDisplayName } from "./board-group-roster";
import {
  handleLinearDialogArrows,
  revealInScrollContainer,
  useMoreButtonFocusHandoff,
  useTelevisionInputEdit,
} from "./dialog-navigation";
import { ChannelIcon } from "./channel-icon";
import {
  channelTitle,
  qualityLabel,
  type VariantPreferences,
} from "./guide-families";
import {
  foldSearchVariants,
  shouldAdvancePastExcludedSearchHits,
  visibleSearchChannels,
  type SearchChannelRow,
} from "./board-search-scope";
import { SearchQualities } from "./search-qualities";
import { searchHitOnNow, type SearchOnNow } from "./search-on-now";
import { minutesLeft } from "./now-next";
import {
  canonicalSearchTerm,
  SEARCH_DEBOUNCE_MS,
  searchTermFits,
} from "./board-search-term";
import { useBoardChannelSearch } from "./use-board-channel-search";

/** Inputs for the dedicated Channel search desk over the guide pane. */
export interface BoardSearchDeskProps {
  /** Keeps Android TV D-pad focus and pagination predictable inside the sheet. */
  readonly television?: boolean;
  readonly client: Pick<SparrowClient, "searchChannels">;
  readonly generation: CatalogGeneration | null;
  readonly term: string;
  readonly excludedGroups: ReadonlySet<string>;
  /**
   * The viewer's chosen quality per guide row. The TV folds a guide row's
   * Quality Variants into one row that tunes the chosen or best one.
   */
  readonly variantPreferences?: VariantPreferences;
  /** What the Guide has read, so a TV hit can say what is on now. */
  readonly onNow?: SearchOnNow;
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly onTermChange: (term: string) => void;
  readonly onGenerationMismatch: () => void;
  readonly onPreparePlayback: () => void;
  /** Resolves the toolbar control to focus when the sheet closes without tuning. */
  readonly returnFocusElement?: () => HTMLElement | null;
  readonly onTune: (channel: ChannelSummary) => void;
}

const SEARCH_AUTO_LOAD_SCROLL_THRESHOLD_PX = 480;
const NO_PREFERENCES: VariantPreferences = new Map();
const SEARCH_AUTO_LOAD_FOCUS_TAIL_ROWS = 6;

/**
 * Full Channel search over the guide pane. Ranking stays catalog-wide; the
 * desk leaves out hidden Channel Groups until the viewer includes them.
 */
export function BoardSearchDesk({
  television = false,
  client,
  generation,
  term,
  excludedGroups,
  variantPreferences = NO_PREFERENCES,
  onNow,
  open,
  onOpenChange,
  onTermChange,
  onGenerationMismatch,
  onPreparePlayback,
  returnFocusElement,
  onTune,
}: BoardSearchDeskProps) {
  const [includeExcluded, setIncludeExcluded] = useState(false);
  const [listElement, setListElement] = useState<HTMLDivElement | null>(null);
  const popupRef = useRef<HTMLDivElement>(null);
  const termInput = useRef<HTMLInputElement>(null);
  const searchEdit = useTelevisionInputEdit(television);
  const closedByTune = useRef(false);
  const pendingInitialResultFocusRef = useRef(false);
  const autoLoadTriggeredForCountRef = useRef<number | null>(null);
  useEffect(() => {
    if (open) {
      closedByTune.current = false;
    } else {
      pendingInitialResultFocusRef.current = false;
    }
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
  // One row per guide row on the TV; one per Channel elsewhere.
  const rows: readonly SearchChannelRow[] = useMemo(
    () =>
      television
        ? foldSearchVariants(visibleChannels, variantPreferences)
        : visibleChannels.map((channel) => ({ channel, variants: [channel] })),
    [television, variantPreferences, visibleChannels],
  );
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

  const canAutoLoad =
    presentation === "ready" && hasMore && !loadingMore && error === null;
  const requestAutoLoad = useCallback(() => {
    if (
      !canAutoLoad ||
      autoLoadTriggeredForCountRef.current === channels.length
    ) {
      return;
    }
    autoLoadTriggeredForCountRef.current = channels.length;
    loadMore();
  }, [canAutoLoad, channels.length, loadMore]);

  useEffect(() => {
    if (
      autoLoadTriggeredForCountRef.current !== null &&
      (autoLoadTriggeredForCountRef.current !== channels.length || !canAutoLoad)
    ) {
      autoLoadTriggeredForCountRef.current = null;
    }
  }, [canAutoLoad, channels.length]);

  useEffect(() => {
    if (listElement === null || !canAutoLoad) {
      return;
    }
    const onScroll = () => {
      if (listElement.scrollTop <= 0) {
        return;
      }
      const threshold = Math.max(
        SEARCH_AUTO_LOAD_SCROLL_THRESHOLD_PX,
        listElement.clientHeight * 1.5,
      );
      if (
        listElement.scrollHeight -
          listElement.scrollTop -
          listElement.clientHeight <=
        threshold
      ) {
        requestAutoLoad();
      }
    };
    listElement.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      listElement.removeEventListener("scroll", onScroll);
    };
  }, [canAutoLoad, listElement, requestAutoLoad]);

  const onListFocus = useCallback(
    (event: FocusEvent<HTMLDivElement>) => {
      if (!canAutoLoad || !(event.target instanceof Element)) {
        return;
      }
      const row = event.target.closest(".board-search-desk__row");
      if (row === null) {
        return;
      }
      let followingRows = 0;
      let sibling = row.nextElementSibling;
      while (
        sibling !== null &&
        followingRows < SEARCH_AUTO_LOAD_FOCUS_TAIL_ROWS
      ) {
        if (sibling.classList.contains("board-search-desk__row")) {
          followingRows += 1;
        }
        sibling = sibling.nextElementSibling;
      }
      if (followingRows < SEARCH_AUTO_LOAD_FOCUS_TAIL_ROWS) {
        requestAutoLoad();
      }
    },
    [canAutoLoad, requestAutoLoad],
  );

  const moreButtonHandlers = useMoreButtonFocusHandoff({
    television,
    active: open,
    busy: loadingMore,
    hasError:
      presentation === "generation-mismatch" ||
      presentation === "error" ||
      error !== null,
    scopeKey: `${searchTerm}:${includeExcluded ? "all" : "visible"}`,
    rowCount: rows.length,
    canAutoLoad,
    onRequestAutoLoad: requestAutoLoad,
    onManualLoadMore: loadMore,
    resolveTarget: ({ existingRows, hasError, buttonConnected }) => {
      if (listElement === null) return null;
      const stateAction = listElement.querySelector<HTMLButtonElement>(
        ".board-search-desk__state button",
      );
      const picks = listElement.querySelectorAll<HTMLButtonElement>(
        ".board-search-desk__pick",
      );
      if (hasError) {
        return stateAction ?? picks[picks.length - 1];
      }
      if (picks[existingRows] !== undefined) {
        return picks[existingRows];
      }
      return buttonConnected ? null : (picks[picks.length - 1] ?? stateAction);
    },
    revealTarget: (target) => {
      revealInScrollContainer(listElement, target);
    },
  });

  useLayoutEffect(() => {
    if (
      !pendingInitialResultFocusRef.current ||
      !open ||
      !television ||
      presentation === "loading"
    ) {
      return;
    }
    pendingInitialResultFocusRef.current = false;
    const popup = popupRef.current;
    if (popup === null) return;
    const closeButton = popup.querySelector<HTMLElement>(
      ".board-search-desk__close",
    );
    if (closeButton === null || document.activeElement !== closeButton) {
      return;
    }
    const target =
      popup.querySelector<HTMLElement>(".board-search-desk__pick") ??
      popup.querySelector<HTMLElement>(".board-search-desk__state button");
    if (target !== null) {
      target.focus({ preventScroll: true });
      revealInScrollContainer(listElement, target);
    }
  }, [listElement, open, presentation, television]);

  return (
    <Dialog.Root
      open={open}
      onOpenChange={(nextOpen) => {
        if (!nextOpen) {
          setIncludeExcluded(false);
          searchEdit.stopEditing();
        }
        onOpenChange(nextOpen);
      }}
    >
      <Dialog.Portal>
        <Dialog.Backdrop className="board-search-desk__backdrop" />
        <Dialog.Popup
          ref={popupRef}
          className="board-search-desk__popup"
          initialFocus={() => {
            if (television && term.trim().length > 0) {
              const readyTarget =
                popupRef.current?.querySelector<HTMLElement>(
                  ".board-search-desk__pick",
                ) ??
                popupRef.current?.querySelector<HTMLElement>(
                  ".board-search-desk__state button",
                );
              if (readyTarget != null) {
                pendingInitialResultFocusRef.current = false;
                return readyTarget;
              }
              const closeButton =
                popupRef.current?.querySelector<HTMLElement>(
                  ".board-search-desk__close",
                );
              if (closeButton != null) {
                pendingInitialResultFocusRef.current =
                  presentation === "loading";
                return closeButton;
              }
              return termInput.current;
            }
            pendingInitialResultFocusRef.current = false;
            return termInput.current;
          }}
          // Stage takes focus on tune; restoring the search input here
          // would reopen Android's keyboard over the new Playback Session.
          finalFocus={() =>
            closedByTune.current ? false : (returnFocusElement?.() ?? true)
          }
          onKeyDown={(event) => {
            if (popupRef.current !== null) {
              handleLinearDialogArrows(event, {
                television,
                popup: popupRef.current,
                scroller: listElement,
                horizontalGroups: [".board-search-desk__toolbar"],
              });
            }
          }}
        >
          <header className="board-search-desk__header">
            <div>
              <Dialog.Title>Find a channel</Dialog.Title>
              <Dialog.Description>
                {television
                  ? "Search every channel by name. Hidden groups are left out."
                  : "Search every channel by name. Hidden groups are left out unless you include them."}
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
                readOnly={searchEdit.readOnly}
                aria-label="Search channels"
                onClick={searchEdit.startEditing}
                onBlur={searchEdit.stopEditing}
                onKeyDown={(event) => {
                  searchEdit.handleKeyDown(event);
                }}
                onChange={(event) => onTermChange(event.target.value)}
              />
            </label>
            <p className="board-search-desk__tally">
              <b>{rows.length}</b> found
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

          <div
            className="board-search-desk__list"
            ref={setListElement}
            role="list"
            onFocus={onListFocus}
          >
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
              rows.map(({ channel, variants }) => {
                const excluded = excludedGroups.has(channel.group);
                return (
                  <div
                    className="board-search-desk__row"
                    data-excluded={excluded}
                    key={variants[0].id}
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
                      {television ? (
                        <>
                          <ChannelIcon
                            className="board-search__icon"
                            icon={channel.icon}
                            number={channel.number}
                          />
                          <DeskHitBody
                            channel={channel}
                            variants={variants}
                            onNow={onNow}
                          />
                        </>
                      ) : (
                        <>
                          <span className="board-search__number">
                            {channel.number}
                          </span>
                          <strong>{channelTitle(channel)}</strong>
                          {channel.variant === null ? null : (
                            <span className="board-search__quality">
                              {qualityLabel(channel.variant.quality)}
                            </span>
                          )}
                        </>
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
            {television && presentation === "ready" && !hasMore ? (
              <p className="board-search-desk__end">
                That is every channel found. To search again, go up to the
                search field and press OK.
              </p>
            ) : null}
            {presentation === "ready" && hasMore ? (
              <button
                className="board-search-desk__more"
                type="button"
                aria-disabled={loadingMore || undefined}
                {...moreButtonHandlers}
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

/**
 * A TV hit's words: its number, name and qualities, and under them, quietly,
 * the Programme on now when the Guide has already read its row.
 */
function DeskHitBody({
  channel,
  variants,
  onNow,
}: {
  readonly channel: ChannelSummary;
  readonly variants: readonly ChannelSummary[];
  readonly onNow: SearchOnNow | undefined;
}) {
  const programme = searchHitOnNow(variants, onNow);
  return (
    <span className="board-search-desk__hit">
      <span className="board-search-desk__hit-name">
        <span className="board-search__channel-number">{channel.number}</span>
        <strong>{channelTitle(channel)}</strong>
        <SearchQualities variants={variants} current={channel} />
      </span>
      {programme === null || onNow === undefined ? null : (
        <span className="board-search-desk__on-now">
          <span className="board-search-desk__on-now-title">{programme.title}</span>
          <span className="board-search-desk__on-now-left">
            {minutesLeft(programme, onNow.now)} min left
          </span>
        </span>
      )}
    </span>
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
