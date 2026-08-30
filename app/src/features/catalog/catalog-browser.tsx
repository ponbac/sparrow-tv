import {
  skipToken,
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { lazy, Suspense, useState } from "react";
import type { HostedPlaybackEngine } from "../playback/mpegts-engine";
import { PlaybackLoadBoundary } from "../playback/playback-load-boundary";
import { SearchConsole } from "../search/search-console";
import type {
  CatalogGeneration,
  CatalogStatus,
  ChannelDetails,
  ChannelGroup,
  ChannelId,
  ChannelSummary,
  ClientError,
  ClientResult,
  Page,
  PageCursor,
  SourceState,
  SparrowClient,
} from "../../client/contracts";
import {
  clientErrorFromQuery,
  successfulQueryResult,
} from "../../client/query-result";
import { useCatalogSynchronization } from "../status/catalog-synchronization";
import { SourceStatusDesk } from "../status/source-status-desk";

const GROUP_PAGE_SIZE = 100;
const CHANNEL_PAGE_SIZE = 24;
const FIRST_PAGE: PageCursor | null = null;
const HostedPlayer = lazy(async () => {
  const module = await import("../playback/hosted-player");
  return { default: module.HostedPlayer };
});

interface CatalogBrowserProps {
  readonly client: SparrowClient;
  readonly playbackEngine?: HostedPlaybackEngine;
}

/** Browses generation-bound Channel Groups and Channels through a Sparrow client. */
export function CatalogBrowser({ client, playbackEngine }: CatalogBrowserProps) {
  const queryClient = useQueryClient();
  const synchronization = useCatalogSynchronization(client);
  const [activeGroup, setActiveGroup] = useState<string | null>(null);
  const [selectedChannel, setSelectedChannel] = useState<ChannelId | null>(null);
  const [playbackChannel, setPlaybackChannel] = useState<ChannelId | null>(null);

  const capabilitiesQuery = useQuery({
    queryKey: ["catalog", "capabilities"],
    queryFn: ({ signal }) =>
      successfulQueryResult(client.capabilities({ signal })),
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
  const catalogGeneration = synchronization.status?.generation ?? null;
  const authoritativeGeneration =
    synchronization.generationHint === undefined
      ? catalogGeneration
      : synchronization.generationHint;
  const groupsQuery = useInfiniteQuery({
    queryKey: ["catalog", "groups"],
    initialPageParam: FIRST_PAGE,
    queryFn: ({ pageParam, signal }) =>
      successfulQueryResult(
        client.listGroups({
          limit: GROUP_PAGE_SIZE,
          ...(pageParam === null ? {} : { cursor: pageParam }),
          signal,
        }),
      ),
    getNextPageParam: (lastPage) =>
      lastPage.ok ? lastPage.value.next : null,
    retry: false,
  });
  const channelsQuery = useInfiniteQuery({
    queryKey: ["catalog", "channels", activeGroup],
    initialPageParam: FIRST_PAGE,
    queryFn: ({ pageParam, signal }) =>
      successfulQueryResult(
        client.listChannels({
          limit: CHANNEL_PAGE_SIZE,
          ...(activeGroup === null ? {} : { group: activeGroup }),
          ...(pageParam === null ? {} : { cursor: pageParam }),
          signal,
        }),
      ),
    getNextPageParam: (lastPage) =>
      lastPage.ok ? lastPage.value.next : null,
    retry: false,
  });
  const channelQuery = useQuery({
    queryKey: ["catalog", "channel", selectedChannel],
    queryFn:
      selectedChannel === null
        ? skipToken
        : ({ signal }) =>
            successfulQueryResult(
              client.channel({ id: selectedChannel, signal }),
            ),
    retry: false,
  });

  const groups = collectItems(groupsQuery.data?.pages);
  const channels = collectItems(channelsQuery.data?.pages);
  const status = synchronization.status;
  const capabilities = successValue(capabilitiesQuery.data);
  const groupGeneration = firstPageGeneration(groupsQuery.data?.pages);
  const channelGeneration = firstPageGeneration(channelsQuery.data?.pages);
  const loadedGeneration =
    (channelGeneration !== authoritativeGeneration ? channelGeneration : null) ??
    (groupGeneration !== authoritativeGeneration ? groupGeneration : null) ??
    channelGeneration ??
    groupGeneration;
  const browseGenerationMismatch =
    authoritativeGeneration !== null &&
    ((groupGeneration !== null && groupGeneration !== authoritativeGeneration) ||
      (channelGeneration !== null &&
        channelGeneration !== authoritativeGeneration));
  const selectedDetails = resultWithQueryError(
    channelQuery.data,
    channelQuery.error,
  );
  const retainedDetailError =
    channelQuery.data?.ok === true
      ? clientErrorFromQuery(channelQuery.error)
      : null;
  const browseError =
    clientErrorFromQuery(capabilitiesQuery.error) ??
    synchronization.statusError ??
    clientErrorFromQuery(groupsQuery.error) ??
    clientErrorFromQuery(channelsQuery.error) ??
    retainedDetailError;
  const initialLoading =
    (capabilitiesQuery.isPending ||
      synchronization.statusPending ||
      groupsQuery.isPending ||
      channelsQuery.isPending) &&
    groups.length === 0 &&
    channels.length === 0 &&
    browseError === null;

  const selectGroup = (group: string | null) => {
    setActiveGroup(group);
    setSelectedChannel(null);
    setPlaybackChannel(null);
  };
  const selectChannel = (id: ChannelId) => {
    setSelectedChannel(id);
    setPlaybackChannel(id);
  };
  const retryCatalog = () => {
    queryClient
      .invalidateQueries({ queryKey: ["catalog"] })
      .catch(() => undefined);
  };
  const loadMoreGroups = () => {
    groupsQuery.fetchNextPage().catch(() => undefined);
  };
  const loadMoreChannels = () => {
    channelsQuery.fetchNextPage().catch(() => undefined);
  };
  const retrySelectedDetails = () => {
    channelQuery.refetch().catch(() => undefined);
  };

  if (initialLoading) {
    return <CatalogLoading />;
  }

  return (
    <div className="catalog-shell">
      <div className="scanlines" aria-hidden="true" />
      <header className="masthead">
        <div className="wordmark-lockup">
          <span className="signal-lamp" aria-hidden="true" />
          <div>
            <p className="eyebrow">Live signal index · hosted desk</p>
            <h1>SPARROW</h1>
          </div>
        </div>
        <div className="masthead-meta">
          <StatusReadout status={status} />
          <span className="transport-chip">
            {capabilities === null ? "LINK —" : "SAME ORIGIN"}
          </span>
        </div>
      </header>

      {status !== null &&
      (isRetainedCatalog(status) || browseGenerationMismatch) ? (
        <aside className="retained-banner" role="status">
          <span>RECORDED SIGNAL</span>
          <p>
            {retainedCatalogCopy(
              status,
              loadedGeneration,
              authoritativeGeneration,
            )}
          </p>
        </aside>
      ) : null}

      {browseError !== null ? (
        <ErrorNotice error={browseError} onRetry={retryCatalog} />
      ) : null}

      <SourceStatusDesk
        status={status}
        refreshing={synchronization.refreshing}
        refreshResult={synchronization.refreshResult}
        latestEvent={synchronization.latestEvent}
        onRefresh={synchronization.requestRefresh}
      />

      <SearchConsole
        client={client}
        status={status}
        catalogGeneration={authoritativeGeneration}
        selectedChannel={selectedChannel}
        selectedDetails={selectedDetails}
        selectedLoading={channelQuery.isPending && selectedChannel !== null}
        onSelectChannel={selectChannel}
        onRetrySelectedDetails={retrySelectedDetails}
      />

      {playbackChannel === null ? null : (
        <PlaybackLoadBoundary
          resetKey={playbackChannel}
          onStop={() => setPlaybackChannel(null)}
          onReload={reloadSparrow}
        >
          <Suspense fallback={<InlineLoading label="Preparing live player" />}>
            <HostedPlayer
              channel={{
                id: playbackChannel,
                name: playbackChannelName(
                  playbackChannel,
                  channels,
                  channelQuery.data,
                ),
              }}
              client={client}
              onStop={() => setPlaybackChannel(null)}
              {...(playbackEngine === undefined ? {} : { engine: playbackEngine })}
            />
          </Suspense>
        </PlaybackLoadBoundary>
      )}

      <main className="catalog-frame">
        <GroupRail
          groups={groups}
          activeGroup={activeGroup}
          hasNextPage={groupsQuery.hasNextPage}
          loadingMore={groupsQuery.isFetchingNextPage}
          replacing={groupsQuery.isRefetching || browseGenerationMismatch}
          onSelect={selectGroup}
          onLoadMore={loadMoreGroups}
        />

        <section className="channel-board" aria-labelledby="channel-heading">
          <div className="board-heading">
            <div>
              <p className="eyebrow">Channel rundown</p>
              <h2 id="channel-heading">{groupHeading(activeGroup)}</h2>
            </div>
            <div
              className="board-count"
              role="group"
              aria-label={`${channels.length} channels loaded`}
            >
              <strong>{String(channels.length).padStart(2, "0")}</strong>
              <span>loaded</span>
            </div>
          </div>

          {channelsQuery.isPending ? (
            <InlineLoading label="Tuning channel list" />
          ) : channels.length === 0 && browseError === null ? (
            <EmptyChannels group={activeGroup} />
          ) : (
            <div className="channel-grid">
              {channels.map((channel, index) => (
                <ChannelCard
                  key={channel.id}
                  channel={channel}
                  index={index}
                  selected={selectedChannel === channel.id}
                  onSelect={selectChannel}
                />
              ))}
            </div>
          )}

          {channelsQuery.hasNextPage ? (
            <button
              className="load-button"
              type="button"
              disabled={
                channelsQuery.isFetchingNextPage ||
                channelsQuery.isRefetching ||
                browseGenerationMismatch
              }
              onClick={loadMoreChannels}
            >
              {channelsQuery.isRefetching || browseGenerationMismatch
                ? "Updating catalog generation…"
                : channelsQuery.isFetchingNextPage
                ? "Receiving next block…"
                : "Receive next 24 channels"}
            </button>
          ) : null}
        </section>

        <ChannelInspector
          selected={selectedChannel}
          result={selectedDetails}
          loading={channelQuery.isPending && selectedChannel !== null}
        />
      </main>

      <footer className="catalog-footer">
        <span>CATALOG / {status?.generation ?? "NO SIGNAL"}</span>
        <span>PRIVATE SOURCES STAY SERVER-SIDE</span>
      </footer>
    </div>
  );
}

function GroupRail({
  groups,
  activeGroup,
  hasNextPage,
  loadingMore,
  replacing,
  onSelect,
  onLoadMore,
}: {
  readonly groups: readonly ChannelGroup[];
  readonly activeGroup: string | null;
  readonly hasNextPage: boolean;
  readonly loadingMore: boolean;
  readonly replacing: boolean;
  readonly onSelect: (group: string | null) => void;
  readonly onLoadMore: () => void;
}) {
  return (
    <nav className="group-rail" aria-label="Channel groups">
      <div className="rail-title">
        <span>01</span>
        <p>Signal banks</p>
      </div>
      <button
        className="group-button"
        data-active={activeGroup === null}
        type="button"
        aria-pressed={activeGroup === null}
        onClick={() => onSelect(null)}
      >
        <span>All channels</span>
        <b>∞</b>
      </button>
      {groups.map((group) => (
        <button
          className="group-button"
          data-active={activeGroup === group.name}
          type="button"
          title={group.name === "" ? "Ungrouped" : group.name}
          aria-pressed={activeGroup === group.name}
          key={group.name}
          onClick={() => onSelect(group.name)}
        >
          <span>{group.name === "" ? "Ungrouped" : group.name}</span>
          <b>{group.channelCount}</b>
        </button>
      ))}
      {groups.length === 0 ? (
        <p className="rail-empty">No source-defined groups.</p>
      ) : null}
      {hasNextPage ? (
        <button
          className="rail-more"
          type="button"
          disabled={loadingMore || replacing}
          onClick={onLoadMore}
        >
          {replacing ? "Updating…" : loadingMore ? "Receiving…" : "More banks +"}
        </button>
      ) : null}
    </nav>
  );
}

function ChannelCard({
  channel,
  index,
  selected,
  onSelect,
}: {
  readonly channel: ChannelSummary;
  readonly index: number;
  readonly selected: boolean;
  readonly onSelect: (id: ChannelId) => void;
}) {
  return (
    <button
      className="channel-card"
      data-selected={selected}
      type="button"
      title={channel.name}
      aria-pressed={selected}
      onClick={() => onSelect(channel.id)}
    >
      <span className="channel-number">CH {String(index + 1).padStart(2, "0")}</span>
      <strong>{channel.name}</strong>
      <span className="channel-group">
        {channel.group === "" ? "Unassigned signal" : channel.group}
      </span>
      <span className="channel-arrow" aria-hidden="true">
        ↗
      </span>
    </button>
  );
}

function ChannelInspector({
  selected,
  result,
  loading,
}: {
  readonly selected: ChannelId | null;
  readonly result: ClientResult<ChannelDetails> | undefined;
  readonly loading: boolean;
}) {
  return (
    <aside
      className="channel-inspector"
      aria-label="Selected channel"
      aria-live="polite"
      aria-atomic="true"
    >
      <div className="rail-title">
        <span>03</span>
        <p>Monitor</p>
      </div>
      {selected === null ? (
        <div className="inspector-idle">
          <span className="monitor-mark" aria-hidden="true">
            ◫
          </span>
          <p>Select a Channel to inspect its catalog record.</p>
        </div>
      ) : loading || result === undefined ? (
        <InlineLoading label="Resolving channel" />
      ) : result.ok ? (
        <div className="inspector-details">
          <p className="eyebrow">Catalog locked</p>
          <h3>{result.value.name}</h3>
          <dl>
            <div>
              <dt>Group</dt>
              <dd>{result.value.group || "Ungrouped"}</dd>
            </div>
            <div>
              <dt>Identifier</dt>
              <dd>{abbreviateId(result.value.id)}</dd>
            </div>
          </dl>
          <p className="inspector-note">
            Its matched Programme schedule is open in the search desk above.
          </p>
        </div>
      ) : (
        <CompactError error={result.error} />
      )}
    </aside>
  );
}

function CatalogLoading() {
  return (
    <main className="catalog-loading" aria-live="polite">
      <div className="loading-dial" aria-hidden="true" />
      <p className="eyebrow">Sparrow hosted desk</p>
      <h1>Tuning catalog</h1>
      <p>Negotiating the private signal index.</p>
    </main>
  );
}

function InlineLoading({ label }: { readonly label: string }) {
  return (
    <div className="inline-loading" role="status">
      <span aria-hidden="true" />
      {label}…
    </div>
  );
}

function EmptyChannels({ group }: { readonly group: string | null }) {
  return (
    <div className="empty-state">
      <span>00</span>
      <h3>No Channels on this frequency</h3>
      <p>
        {group === null
          ? "The current catalog contains no browseable Channels."
          : `The ${group || "Ungrouped"} bank is empty in this generation.`}
      </p>
    </div>
  );
}

function ErrorNotice({
  error,
  onRetry,
}: {
  readonly error: ClientError;
  readonly onRetry: () => void;
}) {
  const copy = errorCopy(error);
  return (
    <section className="error-notice" role="alert">
      <div>
        <p className="eyebrow">Signal exception / {error._tag}</p>
        <h2>{copy.title}</h2>
        <p>{copy.detail}</p>
      </div>
      <button type="button" onClick={onRetry}>
        Check again
      </button>
    </section>
  );
}

function CompactError({ error }: { readonly error: ClientError }) {
  const copy = errorCopy(error);
  return (
    <div className="compact-error" role="alert">
      <strong>{copy.title}</strong>
      <p>{copy.detail}</p>
    </div>
  );
}

function StatusReadout({ status }: { readonly status: CatalogStatus | null }) {
  const state = status === null ? null : status.m3u;
  return (
    <div className="status-readout" data-state={state?._tag ?? "unknown"}>
      <span aria-hidden="true" />
      <div>
        <small>M3U STATUS</small>
        <b>{state === null ? "CHECKING" : sourceStateLabel(state)}</b>
      </div>
    </div>
  );
}

function collectItems<Value>(
  pages: readonly ClientResult<Page<Value>>[] | undefined,
): readonly Value[] {
  if (pages === undefined) {
    return [];
  }
  return pages.flatMap((page) => (page.ok ? page.value.items : []));
}

function firstPageGeneration<Value>(
  pages: readonly ClientResult<Page<Value>>[] | undefined,
): CatalogGeneration | null {
  if (pages === undefined) {
    return null;
  }
  for (const page of pages) {
    if (page.ok) {
      return page.value.generation;
    }
  }
  return null;
}

function successValue<Value>(
  result: ClientResult<Value> | undefined,
): Value | null {
  return result?.ok === true ? result.value : null;
}

function resultWithQueryError<Value>(
  result: ClientResult<Value> | undefined,
  queryError: unknown,
): ClientResult<Value> | undefined {
  if (result !== undefined) {
    return result;
  }
  const error = clientErrorFromQuery(queryError);
  return error === null ? undefined : { ok: false, error };
}

function sourceStateLabel(state: SourceState): string {
  switch (state._tag) {
    case "fresh":
      return "LIVE / FRESH";
    case "stale":
      return "RECORDED / STALE";
    case "refreshing":
      return "RETUNING";
    case "deferred":
      return "HELD";
    case "failed":
      return state.validatedAt === null ? "FAILED" : "RECORDED / FAILED";
    case "unavailable":
      return "UNAVAILABLE";
  }
}

function isRetainedCatalog(status: CatalogStatus): boolean {
  return status.generation !== null && status.m3u._tag !== "fresh";
}

function retainedCatalogCopy(
  status: CatalogStatus,
  loadedGeneration: CatalogGeneration | null,
  authoritativeGeneration: CatalogGeneration | null,
): string {
  if (
    loadedGeneration !== null &&
    authoritativeGeneration !== null &&
    loadedGeneration !== authoritativeGeneration
  ) {
    return `The latest catalog could not replace the recorded browse data. Browsing remains locked to generation ${loadedGeneration}; current source generation is ${authoritativeGeneration}.`;
  }
  return `The live source is not fresh. Browsing remains locked to generation ${loadedGeneration ?? status.generation ?? "—"}.`;
}

function groupHeading(group: string | null): string {
  if (group === null) {
    return "All frequencies";
  }
  return group === "" ? "Ungrouped" : group;
}

function playbackChannelName(
  id: ChannelId,
  loaded: readonly ChannelSummary[],
  details: ClientResult<ChannelDetails> | undefined,
): string {
  if (details?.ok === true && details.value.id === id) {
    return details.value.name;
  }
  return loaded.find((channel) => channel.id === id)?.name ?? "Selected Channel";
}

function abbreviateId(id: ChannelId): string {
  return `${id.slice(0, 12)}…${id.slice(-8)}`;
}

function errorCopy(error: ClientError): {
  readonly title: string;
  readonly detail: string;
} {
  switch (error._tag) {
    case "authentication-required":
      return {
        title: "Access credential required",
        detail: "Authenticate with the hosted Sparrow deployment, then retry this signal.",
      };
    case "service-unavailable":
      return {
        title: "The hosted desk is temporarily unavailable",
        detail: "The catalog remains unchanged. Try the request again shortly.",
      };
    case "invalid-input":
      return {
        title: "The catalog request was rejected",
        detail: `${error.field}: ${error.reason}`,
      };
    case "not-configured":
      return {
        title: "No source is configured",
        detail: "This hosted deployment has not been connected to an M3U source.",
      };
    case "catalog-unavailable":
      return {
        title: "The Channel Catalog is unavailable",
        detail: "No validated Channel snapshot is available yet. The server can be checked again safely.",
      };
    case "not-found":
      return {
        title: "That Channel left the catalog",
        detail: "Choose a Channel from the current generation.",
      };
    case "stale-cursor":
      return {
        title: "A newer catalog is on air",
        detail: `Pagination moved to generation ${error.current}. Reload to continue from its first page.`,
      };
    case "playback-failed":
      return {
        title: "The live signal is unavailable",
        detail: "Browsing remains available. Choose another Channel or retry playback.",
      };
    case "transport":
      return {
        title: "The hosted desk did not answer",
        detail: error.message,
      };
    case "cancelled":
      return {
        title: "The request was cancelled",
        detail: "No catalog state was changed. Retry when ready.",
      };
  }
}

function reloadSparrow(): void {
  window.location.reload();
}
