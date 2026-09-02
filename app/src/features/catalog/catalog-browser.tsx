import { useQueryClient } from "@tanstack/react-query";
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import type { HostedPlaybackEngine } from "../playback/mpegts-engine";
import type { InstalledPlaybackEngine } from "../playback/installed-playback-engine";
import { PlaybackLoadBoundary } from "../playback/playback-load-boundary";
import {
  clientSchemas,
  type CatalogStatus,
  type ChannelSummary,
  type ClientResult,
  type GuideWindowChannel,
  type InstalledSparrowClient,
  type ProgrammeSlot,
  type SourceState,
  type SparrowClient,
} from "../../client/contracts";
import { BoardSearch } from "../guide/board-search";
import {
  resolvedActiveGroup,
  shouldAdvancePastExcludedPage,
  visibleGuideRows,
} from "../guide/board-group-roster";
import { CinemaStage } from "../guide/cinema-stage";
import { FeedsDialog } from "../guide/feeds-dialog";
import { clockLabel, clockWindow, programmeAt } from "../guide/guide-window";
import { ProgrammeGuide, type GuideSelection } from "../guide/programme-guide";
import { useBoardGroupExclusions } from "../guide/use-board-group-exclusions";
import { useGuideClock } from "../guide/use-guide-clock";
import { useCatalogSynchronization } from "../status/catalog-synchronization";
import { useGuideCatalog } from "./use-guide-catalog";
import "../guide/split-stage.css";

const loadHostedPlayer = () => import("../playback/hosted-player");
const loadInstalledPlayer = () => import("../playback/installed-player");
const HostedPlayer = lazy(async () => {
  const module = await loadHostedPlayer();
  return { default: module.HostedPlayer };
});
const InstalledPlayer = lazy(async () => {
  const module = await loadInstalledPlayer();
  return { default: module.InstalledPlayer };
});

interface SelectedSignal {
  readonly channel: ChannelSummary;
  readonly programme: ProgrammeSlot | null;
}

type CatalogBrowserProps =
  | {
      readonly client: SparrowClient;
      readonly runtime?: "hosted";
      readonly playbackEngine?: HostedPlaybackEngine;
      readonly sourceConfiguration?: never;
    }
  | {
      readonly client: InstalledSparrowClient;
      readonly runtime: "installed";
      readonly sourceConfiguration?: Pick<
        InstalledSparrowClient,
        "replaceSourceConfiguration"
      >;
      readonly playbackEngine?: InstalledPlaybackEngine;
    };

/** Owns Split Stage catalog reads, selection, playback, and source controls. */
export function CatalogBrowser(props: CatalogBrowserProps) {
  const runtime = props.runtime ?? "hosted";
  const client = props.client;
  const now = useGuideClock();
  const guideClock = useMemo(() => {
    const window = clockWindow(now);
    return {
      window,
      startsAt: clientSchemas.isoInstant.parse(window.startsAt.toISOString()),
      endsAt: clientSchemas.isoInstant.parse(window.endsAt.toISOString()),
    };
  }, [now]);
  const queryClient = useQueryClient();
  const synchronization = useCatalogSynchronization(client);
  const [activeGroup, setActiveGroup] = useState<string | null>(null);
  const groupExclusions = useBoardGroupExclusions();
  const boardGroup = resolvedActiveGroup(activeGroup, groupExclusions.excluded);
  const [selectedSignal, setSelectedSignal] = useState<SelectedSignal | null>(
    null,
  );
  const [playingChannel, setPlayingChannel] = useState<ChannelSummary | null>(
    null,
  );

  const status = synchronization.status;
  const catalogGeneration = status?.generation ?? null;
  const authoritativeGeneration =
    synchronization.generationHint === undefined
      ? catalogGeneration
      : synchronization.generationHint;
  const browseEnabled =
    runtime === "hosted" ||
    (status?.configuration.configured === true &&
      authoritativeGeneration !== null);
  const guideCatalog = useGuideCatalog({
    client,
    enabled: browseEnabled,
    group: boardGroup,
    startsAt: guideClock.startsAt,
    endsAt: guideClock.endsAt,
    expectedGeneration: authoritativeGeneration,
  });
  const guideError = guideCatalog.error ?? synchronization.statusError;
  const retryGuide =
    guideCatalog.error?._tag === "stale-cursor"
      ? synchronization.retryStatus
      : guideCatalog.error === null
        ? synchronization.retryStatus
        : guideCatalog.retry;
  const groups = guideCatalog.groups;
  const rows = visibleGuideRows(
    guideCatalog.rows,
    groupExclusions.excluded,
    boardGroup,
  );
  const defaultSignal = defaultSelectedSignal(rows, now);
  const selection = selectedSignal ?? defaultSignal;
  const guideSelection: GuideSelection | null =
    selection === null
      ? null
      : { channelId: selection.channel.id, programme: selection.programme };
  const playingRow =
    playingChannel === null
      ? undefined
      : guideCatalog.rows.find((row) => row.channel.id === playingChannel.id);
  const stageSignal: SelectedSignal | null =
    playingChannel === null
      ? selection
      : {
          channel: playingChannel,
          programme:
            selectedSignal?.channel.id === playingChannel.id
              ? selectedSignal.programme
              : playingRow === undefined
                ? null
                : programmeAt(playingRow.programmes, now),
        };
  const stageProgrammes =
    playingRow?.programmes ??
    (stageSignal?.programme === null || stageSignal === null
      ? []
      : [stageSignal.programme]);

  const tune = useCallback(
    (channel: ChannelSummary, programme: ProgrammeSlot | null) => {
      setSelectedSignal({ channel, programme });
      setPlayingChannel(channel);
    },
    [],
  );
  const selectGroup = useCallback((group: string | null) => {
    setActiveGroup(group);
    setSelectedSignal(null);
  }, []);
  const applyInstalledConfiguration = useCallback(
    (nextStatus: CatalogStatus) => {
      setActiveGroup(null);
      setSelectedSignal(null);
      setPlayingChannel(null);
      queryClient.removeQueries({
        predicate: ({ queryKey }) =>
          queryKey[0] === "catalog" && queryKey[1] !== "status",
      });
      queryClient.setQueryData<ClientResult<CatalogStatus>>(
        ["catalog", "status"],
        { ok: true, value: nextStatus },
      );
    },
    [queryClient],
  );
  const preparePlayback =
    runtime === "installed" ? loadInstalledPlayer : loadHostedPlayer;
  const { loadMore } = guideCatalog;
  useEffect(() => {
    if (
      !shouldAdvancePastExcludedPage({
        activeGroup: boardGroup,
        excludedCount: groupExclusions.excluded.size,
        receivedCount: guideCatalog.rows.length,
        visibleCount: rows.length,
        hasMore: guideCatalog.hasMore,
        loading:
          guideCatalog.loading ||
          guideCatalog.loadingMore ||
          guideCatalog.replacing,
      })
    ) {
      return;
    }
    loadMore();
  }, [
    boardGroup,
    groupExclusions.excluded.size,
    guideCatalog.hasMore,
    guideCatalog.loading,
    guideCatalog.loadingMore,
    guideCatalog.replacing,
    guideCatalog.rows.length,
    loadMore,
    rows.length,
  ]);
  if (synchronization.statusPending) {
    return <CatalogLoading runtime={runtime} />;
  }

  const player = renderPlayer({
    props,
    playingChannel,
    onStop: () => setPlayingChannel(null),
  });
  const feeds =
    props.runtime === "installed" ? (
      <FeedsDialog
        runtime="installed"
        client={props.sourceConfiguration ?? props.client}
        status={status}
        refreshing={synchronization.refreshing}
        refreshResult={synchronization.refreshResult}
        latestEvent={synchronization.latestEvent}
        onRefresh={synchronization.requestRefresh}
        onApplied={applyInstalledConfiguration}
      />
    ) : (
      <FeedsDialog
        runtime="hosted"
        status={status}
        refreshing={synchronization.refreshing}
        refreshResult={synchronization.refreshResult}
        latestEvent={synchronization.latestEvent}
        onRefresh={synchronization.requestRefresh}
      />
    );

  return (
    <div className="split-stage" data-acceptance-catalog-shell>
      <SplitStageMasthead status={status} now={now} />
      {status !== null && isRetainedCatalog(status) ? (
        <aside
          className="split-stage__retained"
          data-acceptance-retained
          role="status"
        >
          Retained catalog · a fresh source check is pending
        </aside>
      ) : null}
      <main className="split-stage__workspace">
        <CinemaStage
          programme={stageSignal?.programme ?? null}
          channel={stageSignal?.channel ?? null}
          programmes={stageProgrammes}
          now={now}
          player={player}
          playing={playingChannel !== null}
          onPlay={() => {
            if (selection !== null) {
              setPlayingChannel(selection.channel);
            }
          }}
          onSelectProgramme={(programme) => {
            if (stageSignal !== null) {
              tune(stageSignal.channel, programme);
            }
          }}
        />
        <ProgrammeGuide
          rows={rows}
          groups={groups}
          activeGroup={activeGroup}
          window={guideClock.window}
          now={now}
          selection={guideSelection}
          playingChannel={playingChannel?.id ?? null}
          loading={guideCatalog.loading}
          replacing={guideCatalog.replacing}
          error={guideError}
          hasMore={guideCatalog.hasMore}
          loadingMore={guideCatalog.loadingMore}
          emptyState={guideEmptyState(runtime, status, browseEnabled)}
          onSelectGroup={selectGroup}
          onPrefetchGroup={guideCatalog.prefetchGroup}
          excludedGroups={groupExclusions.excluded}
          onSetGroupExcluded={groupExclusions.setExcluded}
          onRestoreExcludedGroups={groupExclusions.restoreAll}
          onPreparePlayback={preparePlayback}
          onTune={tune}
          onRetry={retryGuide}
          onLoadMore={guideCatalog.loadMore}
          search={
            <BoardSearch
              client={client}
              generation={authoritativeGeneration}
              excludedGroups={groupExclusions.excluded}
              onGenerationMismatch={synchronization.retryStatus}
              onPreparePlayback={preparePlayback}
              onTune={tune}
            />
          }
          feeds={feeds}
        />
        <div className="split-stage__knife" aria-hidden="true">
          <i />
        </div>
      </main>
    </div>
  );
}

function renderPlayer({
  props,
  playingChannel,
  onStop,
}: {
  readonly props: CatalogBrowserProps;
  readonly playingChannel: ChannelSummary | null;
  readonly onStop: () => void;
}): ReactNode {
  if (playingChannel === null) {
    return null;
  }
  return (
    <PlaybackLoadBoundary
      resetKey={playingChannel.id}
      onStop={onStop}
      onReload={reloadSparrow}
    >
      <Suspense fallback={<PlayerLoading />}>
        {props.runtime === "installed" ? (
          <InstalledPlayer
            channel={playingChannel}
            client={props.client}
            onStop={onStop}
            {...(props.playbackEngine === undefined
              ? {}
              : { engine: props.playbackEngine })}
          />
        ) : (
          <HostedPlayer
            channel={playingChannel}
            client={props.client}
            onStop={onStop}
            {...(props.playbackEngine === undefined
              ? {}
              : { engine: props.playbackEngine })}
          />
        )}
      </Suspense>
    </PlaybackLoadBoundary>
  );
}

function SplitStageMasthead({
  status,
  now,
}: {
  readonly status: CatalogStatus | null;
  readonly now: Date;
}) {
  return (
    <header className="split-stage__masthead">
      <div className="split-stage__identity">
        <strong>SPARROW</strong>
        <i aria-hidden="true" />
        <span>LIVE</span>
        <time dateTime={now.toISOString()}>{clockLabel(now)}</time>
      </div>
      <div className="split-stage__freshness">
        <StatusReadout label="Catalog" state={status?.m3u ?? null} now={now} />
        <StatusReadout label="Guide" state={status?.epg ?? null} now={now} />
      </div>
    </header>
  );
}

function StatusReadout({
  label,
  state,
  now,
}: {
  readonly label: string;
  readonly state: SourceState | null;
  readonly now: Date;
}) {
  return (
    <span
      className="split-stage__status"
      data-acceptance-status
      data-state={state?._tag ?? "unavailable"}
    >
      {label}
      <b>{sourceAge(state, now)}</b>
    </span>
  );
}

function CatalogLoading({
  runtime,
}: {
  readonly runtime: "hosted" | "installed";
}) {
  return (
    <main
      className="catalog-loading"
      data-acceptance-catalog-loading
      aria-live="polite"
    >
      <span aria-hidden="true" />
      <p>{runtime === "hosted" ? "Hosted desk" : "Installed receiver"}</p>
      <h1>Tuning catalog</h1>
      <small>Opening one generation-bound guide window</small>
    </main>
  );
}

function PlayerLoading() {
  return (
    <div className="cinema-stage__player-loading" role="status">
      Preparing live signal…
    </div>
  );
}

function defaultSelectedSignal(
  rows: readonly GuideWindowChannel[],
  now: Date,
): SelectedSignal | null {
  const row = rows[0];
  return row === undefined
    ? null
    : { channel: row.channel, programme: programmeAt(row.programmes, now) };
}

function isRetainedCatalog(status: CatalogStatus): boolean {
  return (
    status.generation !== null &&
    (status.m3u._tag === "stale" ||
      status.m3u._tag === "failed" ||
      status.epg?._tag === "stale" ||
      status.epg?._tag === "failed")
  );
}

function sourceAge(state: SourceState | null, now: Date): string {
  if (state === null || state._tag === "unavailable") {
    return "—";
  }
  const validatedAt = state.validatedAt;
  if (validatedAt === null) {
    return state._tag === "refreshing" ? "SYNC" : "—";
  }
  const minutes = Math.max(
    0,
    Math.floor((now.getTime() - Date.parse(validatedAt)) / 60_000),
  );
  return minutes < 60 ? `${minutes}M` : `${Math.floor(minutes / 60)}H`;
}

function guideEmptyState(
  runtime: "hosted" | "installed",
  status: CatalogStatus | null,
  browseEnabled: boolean,
): { readonly title: string; readonly detail: string } | undefined {
  if (runtime === "hosted" || browseEnabled) {
    return undefined;
  }
  return status?.configuration.configured === true
    ? {
        title: "Waiting for the first catalog",
        detail:
          "The configured feeds have not published a validated snapshot yet.",
      }
    : {
        title: "Patch a feed to this receiver",
        detail:
          "Open Feeds to configure the installed catalog before browsing.",
      };
}

function reloadSparrow(): void {
  window.location.reload();
}
