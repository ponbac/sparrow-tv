import { useQueryClient } from "@tanstack/react-query";
import { List } from "lucide-react";
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
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
  type InstalledSparrowClient,
  type SparrowClient,
} from "../../client/contracts";
import { agentControlChannelLimit } from "../agent-control/agent-control";
import { bindAgentControlCatalog } from "../agent-control/agent-control-binding";
import { BoardSearch } from "../guide/board-search";
import {
  resolvedActiveGroup,
  shouldAdvancePastExcludedPage,
  visibleGuideRows,
} from "../guide/board-group-roster";
import { FeedsDialog } from "../guide/feeds-dialog";
import { familyProgrammes } from "../guide/guide-families";
import { clockLabel, clockWindow } from "../guide/guide-window";
import { ProgrammeGuide } from "../guide/programme-guide";
import { useBoardGroupExclusions } from "../guide/use-board-group-exclusions";
import { useGuideClock } from "../guide/use-guide-clock";
import { useVariantPreferences } from "../guide/use-variant-preferences";
import { NowPlaying, type NowPlayingSubject } from "../stage/now-playing";
import { Stage } from "../stage/stage";
import {
  StageChromeProvider,
  type StageChrome,
  type StagePicture,
} from "../stage/stage-chrome";
import { useIdleChrome } from "../stage/use-idle-chrome";
import { guideRowProgramme, useNowPlaying } from "../stage/use-now-playing";
import { useStageKeys } from "../stage/use-stage-keys";
import {
  usePictureOverlay,
  useStageFullscreenTarget,
  useStageLayout,
} from "../stage/use-stage-layout";
import {
  neighbouringZapStop,
  zapRailStops,
  zapStopOf,
  zapStops,
  type ZapStop,
} from "../stage/zap";
import { ZapRail } from "../stage/zap-rail";
import { useCatalogSynchronization } from "../status/catalog-synchronization";
import { sourceFreshness } from "../status/source-freshness";
import { useGuideCatalog } from "./use-guide-catalog";
import "../stage/shell.css";
import "../stage/theater.css";

const loadHostedPlayer = () => import("../playback/hosted-player");
const loadInstalledPlayer = () => import("../playback/installed-player");
// Rows read around the playing Channel in the stacked layout: enough to hold
// every Quality Variant of its guide row.
const STACKED_NEIGHBOURHOOD_SIZE = 9;
// Theater also offers the Channels on either side of the playing one.
const THEATER_NEIGHBOURHOOD_SIZE = 61;
// How long the last arrow press waits for another before its Channel is tuned.
const ZAP_COMMIT_MS = 350;

const HostedPlayer = lazy(async () => {
  const module = await loadHostedPlayer();
  return { default: module.HostedPlayer };
});
const InstalledPlayer = lazy(async () => {
  const module = await loadInstalledPlayer();
  return { default: module.InstalledPlayer };
});

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

/** Owns the shell's catalog reads, the playing Channel, and source controls. */
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
  const { preferences: variantPreferences, prefer: preferVariant } =
    useVariantPreferences();
  const [playingChannel, setPlayingChannel] = useState<ChannelSummary | null>(
    null,
  );
  // The Channel an arrow press has moved to. The info block shows it at once;
  // it is tuned when no further press follows.
  const [pendingZap, setPendingZap] = useState<ChannelSummary | null>(null);
  const zapTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cancelZap = useCallback(() => {
    if (zapTimer.current !== null) {
      clearTimeout(zapTimer.current);
      zapTimer.current = null;
    }
    setPendingZap(null);
  }, []);
  // The one writer of the playing Channel, apart from a zap that commits.
  // Whatever it sets, a zap still waiting must not tune over it afterwards.
  const setPlaying = useCallback(
    (next: ChannelSummary | null) => {
      cancelZap();
      setPlayingChannel(next);
    },
    [cancelZap],
  );
  useEffect(
    () => () => {
      if (zapTimer.current !== null) {
        clearTimeout(zapTimer.current);
      }
    },
    [],
  );
  const pictureOverlay = usePictureOverlay(runtime, client);
  const layout = useStageLayout(pictureOverlay);
  const fullscreenTarget = useStageFullscreenTarget(layout);
  // The viewer's choice between the full picture and the guide. The shell
  // shows the guide regardless while there is no picture in the page.
  const [mode, setMode] = useState<"watch" | "guide">("watch");
  const [picture, setPicture] = useState<StagePicture | null>(null);
  const reportPicture = useCallback((next: StagePicture | null) => {
    setPicture((current) => (samePicture(current, next) ? current : next));
  }, []);
  const [controlsSlot, setControlsSlot] = useState<HTMLElement | null>(null);
  const chrome = useMemo<StageChrome>(
    () => ({
      controlsSlot: layout === "theater" ? controlsSlot : null,
      controls: layout === "theater" ? "compact" : "bar",
      // Only roomy Theater enters root fullscreen; phones keep controls inline.
      fullscreenTarget,
      reportPicture,
    }),
    [controlsSlot, fullscreenTarget, layout, reportPicture],
  );
  const guideForced = playingChannel === null || picture?.external === true;
  const effectiveMode = guideForced ? "guide" : mode;
  const watching = layout === "theater" && effectiveMode === "watch";
  const chromeVisibility = useIdleChrome(
    watching && picture?.playing === true,
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
  // The guide folds these into rows; a stable array keeps its rows memoised.
  const rows = useMemo(
    () =>
      visibleGuideRows(guideCatalog.rows, groupExclusions.excluded, boardGroup),
    [boardGroup, groupExclusions.excluded, guideCatalog.rows],
  );
  const nowPlaying = useNowPlaying({
    client,
    channel: playingChannel?.id ?? null,
    generation: authoritativeGeneration,
    startsAt: guideClock.startsAt,
    endsAt: guideClock.endsAt,
    channelLimit:
      layout === "theater"
        ? THEATER_NEIGHBOURHOOD_SIZE
        : STACKED_NEIGHBOURHOOD_SIZE,
    onGenerationMismatch: synchronization.retryStatus,
  });

  // `tune` and `stop` never change: Agent Control binds to them once.
  const tune = useCallback(
    (channel: ChannelSummary) => {
      setPlaying(channel);
      setMode("watch");
    },
    [setPlaying],
  );
  const tuneVariant = useCallback(
    (channel: ChannelSummary) => {
      preferVariant(channel);
      tune(channel);
    },
    [preferVariant, tune],
  );
  const stop = useCallback(() => {
    setPlaying(null);
  }, [setPlaying]);
  const applyInstalledConfiguration = useCallback(
    (nextStatus: CatalogStatus) => {
      setActiveGroup(null);
      setPlaying(null);
      queryClient.removeQueries({
        predicate: ({ queryKey }) =>
          queryKey[0] === "catalog" && queryKey[1] !== "status",
      });
      queryClient.setQueryData<ClientResult<CatalogStatus>>(
        ["catalog", "status"],
        { ok: true, value: nextStatus },
      );
    },
    [queryClient, setPlaying],
  );
  useEffect(() => {
    if (runtime !== "installed") {
      return;
    }
    return bindAgentControlCatalog({
      searchChannels: (term, signal) =>
        client.searchChannels({
          term,
          limit: agentControlChannelLimit(),
          ...(signal === undefined ? {} : { signal }),
        }),
      cancelPendingTune: stop,
      tune,
    });
  }, [client, runtime, stop, tune]);
  const preparePlayback =
    runtime === "installed" ? loadInstalledPlayer : loadHostedPlayer;

  // The guide rows a zap moves through; empty until the rows around the
  // playing Channel are known.
  const stops = useMemo(
    () =>
      playingChannel === null || nowPlaying.rows === null
        ? []
        : zapStops(
            nowPlaying.rows,
            playingChannel.id,
            groupExclusions.excluded,
            variantPreferences,
          ),
    [
      groupExclusions.excluded,
      nowPlaying.rows,
      playingChannel,
      variantPreferences,
    ],
  );
  const zap = (direction: -1 | 1) => {
    if (playingChannel === null) {
      return;
    }
    const next = neighbouringZapStop(
      stops,
      (pendingZap ?? playingChannel).id,
      direction,
    );
    if (next === null) {
      return;
    }
    cancelZap();
    const target = next.target.channel;
    setPendingZap(target);
    zapTimer.current = setTimeout(() => {
      zapTimer.current = null;
      setPendingZap(null);
      // Changes the Channel and nothing else, and never starts one after a Stop.
      setPlayingChannel((current) => (current === null ? current : target));
    }, ZAP_COMMIT_MS);
  };
  const toggleGuide = useCallback(() => {
    setMode((current) => (current === "guide" ? "watch" : "guide"));
  }, []);
  const showPicture = useCallback(() => {
    setMode("watch");
  }, []);
  useStageKeys({
    active: layout === "theater",
    mode: effectiveMode,
    guideForced,
    onToggleGuide: toggleGuide,
    onWatch: showPicture,
    onZap: zap,
  });
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
    return <CatalogLoading />;
  }

  const player = renderPlayer({ props, playingChannel, onStop: stop });
  // The info block and the rail follow a zap at once, ahead of the player.
  const shownChannel = pendingZap ?? playingChannel;
  const subject: NowPlayingSubject | null =
    playingChannel === null
      ? null
      : pendingZap === null
        ? {
            // The Channel as tuned keeps the number it had then; its row in
            // the current catalog has the number the guide and rail show.
            channel:
              nowPlaying.family?.variants.find(
                (variant) => variant.channel.id === playingChannel.id,
              )?.channel ?? playingChannel,
            variants:
              nowPlaying.family?.variants.map((variant) => variant.channel) ??
              [],
            programmes: nowPlaying.programmes,
            loading: nowPlaying.loading,
          }
        : zapSubject(pendingZap, zapStopOf(stops, pendingZap.id));
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

  // The one element whose parent depends on the layout: Theater shows it in
  // the masthead, stacked in the guide's toolbar.
  const search = (
    <BoardSearch
      client={client}
      generation={authoritativeGeneration}
      excludedGroups={groupExclusions.excluded}
      onGenerationMismatch={synchronization.retryStatus}
      onPreparePlayback={preparePlayback}
      onTune={tuneVariant}
    />
  );

  return (
    <div
      className="shell"
      data-layout={layout}
      data-mode={effectiveMode}
      data-chrome={chromeVisibility}
      data-acceptance-catalog-shell
    >
      <ShellMasthead
        now={now}
        theater={
          layout === "theater"
            ? {
                search,
                guideOpen: effectiveMode === "guide",
                guideForced,
                onToggleGuide: toggleGuide,
              }
            : null
        }
      />
      {status !== null && isRetainedCatalog(status) ? (
        <aside
          className="shell__retained"
          data-acceptance-retained
          role="status"
        >
          Showing the saved catalog. A fresh source check is pending.
        </aside>
      ) : null}
      <main className="shell__workspace">
        <StageChromeProvider value={chrome}>
          <Stage
            playingChannel={playingChannel?.id ?? null}
            player={player}
            info={
              <NowPlaying
                subject={subject}
                playingChannel={playingChannel?.id ?? null}
                now={now}
                layout={layout}
                controlsRef={setControlsSlot}
                onPreparePlayback={preparePlayback}
                onTuneVariant={tuneVariant}
              />
            }
            rail={
              watching && shownChannel !== null && stops.length > 0 ? (
                <ZapRail
                  stops={zapRailStops(stops, shownChannel.id)}
                  current={shownChannel.id}
                  now={now}
                  onPreparePlayback={preparePlayback}
                  onTune={tune}
                />
              ) : null
            }
            keyHints={watching}
          />
        </StageChromeProvider>
        <ProgrammeGuide
          rows={rows}
          groups={groups}
          activeGroup={activeGroup}
          window={guideClock.window}
          now={now}
          playingChannel={playingChannel?.id ?? null}
          variantPreferences={variantPreferences}
          layout={layout}
          loading={guideCatalog.loading}
          replacing={guideCatalog.replacing}
          error={guideError}
          hasMore={guideCatalog.hasMore}
          loadingMore={guideCatalog.loadingMore}
          emptyState={guideEmptyState(runtime, status, browseEnabled)}
          onSelectGroup={setActiveGroup}
          onPrefetchGroup={guideCatalog.prefetchGroup}
          excludedGroups={groupExclusions.excluded}
          onSetGroupExcluded={groupExclusions.setExcluded}
          onRestoreExcludedGroups={groupExclusions.restoreAll}
          onPreparePlayback={preparePlayback}
          onTune={tune}
          onTuneVariant={tuneVariant}
          onRetry={retryGuide}
          onLoadMore={guideCatalog.loadMore}
          search={layout === "theater" ? null : search}
          feeds={feeds}
          status={
            <>
              <StatusReadout source="m3u" status={status} now={now} />
              <StatusReadout source="epg" status={status} now={now} />
            </>
          }
        />
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

function ShellMasthead({
  now,
  theater,
}: {
  readonly now: Date;
  /** What only the Theater masthead carries; null in the stacked layout. */
  readonly theater: {
    readonly search: ReactNode;
    readonly guideOpen: boolean;
    /** The guide stays open while there is no picture in the page. */
    readonly guideForced: boolean;
    readonly onToggleGuide: () => void;
  } | null;
}) {
  return (
    <header className="shell__masthead">
      <strong className="shell__wordmark">Sparrow</strong>
      {theater === null ? null : (
        <>
          <div className="shell__search">{theater.search}</div>
          <button
            className="shell__guide-toggle"
            type="button"
            aria-pressed={theater.guideOpen}
            disabled={theater.guideForced}
            onClick={theater.onToggleGuide}
          >
            <List aria-hidden="true" />
            Guide
            <kbd aria-hidden="true">G</kbd>
          </button>
        </>
      )}
      <time className="shell__clock" dateTime={now.toISOString()}>
        {clockLabel(now)}
      </time>
    </header>
  );
}

function StatusReadout({
  source,
  status,
  now,
}: {
  readonly source: "m3u" | "epg";
  readonly status: CatalogStatus | null;
  readonly now: Date;
}) {
  const freshness = sourceFreshness(source, status, now);
  return (
    <span
      className="shell__status"
      data-acceptance-status
      data-state={freshness.state}
    >
      {freshness.sentence}
    </span>
  );
}

function CatalogLoading() {
  return (
    <main
      className="catalog-loading"
      data-acceptance-catalog-loading
      aria-live="polite"
    >
      <span aria-hidden="true" />
      <h1>Opening your channels</h1>
    </main>
  );
}

function PlayerLoading() {
  return (
    <div className="stage__player-loading" role="status">
      Loading the player…
    </div>
  );
}

/** What the info block says about a Channel a zap has moved to but not yet tuned. */
function zapSubject(
  channel: ChannelSummary,
  stop: ZapStop | null,
): NowPlayingSubject {
  return {
    channel,
    variants: stop?.family.variants.map((variant) => variant.channel) ?? [],
    programmes:
      stop === null
        ? []
        : familyProgrammes(stop.family, stop.target).map(guideRowProgramme),
    loading: false,
  };
}

function samePicture(
  left: StagePicture | null,
  right: StagePicture | null,
): boolean {
  return left === null || right === null
    ? left === right
    : left.playing === right.playing && left.external === right.external;
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
        detail: "The sources have not loaded yet.",
      }
    : {
        title: "Add your sources",
        detail: "Open Sources to set up this device before browsing.",
      };
}

function reloadSparrow(): void {
  window.location.reload();
}
