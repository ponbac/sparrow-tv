import { useQueryClient } from "@tanstack/react-query";
import { List, Search } from "lucide-react";
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { InstalledPlaybackChannel } from "../playback/installed-playback-state";
import type { HostedPlaybackEngine } from "../playback/mpegts-engine";
import type { InstalledPlaybackEngine } from "../playback/installed-playback-engine";
import { useAndroidPlayerWindow } from "../playback/android-player-bridge";
import { PlaybackLoadBoundary } from "../playback/playback-load-boundary";
import {
  clientSchemas,
  type CatalogStatus,
  type ChannelId,
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
import { familyProgrammes, lowerQualityVariant } from "../guide/guide-families";
import { focusGuideAfterFullscreen } from "../guide/guide-navigation";
import {
  clockLabel,
  clockWindow,
  GUIDE_SPAN_MS,
  laterTimes,
  LATER_SPAN_MS,
} from "../guide/guide-window";
import type { GuideView } from "../guide/channel-group-lane";
import type { GuidePreviewSource } from "../guide/guide-preview";
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
  type TelevisionFullscreenTune,
} from "../stage/stage-chrome";
import {
  focusPictureReturn,
  guideSearchInput,
  showTelevisionPicture,
} from "../stage/stage-dom";
import { useIdleChrome } from "../stage/use-idle-chrome";
import { guideRowProgramme, useNowPlaying } from "../stage/use-now-playing";
import { usePicturePin } from "../stage/use-picture-pin";
import { useStageKeys } from "../stage/use-stage-keys";
import {
  usePictureOverlay,
  useSplitPocket,
  useStageFullscreenTarget,
  useStageLayout,
} from "../stage/use-stage-layout";
import { useTurnFullscreen } from "../stage/use-turn-fullscreen";
import { useTelevisionControls } from "../stage/use-television-controls";
import { TelevisionNowProvider, type TelevisionNow } from "../stage/television-now-context";
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
import {
  guideProgrammesByChannel,
  type SearchOnNow,
} from "../guide/search-on-now";
import "../stage/shell.css";
import "../stage/pocket.css";
import "../stage/theater.css";
import "../stage/television.css";

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

type PlayingChannel = ChannelSummary & Pick<InstalledPlaybackChannel, "qualityFallback">;

/** Owns the shell's catalog reads, the playing Channel, and source controls. */
export function CatalogBrowser(props: CatalogBrowserProps) {
  const runtime = props.runtime ?? "hosted";
  // Android injects this marker before the installed document starts.
  const television =
    runtime === "installed" && document.documentElement.hasAttribute("data-television");
  const client = props.client;
  const now = useGuideClock();
  const queryClient = useQueryClient();
  const synchronization = useCatalogSynchronization(client);
  const [activeGroup, setActiveGroup] = useState<string | null>(null);
  const groupExclusions = useBoardGroupExclusions();
  const boardGroup = resolvedActiveGroup(activeGroup, groupExclusions.excluded);
  const { preferences: variantPreferences, prefer: preferVariant } =
    useVariantPreferences();
  const [playingChannel, setPlayingChannel] = useState<PlayingChannel | null>(
    null,
  );
  const [lastPlayedChannel, setLastPlayedChannel] = useState<ChannelId | null>(
    null,
  );
  if (playingChannel !== null && lastPlayedChannel !== playingChannel.id) {
    setLastPlayedChannel(playingChannel.id);
  }
  const [fullscreenTune, setFullscreenTune] =
    useState<TelevisionFullscreenTune | null>(null);
  // The Channel an arrow press has moved to. The info block shows it at once;
  // it is tuned when no further press follows.
  const [pendingZap, setPendingZap] = useState<{ readonly channel: ChannelSummary } | null>(null);
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
      setFullscreenTune(null);
      setPlayingChannel(next);
    },
    [cancelZap],
  );
  const pictureOverlay = usePictureOverlay(runtime, client);
  const layout = useStageLayout(pictureOverlay);
  const splitPocket = useSplitPocket(television);
  const fullscreenTarget = useStageFullscreenTarget(layout);
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
  // The TV Guide's two layouts. The timeline browses along time, so from the
  // first time it opens the guide reads the longer window, as the time row does.
  const [guideView, setGuideView] = useState<"now" | "timeline">("now");
  const [timelineOpened, setTimelineOpened] = useState(false);
  const toggleGuideView = useCallback(() => {
    setTimelineOpened(true);
    setGuideView((current) => (current === "now" ? "timeline" : "now"));
  }, []);
  const tvGuideView = useMemo<GuideView | null>(
    () => (television ? { current: guideView, onToggle: toggleGuideView } : null),
    [guideView, television, toggleGuideView],
  );
  const guideSpan =
    (layout === "pocket" && timeRow !== "unused") || (television && timelineOpened)
      ? LATER_SPAN_MS
      : GUIDE_SPAN_MS;
  const guideClock = useMemo(() => {
    const window = clockWindow(now, guideSpan);
    return {
      window,
      startsAt: clientSchemas.isoInstant.parse(window.startsAt.toISOString()),
      endsAt: clientSchemas.isoInstant.parse(window.endsAt.toISOString()),
    };
  }, [guideSpan, now]);
  // The viewer's choice between the full picture and the guide. The shell
  // shows the guide regardless while there is no picture in the page, and
  // always on a TV: there the picture plays docked in the Guide's band, and
  // the full picture is the player's own fullscreen.
  const [mode, setMode] = useState<"watch" | "guide">("watch");
  const [picture, setPicture] = useState<StagePicture | null>(null);
  const reportPicture = useCallback((next: StagePicture | null) => {
    setPicture((current) => (samePicture(current, next) ? current : next));
  }, []);
  const [controlsSlot, setControlsSlot] = useState<HTMLElement | null>(null);
  const chrome = useMemo<StageChrome>(
    () => ({
      // Both layouts keep the controls in the info block, clear of the picture.
      // A fullscreen player section draws its own bar (controlPresentation).
      controlsSlot,
      television,
      fullscreenTune,
      // A TV row has room for every control; the focused one shows its label.
      controls: television ? "bar" : "compact",
      // Only roomy Theater enters root fullscreen, which outlives Stop and a
      // change of Channel; pocket takes the player section alone.
      fullscreenTarget,
      // Above pocket's controls is the picture, which nothing may cover.
      menuSide: layout === "theater" ? "top" : "bottom",
      reportPicture,
    }),
    [controlsSlot, fullscreenTarget, fullscreenTune, layout, reportPicture, television],
  );
  const guideForced =
    television || playingChannel === null || picture?.external === true;
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
  const { pictureInPicture } = useAndroidPlayerWindow();
  usePicturePin(follows, pictureInPicture, television, effectiveMode);
  const watching = layout === "theater" && effectiveMode === "watch";
  const chromeVisibility = useIdleChrome(
    watching && picture?.state === "playing",
  );
  // A phone turned on its side while its picture plays wants the picture.
  useTurnFullscreen({
    watching:
      !television && layout === "pocket" &&
      effectiveMode === "watch" &&
      picture?.state === "playing" &&
      !picture.external,
    follows,
  });

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

  // `tune` and `stop` never change: Agent Control binds to them once. A TV
  // has no watch mode: a tune there plays in the Guide's docked picture.
  const tune = useCallback(
    (channel: ChannelSummary) => {
      setPlaying(channel);
      if (!television) setMode("watch");
    },
    [setPlaying, television],
  );
  const tuneVariant = useCallback(
    (channel: ChannelSummary) => {
      preferVariant(channel);
      tune(channel);
    },
    [preferVariant, tune],
  );
  // What the Guide's handlers compare a Center press with. A ref, so the
  // Guide's rows keep one handler while Channels change.
  const playingRef = useRef(playingChannel);
  useLayoutEffect(() => {
    playingRef.current = playingChannel;
  });
  // A held OK repeats its keydown, and each repeat presses the focused
  // control again: the first press tunes a Channel, a repeat would take it
  // fullscreen. Only a fresh press counts as the fullscreen gesture.
  // Android's WebView never flags a held key's repeats, so a keydown of an
  // activation key that is still down counts as a repeat whatever the flag
  // says. The press that follows a keydown is judged by that keydown.
  const heldPress = useRef(false);
  useEffect(() => {
    if (!television) return;
    const down = new Set<string>();
    const press = (event: KeyboardEvent) => {
      const key = activationKey(event);
      if (key === null) {
        heldPress.current = false;
        return;
      }
      heldPress.current = event.repeat || down.has(key);
      down.add(key);
    };
    const lift = (event: KeyboardEvent) => {
      const key = activationKey(event);
      if (key !== null) down.delete(key);
      heldPress.current = false;
    };
    // A keyup lost to another window or a hidden page must not leave a key
    // held for good.
    const forget = () => {
      down.clear();
      heldPress.current = false;
    };
    const release = () => {
      heldPress.current = false;
    };
    document.addEventListener("keydown", press, true);
    document.addEventListener("keyup", lift, true);
    document.addEventListener("pointerdown", release, true);
    document.addEventListener("visibilitychange", forget);
    window.addEventListener("blur", forget);
    return () => {
      document.removeEventListener("keydown", press, true);
      document.removeEventListener("keyup", lift, true);
      document.removeEventListener("pointerdown", release, true);
      document.removeEventListener("visibilitychange", forget);
      window.removeEventListener("blur", forget);
    };
  }, [television]);
  // The Guide control that took the picture fullscreen, where focus returns.
  const fullscreenOrigin = useRef<HTMLElement | null>(null);
  const enterTelevisionFullscreen = useCallback((channel: ChannelId) => {
    if (document.fullscreenElement != null || heldPress.current) return;
    // The viewer chose the picture that plays: a Channel change still
    // queued from the Channel keys must not switch it underneath.
    cancelZap();
    fullscreenOrigin.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const shown = showTelevisionPicture();
    if (shown === "fullscreen") return;
    fullscreenOrigin.current = null;
    if (shown === "load-notice") return;
    // The player's code is still loading: it takes the request when it mounts,
    // if the press still counts by then.
    if (navigator.userActivation?.isActive !== true) return;
    let pending = true;
    const request: TelevisionFullscreenTune = {
      channel,
      take: () => {
        if (!pending) return false;
        pending = false;
        setFullscreenTune((current) => (current === request ? null : current));
        return true;
      },
    };
    setFullscreenTune(request);
  }, [cancelZap]);
  // Leaving fullscreen on a TV returns to the Guide, with focus on the
  // control that left it, or the playing Channel's row.
  useEffect(() => {
    if (!television) return;
    const returnToGuide = () => {
      if (document.fullscreenElement != null) return;
      const origin = fullscreenOrigin.current;
      fullscreenOrigin.current = null;
      focusGuideAfterFullscreen(origin);
    };
    document.addEventListener("fullscreenchange", returnToGuide);
    return () => document.removeEventListener("fullscreenchange", returnToGuide);
  }, [television]);
  // The TV's Center on a Channel: a Channel that is not playing starts in the
  // docked picture; the exact Channel that plays goes fullscreen.
  const tuneFromGuide = useCallback(
    (channel: ChannelSummary) => {
      if (television && playingRef.current?.id === channel.id) {
        enterTelevisionFullscreen(channel.id);
        return;
      }
      tune(channel);
    },
    [enterTelevisionFullscreen, television, tune],
  );
  // The band's caption, and a tap on the docked picture, on a TV.
  const showTelevisionFullscreen = useCallback(() => {
    const playing = playingRef.current;
    if (playing !== null) enterTelevisionFullscreen(playing.id);
  }, [enterTelevisionFullscreen]);
  const tuneVariantFromGuide = useCallback(
    (channel: ChannelSummary) => {
      preferVariant(channel);
      tuneFromGuide(channel);
    },
    [preferVariant, tuneFromGuide],
  );
  const stop = useCallback(() => {
    setPlaying(null);
  }, [setPlaying]);
  const applyInstalledConfiguration = useCallback(
    (nextStatus: CatalogStatus) => {
      setActiveGroup(null);
      setLastPlayedChannel(null);
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
  const shownChannel = pendingZap?.channel ?? playingChannel;
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
    setPendingZap((pending) => {
      const from = pending?.channel ?? playingChannel;
      const next = neighbouringZapStop(stops, from.id, direction);
      return next === null ? pending : { channel: next.target.channel };
    });
  };
  useLayoutEffect(() => {
    if (pendingZap === null) return;
    const timer = setTimeout(() => {
      zapTimer.current = null;
      setPendingZap(null);
      setFullscreenTune(null);
      // A queued zap never starts a Playback Session after Stop.
      setPlayingChannel((current) =>
        current === null ? current : pendingZap.channel,
      );
    }, ZAP_COMMIT_MS);
    zapTimer.current = timer;
    return () => {
      clearTimeout(timer);
      if (zapTimer.current === timer) zapTimer.current = null;
    };
  }, [pendingZap]);
  const toggleGuide = useCallback(() => {
    setMode((current) => (current === "guide" ? "watch" : "guide"));
  }, []);
  const showPicture = useCallback(() => {
    setMode("watch");
  }, []);
  const showGuide = useCallback(() => {
    setFullscreenTune(null);
    setMode("guide");
  }, []);
  const showGuideFromBar = useCallback(() => {
    showGuide();
    // The bar leaves the layout with the watch screen. Focus moves to the
    // way back once the band is laid out, so a keyboard keeps its place.
    if (!television) requestAnimationFrame(focusPictureReturn);
  }, [showGuide, television]);
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
    television,
    mode: effectiveMode,
    guideForced,
    onToggleGuide: toggleGuide,
    onWatch: showPicture,
    onZap: zap,
  });
  useTelevisionControls({
    enabled: television,
    onShowGuide: showGuide,
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
  const fallBack = useCallback(
    (failedId: ChannelSummary["id"]) => {
      if (playingChannel?.id !== failedId || !playingChannel.variant ||
          playingChannel.qualityFallback || pendingZap !== null) return;

      const lower =
        lowerQualityVariant(nowPlaying.family?.variants ?? [], playingChannel) ??
        lowerQualityVariant(guideCatalog.rows, playingChannel);
      if (!lower?.variant) return;
      const qualityFallback = { from: playingChannel.variant.quality, to: lower.variant.quality };
      setPlayingChannel((current) =>
        current === playingChannel ? { ...lower, qualityFallback } : current,
      );
    },
    [guideCatalog.rows, nowPlaying.family, pendingZap, playingChannel],
  );
  const visibleGuideNowRef = useRef(now);
  const guideHiddenInPocket =
    layout === "pocket" && !splitPocket && effectiveMode === "watch";
  if (!guideHiddenInPocket) {
    visibleGuideNowRef.current = now;
  }
  const guideNow = visibleGuideNowRef.current;

  const guideTime = useMemo(
    () =>
      // The TV timeline replaces the row of times.
      layout === "pocket" && !television
        ? {
            open: timeRow === "open",
            // A chosen time the clock has reached is now.
            chosen:
              chosenTime !== null && chosenTime.getTime() > guideNow.getTime()
                ? chosenTime
                : null,
            options: laterTimes(guideNow, guideClock.window.endsAt),
            onToggle: toggleTimeRow,
            onChoose: setChosenTime,
          }
        : null,
    [chosenTime, guideClock.window.endsAt, guideNow, layout, television, timeRow, toggleTimeRow],
  );
  const guidePreview = useMemo<GuidePreviewSource | null>(
    () => (television ? { client, generation: authoritativeGeneration } : null),
    [authoritativeGeneration, client, television],
  );
  const guideEmpty = useMemo(
    () => guideEmptyState(runtime, status, browseEnabled),
    [browseEnabled, runtime, status],
  );
  const feeds = useMemo(
    () =>
      props.runtime === "installed" ? (
        <FeedsDialog
          television={television}
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
          television={television}
          runtime="hosted"
          status={status}
          refreshing={synchronization.refreshing}
          refreshResult={synchronization.refreshResult}
          latestEvent={synchronization.latestEvent}
          onRefresh={synchronization.requestRefresh}
        />
      ),
    [
      applyInstalledConfiguration,
      props,
      status,
      synchronization.latestEvent,
      synchronization.refreshResult,
      synchronization.refreshing,
      synchronization.requestRefresh,
      television,
    ],
  );

  // What the Guide has read, so the TV's Find a channel sheet can say what
  // is on for a hit without a read of its own.
  const guideProgrammes = useMemo(
    () => guideProgrammesByChannel(guideCatalog.rows),
    [guideCatalog.rows],
  );
  const searchOnNow = useMemo<SearchOnNow | undefined>(
    () => (television ? { programmes: guideProgrammes, now: guideNow } : undefined),
    [guideNow, guideProgrammes, television],
  );
  // The one element whose parent depends on the layout: Theater shows it in
  // the masthead, pocket in the guide's toolbar.
  const search = useMemo(
    () => (
      <BoardSearch
        television={television}
        onNow={searchOnNow}
        client={client}
        generation={authoritativeGeneration}
        excludedGroups={groupExclusions.excluded}
        variantPreferences={variantPreferences}
        onGenerationMismatch={synchronization.retryStatus}
        onPreparePlayback={preparePlayback}
        // A TV result is a whole guide row: it tunes like the row does and
        // leaves the quality preference to the Guide's quality chips.
        onTune={television ? tuneFromGuide : tuneVariantFromGuide}
      />
    ),
    [
      authoritativeGeneration,
      client,
      groupExclusions.excluded,
      preparePlayback,
      searchOnNow,
      synchronization.retryStatus,
      television,
      tuneFromGuide,
      tuneVariantFromGuide,
      variantPreferences,
    ],
  );
  const guideStatus = useMemo(
    () => (
      <>
        <StatusReadout source="m3u" status={status} now={guideNow} />
        <StatusReadout source="epg" status={status} now={guideNow} />
      </>
    ),
    [guideNow, status],
  );

  if (synchronization.statusPending) {
    return <CatalogLoading />;
  }

  const player = renderPlayer({
    props,
    playingChannel,
    ...(shownChannel === null ? {} : { displayName: shownChannel.name }),
    onStop: stop,
    ...(television ? { onInitialStall: fallBack } : {}),
  });
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
        : zapSubject(pendingZap.channel, zapStopOf(stops, pendingZap.channel.id));

  const televisionNow: TelevisionNow | null = television
    ? {
        subject,
        playingChannel: playingChannel?.id ?? null,
        now,
        onPreparePlayback: preparePlayback,
        onTuneVariant: tuneVariant,
      }
    : null;

  // Keep a held native picture at the same origin when the viewer changes mode.
  const inlineMasthead = television && layout === "pocket" && (playingChannel === null || dock);
  const mastheadInGuide = inlineMasthead && effectiveMode === "guide";
  const identity = {
    wordmark: <strong className="shell__wordmark">Sparrow</strong>,
    clock: (
      <time className="shell__clock" dateTime={now.toISOString()}>
        {clockLabel(now)}
      </time>
    ),
  };
  return (
    <div
      className="shell"
      data-layout={layout}
      data-pocket={
        layout !== "pocket"
          ? undefined
          : television
            ? // The TV lays its own pocket out: television.css.
              "television"
            : splitPocket
              ? "split"
              : "stacked"
      }
      data-mode={effectiveMode}
      data-chrome={chromeVisibility}
      data-playing={playingChannel !== null}
      data-dock={dock}
      data-inline-masthead={inlineMasthead || undefined}
      data-external={picture?.external === true}
      data-acceptance-catalog-shell
    >
      {mastheadInGuide ? null : (
        <ShellMasthead
          {...identity}
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
      )}
      {status !== null && isRetainedCatalog(status) ? (
        <aside
          className="shell__retained"
          data-acceptance-retained
          role="status"
        >
          Showing the saved catalog. A fresh source check is pending.
        </aside>
      ) : null}
      <main
        className="shell__workspace"
        onKeyDown={(event) => {
          if (
            layout === "pocket" &&
            timeRow === "open" &&
            effectiveMode === "guide" &&
            !event.defaultPrevented &&
            !event.nativeEvent.isComposing &&
            event.key === "Escape"
          ) {
            event.preventDefault();
            toggleTimeRow();
          }
        }}
      >
        <StageChromeProvider value={chrome}>
          <TelevisionNowProvider value={televisionNow}>
          <Stage
            television={television}
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
                qualitySwitch={!television}
                television={television}
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
            onShowPicture={
              television
                ? showTelevisionFullscreen
                : layout === "pocket"
                  ? showPicture
                  : null
            }
          />
          </TelevisionNowProvider>
        </StageChromeProvider>
        <ProgrammeGuide
          television={television}
          masthead={mastheadInGuide ? identity : undefined}
          rows={rows}
          groups={groups}
          activeGroup={boardGroup}
          window={guideClock.window}
          now={guideNow}
          playingChannel={playingChannel?.id ?? null}
          lastPlayedChannel={lastPlayedChannel}
          variantPreferences={variantPreferences}
          layout={layout}
          mode={effectiveMode}
          time={guideTime}
          view={tvGuideView}
          preview={guidePreview}
          loading={guideCatalog.loading}
          replacing={guideCatalog.replacing}
          error={guideError}
          hasMore={guideCatalog.hasMore}
          loadingMore={guideCatalog.loadingMore}
          emptyState={guideEmpty}
          onSelectGroup={setActiveGroup}
          onPrefetchGroup={guideCatalog.prefetchGroup}
          excludedGroups={groupExclusions.excluded}
          onSetGroupExcluded={groupExclusions.setExcluded}
          onRestoreExcludedGroups={groupExclusions.restoreAll}
          onPreparePlayback={preparePlayback}
          onTune={tuneFromGuide}
          onTuneVariant={tuneVariantFromGuide}
          onRetry={retryGuide}
          onLoadMore={guideCatalog.loadMore}
          search={layout === "theater" ? null : search}
          feeds={feeds}
          status={guideStatus}
        />
      </main>
    </div>
  );
}

function renderPlayer({
  props,
  playingChannel,
  displayName,
  onInitialStall,
  onStop,
}: {
  readonly props: CatalogBrowserProps;
  readonly playingChannel: PlayingChannel | null;
  readonly displayName?: string;
  readonly onInitialStall?: (channel: ChannelSummary["id"]) => void;
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
            {...(displayName === undefined ? {} : { displayName })}
            {...(onInitialStall ? { onInitialStall } : {})}
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
  wordmark,
  clock,
  chrome,
}: {
  readonly wordmark: ReactNode;
  readonly clock: ReactNode;
  readonly chrome: MastheadChrome;
}) {
  return (
    <header className="shell__masthead">
      {wordmark}
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
      {clock}
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

/**
 * The key that presses a focused control, named the same for both Enters,
 * or null for any other key.
 */
function activationKey(event: KeyboardEvent): "Enter" | " " | null {
  if (event.key === "Enter" || event.code === "NumpadEnter") return "Enter";
  if (event.key === " " || event.key === "Spacebar") return " ";
  return null;
}
