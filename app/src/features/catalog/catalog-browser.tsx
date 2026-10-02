import { useQueryClient } from "@tanstack/react-query";
import { List, Search } from "lucide-react";
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
import {
  clockLabel,
  clockWindow,
  GUIDE_SPAN_MS,
  laterTimes,
  LATER_SPAN_MS,
} from "../guide/guide-window";
import { ProgrammeGuide } from "../guide/programme-guide";
import { useBoardGroupExclusions } from "../guide/use-board-group-exclusions";
import { useGuideClock } from "../guide/use-guide-clock";
import { useVariantPreferences } from "../guide/use-variant-preferences";
import { ChannelBar } from "../stage/channel-bar";
import { nextDock, pictureFollows } from "../stage/dock";
import { NowPlaying, type NowPlayingSubject } from "../stage/now-playing";
import { Stage } from "../stage/stage";
import {
  samePicture,
  StageChromeProvider,
  type StageChrome,
  type StagePicture,
} from "../stage/stage-chrome";
import { focusPictureReturn, guideSearchInput } from "../stage/stage-dom";
import { useIdleChrome } from "../stage/use-idle-chrome";
import { guideRowProgramme, useNowPlaying } from "../stage/use-now-playing";
import { usePicturePin } from "../stage/use-picture-pin";
import { useStageKeys } from "../stage/use-stage-keys";
import { usePictureOverlay, useStageLayout } from "../stage/use-stage-layout";
import { useZapStops } from "../stage/use-zap-stops";
import {
  neighbouringZapStop,
  zapRailStops,
  zapStopOf,
  type ZapStop,
} from "../stage/zap";
import { ZapRail } from "../stage/zap-rail";
import { useCatalogSynchronization } from "../status/catalog-synchronization";
import { sourceFreshness } from "../status/source-freshness";
import { useGuideCatalog } from "./use-guide-catalog";
import "../stage/shell.css";
import "../stage/pocket.css";
import "../stage/theater.css";

const loadHostedPlayer = () => import("../playback/hosted-player");
const loadInstalledPlayer = () => import("../playback/installed-player");
// Rows read around the playing Channel in the pocket layout: enough to hold
// its guide row and the one on either side, each with every Quality Variant.
// A zap that runs past them reads further.
const POCKET_NEIGHBOURHOOD_SIZE = 21;
// Theater offers a whole row of the Channels on either side.
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
  // Pocket's row of times. Until the viewer first opens it the guide reads
  // what the timeline shows; from then on one longer read covers every time
  // on offer, so choosing one asks for nothing.
  const [timeRow, setTimeRow] = useState<"unused" | "open" | "closed">(
    "unused",
  );
  const toggleTimeRow = useCallback(() => {
    setTimeRow((current) => (current === "open" ? "closed" : "open"));
  }, []);
  const [chosenTime, setChosenTime] = useState<Date | null>(null);
  const guideSpan =
    layout === "pocket" && timeRow !== "unused" ? LATER_SPAN_MS : GUIDE_SPAN_MS;
  const guideClock = useMemo(() => {
    const window = clockWindow(now, guideSpan);
    return {
      window,
      startsAt: clientSchemas.isoInstant.parse(window.startsAt.toISOString()),
      endsAt: clientSchemas.isoInstant.parse(window.endsAt.toISOString()),
    };
  }, [guideSpan, now]);
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
      // Both layouts keep the controls in the info block, clear of the picture.
      controlsSlot,
      controls: "compact",
      // Fullscreen on the document root outlives Stop and a change of Channel.
      fullscreenTarget: pictureOverlay ? document.documentElement : null,
      // Above pocket's controls is the picture, which nothing may cover.
      menuSide: layout === "theater" ? "top" : "bottom",
      reportPicture,
    }),
    [controlsSlot, layout, pictureOverlay, reportPicture],
  );
  const guideForced = playingChannel === null || picture?.external === true;
  const effectiveMode = guideForced ? "guide" : mode;
  // Whether pocket's picture is the small band beside the guide. It follows
  // the mode, but a picture that cannot follow its box keeps the size it has.
  const [docked, setDocked] = useState(false);
  const follows = pictureFollows(pictureOverlay, picture);
  const dock = nextDock({ mode: effectiveMode, previous: docked, follows });
  if (dock !== docked) {
    setDocked(dock);
  }
  // The window can change size as well as the mode: a picture that cannot
  // follow keeps the very box it has, in pixels.
  usePicturePin(follows);
  const watching = layout === "theater" && effectiveMode === "watch";
  const chromeVisibility = useIdleChrome(
    watching && picture?.state === "playing",
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
        : POCKET_NEIGHBOURHOOD_SIZE,
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

  // The info block and the rail follow a zap at once, ahead of the player.
  const shownChannel = pendingZap ?? playingChannel;
  const stops = useZapStops({
    neighbourhood: nowPlaying.neighbourhood,
    playing: playingChannel?.id ?? null,
    shown: shownChannel?.id ?? null,
    excludedGroups: groupExclusions.excluded,
    preferences: variantPreferences,
    onExtend: nowPlaying.extend,
  });
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
  const showGuide = useCallback(() => {
    setMode("guide");
  }, []);
  const showGuideFromBar = useCallback(() => {
    showGuide();
    // The bar leaves the layout with the watch screen. Focus moves to the
    // way back once the band is laid out, so a keyboard keeps its place.
    requestAnimationFrame(focusPictureReturn);
  }, [showGuide]);
  const findInGuide = useCallback(() => {
    showGuide();
    // The field can take focus once the guide is back in the layout. The
    // picture must stay where it is, so nothing scrolls to reach the field.
    requestAnimationFrame(() => {
      guideSearchInput()?.focus({ preventScroll: true });
    });
  }, [showGuide]);
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
            reading: nowPlaying.reading,
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
  // the masthead, pocket in the guide's toolbar.
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
      data-playing={playingChannel !== null}
      data-dock={dock}
      data-external={picture?.external === true}
      data-acceptance-catalog-shell
    >
      <ShellMasthead
        now={now}
        chrome={
          layout === "theater"
            ? {
                layout,
                search,
                guideOpen: effectiveMode === "guide",
                guideForced,
                onToggleGuide: toggleGuide,
              }
            : { layout, onFind: findInGuide }
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
                picture={picture}
                controlsRef={setControlsSlot}
                onPreparePlayback={preparePlayback}
                onTuneVariant={tuneVariant}
              />
            }
            rail={
              shownChannel === null ? null : layout === "pocket" ? (
                <ChannelBar
                  stops={stops}
                  current={shownChannel.id}
                  now={now}
                  onZap={zap}
                  onShowGuide={showGuideFromBar}
                />
              ) : watching && stops.length > 0 ? (
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
            onShowPicture={layout === "pocket" ? showPicture : null}
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
          mode={effectiveMode}
          time={
            layout === "pocket"
              ? {
                  open: timeRow === "open",
                  // A chosen time the clock has reached is now.
                  chosen:
                    chosenTime !== null && chosenTime.getTime() > now.getTime()
                      ? chosenTime
                      : null,
                  options: laterTimes(now, guideClock.window.endsAt),
                  onToggle: toggleTimeRow,
                  onChoose: setChosenTime,
                }
              : null
          }
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

/** What the masthead carries between the wordmark and the clock. */
type MastheadChrome =
  | {
      readonly layout: "theater";
      readonly search: ReactNode;
      readonly guideOpen: boolean;
      /** The guide stays open while there is no picture in the page. */
      readonly guideForced: boolean;
      readonly onToggleGuide: () => void;
    }
  | {
      readonly layout: "pocket";
      /** Opens the guide with its search field ready to type in. */
      readonly onFind: () => void;
    };

function ShellMasthead({
  now,
  chrome,
}: {
  readonly now: Date;
  readonly chrome: MastheadChrome;
}) {
  return (
    <header className="shell__masthead">
      <strong className="shell__wordmark">Sparrow</strong>
      {chrome.layout === "pocket" ? (
        <button
          className="shell__find"
          type="button"
          aria-label="Search"
          onClick={chrome.onFind}
        >
          <Search aria-hidden="true" />
        </button>
      ) : (
        <>
          <div className="shell__search">{chrome.search}</div>
          <button
            className="shell__guide-toggle"
            type="button"
            aria-pressed={chrome.guideOpen}
            disabled={chrome.guideForced}
            onClick={chrome.onToggleGuide}
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
    // The schedule, and with it the descriptions, is read once it is tuned.
    reading: "descriptions",
  };
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
