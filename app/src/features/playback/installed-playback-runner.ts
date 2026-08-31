import type {
  AudioTrackId,
  InstalledPlaybackSession,
  InstalledPlaybackTransport,
  InstalledSparrowClient,
} from "../../client/contracts";
import { installedClientPlaybackFailure } from "./playback-failure";
import {
  installedPlaybackEngine,
  type InstalledPlaybackEngine,
  type InstalledPlaybackHandle,
} from "./installed-playback-engine";
import {
  copyInstalledPlaybackDiagnostics,
  installedPlaybackDiagnostics,
  type InstalledPlaybackTransition,
  type PlaybackDiagnosticsClipboard,
} from "./installed-playback-diagnostics";
import {
  createInstalledPlaybackState,
  reduceInstalledPlaybackState,
  type InstalledPlaybackChannel,
  type InstalledPlaybackEvent,
  type InstalledPlaybackFailure,
  type InstalledPlaybackPauseCause,
  type InstalledPlaybackStartReason,
  type InstalledPlaybackState,
} from "./installed-playback-state";

const DEFAULT_RECOVERY_DELAYS_MS = [1_000, 5_000, 15_000] as const;
const DEFAULT_STABLE_RESET_MS = 60_000;
const MAX_RETAINED_TRANSITIONS = 20;

/** Narrow installed client seam required by one Playback Session runner. */
export type InstalledPlaybackSessionClient = Pick<
  InstalledSparrowClient,
  "createPlaybackSession"
>;

/** Controlled clock used for recovery deadlines and bounded diagnostics. */
export interface InstalledPlaybackClock {
  readonly now: () => number;
}

/** Controlled timer seam whose returned release is idempotent. */
export interface InstalledPlaybackScheduler {
  readonly schedule: (delayMs: number, task: () => void) => () => void;
}

/** Construction options for the session-owning installed playback runner. */
export interface InstalledPlaybackRunnerOptions {
  readonly client: InstalledPlaybackSessionClient;
  readonly engine?: InstalledPlaybackEngine;
  readonly clock?: InstalledPlaybackClock;
  readonly scheduler?: InstalledPlaybackScheduler;
  readonly recoveryDelaysMs?: readonly [number, number, number];
  readonly stableResetMs?: number;
  readonly initiallyVisible?: boolean;
  readonly initiallyForeground?: boolean;
}

/**
 * Owns all asynchronous work for one installed player surface. Operations are
 * serialized, every callback is epoch-correlated, and replacements wait for a
 * confirmed release of their predecessor.
 */
export class InstalledPlaybackRunner {
  readonly #client: InstalledPlaybackSessionClient;
  readonly #engine: InstalledPlaybackEngine;
  readonly #clock: InstalledPlaybackClock;
  readonly #scheduler: InstalledPlaybackScheduler;
  readonly #recoveryDelaysMs: readonly [number, number, number];
  readonly #stableResetMs: number;
  readonly #listeners = new Set<() => void>();
  readonly #transitions: InstalledPlaybackTransition[] = [];
  readonly #deferredAndroidHandles = new Map<
    InstalledPlaybackSession,
    Set<InstalledPlaybackHandle>
  >();
  #state: InstalledPlaybackState;
  #session: InstalledPlaybackSession | null = null;
  #video: HTMLVideoElement | null = null;
  #handle: InstalledPlaybackHandle | null = null;
  #transport: InstalledPlaybackTransport | null = null;
  #openController: AbortController | null = null;
  #cancelRetry: (() => void) | null = null;
  #cancelStableReset: (() => void) | null = null;
  #queue: Promise<void> = Promise.resolve();
  #sessionEpoch = 0;
  #transportEpoch = 0;
  #activityEpoch = 0;
  #cleanupBlocked = false;
  #documentVisible: boolean;
  #foreground: boolean;

  constructor(options: InstalledPlaybackRunnerOptions) {
    this.#client = options.client;
    this.#engine = options.engine ?? installedPlaybackEngine;
    this.#clock = options.clock ?? { now: () => Date.now() };
    this.#scheduler = options.scheduler ?? browserScheduler();
    this.#recoveryDelaysMs = normalizeRecoveryDelays(
      options.recoveryDelaysMs ?? DEFAULT_RECOVERY_DELAYS_MS,
    );
    this.#stableResetMs = normalizeDelay(
      options.stableResetMs ?? DEFAULT_STABLE_RESET_MS,
      DEFAULT_STABLE_RESET_MS,
    );
    this.#documentVisible = options.initiallyVisible ?? true;
    this.#foreground = options.initiallyForeground ?? true;
    this.#state = createInstalledPlaybackState(
      this.#documentVisible && this.#foreground,
    );
  }

  /** Stable subscription interface used by React's external-store hook. */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  /** Returns the current immutable Playback Session state. */
  readonly getSnapshot = (): InstalledPlaybackState => this.#state;

  /**
   * Replaces the selected Channel after final cleanup of any predecessor. Rapid
   * selections coalesce so only the latest pending Channel creates a session.
   */
  select(
    channel: InstalledPlaybackChannel,
    video: HTMLVideoElement,
  ): Promise<void> {
    const sessionEpoch = ++this.#sessionEpoch;
    const transportEpoch = ++this.#transportEpoch;
    const hadSession = this.#session !== null;
    this.#video = video;
    this.#cancelLocalTransport();
    this.#dispatch(
      hadSession
        ? {
            _tag: "stopping",
            nextChannel: channel,
            sessionEpoch,
            transportEpoch,
          }
        : { _tag: "select", channel, sessionEpoch, transportEpoch },
    );

    return this.#enqueue(async () => {
      const cleaned = await this.#stopCurrentSession();
      if (!cleaned) {
        if (sessionEpoch === this.#sessionEpoch) {
          this.#dispatch({
            _tag: "select",
            channel,
            sessionEpoch,
            transportEpoch: this.#transportEpoch,
          });
          this.#failCleanup();
        }
        return;
      }
      if (sessionEpoch !== this.#sessionEpoch || this.#cleanupBlocked) {
        return;
      }

      let session: InstalledPlaybackSession;
      try {
        session = this.#client.createPlaybackSession({ id: channel.id });
      } catch {
        this.#dispatch({
          _tag: "select",
          channel,
          sessionEpoch,
          transportEpoch: this.#transportEpoch,
        });
        this.#dispatch({
          _tag: "failed",
          failure: "source-unavailable",
          attemptsUsed: 0,
          canRestart: true,
        });
        return;
      }
      if (sessionEpoch !== this.#sessionEpoch) {
        await safeStop(session);
        return;
      }
      this.#session = session;
      this.#dispatch({
        _tag: "select",
        channel,
        sessionEpoch,
        transportEpoch: this.#transportEpoch,
      });
      await this.#open(sessionEpoch, "selection", true);
    });
  }

  /** Releases live transport work while retaining the pinned session intent. */
  pause(): Promise<void> {
    return this.#suspendFor("user");
  }

  /** Reopens a user- or visibility-paused session at the current live edge. */
  resume(): Promise<void> {
    const phase = this.#state.phase;
    if (phase._tag !== "paused" || this.#session === null) {
      return Promise.resolve();
    }
    if (!this.#state.visible) {
      this.#dispatch({
        _tag: "paused",
        cause: this.#foreground ? "visibility" : "lifecycle",
        resumeWhenVisible: true,
      });
      return Promise.resolve();
    }
    const sessionEpoch = this.#sessionEpoch;
    return this.#enqueue(() => this.#open(sessionEpoch, "resume", false));
  }

  /** Manually restarts with a cleared recovery budget and confirmed no-overlap. */
  restart(): Promise<void> {
    const channel = this.#state.channel;
    const video = this.#video;
    if (channel === null || video === null || this.#cleanupBlocked) {
      return Promise.resolve();
    }
    const phase = this.#state.phase;
    if (phase._tag === "failed") {
      return phase.canRestart ? this.select(channel, video) : Promise.resolve();
    }
    const session = this.#session;
    if (session === null) {
      return this.select(channel, video);
    }

    const sessionEpoch = this.#sessionEpoch;
    const transportEpoch = ++this.#transportEpoch;
    this.#cancelLocalTransport();
    this.#dispatch({
      _tag: "suspending",
      next: { _tag: "restart" },
      transportEpoch,
    });
    return this.#enqueue(async () => {
      if (!this.#matchesSession(sessionEpoch, session)) {
        return;
      }
      const suspended = await this.#suspendSessionTransport(session);
      if (!this.#matchesSession(sessionEpoch, session)) {
        return;
      }
      if (!suspended) {
        this.#failCleanup();
        return;
      }
      this.#dispatch({ _tag: "stable" });
      if (!this.#state.visible) {
        this.#dispatch({
          _tag: "paused",
          cause: this.#inactivityCause(),
          resumeWhenVisible: true,
        });
        return;
      }
      await this.#open(sessionEpoch, "restart", false);
    });
  }

  /**
   * Atomically replaces the active native transport with one filtered to the
   * requested Audio Track. The expected handle and both epochs make queued or
   * late selections harmless after any competing lifecycle transition.
   */
  selectAudio(trackId: AudioTrackId): Promise<void> {
    const session = this.#session;
    const transport = this.#transport;
    if (
      session === null ||
      transport === null ||
      transport._tag !== "tauri-native-stream" ||
      this.#cleanupBlocked ||
      !this.#state.visible ||
      !transport.tracks.some((track) => track.id === trackId) ||
      transport.tracks.some((track) => track.id === trackId && track.selected)
    ) {
      return Promise.resolve();
    }

    const sessionEpoch = this.#sessionEpoch;
    const transportEpoch = ++this.#transportEpoch;
    const expectedStreamHandle = transport.streamHandle;
    this.#cancelLocalTransport();
    this.#dispatch({
      _tag: "replacing-audio",
      requestedTrackId: trackId,
      transportEpoch,
    });

    return this.#enqueue(async () => {
      if (
        !this.#matchesTransport(sessionEpoch, transportEpoch, session) ||
        this.#state.phase._tag !== "replacing-audio"
      ) {
        return;
      }

      const controller = new AbortController();
      this.#openController = controller;
      let result: Awaited<ReturnType<InstalledPlaybackSession["restart"]>>;
      try {
        result = await session.restart({
          expectedStreamHandle,
          intent: { _tag: "select-audio", audioTrackId: trackId },
          signal: controller.signal,
        });
      } catch {
        if (this.#openController === controller) {
          this.#openController = null;
        }
        if (this.#matchesTransport(sessionEpoch, transportEpoch, session)) {
          await this.#afterFailure(
            sessionEpoch,
            transportEpoch,
            "source-unavailable",
            true,
          );
        }
        return;
      }
      if (this.#openController === controller) {
        this.#openController = null;
      }
      if (!this.#matchesTransport(sessionEpoch, transportEpoch, session)) {
        return;
      }
      if (!this.#state.visible) {
        const suspended = await this.#suspendSessionTransport(session);
        if (!this.#matchesTransport(sessionEpoch, transportEpoch, session)) {
          return;
        }
        if (!suspended) {
          this.#failCleanup();
          return;
        }
        this.#dispatch({
          _tag: "paused",
          cause: "visibility",
          resumeWhenVisible: true,
        });
        return;
      }
      if (!result.ok) {
        if (result.error._tag === "cancelled") {
          await this.#pauseAfterNativeCancellation(
            sessionEpoch,
            transportEpoch,
            session,
          );
          return;
        }
        const mapped = installedClientPlaybackFailure(result.error);
        await this.#afterFailure(
          sessionEpoch,
          transportEpoch,
          mapped.failure,
          mapped.retryable,
        );
        return;
      }

      this.#releaseDeferredAndroidHandles(session);
      await this.#startTransport(
        sessionEpoch,
        transportEpoch,
        session,
        this.#video,
        result.value,
      );
    });
  }

  /**
   * Final-stops the current resource exactly once. The result is false when the
   * installed shell does not confirm cleanup, in which case replacement blocks.
   */
  stop(): Promise<boolean> {
    const sessionEpoch = ++this.#sessionEpoch;
    const transportEpoch = ++this.#transportEpoch;
    this.#cancelLocalTransport();
    if (this.#state.channel !== null) {
      this.#dispatch({
        _tag: "stopping",
        nextChannel: null,
        sessionEpoch,
        transportEpoch,
      });
    }
    return this.#enqueue(async () => {
      const cleaned = await this.#stopCurrentSession();
      if (sessionEpoch !== this.#sessionEpoch) {
        return cleaned;
      }
      if (cleaned) {
        this.#cleanupBlocked = false;
        this.#dispatch({ _tag: "stopped" });
      } else if (this.#state.channel !== null) {
        this.#failCleanup();
      }
      return cleaned;
    });
  }

  /** Suspends active/recovering work while hidden and resumes only visibility-owned pauses. */
  setVisible(visible: boolean): Promise<void> {
    if (visible === this.#documentVisible) {
      return Promise.resolve();
    }
    this.#documentVisible = visible;
    return this.#setPresentationActive("visibility");
  }

  /** Suspends for native app background/lock and resumes only prior-active intent. */
  setForeground(foreground: boolean): Promise<void> {
    if (foreground === this.#foreground) {
      return Promise.resolve();
    }
    this.#foreground = foreground;
    return this.#setPresentationActive("lifecycle");
  }

  #setPresentationActive(
    cause: Extract<InstalledPlaybackPauseCause, "visibility" | "lifecycle">,
  ): Promise<void> {
    const active = this.#documentVisible && this.#foreground;
    if (active === this.#state.visible) {
      return Promise.resolve();
    }
    this.#dispatch({ _tag: "visibility", visible: active });
    if (!active) {
      switch (this.#state.phase._tag) {
        case "starting":
        case "playing":
        case "autoplay-blocked":
        case "replacing-audio":
        case "recovering":
          return this.#suspendFor(cause);
        case "idle":
        case "suspending":
        case "paused":
        case "failed":
        case "stopping":
          return Promise.resolve();
      }
    }
    const phase = this.#state.phase;
    return phase._tag === "paused" &&
      phase.cause !== "user" &&
      phase.resumeWhenVisible
      ? this.resume()
      : Promise.resolve();
  }

  /** Applies a parsed 0..1 volume to state and the current/recreated video. */
  setVolume(volume: number): void {
    this.#dispatch({ _tag: "volume", volume });
    this.#applyControls();
  }

  /** Toggles the state-owned mute preference without recreating transport. */
  toggleMuted(): void {
    this.#dispatch({
      _tag: "muted",
      muted: !this.#state.controls.muted,
    });
    this.#applyControls();
  }

  /** Requests fullscreen through the runner and records only the safe result. */
  async requestFullscreen(): Promise<boolean> {
    const video = this.#video;
    if (video === null) {
      return false;
    }
    const sessionEpoch = this.#sessionEpoch;
    const requestedFullscreen = !this.#state.controls.fullscreen;
    const engineFullscreen = this.#handle?.requestFullscreen;
    if (engineFullscreen !== undefined) {
      try {
        const applied = await engineFullscreen(requestedFullscreen);
        if (
          !applied ||
          sessionEpoch !== this.#sessionEpoch ||
          video !== this.#video
        ) {
          return false;
        }
        this.#dispatch({
          _tag: "fullscreen",
          fullscreen: requestedFullscreen,
        });
        return true;
      } catch {
        return false;
      }
    }
    try {
      if (requestedFullscreen) {
        if (video.requestFullscreen === undefined) {
          return false;
        }
        await video.requestFullscreen();
      } else {
        if (
          document.fullscreenElement !== video ||
          document.exitFullscreen === undefined
        ) {
          return false;
        }
        await document.exitFullscreen();
      }
      if (sessionEpoch !== this.#sessionEpoch || video !== this.#video) {
        return false;
      }
      this.#dispatch({
        _tag: "fullscreen",
        fullscreen: requestedFullscreen,
      });
      return true;
    } catch {
      return false;
    }
  }

  /** Correlates the browser fullscreen event back into session control state. */
  setFullscreen(fullscreen: boolean): void {
    if (fullscreen !== this.#state.controls.fullscreen) {
      this.#dispatch({ _tag: "fullscreen", fullscreen });
    }
  }

  #markPlaying(
    sessionEpoch: number,
    transportEpoch: number,
    session: InstalledPlaybackSession,
    video: HTMLVideoElement,
  ): void {
    if (
      video !== this.#video ||
      !this.#matchesTransport(sessionEpoch, transportEpoch, session)
    ) {
      return;
    }
    const now = this.#clock.now();
    this.#dispatch({ _tag: "playing", now });
    this.#clearStableReset();
    this.#cancelStableReset = this.#scheduler.schedule(
      this.#stableResetMs,
      () => {
        this.#cancelStableReset = null;
        if (
          sessionEpoch === this.#sessionEpoch &&
          transportEpoch === this.#transportEpoch &&
          this.#state.phase._tag === "playing"
        ) {
          this.#dispatch({ _tag: "stable" });
        }
      },
    );
  }

  /** Converts a rejected user-gesture play into a non-retryable safe failure. */
  reportAutoplayFailure(): Promise<void> {
    const session = this.#session;
    if (session === null) {
      return Promise.resolve();
    }
    const sessionEpoch = this.#sessionEpoch;
    const transportEpoch = this.#transportEpoch;
    return this.#enqueue(() =>
      this.#afterFailure(
        sessionEpoch,
        transportEpoch,
        "media-unsupported",
        false,
      ),
    );
  }

  /** Returns the bounded, already-redacted diagnostic JSON projection. */
  diagnostics(): string {
    return installedPlaybackDiagnostics(
      this.#state,
      this.#transitions,
      this.#clock.now(),
    );
  }

  /** Copies bounded diagnostics through an injected clipboard seam. */
  copyDiagnostics(clipboard: PlaybackDiagnosticsClipboard): Promise<void> {
    return copyInstalledPlaybackDiagnostics(
      clipboard,
      this.#state,
      this.#transitions,
      this.#clock.now(),
    );
  }

  /** Resolves after every operation requested before this call has settled. */
  whenIdle(): Promise<void> {
    return this.#queue;
  }

  #suspendFor(cause: InstalledPlaybackPauseCause): Promise<void> {
    const session = this.#session;
    const phase = this.#state.phase;
    if (
      session === null ||
      phase._tag === "idle" ||
      phase._tag === "failed" ||
      phase._tag === "stopping" ||
      (phase._tag === "paused" && phase.cause === "user")
    ) {
      return Promise.resolve();
    }
    const sessionEpoch = this.#sessionEpoch;
    const transportEpoch = ++this.#transportEpoch;
    this.#cancelLocalTransport();
    this.#dispatch({
      _tag: "suspending",
      next: { _tag: "paused", cause },
      transportEpoch,
    });
    return this.#enqueue(async () => {
      if (!this.#matchesSession(sessionEpoch, session)) {
        return;
      }
      const suspended = await this.#suspendSessionTransport(session);
      if (!this.#matchesSession(sessionEpoch, session)) {
        return;
      }
      if (!suspended) {
        this.#failCleanup();
        return;
      }
      this.#dispatch({
        _tag: "paused",
        cause,
        resumeWhenVisible: cause !== "user",
      });
      if (cause !== "user" && this.#state.visible) {
        await this.#open(sessionEpoch, "resume", false);
      }
    });
  }

  async #open(
    sessionEpoch: number,
    reason: InstalledPlaybackStartReason,
    initial: boolean,
  ): Promise<void> {
    const session = this.#session;
    const video = this.#video;
    if (
      session === null ||
      video === null ||
      !this.#matchesSession(sessionEpoch, session)
    ) {
      return;
    }
    const transportEpoch = ++this.#transportEpoch;
    const controller = new AbortController();
    this.#openController = controller;
    this.#dispatch({ _tag: "starting", reason, transportEpoch });

    let result: Awaited<ReturnType<InstalledPlaybackSession["start"]>>;
    try {
      const opening = initial
        ? session.start({ signal: controller.signal })
        : session.reopen({ signal: controller.signal });
      if (!this.#state.visible) {
        controller.abort();
      }
      result = await opening;
    } catch {
      if (this.#openController === controller) {
        this.#openController = null;
      }
      if (this.#matchesTransport(sessionEpoch, transportEpoch, session)) {
        await this.#afterFailure(
          sessionEpoch,
          transportEpoch,
          "source-unavailable",
          true,
        );
      }
      return;
    }
    if (this.#openController === controller) {
      this.#openController = null;
    }
    if (!this.#matchesTransport(sessionEpoch, transportEpoch, session)) {
      return;
    }
    if (!this.#state.visible) {
      const suspended = await this.#suspendSessionTransport(session);
      if (!this.#matchesTransport(sessionEpoch, transportEpoch, session)) {
        return;
      }
      if (!suspended) {
        this.#failCleanup();
        return;
      }
      this.#dispatch({
        _tag: "paused",
        cause: this.#inactivityCause(),
        resumeWhenVisible: true,
      });
      return;
    }
    if (!result.ok) {
      if (result.error._tag === "cancelled") {
        await this.#pauseAfterNativeCancellation(
          sessionEpoch,
          transportEpoch,
          session,
        );
        return;
      }
      const mapped = installedClientPlaybackFailure(result.error);
      await this.#afterFailure(
        sessionEpoch,
        transportEpoch,
        mapped.failure,
        mapped.retryable,
      );
      return;
    }

    await this.#startTransport(
      sessionEpoch,
      transportEpoch,
      session,
      video,
      result.value,
    );
  }

  async #pauseAfterNativeCancellation(
    sessionEpoch: number,
    transportEpoch: number,
    session: InstalledPlaybackSession,
  ): Promise<void> {
    const suspended = await this.#suspendSessionTransport(session);
    if (!this.#matchesTransport(sessionEpoch, transportEpoch, session)) {
      return;
    }
    if (!suspended) {
      this.#failCleanup();
      return;
    }
    this.#dispatch({
      _tag: "paused",
      cause: this.#state.visible ? "lifecycle" : this.#inactivityCause(),
      resumeWhenVisible: true,
    });
  }

  async #startTransport(
    sessionEpoch: number,
    transportEpoch: number,
    session: InstalledPlaybackSession,
    video: HTMLVideoElement | null,
    transport: InstalledPlaybackTransport,
  ): Promise<void> {
    if (
      video === null ||
      video !== this.#video ||
      !this.#matchesTransport(sessionEpoch, transportEpoch, session)
    ) {
      return;
    }
    this.#transport = transport;
    const nativeTransport =
      transport._tag === "tauri-native-stream" ? transport : null;
    this.#dispatch({
      _tag: "transport-opened",
      presentation:
        nativeTransport === null
          ? "linux-mpv"
          : nativeTransport.presentation,
      tracks: nativeTransport?.tracks ?? [],
      selection: nativeTransport?.selection ?? { _tag: "none" },
      ...(nativeTransport?.preferenceStatus === undefined
        ? {}
        : { preferenceStatus: nativeTransport.preferenceStatus }),
    });
    this.#applyControls();
    let registered = false;
    const synchronousOutcome: {
      failure?: {
        readonly failure: InstalledPlaybackFailure;
        readonly retryable: boolean;
      };
      autoplayBlocked: boolean;
    } = { autoplayBlocked: false };
    const started = this.#engine.start({
      session,
      descriptor: transport,
      video,
      onFailure: (failure, retryable) => {
        if (!registered) {
          synchronousOutcome.failure = { failure, retryable };
          return;
        }
        void this.#enqueue(() =>
          this.#afterFailure(sessionEpoch, transportEpoch, failure, retryable),
        );
      },
      onAutoplayBlocked: () => {
        if (!registered) {
          synchronousOutcome.autoplayBlocked = true;
          return;
        }
        if (this.#matchesTransport(sessionEpoch, transportEpoch, session)) {
          this.#dispatch({ _tag: "autoplay-blocked" });
        }
      },
      onPlaying: () => {
        this.#markPlaying(sessionEpoch, transportEpoch, session, video);
      },
    });
    registered = true;
    if (typeof started === "string") {
      await this.#afterFailure(sessionEpoch, transportEpoch, started, true);
      return;
    }
    if (!this.#matchesTransport(sessionEpoch, transportEpoch, session)) {
      safelyStopHandle(started);
      return;
    }
    this.#handle = started;
    this.#applyControls();
    const synchronousFailure = synchronousOutcome.failure;
    if (synchronousFailure !== undefined) {
      await this.#afterFailure(
        sessionEpoch,
        transportEpoch,
        synchronousFailure.failure,
        synchronousFailure.retryable,
      );
    } else if (synchronousOutcome.autoplayBlocked) {
      this.#dispatch({ _tag: "autoplay-blocked" });
    }
  }

  async #afterFailure(
    sessionEpoch: number,
    failedTransportEpoch: number,
    failure: InstalledPlaybackFailure,
    adapterRetryable: boolean,
  ): Promise<void> {
    const session = this.#session;
    if (
      session === null ||
      !this.#matchesTransport(sessionEpoch, failedTransportEpoch, session)
    ) {
      return;
    }
    const transportEpoch = ++this.#transportEpoch;
    this.#cancelLocalTransport();
    this.#dispatch({
      _tag: "suspending",
      next: { _tag: "recovering" },
      transportEpoch,
    });
    const suspended = await this.#suspendSessionTransport(session);
    if (!this.#matchesTransport(sessionEpoch, transportEpoch, session)) {
      return;
    }
    if (!suspended) {
      this.#failCleanup();
      return;
    }

    if (!this.#state.visible) {
      this.#dispatch({
        _tag: "paused",
        cause: this.#inactivityCause(),
        resumeWhenVisible: true,
      });
      return;
    }

    const attemptsUsed = this.#state.recoveryCount;
    const canRecover =
      adapterRetryable &&
      isRecoverableFailure(failure) &&
      attemptsUsed < this.#recoveryDelaysMs.length;
    if (!canRecover) {
      await this.#finishTerminal(sessionEpoch, session, failure, attemptsUsed);
      return;
    }

    const attempt = attemptsUsed + 1;
    const delayMs = this.#recoveryDelaysMs[attempt - 1];
    if (delayMs === undefined) {
      await this.#finishTerminal(sessionEpoch, session, failure, attemptsUsed);
      return;
    }
    const retryAt = this.#clock.now() + delayMs;
    this.#dispatch({
      _tag: "recovering",
      attempt,
      retryAt,
      failure,
    });
    this.#clearRetry();
    this.#cancelRetry = this.#scheduler.schedule(delayMs, () => {
      this.#cancelRetry = null;
      void this.#enqueue(async () => {
        if (
          !this.#matchesTransport(sessionEpoch, transportEpoch, session) ||
          this.#state.phase._tag !== "recovering"
        ) {
          return;
        }
        if (!this.#state.visible) {
          this.#dispatch({
            _tag: "paused",
            cause: this.#inactivityCause(),
            resumeWhenVisible: true,
          });
          return;
        }
        await this.#open(sessionEpoch, "recovery", false);
      });
    });
  }

  async #finishTerminal(
    sessionEpoch: number,
    session: InstalledPlaybackSession,
    failure: InstalledPlaybackFailure,
    attemptsUsed: number,
  ): Promise<void> {
    this.#cancelLocalTransport();
    const stopped = await this.#stopSessionTransport(session);
    if (this.#session === session && !stopped) {
      this.#cleanupBlocked = true;
    }
    if (sessionEpoch !== this.#sessionEpoch) {
      return;
    }
    if (!stopped) {
      this.#failCleanup();
      return;
    }
    this.#dispatch({
      _tag: "failed",
      failure,
      attemptsUsed,
      canRestart: true,
    });
  }

  async #stopCurrentSession(): Promise<boolean> {
    const session = this.#session;
    if (session === null) {
      return !this.#cleanupBlocked;
    }
    const stopped = await this.#stopSessionTransport(session);
    if (!stopped) {
      this.#cleanupBlocked = true;
      return false;
    }
    if (this.#session === session) {
      this.#session = null;
      this.#cleanupBlocked = false;
    }
    return true;
  }

  #failCleanup(): void {
    this.#cleanupBlocked = true;
    if (this.#state.channel !== null) {
      this.#dispatch({
        _tag: "failed",
        failure: "cleanup-unconfirmed",
        attemptsUsed: this.#state.recoveryCount,
        canRestart: false,
      });
    }
  }

  #cancelLocalTransport(): void {
    this.#openController?.abort();
    this.#openController = null;
    const transport = this.#transport;
    this.#transport = null;
    const handle = this.#handle;
    this.#handle = null;
    if (handle !== null) {
      const session = this.#session;
      if (
        session !== null &&
        transport?._tag === "tauri-native-stream" &&
        transport.presentation === "android-media3"
      ) {
        const handles = this.#deferredAndroidHandles.get(session);
        if (handles === undefined) {
          this.#deferredAndroidHandles.set(session, new Set([handle]));
        } else {
          handles.add(handle);
        }
      } else {
        safelyStopHandle(handle);
      }
    }
    this.#clearRetry();
    this.#clearStableReset();
  }

  async #suspendSessionTransport(
    session: InstalledPlaybackSession,
  ): Promise<boolean> {
    const suspended = await safeSuspend(session);
    this.#releaseDeferredAndroidHandles(session);
    return suspended;
  }

  async #stopSessionTransport(
    session: InstalledPlaybackSession,
  ): Promise<boolean> {
    const stopped = await safeStop(session);
    this.#releaseDeferredAndroidHandles(session);
    return stopped;
  }

  #releaseDeferredAndroidHandles(session: InstalledPlaybackSession): void {
    const handles = this.#deferredAndroidHandles.get(session);
    if (handles === undefined) {
      return;
    }
    this.#deferredAndroidHandles.delete(session);
    for (const handle of handles) {
      safelyStopHandle(handle);
    }
  }

  #clearRetry(): void {
    this.#cancelRetry?.();
    this.#cancelRetry = null;
  }

  #clearStableReset(): void {
    this.#cancelStableReset?.();
    this.#cancelStableReset = null;
  }

  #applyControls(): void {
    const video = this.#video;
    if (video === null) {
      return;
    }
    video.volume = this.#state.controls.volume;
    video.muted = this.#state.controls.muted;
    this.#handle?.setControls?.({
      volume: this.#state.controls.volume,
      muted: this.#state.controls.muted,
    });
  }

  #inactivityCause(): Extract<
    InstalledPlaybackPauseCause,
    "visibility" | "lifecycle"
  > {
    return this.#foreground ? "visibility" : "lifecycle";
  }

  #matchesSession(
    sessionEpoch: number,
    session: InstalledPlaybackSession,
  ): boolean {
    return sessionEpoch === this.#sessionEpoch && session === this.#session;
  }

  #matchesTransport(
    sessionEpoch: number,
    transportEpoch: number,
    session: InstalledPlaybackSession,
  ): boolean {
    return (
      this.#matchesSession(sessionEpoch, session) &&
      transportEpoch === this.#transportEpoch
    );
  }

  #dispatch(event: InstalledPlaybackEvent): void {
    const previous = this.#state;
    const next = reduceInstalledPlaybackState(previous, event);
    this.#state = next;
    if (ownsScreenWake(previous.phase) !== ownsScreenWake(next.phase)) {
      const activityEpoch = ++this.#activityEpoch;
      const session = this.#session;
      const sessionEpoch = this.#sessionEpoch;
      const active = ownsScreenWake(next.phase);
      if (session !== null) {
        void this.#enqueue(async () => {
          if (
            !this.#matchesSession(sessionEpoch, session) ||
            activityEpoch !== this.#activityEpoch ||
            ownsScreenWake(this.#state.phase) !== active
          ) {
            return;
          }
          const confirmed = await safeSetActivity(session, active);
          if (
            !confirmed &&
            active &&
            this.#matchesSession(sessionEpoch, session) &&
            activityEpoch === this.#activityEpoch &&
            ownsScreenWake(this.#state.phase)
          ) {
            this.#cancelLocalTransport();
            const stopped = await this.#stopSessionTransport(session);
            if (this.#session === session && stopped) {
              this.#session = null;
            }
            if (
              this.#matchesSession(sessionEpoch, session) ||
              this.#session === null
            ) {
              this.#cleanupBlocked = true;
              this.#dispatch({
                _tag: "failed",
                failure: "cleanup-unconfirmed",
                attemptsUsed: this.#state.recoveryCount,
                canRestart: false,
              });
            }
          }
        });
      }
    }
    if (previous.phase._tag !== next.phase._tag) {
      this.#transitions.push({
        from: previous.phase._tag,
        to: next.phase._tag,
      });
      if (this.#transitions.length > MAX_RETAINED_TRANSITIONS) {
        this.#transitions.splice(
          0,
          this.#transitions.length - MAX_RETAINED_TRANSITIONS,
        );
      }
    }
    for (const listener of this.#listeners) {
      try {
        listener();
      } catch {
        // One view listener cannot break resource ownership for the others.
      }
    }
  }

  #enqueue<Result>(operation: () => Promise<Result>): Promise<Result> {
    const flight = this.#queue.then(operation);
    this.#queue = flight.then(
      () => undefined,
      () => undefined,
    );
    return flight;
  }
}

/** Creates an installed runner without acquiring playback resources. */
export function createInstalledPlaybackRunner(
  options: InstalledPlaybackRunnerOptions,
): InstalledPlaybackRunner {
  return new InstalledPlaybackRunner(options);
}

function isRecoverableFailure(failure: InstalledPlaybackFailure): boolean {
  switch (failure) {
    case "source-timeout":
    case "source-unavailable":
    case "stream-interrupted":
    case "system-player-unavailable":
      return true;
    case "authentication-required":
    case "channel-not-found":
    case "source-rejected":
    case "source-invalid":
    case "media-unsupported":
    case "browser-unsupported":
    case "system-player-missing":
    case "system-player-incompatible":
    case "cleanup-unconfirmed":
      return false;
  }
}

function ownsScreenWake(phase: InstalledPlaybackState["phase"]): boolean {
  switch (phase._tag) {
    case "playing":
    case "recovering":
      return true;
    case "starting":
      return phase.reason !== "selection";
    case "idle":
    case "autoplay-blocked":
    case "replacing-audio":
    case "suspending":
    case "paused":
    case "failed":
    case "stopping":
      return false;
  }
}

async function safeSuspend(
  session: InstalledPlaybackSession,
): Promise<boolean> {
  try {
    const result = await session.suspend();
    return result.ok;
  } catch {
    return false;
  }
}

async function safeSetActivity(
  session: InstalledPlaybackSession,
  active: boolean,
): Promise<boolean> {
  try {
    const result = await session.setActivity(active);
    return result.ok;
  } catch {
    return false;
  }
}

async function safeStop(session: InstalledPlaybackSession): Promise<boolean> {
  try {
    const result = await session.stop();
    return result.ok;
  } catch {
    return false;
  }
}

function safelyStopHandle(handle: InstalledPlaybackHandle): void {
  try {
    handle.stop();
  } catch {
    // Partially initialized media teardown remains local and best effort.
  }
}

function browserScheduler(): InstalledPlaybackScheduler {
  return {
    schedule: (delayMs, task) => {
      const timeout = globalThis.setTimeout(task, delayMs);
      let active = true;
      return () => {
        if (!active) {
          return;
        }
        active = false;
        globalThis.clearTimeout(timeout);
      };
    },
  };
}

function normalizeRecoveryDelays(
  delays: readonly [number, number, number],
): readonly [number, number, number] {
  return [
    normalizeDelay(delays[0], DEFAULT_RECOVERY_DELAYS_MS[0]),
    normalizeDelay(delays[1], DEFAULT_RECOVERY_DELAYS_MS[1]),
    normalizeDelay(delays[2], DEFAULT_RECOVERY_DELAYS_MS[2]),
  ];
}

function normalizeDelay(value: number, fallback: number): number {
  return Number.isFinite(value) && value >= 0
    ? Math.min(Number.MAX_SAFE_INTEGER, Math.floor(value))
    : fallback;
}
