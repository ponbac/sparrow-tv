import { describe, expect, it, vi } from "vitest";
import {
  clientSchemas,
  type ChannelId,
  type ClientResult,
  type InstalledPlaybackSession,
  type InstalledPlaybackTransport,
  type NativePlaybackDescriptor,
} from "../../client/contracts";
import {
  createInstalledPlaybackRunner,
  type InstalledPlaybackClock,
  type InstalledPlaybackScheduler,
} from "./installed-playback-runner";
import type {
  InstalledPlaybackEngine,
  InstalledPlaybackRequest,
} from "./installed-playback-engine";

const CHANNEL_A = channel("channel-a", "Channel A");
const CHANNEL_B = channel("channel-b", "Channel B");
const CHANNEL_C = channel("channel-c", "Channel C");
const ENGLISH_AUDIO_ID = clientSchemas.audioTrackId.parse(
  `atrk1_${"a".repeat(32)}`,
);
const SPANISH_AUDIO_ID = clientSchemas.audioTrackId.parse(
  `atrk1_${"b".repeat(32)}`,
);

describe("InstalledPlaybackRunner", () => {
  it("confirms suspend before pause/resume/restart and never overlaps engines", async () => {
    const order: string[] = [];
    const suspended = deferred<ClientResult<void>>();
    const session = sessionFixture(1, {
      suspend: vi.fn(() => {
        order.push("suspend");
        return suspended.promise;
      }),
      reopen: vi.fn(async () => {
        order.push("reopen");
        return success(descriptor(1, 2));
      }),
    });
    const engine = recordingEngine(order);
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: engine.value,
    });
    const video = document.createElement("video");

    await runner.select(CHANNEL_A, video);
    engine.playing();
    expect(runner.getSnapshot().phase._tag).toBe("playing");
    const pausing = runner.pause();
    await until(() => order.includes("suspend"));
    expect(order.slice(-2)).toEqual(["engine-stop:1", "suspend"]);
    expect(session.reopen).not.toHaveBeenCalled();

    suspended.resolve(success(undefined));
    await pausing;
    expect(runner.getSnapshot().phase._tag).toBe("paused");
    await runner.resume();
    expect(order.indexOf("suspend")).toBeLessThan(order.indexOf("reopen"));
    expect(engine.maximumActive).toBe(1);

    const restartSuspend = deferred<ClientResult<void>>();
    session.suspend.mockImplementationOnce(() => restartSuspend.promise);
    const restarting = runner.restart();
    expect(session.reopen).toHaveBeenCalledTimes(1);
    restartSuspend.resolve(success(undefined));
    await restarting;
    expect(session.reopen).toHaveBeenCalledTimes(2);
    expect(engine.maximumActive).toBe(1);
  });

  it("releases Android Media3 only after session suspension settles", async () => {
    const order: string[] = [];
    const suspension = deferred<ClientResult<void>>();
    const session = sessionFixture(41, {
      start: async () => success(androidDescriptor(41, 1)),
      suspend: () => {
        order.push("suspend");
        return suspension.promise;
      },
    });
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: recordingEngine(order).value,
    });

    await runner.select(CHANNEL_A, document.createElement("video"));
    const pausing = runner.pause();
    await until(() => session.suspend.mock.calls.length === 1);

    expect(order).toEqual(["engine-start:1", "suspend"]);
    suspension.resolve(success(undefined));
    await pausing;
    expect(order.slice(-2)).toEqual(["suspend", "engine-stop:1"]);
  });

  it("suspends Android transport before releasing Media3 during restart", async () => {
    const order: string[] = [];
    const session = sessionFixture(42, {
      start: async () => success(androidDescriptor(42, 1)),
      suspend: async () => {
        order.push("suspend");
        return success(undefined);
      },
      reopen: async () => {
        order.push("reopen");
        return success(androidDescriptor(42, 2));
      },
    });
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: recordingEngine(order).value,
    });

    await runner.select(CHANNEL_A, document.createElement("video"));
    await runner.restart();

    expect(order).toEqual([
      "engine-start:1",
      "suspend",
      "engine-stop:1",
      "reopen",
      "engine-start:2",
    ]);
  });

  it("restarts Android transport before releasing Media3 for Audio Track replacement", async () => {
    const order: string[] = [];
    const replacement = deferred<ClientResult<NativePlaybackDescriptor>>();
    const session = sessionFixture(43, {
      start: async () =>
        success(
          androidAudioDescriptor(43, 1, ENGLISH_AUDIO_ID, {
            _tag: "selected",
            trackId: ENGLISH_AUDIO_ID,
            reason: "first-available",
          }),
        ),
      restart: () => {
        order.push("restart");
        return replacement.promise;
      },
    });
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: recordingEngine(order).value,
    });

    await runner.select(CHANNEL_A, document.createElement("video"));
    const selecting = runner.selectAudio(SPANISH_AUDIO_ID);
    await until(() => session.restart.mock.calls.length === 1);
    expect(order).toEqual(["engine-start:1", "restart"]);

    replacement.resolve(
      success(
        androidAudioDescriptor(43, 2, SPANISH_AUDIO_ID, {
          _tag: "selected",
          trackId: SPANISH_AUDIO_ID,
          reason: "requested",
        }),
      ),
    );
    await selecting;
    expect(order).toEqual([
      "engine-start:1",
      "restart",
      "engine-stop:1",
      "engine-start:2",
    ]);
  });

  it("final-stops Android transport before releasing Media3", async () => {
    const order: string[] = [];
    const stopped = deferred<ClientResult<void>>();
    const session = sessionFixture(44, {
      start: async () => success(androidDescriptor(44, 1)),
      stop: () => {
        order.push("stop");
        return stopped.promise;
      },
    });
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: recordingEngine(order).value,
    });

    await runner.select(CHANNEL_A, document.createElement("video"));
    const stopping = runner.stop();
    await until(() => session.stop.mock.calls.length === 1);
    expect(order).toEqual(["engine-start:1", "stop"]);

    stopped.resolve(success(undefined));
    await stopping;
    expect(order.slice(-2)).toEqual(["stop", "engine-stop:1"]);
  });

  it("replaces Audio Tracks through the exact active handle without overlapping engines", async () => {
    const order: string[] = [];
    const replacement = deferred<ClientResult<NativePlaybackDescriptor>>();
    const session = sessionFixture(16, {
      start: async () =>
        success(
          audioDescriptor(16, 1, ENGLISH_AUDIO_ID, {
            _tag: "selected",
            trackId: ENGLISH_AUDIO_ID,
            reason: "first-available",
          }),
        ),
      restart: (input) => {
        order.push("restart");
        expect(input).toEqual({
          expectedStreamHandle: descriptor(16, 1).streamHandle,
          intent: {
            _tag: "select-audio",
            audioTrackId: SPANISH_AUDIO_ID,
          },
          signal: expect.any(AbortSignal),
        });
        return replacement.promise;
      },
    });
    const engine = recordingEngine(order);
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: engine.value,
    });

    await runner.select(CHANNEL_A, document.createElement("video"));
    expect(runner.getSnapshot().audio.tracks).toHaveLength(2);
    const selecting = runner.selectAudio(SPANISH_AUDIO_ID);
    await until(() => session.restart.mock.calls.length === 1);
    expect(order.slice(-2)).toEqual(["engine-stop:1", "restart"]);
    expect(engine.maximumActive).toBe(1);

    replacement.resolve(
      success(
        audioDescriptor(
          16,
          2,
          SPANISH_AUDIO_ID,
          {
            _tag: "selected",
            trackId: SPANISH_AUDIO_ID,
            reason: "requested",
          },
          "saved",
        ),
      ),
    );
    await selecting;

    expect(engine.maximumActive).toBe(1);
    expect(runner.getSnapshot().audio).toMatchObject({
      discovered: true,
      selection: {
        _tag: "selected",
        trackId: SPANISH_AUDIO_ID,
        reason: "requested",
      },
      preferenceStatus: "saved",
    });
    expect(
      runner.getSnapshot().audio.tracks.find((track) => track.selected)?.id,
    ).toBe(SPANISH_AUDIO_ID);
  });

  it("keeps a missing requested Audio Track fallback visible without persisting it", async () => {
    const session = sessionFixture(17, {
      start: async () =>
        success(
          audioDescriptor(17, 1, ENGLISH_AUDIO_ID, {
            _tag: "selected",
            trackId: ENGLISH_AUDIO_ID,
            reason: "saved-preference",
          }),
        ),
      restart: async () =>
        success({
          ...descriptor(17, 2),
          tracks: [audioTracks(ENGLISH_AUDIO_ID)[0]],
          selection: {
            _tag: "fallback" as const,
            trackId: ENGLISH_AUDIO_ID,
            missing: "requested" as const,
          },
          preferenceStatus: "not-saved" as const,
        }),
    });
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: recordingEngine().value,
    });

    await runner.select(CHANNEL_A, document.createElement("video"));
    await runner.selectAudio(SPANISH_AUDIO_ID);

    expect(runner.getSnapshot().audio).toMatchObject({
      selection: {
        _tag: "fallback",
        trackId: ENGLISH_AUDIO_ID,
        missing: "requested",
      },
      preferenceStatus: "not-saved",
    });
  });

  it("ignores a late Audio Track replacement after a Channel switch", async () => {
    const replacement = deferred<ClientResult<NativePlaybackDescriptor>>();
    const first = sessionFixture(18, {
      start: async () =>
        success(
          audioDescriptor(18, 1, ENGLISH_AUDIO_ID, {
            _tag: "selected",
            trackId: ENGLISH_AUDIO_ID,
            reason: "first-available",
          }),
        ),
      restart: () => replacement.promise,
    });
    const second = sessionFixture(19);
    const engine = recordingEngine();
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(({ id }) =>
        id === CHANNEL_A.id ? first.value : second.value,
      ),
      engine: engine.value,
    });
    const video = document.createElement("video");
    await runner.select(CHANNEL_A, video);
    const selectingAudio = runner.selectAudio(SPANISH_AUDIO_ID);
    await until(() => first.restart.mock.calls.length === 1);
    const switching = runner.select(CHANNEL_B, video);

    replacement.resolve(
      success(
        audioDescriptor(
          18,
          2,
          SPANISH_AUDIO_ID,
          {
            _tag: "selected",
            trackId: SPANISH_AUDIO_ID,
            reason: "requested",
          },
          "saved",
        ),
      ),
    );
    await Promise.all([selectingAudio, switching]);

    expect(first.stop).toHaveBeenCalledTimes(1);
    expect(second.start).toHaveBeenCalledTimes(1);
    expect(runner.getSnapshot().channel?.id).toBe(CHANNEL_B.id);
    expect(runner.getSnapshot().audio.tracks).toEqual([]);
    expect(engine.maximumActive).toBe(1);
  });

  it("uses exactly 1s/5s/15s recovery and ends visibly after the finite budget", async () => {
    const time = new ControlledTime();
    const session = sessionFixture(2);
    const engine = recordingEngine([], () => "stream-interrupted" as const);
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: engine.value,
      clock: time,
      scheduler: time,
    });

    await runner.select(CHANNEL_A, document.createElement("video"));
    expectRecovery(runner.getSnapshot().phase, 1);
    expect(time.scheduledDelays).toEqual([1_000]);

    await time.advanceBy(1_000, runner);
    expectRecovery(runner.getSnapshot().phase, 2);
    await time.advanceBy(5_000, runner);
    expectRecovery(runner.getSnapshot().phase, 3);
    await time.advanceBy(15_000, runner);

    expect(runner.getSnapshot().phase).toEqual({
      _tag: "failed",
      failure: "stream-interrupted",
      attemptsUsed: 3,
      canRestart: true,
    });
    expect(time.scheduledDelays).toEqual([1_000, 5_000, 15_000]);
    expect(session.reopen).toHaveBeenCalledTimes(3);
    expect(session.suspend).toHaveBeenCalledTimes(4);
    expect(session.stop).toHaveBeenCalledTimes(1);
    expect(engine.maximumActive).toBe(1);
  });

  it.each([
    "authentication-required",
    "channel-not-found",
    "source-rejected",
    "source-invalid",
    "media-unsupported",
    "browser-unsupported",
  ] as const)(
    "makes %s terminal without scheduling recovery",
    async (failure) => {
      const time = new ControlledTime();
      const session = sessionFixture(20);
      const engine: InstalledPlaybackEngine = {
        start: (request) => {
          request.onFailure(failure, true);
          return { stop: () => undefined };
        },
      };
      const runner = createInstalledPlaybackRunner({
        client: clientFixture(() => session.value),
        engine,
        clock: time,
        scheduler: time,
      });

      await runner.select(CHANNEL_A, document.createElement("video"));

      expect(runner.getSnapshot().phase).toEqual({
        _tag: "failed",
        failure,
        attemptsUsed: 0,
        canRestart: true,
      });
      expect(time.scheduledDelays).toEqual([]);
      expect(session.stop).toHaveBeenCalledTimes(1);
    },
  );

  it("resets recovery only after 60 seconds of continuous playing", async () => {
    const time = new ControlledTime();
    let starts = 0;
    let activeFailure:
      ((failure: "stream-interrupted", retryable: boolean) => void) | undefined;
    const engine = recordingEngine([], (request) => {
      starts += 1;
      if (starts === 1) {
        return "stream-interrupted";
      }
      activeFailure = request.onFailure;
      return null;
    });
    const session = sessionFixture(3);
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: engine.value,
      clock: time,
      scheduler: time,
    });
    const video = document.createElement("video");

    await runner.select(CHANNEL_A, video);
    await time.advanceBy(1_000, runner);
    engine.playing();
    expect(runner.getSnapshot().recoveryCount).toBe(1);
    await time.advanceBy(59_999, runner);
    expect(runner.getSnapshot().recoveryCount).toBe(1);
    await time.advanceBy(1, runner);
    expect(runner.getSnapshot().recoveryCount).toBe(0);

    activeFailure?.("stream-interrupted", true);
    await runner.whenIdle();
    expectRecovery(runner.getSnapshot().phase, 1);
    expect(time.scheduledDelays.at(-1)).toBe(1_000);
  });

  it("reopens the same pinned resource after an initial access failure and a read failure", async () => {
    const time = new ControlledTime();
    const session = sessionFixture(4);
    session.start.mockResolvedValueOnce({
      ok: false,
      error: {
        _tag: "playback-failed",
        reason: "unavailable",
        retryable: true,
      },
    });
    let readFailure: (() => void) | undefined;
    const engine = recordingEngine([], (request) => {
      readFailure = () => request.onFailure("stream-interrupted", true);
      return null;
    });
    const client = clientFixture(() => session.value);
    const runner = createInstalledPlaybackRunner({
      client,
      engine: engine.value,
      clock: time,
      scheduler: time,
    });

    await runner.select(CHANNEL_A, document.createElement("video"));
    expectRecovery(runner.getSnapshot().phase, 1);
    await time.advanceBy(1_000, runner);
    expect(client.createPlaybackSession).toHaveBeenCalledTimes(1);
    expect(session.reopen).toHaveBeenCalledTimes(1);

    readFailure?.();
    await runner.whenIdle();
    expectRecovery(runner.getSnapshot().phase, 2);
    await time.advanceBy(5_000, runner);
    expect(client.createPlaybackSession).toHaveBeenCalledTimes(1);
    expect(session.reopen).toHaveBeenCalledTimes(2);
  });

  it("keeps a missing system mpv distinct from provider availability", async () => {
    const session = sessionFixture(40, {
      start: async () => ({
        ok: false,
        error: {
          _tag: "mpv-failed",
          reason: "not-installed",
          retryable: false,
        },
      }),
    });
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: recordingEngine().value,
    });

    await runner.select(CHANNEL_A, document.createElement("video"));

    expect(runner.getSnapshot().phase).toEqual({
      _tag: "failed",
      failure: "system-player-missing",
      attemptsUsed: 0,
      canRestart: true,
    });
    expect(session.stop).toHaveBeenCalledTimes(1);
  });

  it("coalesces A to B to C and waits for A cleanup before C starts", async () => {
    const aStop = deferred<ClientResult<void>>();
    const a = sessionFixture(5, {
      start: cancellationAwareStart(descriptor(5, 1)),
      stop: vi.fn(() => aStop.promise),
    });
    const c = sessionFixture(6);
    const created: ChannelId[] = [];
    const client = clientFixture(({ id }) => {
      created.push(id);
      return id === CHANNEL_A.id ? a.value : c.value;
    });
    const runner = createInstalledPlaybackRunner({
      client,
      engine: recordingEngine().value,
    });

    const selectingA = runner.select(
      CHANNEL_A,
      document.createElement("video"),
    );
    await until(() => a.start.mock.calls.length === 1);
    const selectingB = runner.select(
      CHANNEL_B,
      document.createElement("video"),
    );
    const selectingC = runner.select(
      CHANNEL_C,
      document.createElement("video"),
    );
    await until(() => a.stop.mock.calls.length === 1);
    expect(created).toEqual([CHANNEL_A.id]);
    expect(c.start).not.toHaveBeenCalled();

    aStop.resolve(success(undefined));
    await Promise.all([selectingA, selectingB, selectingC]);
    expect(created).toEqual([CHANNEL_A.id, CHANNEL_C.id]);
    expect(c.start).toHaveBeenCalledTimes(1);
    expect(runner.getSnapshot().channel?.id).toBe(CHANNEL_C.id);
  });

  it("ignores stale engine callbacks after a confirmed Channel replacement", async () => {
    const first = sessionFixture(7);
    const second = sessionFixture(8);
    let staleFailure: (() => void) | undefined;
    let engineStarts = 0;
    const engine = recordingEngine([], (request) => {
      engineStarts += 1;
      if (engineStarts === 1) {
        staleFailure = () => request.onFailure("stream-interrupted", true);
      }
      return null;
    });
    const client = clientFixture(({ id }) =>
      id === CHANNEL_A.id ? first.value : second.value,
    );
    const runner = createInstalledPlaybackRunner({
      client,
      engine: engine.value,
    });

    await runner.select(CHANNEL_A, document.createElement("video"));
    await runner.select(CHANNEL_B, document.createElement("video"));
    staleFailure?.();
    await runner.whenIdle();

    expect(second.suspend).not.toHaveBeenCalled();
    expect(runner.getSnapshot().channel?.id).toBe(CHANNEL_B.id);
    expect(runner.getSnapshot().phase._tag).toBe("starting");
  });

  it("blocks replacement when final cleanup is not confirmed", async () => {
    const first = sessionFixture(9);
    first.stop.mockResolvedValue({
      ok: false,
      error: {
        _tag: "transport",
        retryable: false,
        message: "private-error-canary",
      },
    });
    const second = sessionFixture(10);
    const client = clientFixture(({ id }) =>
      id === CHANNEL_A.id ? first.value : second.value,
    );
    const runner = createInstalledPlaybackRunner({
      client,
      engine: recordingEngine().value,
    });

    await runner.select(CHANNEL_A, document.createElement("video"));
    await runner.select(CHANNEL_B, document.createElement("video"));

    expect(client.createPlaybackSession).toHaveBeenCalledTimes(1);
    expect(second.start).not.toHaveBeenCalled();
    expect(runner.getSnapshot().phase).toEqual({
      _tag: "failed",
      failure: "cleanup-unconfirmed",
      attemptsUsed: 0,
      canRestart: false,
    });
    expect(runner.diagnostics()).not.toContain("private-error-canary");
  });

  it("resumes visibility-owned pauses while a user pause wins across hiding", async () => {
    const session = sessionFixture(11);
    const engine = recordingEngine();
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: engine.value,
    });
    const video = document.createElement("video");
    await runner.select(CHANNEL_A, video);
    engine.playing();

    await runner.setVisible(false);
    expect(runner.getSnapshot().phase).toEqual({
      _tag: "paused",
      cause: "visibility",
      resumeWhenVisible: true,
    });
    await runner.setVisible(true);
    expect(session.reopen).toHaveBeenCalledTimes(1);

    await runner.pause();
    await runner.setVisible(false);
    await runner.setVisible(true);
    expect(runner.getSnapshot().phase).toEqual({
      _tag: "paused",
      cause: "user",
      resumeWhenVisible: false,
    });
    expect(session.reopen).toHaveBeenCalledTimes(1);
    await runner.resume();
    expect(session.reopen).toHaveBeenCalledTimes(2);
  });

  it("combines native lifecycle and document visibility without duplicate resume", async () => {
    const session = sessionFixture(16);
    const engine = recordingEngine();
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: engine.value,
    });
    await runner.select(CHANNEL_A, document.createElement("video"));
    engine.playing();
    await runner.whenIdle();
    expect(session.setActivity.mock.calls.map(([active]) => active)).toEqual([
      true,
    ]);

    await runner.setForeground(false);
    expect(runner.getSnapshot().phase).toEqual({
      _tag: "paused",
      cause: "lifecycle",
      resumeWhenVisible: true,
    });
    expect(session.suspend).toHaveBeenCalledTimes(1);
    expect(session.setActivity.mock.calls.map(([active]) => active)).toEqual([
      true,
      false,
    ]);

    await runner.setVisible(false);
    await runner.setForeground(true);
    expect(session.reopen).not.toHaveBeenCalled();
    await runner.setVisible(true);
    await runner.whenIdle();
    expect(session.reopen).toHaveBeenCalledTimes(1);
    expect(session.setActivity.mock.calls.map(([active]) => active)).toEqual([
      true,
      false,
      true,
    ]);

    await runner.setForeground(true);
    await runner.setVisible(true);
    expect(session.reopen).toHaveBeenCalledTimes(1);

    await runner.pause();
    await runner.setForeground(false);
    await runner.setForeground(true);
    expect(runner.getSnapshot().phase).toEqual({
      _tag: "paused",
      cause: "user",
      resumeWhenVisible: false,
    });
    expect(session.reopen).toHaveBeenCalledTimes(1);
  });

  it("resumes once when visibility returns before suspension is confirmed", async () => {
    const suspension = deferred<ClientResult<void>>();
    const session = sessionFixture(20, {
      suspend: vi.fn(() => suspension.promise),
    });
    const engine = recordingEngine();
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: engine.value,
    });
    await runner.select(CHANNEL_A, document.createElement("video"));
    engine.playing();

    const hiding = runner.setVisible(false);
    await until(() => session.suspend.mock.calls.length === 1);
    await runner.setForeground(false);
    await runner.setForeground(true);
    await runner.setVisible(true);
    expect(session.reopen).not.toHaveBeenCalled();

    suspension.resolve(success(undefined));
    await hiding;
    await runner.whenIdle();

    expect(session.suspend).toHaveBeenCalledTimes(1);
    expect(session.reopen).toHaveBeenCalledTimes(1);
    expect(engine.maximumActive).toBe(1);
  });

  it("drops a queued wake activation when lifecycle suspension wins", async () => {
    const session = sessionFixture(18);
    const engine = recordingEngine();
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: engine.value,
    });
    await runner.select(CHANNEL_A, document.createElement("video"));

    engine.playing();
    await runner.setForeground(false);
    await runner.whenIdle();

    expect(runner.getSnapshot().phase).toEqual({
      _tag: "paused",
      cause: "lifecycle",
      resumeWhenVisible: true,
    });
    expect(session.setActivity.mock.calls.map(([active]) => active)).toEqual([
      false,
    ]);
    expect(session.suspend).toHaveBeenCalledTimes(1);
  });

  it("retains a natively cancelled opening until the lifecycle signal arrives", async () => {
    const opening = deferred<ClientResult<InstalledPlaybackTransport>>();
    const session = sessionFixture(19, {
      start: vi.fn(() => opening.promise),
    });
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: recordingEngine().value,
    });
    const selecting = runner.select(CHANNEL_A, document.createElement("video"));
    await until(() => session.start.mock.calls.length === 1);

    opening.resolve({ ok: false, error: { _tag: "cancelled" } });
    await selecting;

    expect(runner.getSnapshot().phase).toEqual({
      _tag: "paused",
      cause: "lifecycle",
      resumeWhenVisible: true,
    });
    expect(session.suspend).toHaveBeenCalledTimes(1);
    expect(session.stop).not.toHaveBeenCalled();

    await runner.setForeground(false);
    await runner.setForeground(true);
    expect(session.reopen).toHaveBeenCalledTimes(1);
  });

  it("fails closed when native wake ownership cannot be confirmed", async () => {
    const session = sessionFixture(17, {
      setActivity: vi.fn<InstalledPlaybackSession["setActivity"]>(
        async (active) =>
          active
            ? {
                ok: false,
                error: {
                  _tag: "transport",
                  retryable: false,
                  message: "wake-private-canary",
                },
              }
            : success(undefined),
      ),
    });
    const engine = recordingEngine();
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: engine.value,
    });
    await runner.select(CHANNEL_A, document.createElement("video"));
    engine.playing();
    await runner.whenIdle();

    expect(session.stop).toHaveBeenCalledTimes(1);
    expect(runner.getSnapshot().phase).toEqual({
      _tag: "failed",
      failure: "cleanup-unconfirmed",
      attemptsUsed: 0,
      canRestart: false,
    });
    expect(runner.diagnostics()).not.toContain("wake-private-canary");
  });

  it("cancels recovery timers on pause, switch, and final stop", async () => {
    const time = new ControlledTime();
    const first = sessionFixture(12);
    const second = sessionFixture(13);
    const engine = recordingEngine([], () => "stream-interrupted" as const);
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(({ id }) =>
        id === CHANNEL_A.id ? first.value : second.value,
      ),
      engine: engine.value,
      clock: time,
      scheduler: time,
    });

    await runner.select(CHANNEL_A, document.createElement("video"));
    expect(time.activeTasks).toBe(1);
    await runner.pause();
    expect(time.activeTasks).toBe(0);
    await runner.resume();
    expect(time.activeTasks).toBe(1);
    await runner.select(CHANNEL_B, document.createElement("video"));
    expect(time.activeTasks).toBe(1);
    await runner.stop();
    expect(time.activeTasks).toBe(0);
  });

  it("does not create a recovery timer when visibility changes during suspension", async () => {
    const time = new ControlledTime();
    const suspension = deferred<ClientResult<void>>();
    const session = sessionFixture(15, {
      suspend: vi.fn(() => suspension.promise),
    });
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: recordingEngine([], () => "stream-interrupted" as const).value,
      clock: time,
      scheduler: time,
    });

    const selecting = runner.select(CHANNEL_A, document.createElement("video"));
    await until(() => session.suspend.mock.calls.length === 1);
    await runner.setVisible(false);
    suspension.resolve(success(undefined));
    await selecting;

    expect(runner.getSnapshot().phase).toEqual({
      _tag: "paused",
      cause: "visibility",
      resumeWhenVisible: true,
    });
    expect(time.activeTasks).toBe(0);
    expect(time.scheduledDelays).toEqual([]);
  });

  it("keeps volume/mute across recreation and excludes every private canary from diagnostics", async () => {
    const applied: Array<{ readonly volume: number; readonly muted: boolean }> =
      [];
    const engine = recordingEngine([], (request) => {
      applied.push({
        volume: request.video.volume,
        muted: request.video.muted,
      });
      return null;
    });
    const session = sessionFixture(14);
    const runner = createInstalledPlaybackRunner({
      client: clientFixture(() => session.value),
      engine: engine.value,
    });
    const video = document.createElement("video");
    Object.defineProperty(video, "requestFullscreen", {
      configurable: true,
      value: vi.fn(async () => undefined),
    });
    await runner.select(
      channel("private-id-canary", "Private Name Canary"),
      video,
    );
    runner.setVolume(0.42);
    runner.toggleMuted();
    expect(await runner.requestFullscreen()).toBe(true);
    await runner.restart();

    expect(applied).toEqual([
      { volume: 1, muted: false },
      { volume: 0.42, muted: true },
    ]);
    const diagnostics = runner.diagnostics();
    for (const canary of [
      "private-id-canary",
      "Private Name Canary",
      descriptor(14, 1).sessionId,
      descriptor(14, 1).streamHandle,
      "https://user:secret@provider.invalid/live",
      "header-canary",
      "payload-canary",
      "fingerprint-canary",
    ]) {
      expect(diagnostics).not.toContain(canary);
    }
    expect(diagnostics).toContain('"volumePercent":42');
    expect(diagnostics).toContain('"muted":true');
    expect(diagnostics).toContain('"fullscreen":true');
  });
});

interface SessionOverrides {
  readonly start?: InstalledPlaybackSession["start"];
  readonly reopen?: InstalledPlaybackSession["reopen"];
  readonly restart?: InstalledPlaybackSession["restart"];
  readonly startAndroidPresentation?: InstalledPlaybackSession["startAndroidPresentation"];
  readonly controlMpv?: InstalledPlaybackSession["controlMpv"];
  readonly suspend?: InstalledPlaybackSession["suspend"];
  readonly setActivity?: InstalledPlaybackSession["setActivity"];
  readonly stop?: InstalledPlaybackSession["stop"];
}

function sessionFixture(
  sessionNumber: number,
  overrides: SessionOverrides = {},
): {
  readonly value: InstalledPlaybackSession;
  readonly start: ReturnType<typeof vi.fn<InstalledPlaybackSession["start"]>>;
  readonly reopen: ReturnType<typeof vi.fn<InstalledPlaybackSession["reopen"]>>;
  readonly restart: ReturnType<
    typeof vi.fn<InstalledPlaybackSession["restart"]>
  >;
  readonly startAndroidPresentation: ReturnType<
    typeof vi.fn<InstalledPlaybackSession["startAndroidPresentation"]>
  >;
  readonly controlMpv: ReturnType<
    typeof vi.fn<InstalledPlaybackSession["controlMpv"]>
  >;
  readonly suspend: ReturnType<
    typeof vi.fn<InstalledPlaybackSession["suspend"]>
  >;
  readonly setActivity: ReturnType<
    typeof vi.fn<InstalledPlaybackSession["setActivity"]>
  >;
  readonly stop: ReturnType<typeof vi.fn<InstalledPlaybackSession["stop"]>>;
} {
  const start = vi.fn<InstalledPlaybackSession["start"]>(
    overrides.start ?? (async () => success(descriptor(sessionNumber, 1))),
  );
  const reopen = vi.fn<InstalledPlaybackSession["reopen"]>(
    overrides.reopen ?? (async () => success(descriptor(sessionNumber, 2))),
  );
  const restart = vi.fn<InstalledPlaybackSession["restart"]>(
    overrides.restart ?? (async () => success(descriptor(sessionNumber, 3))),
  );
  const startAndroidPresentation = vi.fn<
    InstalledPlaybackSession["startAndroidPresentation"]
  >(
    overrides.startAndroidPresentation ??
      (async () => success(androidPresentationFixture())),
  );
  const controlMpv = vi.fn<InstalledPlaybackSession["controlMpv"]>(
    overrides.controlMpv ?? (async () => success(undefined)),
  );
  const suspend = vi.fn<InstalledPlaybackSession["suspend"]>(
    overrides.suspend ?? (async () => success(undefined)),
  );
  const stop = vi.fn<InstalledPlaybackSession["stop"]>(
    overrides.stop ?? (async () => success(undefined)),
  );
  const setActivity = vi.fn<InstalledPlaybackSession["setActivity"]>(
    overrides.setActivity ?? (async () => success(undefined)),
  );
  return {
    value: {
      start,
      reopen,
      restart,
      read: vi.fn(async () => success(new ArrayBuffer(0))),
      startAndroidPresentation,
      controlMpv,
      suspend,
      setActivity,
      stop,
    },
    start,
    reopen,
    restart,
    startAndroidPresentation,
    controlMpv,
    suspend,
    setActivity,
    stop,
  };
}

function clientFixture(
  create: (input: { readonly id: ChannelId }) => InstalledPlaybackSession,
): {
  readonly createPlaybackSession: ReturnType<typeof vi.fn>;
} {
  return { createPlaybackSession: vi.fn(create) };
}

function recordingEngine(
  order: string[] = [],
  behavior:
    | ((request: InstalledPlaybackRequest) => "stream-interrupted" | null)
    | (() => "stream-interrupted") = () => null,
): {
  readonly value: InstalledPlaybackEngine;
  readonly maximumActive: number;
  readonly playing: () => void;
} {
  let active = 0;
  let maximumActive = 0;
  let sequence = 0;
  let currentRequest: InstalledPlaybackRequest | null = null;
  return {
    value: {
      start: (request) => {
        currentRequest = request;
        sequence += 1;
        const current = sequence;
        active += 1;
        maximumActive = Math.max(maximumActive, active);
        order.push(`engine-start:${current}`);
        const failure = behavior(request);
        if (failure !== null) {
          request.onFailure(failure, true);
        }
        let running = true;
        return {
          stop: () => {
            if (!running) {
              return;
            }
            running = false;
            active -= 1;
            order.push(`engine-stop:${current}`);
          },
        };
      },
    },
    get maximumActive() {
      return maximumActive;
    },
    playing: () => currentRequest?.onPlaying(),
  };
}

class ControlledTime
  implements InstalledPlaybackClock, InstalledPlaybackScheduler
{
  #now = 0;
  #sequence = 0;
  readonly #tasks: Array<{
    readonly id: number;
    readonly dueAt: number;
    readonly task: () => void;
    active: boolean;
  }> = [];
  readonly scheduledDelays: number[] = [];

  now = (): number => this.#now;

  schedule = (delayMs: number, task: () => void): (() => void) => {
    const scheduled = {
      id: ++this.#sequence,
      dueAt: this.#now + delayMs,
      task,
      active: true,
    };
    this.#tasks.push(scheduled);
    this.scheduledDelays.push(delayMs);
    return () => {
      scheduled.active = false;
    };
  };

  get activeTasks(): number {
    return this.#tasks.filter((task) => task.active).length;
  }

  async advanceBy(
    durationMs: number,
    runner: { readonly whenIdle: () => Promise<void> },
  ): Promise<void> {
    const target = this.#now + durationMs;
    while (true) {
      const next = this.#tasks
        .filter((task) => task.active && task.dueAt <= target)
        .sort(
          (left, right) => left.dueAt - right.dueAt || left.id - right.id,
        )[0];
      if (next === undefined) {
        break;
      }
      this.#now = next.dueAt;
      next.active = false;
      next.task();
      await runner.whenIdle();
    }
    this.#now = target;
    await runner.whenIdle();
  }
}

function descriptor(
  sessionNumber: number,
  streamNumber: number,
): NativePlaybackDescriptor {
  const sessionHex = sessionNumber.toString(16).padStart(32, "0").slice(-32);
  const streamHex = streamNumber.toString(16).padStart(16, "0").slice(-16);
  return clientSchemas.nativePlaybackDescriptor.parse({
    _tag: "tauri-native-stream",
    sessionId: `play1_${sessionHex}_1`,
    streamHandle: `stream1_${streamHex}`,
    presentation: "webview-mse",
    tracks: [],
    selection: { _tag: "none" },
  });
}

function androidDescriptor(
  sessionNumber: number,
  streamNumber: number,
): NativePlaybackDescriptor {
  return {
    ...descriptor(sessionNumber, streamNumber),
    presentation: "android-media3",
  };
}

function audioTracks(selected: typeof ENGLISH_AUDIO_ID) {
  return [
    {
      id: ENGLISH_AUDIO_ID,
      language: "eng",
      label: "Original",
      codec: "aac-adts" as const,
      selected: selected === ENGLISH_AUDIO_ID,
    },
    {
      id: SPANISH_AUDIO_ID,
      language: "spa",
      codec: "ac-3" as const,
      selected: selected === SPANISH_AUDIO_ID,
    },
  ];
}

function audioDescriptor(
  sessionNumber: number,
  streamNumber: number,
  selected: typeof ENGLISH_AUDIO_ID,
  selection: NativePlaybackDescriptor["selection"],
  preferenceStatus?: NativePlaybackDescriptor["preferenceStatus"],
): NativePlaybackDescriptor {
  return {
    ...descriptor(sessionNumber, streamNumber),
    tracks: audioTracks(selected),
    selection,
    ...(preferenceStatus === undefined ? {} : { preferenceStatus }),
  };
}

function androidAudioDescriptor(
  sessionNumber: number,
  streamNumber: number,
  selected: typeof ENGLISH_AUDIO_ID,
  selection: NativePlaybackDescriptor["selection"],
): NativePlaybackDescriptor {
  return {
    ...audioDescriptor(sessionNumber, streamNumber, selected, selection),
    presentation: "android-media3",
  };
}

function channel(
  id: string,
  name: string,
): {
  readonly id: ChannelId;
  readonly name: string;
} {
  const parsed = clientSchemas.channel.safeParse({
    id,
    name,
    group: "Fixtures",
  });
  if (!parsed.success) {
    throw new Error("expected a valid Channel fixture");
  }
  return { id: parsed.data.id, name };
}

function cancellationAwareStart(
  value: NativePlaybackDescriptor,
): InstalledPlaybackSession["start"] {
  return (options = {}) =>
    new Promise((resolve) => {
      if (options.signal?.aborted === true) {
        resolve({ ok: false, error: { _tag: "cancelled" } });
        return;
      }
      options.signal?.addEventListener(
        "abort",
        () => resolve({ ok: false, error: { _tag: "cancelled" } }),
        { once: true },
      );
      void value;
    });
}

function expectRecovery(
  phase: { readonly _tag: string; readonly attempt?: number },
  attempt: number,
): void {
  expect(phase._tag).toBe("recovering");
  expect(phase.attempt).toBe(attempt);
}

function androidPresentationFixture() {
  return {
    status: async () =>
      success({
        state: "stopped" as const,
        decodedFrames: 0,
        droppedFrames: 0,
        bufferedDurationMs: 0,
        silent: true,
      }),
    pause: async () => success(undefined),
    resume: async () => success(undefined),
    setVolume: async () => success(undefined),
    setMuted: async () => success(undefined),
    setViewport: async () => success(undefined),
    stop: async () => success(undefined),
  };
}

function success<Value>(value: Value): {
  readonly ok: true;
  readonly value: Value;
} {
  return { ok: true, value };
}

function deferred<Value>(): {
  readonly promise: Promise<Value>;
  readonly resolve: (value: Value) => void;
} {
  let resolve: ((value: Value) => void) | undefined;
  const promise = new Promise<Value>((next) => {
    resolve = next;
  });
  return {
    promise,
    resolve: (value) => {
      if (resolve === undefined) {
        throw new Error("deferred fixture was not initialized");
      }
      resolve(value);
    },
  };
}

async function until(predicate: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 30; attempt += 1) {
    if (predicate()) {
      return;
    }
    await Promise.resolve();
  }
  throw new Error("asynchronous fixture did not settle");
}
