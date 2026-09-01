import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import {
  chmod,
  copyFile,
  mkdir,
  mkdtemp,
  open,
  rm,
  stat,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  parseApkIdentity,
  parseAdbSerial,
  parseCliArguments,
  parseInstalledPackageIdentity,
  verifyDeviceIdentity,
} from "./android-catalog-acceptance-domain.ts";
import {
  combineModernFrameStats,
  evaluatePlaybackGates,
  MAX_DROPPED_FRAMES,
  MAX_MODERN_JANK_PERCENT,
  MIN_DECODED_FRAMES_PER_SECOND,
  MIN_MODERN_UI_FRAMES,
  parseModernFrameStats,
  parsePlaybackMarker,
  REQUIRED_SUSTAINED_MS,
  summarizePlaybackSamples,
  type PlaybackActions,
  type PlaybackMarker,
  type PlaybackUiFrameJourneys,
} from "./android-playback-acceptance-domain.ts";

const HARNESS_VERSION = "3";
const PACKAGE_NAME = "xyz.ponbac.sparrow";
const MAIN_COMPONENT = `${PACKAGE_NAME}/.MainActivity`;
const SILENT_PLAYBACK_EXTRA = `${PACKAGE_NAME}.ACCEPTANCE_SILENT`;
const COMMAND_OUTPUT_LIMIT = 8 * 1024 * 1024;
const START_TIMEOUT_MS = 30_000;
const TRANSITION_TIMEOUT_MS = 15_000;
const SAMPLE_INTERVAL_MS = 1_000;
const WARM_MEDIA_REPLACEMENT_ITERATIONS = 5;

const CATALOG_MARKER_EXPRESSION = `(() => {
  const routineUrlCount = Array.from(document.querySelectorAll("[href], [src], input, textarea"))
    .filter((node) => {
      const liveValue = node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement
        ? node.value
        : "";
      return [liveValue, node.getAttribute("value"), node.getAttribute("href"), node.getAttribute("src")]
        .some((candidate) => /^https?:\\/\\//iu.test((candidate ?? "").trim()));
    }).length;
  return {
    channelCount: document.querySelectorAll("[data-acceptance-channel]").length,
    routineUrlCount,
  };
})()`;

const CLICK_FIRST_CHANNEL_EXPRESSION = `(() => {
  const card = document.querySelectorAll("[data-acceptance-channel]").item(0);
  if (!(card instanceof HTMLButtonElement)) return false;
  card.click();
  return true;
})()`;

const ENFORCE_PROCESS_SILENCE_EXPRESSION = `(() => {
  const video = document.querySelector(".hosted-player video");
  if (!(video instanceof HTMLVideoElement)) return false;
  const mute = Array.from(document.querySelectorAll(".hosted-player__controls button"))
    .find((button) => button.textContent?.trim() === "Mute");
  if (mute instanceof HTMLButtonElement) mute.click();
  const range = document.querySelector('.hosted-player__controls input[aria-label="Volume"]');
  if (range instanceof HTMLInputElement && range.value !== "0") {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    setter?.call(range, "0");
    range.dispatchEvent(new Event("input", { bubbles: true }));
    range.dispatchEvent(new Event("change", { bubbles: true }));
  }
  video.muted = true;
  video.volume = 0;
  video.dispatchEvent(new Event("volumechange"));
  return video.muted && video.volume === 0;
})()`;

const PLAYBACK_MARKER_EXPRESSION = `(() => {
  const video = document.querySelector(".hosted-player video");
  if (!(video instanceof HTMLVideoElement)) return null;
  const engine = video.getAttribute("data-playback-engine");
  const state = video.getAttribute("data-playback-state");
  const dropped = video.getAttribute("data-dropped-frames");
  const buffered = video.getAttribute("data-buffered-duration-ms");
  const processSilent = video.getAttribute("data-process-silent");
  if (engine === null || state === null || dropped === null || buffered === null || processSilent === null) return null;
  const decoded = video.getAttribute("data-decoded-frames");
  const routineUrlCount = Array.from(document.querySelectorAll("[href], [src], input, textarea"))
    .filter((node) => {
      const liveValue = node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement
        ? node.value
        : "";
      return [liveValue, node.getAttribute("value"), node.getAttribute("href"), node.getAttribute("src")]
        .some((candidate) => /^https?:\\/\\//iu.test((candidate ?? "").trim()));
    }).length;
  return {
    engine,
    state,
    droppedFrames: Number(dropped),
    bufferedDurationMs: Number(buffered),
    decodedFrames: decoded === null ? null : Number(decoded),
    processSilent: processSilent === "true",
    muted: video.muted,
    volume: video.volume,
    routineUrlCount,
  };
})()`;

const PRESENTATION_ABSENT_EXPRESSION = `(() =>
  document.querySelector('[data-playback-engine="android-media3"]') === null
)()`;

const PAUSED_WITHOUT_PRESENTATION_EXPRESSION = `(() =>
  document.querySelector('.hosted-player__state[data-state="paused"]') !== null &&
  document.querySelector('[data-playback-engine="android-media3"]') === null
)()`;

const CLICK_PAUSE_EXPRESSION = buttonExpression("Pause");
const CLICK_RESUME_EXPRESSION = buttonExpression("Resume");

/** CDP action requesting the other controlled Audio Track option. */
export const ALTERNATE_AUDIO_SELECTION_EXPRESSION = `(() => {
  const select = document.querySelector('select[aria-label="Audio track"]');
  if (!(select instanceof HTMLSelectElement) || select.options.length < 2) {
    return { available: false, selected: false };
  }
  const next = select.selectedIndex === 0 ? 1 : 0;
  const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "selectedIndex")?.set;
  if (setter === undefined) {
    return { available: true, selected: false };
  }
  setter.call(select, next);
  select.dispatchEvent(new Event("change", { bubbles: true }));
  return { available: true, selected: true };
})()`;

const SELECT_ALTERNATE_AUDIO_EXPRESSION =
  ALTERNATE_AUDIO_SELECTION_EXPRESSION;

const CLICK_STOP_EXPRESSION = `(() => {
  const button = Array.from(document.querySelectorAll(".hosted-player__controls button"))
    .find((candidate) => candidate.textContent?.trim().startsWith("Stop") === true);
  if (!(button instanceof HTMLButtonElement)) return false;
  button.click();
  return true;
})()`;

const PLAYER_REMOVED_EXPRESSION = `(() =>
  document.querySelector(".hosted-player") === null &&
  document.querySelector('[data-playback-engine="android-media3"]') === null
)()`;

interface CommandResult {
  readonly exitCode: number;
  readonly signal: NodeJS.Signals | null;
  readonly stdout: string;
  readonly stderr: string;
}

interface StagedApk {
  readonly root: string;
  readonly path: string;
}

interface SafeFailureEvidence {
  readonly schemaVersion: 1;
  readonly harnessVersion: string;
  readonly recordedAt: string;
  readonly verdict: "rejected";
  readonly failure: { readonly code: string; readonly detail: string };
}

class HarnessFailure extends Error {
  readonly code: string;

  constructor(code: string, detail: string) {
    super(detail);
    this.name = "HarnessFailure";
    this.code = code;
  }
}

class CdpSession {
  private nextId = 1;
  private readonly pending = new Map<
    number,
    {
      readonly resolve: (value: unknown) => void;
      readonly reject: (error: Error) => void;
      readonly timeout: ReturnType<typeof setTimeout>;
    }
  >();

  private constructor(private readonly socket: WebSocket) {
    socket.addEventListener("message", (event) => this.receive(event));
    socket.addEventListener("close", () => this.failPending());
  }

  static async connect(url: string): Promise<CdpSession> {
    const socket = new WebSocket(url);
    await new Promise<void>((resolveConnection, rejectConnection) => {
      const timeout = setTimeout(() => {
        socket.close();
        rejectConnection(new HarnessFailure("cdp-timeout", "the WebView debugger did not open"));
      }, 3_000);
      socket.addEventListener(
        "open",
        () => {
          clearTimeout(timeout);
          resolveConnection();
        },
        { once: true },
      );
      socket.addEventListener(
        "error",
        () => {
          clearTimeout(timeout);
          rejectConnection(new HarnessFailure("cdp-unavailable", "the WebView debugger was unavailable"));
        },
        { once: true },
      );
    });
    return new CdpSession(socket);
  }

  async evaluate(expression: string): Promise<unknown> {
    const response = await this.request("Runtime.evaluate", {
      expression,
      returnByValue: true,
      awaitPromise: true,
    });
    if (!isRecord(response) || !isRecord(response.result)) {
      throw new HarnessFailure("cdp-invalid", "the WebView returned an invalid aggregate probe");
    }
    if (response.exceptionDetails !== undefined || response.result.value === undefined) {
      throw new HarnessFailure("cdp-evaluation", "the aggregate WebView probe failed");
    }
    return response.result.value;
  }

  close(): void {
    this.socket.close();
  }

  private request(method: string, params: Readonly<Record<string, unknown>>): Promise<unknown> {
    const id = this.nextId;
    this.nextId += 1;
    return new Promise<unknown>((resolveRequest, rejectRequest) => {
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        rejectRequest(new HarnessFailure("cdp-timeout", "the WebView aggregate probe timed out"));
      }, 3_000);
      this.pending.set(id, { resolve: resolveRequest, reject: rejectRequest, timeout });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  private receive(event: MessageEvent): void {
    if (typeof event.data !== "string") return;
    let message: unknown;
    try {
      message = JSON.parse(event.data);
    } catch {
      return;
    }
    if (!isRecord(message) || typeof message.id !== "number") return;
    const pending = this.pending.get(message.id);
    if (pending === undefined) return;
    clearTimeout(pending.timeout);
    this.pending.delete(message.id);
    if (message.error !== undefined) {
      pending.reject(new HarnessFailure("cdp-command", "the WebView rejected an aggregate probe"));
      return;
    }
    pending.resolve(message.result);
  }

  private failPending(): void {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeout);
      pending.reject(new HarnessFailure("cdp-closed", "the WebView debugger closed early"));
    }
    this.pending.clear();
  }
}

class Adb {
  constructor(private readonly serial: string) {}

  serialValue(): string {
    return this.serial;
  }

  run(args: readonly string[], label: string, timeoutMs = 30_000): Promise<CommandResult> {
    return requireCommand("adb", args, label, timeoutMs);
  }

  execute(args: readonly string[], label: string, timeoutMs = 30_000): Promise<CommandResult> {
    return executeCommand("adb", args, label, timeoutMs);
  }

  async property(name: string): Promise<string> {
    return (await this.run(["shell", "getprop", name], "device-property")).stdout.trim();
  }
}

async function main(): Promise<void> {
  const parsedArguments = parseCliArguments(process.argv.slice(2));
  const parsedSerial = parseAdbSerial(process.env.ANDROID_SERIAL);
  if (!parsedArguments.ok || !parsedSerial.ok) {
    console.error(
      "usage: ANDROID_SERIAL=<private-adb-serial> bun scripts/android-playback-acceptance.ts --apk <arm64-apk> --output <new-json-file>",
    );
    process.exitCode = 2;
    return;
  }
  const args = parsedArguments.value;
  try {
    if (resolve(args.apk) === resolve(args.output)) {
      throw new HarnessFailure("unsafe-output", "the evidence output cannot replace the APK");
    }
    const evidence = await runAcceptance(parsedSerial.value, args.apk);
    await writeNewEvidence(args.output, evidence);
    console.log(`verdict=${evidence.verdict}`);
    console.log("evidence=written");
    if (evidence.verdict !== "accepted") process.exitCode = 1;
  } catch (error) {
    const failure = safeFailure(error);
    const evidence: SafeFailureEvidence = {
      schemaVersion: 1,
      harnessVersion: HARNESS_VERSION,
      recordedAt: new Date().toISOString(),
      verdict: "rejected",
      failure,
    };
    try {
      await writeNewEvidence(args.output, evidence);
      console.error(`rejected=${failure.code}`);
      console.error("evidence=written");
    } catch {
      console.error(`rejected=${failure.code}`);
      console.error("evidence=unavailable");
    }
    process.exitCode = 1;
  }
}

async function runAcceptance(serial: string, apkPath: string) {
  await requireCommand("adb", ["version"], "required-tool");
  await requireCommand("which", ["apkanalyzer"], "required-tool");
  const staged = await stageApk(apkPath);
  try {
    return await runStagedAcceptance(new Adb(serial), staged.path);
  } finally {
    await rm(staged.root, { recursive: true, force: false });
  }
}

async function runStagedAcceptance(adb: Adb, stagedApk: string) {
  await requireTargetDevice(adb);
  const apkIdentity = unwrap(
    parseApkIdentity({
      summary: (await requireCommand("apkanalyzer", ["apk", "summary", stagedApk], "apk-summary")).stdout,
      targetSdk: (await requireCommand("apkanalyzer", ["manifest", "target-sdk", stagedApk], "apk-target-sdk")).stdout,
      debuggable: (await requireCommand("apkanalyzer", ["manifest", "debuggable", stagedApk], "apk-debuggable")).stdout,
      fileList: (await requireCommand("apkanalyzer", ["files", "list", stagedApk], "apk-files")).stdout,
    }),
    "invalid-apk",
  );
  const apkSha256 = await sha256File(stagedApk);
  const install = await adb.run(
    ["install", "-r", "-t", "--no-streaming", stagedApk],
    "apk-install",
    180_000,
  );
  if (!/Success/u.test(`${install.stdout}\n${install.stderr}`)) {
    throw new HarnessFailure("apk-install", "the candidate APK was not installed");
  }
  if ((await sha256File(stagedApk)) !== apkSha256) {
    throw new HarnessFailure("apk-mutated", "the immutable staged APK changed during installation");
  }
  unwrap(
    parseInstalledPackageIdentity(
      (await adb.run(["shell", "dumpsys", "package", PACKAGE_NAME], "installed-package")).stdout,
      apkIdentity,
    ),
    "installed-package-mismatch",
  );

  const observation = await runPlaybackObservation(adb);
  const gateFailures = evaluatePlaybackGates(observation);
  return {
    schemaVersion: 1 as const,
    harnessVersion: HARNESS_VERSION,
    recordedAt: new Date().toISOString(),
    verdict: gateFailures.length === 0 ? ("accepted" as const) : ("rejected" as const),
    gateFailures,
    gates: {
      sustainedDurationMs: REQUIRED_SUSTAINED_MS,
      minimumDecodedFramesPerSecond: MIN_DECODED_FRAMES_PER_SECOND,
      maximumDroppedFramesWithoutDecodedCounter: MAX_DROPPED_FRAMES,
      maximumDroppedFramePercent: 1,
      minimumModernUiFrames: MIN_MODERN_UI_FRAMES,
      maximumModernJankPercent: MAX_MODERN_JANK_PERCENT,
      modernUiScope: "warm-media-replacement" as const,
      warmMediaReplacementIterations: WARM_MEDIA_REPLACEMENT_ITERATIONS,
      requiredEngine: "android-media3" as const,
      requiredPerProcessSilence: true,
      requiredNativeSilentStatus: true,
      requiredRepresentativeMpegTs: true,
      requiredModernFrameDeadlines: true,
    },
    build: {
      versionCode: apkIdentity.versionCode,
      versionName: apkIdentity.versionName,
      targetSdk: apkIdentity.targetSdk,
      arm64Runtime: apkIdentity.hasArm64Runtime,
      apkSha256,
      installedIdentityVerified: true,
    },
    deviceGate: {
      physicalTargetVerified: true,
      apiLevel: 36 as const,
      primaryAbi: "arm64-v8a" as const,
      serialRecorded: false,
    },
    media: observation.sustained,
    actions: observation.actions,
    uiFrames: observation.uiFrames,
    uiFrameJourneys: observation.uiFrameJourneys,
    warmReplacementUiFrames: observation.warmReplacementUiFrames,
    privacy: {
      providerDetailsRecorded: false,
      catalogContentsRecorded: false,
      deviceSerialRecorded: false,
      routineUrlCount: observation.sustained.routineUrlCount,
    },
  };
}

async function runPlaybackObservation(adb: Adb) {
  await forceStop(adb);
  let pid: string | null = null;
  let cdp: CdpSession | null = null;
  try {
    await adb.run(["shell", "dumpsys", "gfxinfo", PACKAGE_NAME, "reset"], "gfxinfo-reset");
    const launch = await adb.run(
      [
        "shell",
        "am",
        "start",
        "-W",
        "-n",
        MAIN_COMPONENT,
        "--ez",
        SILENT_PLAYBACK_EXTRA,
        "true",
      ],
      "activity-launch",
      15_000,
    );
    if (/Error:/u.test(`${launch.stdout}\n${launch.stderr}`)) {
      throw new HarnessFailure("activity-launch", "the Sparrow activity did not launch");
    }
    pid = await waitForPid(adb, performance.now() + 5_000);
    const forward = await adb.execute(
      ["forward", "tcp:0", `localabstract:webview_devtools_remote_${pid}`],
      "webview-forward",
    );
    requireExpectedExit(forward, [0], "webview-forward");
    const port = forward.stdout.trim();
    if (!/^\d{2,5}$/u.test(port)) {
      throw new HarnessFailure("webview-forward", "adb did not allocate a debugger port");
    }
    await requireWebviewForward(adb, pid, port);
    cdp = await connectCdp(port, performance.now() + 10_000);
    await waitForCatalog(cdp);
    if ((await cdp.evaluate(CLICK_FIRST_CHANNEL_EXPRESSION)) !== true) {
      throw new HarnessFailure("initial-channel", "the first representative Channel could not be selected");
    }
    await waitForNativeSilentSink(cdp);
    if ((await cdp.evaluate(ENFORCE_PROCESS_SILENCE_EXPRESSION)) !== true) {
      throw new HarnessFailure("silent-sink", "the app-owned playback sink could not be set to silence");
    }
    const initial = await waitForPlaying(cdp, START_TIMEOUT_MS);
    const startup = await readAndResetUiFrames(adb, "startup");

    const samples: PlaybackMarker[] = [initial];
    const sustainedStarted = performance.now();
    while (performance.now() - sustainedStarted < REQUIRED_SUSTAINED_MS) {
      await delay(SAMPLE_INTERVAL_MS);
      samples.push(await readPlayingMarker(cdp));
    }
    const durationMs = Math.round(performance.now() - sustainedStarted);
    const sustained = unwrap(
      summarizePlaybackSamples(samples, durationMs),
      "sustained-sample",
    );

    const sustainedPlayback = await readAndResetUiFrames(adb, "sustained-playback");

    const pauseReleasedPresentation = await exercisePause(cdp);
    const resumeReturnedToPlaying = await exerciseResume(cdp);
    const pauseResume = await readAndResetUiFrames(adb, "pause-resume");

    const backgroundReleasedPresentation = await exerciseBackground(adb, cdp);
    const foregroundReturnedToPlaying = await exerciseForeground(adb, cdp);
    const backgroundForeground = await readAndResetUiFrames(adb, "background-foreground");

    const audioSelectionResults = [];
    for (let iteration = 0; iteration < WARM_MEDIA_REPLACEMENT_ITERATIONS; iteration += 1) {
      audioSelectionResults.push(await exerciseAudioSelection(cdp));
    }
    const audioSelection = {
      alternateAudioAvailable: audioSelectionResults.every(
        (result) => result.alternateAudioAvailable,
      ),
      alternateAudioSelected: audioSelectionResults.every(
        (result) => result.alternateAudioSelected,
      ),
      audioSelectionReleasedPresentation: audioSelectionResults.every(
        (result) => result.audioSelectionReleasedPresentation,
      ),
      audioSelectionReturnedToPlaying: audioSelectionResults.every(
        (result) => result.audioSelectionReturnedToPlaying,
      ),
    };
    const audioSelectionFrames = await readAndResetUiFrames(adb, "audio-selection");
    const channelSwitchResults = [];
    for (let iteration = 0; iteration < WARM_MEDIA_REPLACEMENT_ITERATIONS; iteration += 1) {
      const targetIndex = iteration % 2 === 0 ? 1 : 0;
      channelSwitchResults.push(await exerciseChannelSwitch(cdp, targetIndex));
    }
    const channelSwitch = {
      channelSwitched: channelSwitchResults.every((result) => result.channelSwitched),
      channelSwitchReturnedToPlaying: channelSwitchResults.every(
        (result) => result.channelSwitchReturnedToPlaying,
      ),
    };
    const channelSwitchFrames = await readAndResetUiFrames(adb, "channel-switch");
    const stop = await exerciseStop(cdp);
    const stopFrames = await readAndResetUiFrames(adb, "stop");

    const actions: PlaybackActions = {
      initialPlayback: initial.state === "playing",
      pauseReleasedPresentation,
      resumeReturnedToPlaying,
      backgroundReleasedPresentation,
      foregroundReturnedToPlaying,
      ...audioSelection,
      ...channelSwitch,
      ...stop,
    };
    const uiFrameJourneys: PlaybackUiFrameJourneys = {
      startup,
      sustainedPlayback,
      pauseResume,
      backgroundForeground,
      audioSelection: audioSelectionFrames,
      channelSwitch: channelSwitchFrames,
      stop: stopFrames,
    };
    const uiFrames = unwrap(
      combineModernFrameStats(Object.values(uiFrameJourneys)),
      "gfxinfo-invalid",
    );
    const warmReplacementUiFrames = unwrap(
      combineModernFrameStats([audioSelectionFrames, channelSwitchFrames]),
      "gfxinfo-invalid",
    );
    return {
      sustained,
      actions,
      uiFrames,
      uiFrameJourneys,
      warmReplacementUiFrames,
    };
  } finally {
    cdp?.close();
    await cleanupPlaybackObservation(adb, pid);
  }
}

async function readAndResetUiFrames(
  adb: Adb,
  journey: string,
) {
  const uiFrames = unwrap(
    parseModernFrameStats(
      (await adb.run(
        ["shell", "dumpsys", "gfxinfo", PACKAGE_NAME],
        `${journey}-gfxinfo-read`,
      )).stdout,
    ),
    "gfxinfo-invalid",
  );
  await adb.run(
    ["shell", "dumpsys", "gfxinfo", PACKAGE_NAME, "reset"],
    `${journey}-gfxinfo-reset`,
  );
  return uiFrames;
}

async function cleanupPlaybackObservation(adb: Adb, pid: string | null): Promise<void> {
  let confirmed = true;
  if (pid !== null) {
    try {
      await removeWebviewForwards(adb, pid);
    } catch {
      confirmed = false;
    }
  }
  try {
    await forceStop(adb);
  } catch {
    confirmed = false;
  }
  if (!confirmed) {
    throw new HarnessFailure(
      "cleanup-unconfirmed",
      "the app process or exact debugger forward was not proven released",
    );
  }
}

async function waitForCatalog(cdp: CdpSession): Promise<void> {
  const deadline = performance.now() + START_TIMEOUT_MS;
  while (performance.now() < deadline) {
    const marker = await cdp.evaluate(CATALOG_MARKER_EXPRESSION);
    if (
      isRecord(marker) &&
      Number.isSafeInteger(marker.channelCount) &&
      Number(marker.channelCount) >= 2 &&
      marker.routineUrlCount === 0
    ) {
      return;
    }
    await delay(100);
  }
  throw new HarnessFailure(
    "catalog-unavailable",
    "two representative Channels were not available without routine URL exposure",
  );
}

async function waitForNativeSilentSink(cdp: CdpSession): Promise<void> {
  const deadline = performance.now() + START_TIMEOUT_MS;
  while (performance.now() < deadline) {
    const raw = await cdp.evaluate(PLAYBACK_MARKER_EXPRESSION);
    if (raw !== null) {
      const marker = unwrap(parsePlaybackMarker(raw), "playback-marker");
      if (marker.processSilent) return;
    }
    await delay(25);
  }
  throw new HarnessFailure(
    "initial-audio",
    "the debug candidate did not confirm its per-process silent sink before playback",
  );
}

async function waitForPlaying(cdp: CdpSession, timeoutMs: number): Promise<PlaybackMarker> {
  const deadline = performance.now() + timeoutMs;
  while (performance.now() < deadline) {
    const raw = await cdp.evaluate(PLAYBACK_MARKER_EXPRESSION);
    if (raw !== null) {
      const marker = unwrap(parsePlaybackMarker(raw), "playback-marker");
      if (
        marker.state === "playing" &&
        marker.bufferedDurationMs > 0 &&
        marker.muted &&
        marker.volume === 0 &&
        marker.routineUrlCount === 0
      ) {
        return marker;
      }
      if (marker.state === "failed" || marker.state === "stopped") {
        throw new HarnessFailure("playback-failed", "native playback stopped before acceptance");
      }
    }
    await delay(100);
  }
  throw new HarnessFailure("playback-timeout", "native playback did not reach a buffered silent playing state");
}

async function readPlayingMarker(cdp: CdpSession): Promise<PlaybackMarker> {
  const marker = unwrap(
    parsePlaybackMarker(await cdp.evaluate(PLAYBACK_MARKER_EXPRESSION)),
    "playback-marker",
  );
  return marker;
}

async function exercisePause(cdp: CdpSession): Promise<boolean> {
  if ((await cdp.evaluate(CLICK_PAUSE_EXPRESSION)) !== true) return false;
  return waitForBoolean(cdp, PAUSED_WITHOUT_PRESENTATION_EXPRESSION, TRANSITION_TIMEOUT_MS);
}

async function exerciseResume(cdp: CdpSession): Promise<boolean> {
  if ((await cdp.evaluate(CLICK_RESUME_EXPRESSION)) !== true) return false;
  await enforceSilenceWhenMounted(cdp);
  return (await waitForPlaying(cdp, TRANSITION_TIMEOUT_MS)).state === "playing";
}

async function exerciseBackground(adb: Adb, cdp: CdpSession): Promise<boolean> {
  await adb.run(["shell", "input", "keyevent", "KEYCODE_HOME"], "background-app");
  return waitForBoolean(cdp, PAUSED_WITHOUT_PRESENTATION_EXPRESSION, TRANSITION_TIMEOUT_MS);
}

async function exerciseForeground(adb: Adb, cdp: CdpSession): Promise<boolean> {
  const launch = await adb.run(
    [
      "shell",
      "am",
      "start",
      "-W",
      "-n",
      MAIN_COMPONENT,
      "--ez",
      SILENT_PLAYBACK_EXTRA,
      "true",
    ],
    "foreground-app",
    15_000,
  );
  if (/Error:/u.test(`${launch.stdout}\n${launch.stderr}`)) return false;
  await enforceSilenceWhenMounted(cdp);
  return (await waitForPlaying(cdp, TRANSITION_TIMEOUT_MS)).state === "playing";
}

async function exerciseAudioSelection(cdp: CdpSession): Promise<{
  readonly alternateAudioAvailable: boolean;
  readonly alternateAudioSelected: boolean;
  readonly audioSelectionReleasedPresentation: boolean;
  readonly audioSelectionReturnedToPlaying: boolean;
}> {
  const result = await cdp.evaluate(SELECT_ALTERNATE_AUDIO_EXPRESSION);
  if (
    !isRecord(result) ||
    typeof result.available !== "boolean" ||
    typeof result.selected !== "boolean"
  ) {
    return {
      alternateAudioAvailable: false,
      alternateAudioSelected: false,
      audioSelectionReleasedPresentation: false,
      audioSelectionReturnedToPlaying: false,
    };
  }
  if (!result.available || !result.selected) {
    return {
      alternateAudioAvailable: result.available,
      alternateAudioSelected: result.selected,
      audioSelectionReleasedPresentation: false,
      audioSelectionReturnedToPlaying: false,
    };
  }
  const released = await waitForBoolean(
    cdp,
    PRESENTATION_ABSENT_EXPRESSION,
    TRANSITION_TIMEOUT_MS,
  );
  if (!released) {
    return {
      alternateAudioAvailable: true,
      alternateAudioSelected: true,
      audioSelectionReleasedPresentation: false,
      audioSelectionReturnedToPlaying: false,
    };
  }
  await enforceSilenceWhenMounted(cdp);
  return {
    alternateAudioAvailable: true,
    alternateAudioSelected: true,
    audioSelectionReleasedPresentation: true,
    audioSelectionReturnedToPlaying:
      (await waitForPlaying(cdp, TRANSITION_TIMEOUT_MS)).state === "playing",
  };
}

async function exerciseChannelSwitch(cdp: CdpSession, targetIndex: 0 | 1): Promise<{
  readonly channelSwitched: boolean;
  readonly channelSwitchReturnedToPlaying: boolean;
}> {
  if ((await cdp.evaluate(clickChannelExpression(targetIndex))) !== true) {
    return { channelSwitched: false, channelSwitchReturnedToPlaying: false };
  }
  const selected = await waitForBoolean(
    cdp,
    channelSelectedExpression(targetIndex),
    TRANSITION_TIMEOUT_MS,
  );
  if (!selected) return { channelSwitched: false, channelSwitchReturnedToPlaying: false };
  await enforceSilenceWhenMounted(cdp);
  return {
    channelSwitched: true,
    channelSwitchReturnedToPlaying:
      (await waitForPlaying(cdp, TRANSITION_TIMEOUT_MS)).state === "playing",
  };
}

function clickChannelExpression(targetIndex: 0 | 1): string {
  return `(() => {
    const video = document.querySelector(".hosted-player video");
    if (video instanceof HTMLVideoElement) {
      video.setAttribute("data-acceptance-old-channel", "true");
    }
    const card = document.querySelectorAll("[data-acceptance-channel]").item(${targetIndex});
    if (!(card instanceof HTMLButtonElement)) return false;
    card.click();
    return true;
  })()`;
}

function channelSelectedExpression(targetIndex: 0 | 1): string {
  return `(() => {
    const cards = document.querySelectorAll("[data-acceptance-channel]");
    const video = document.querySelector(".hosted-player video");
    return cards.length >= 2 &&
      cards.item(${targetIndex}).getAttribute("aria-pressed") === "true" &&
      video instanceof HTMLVideoElement &&
      !video.hasAttribute("data-acceptance-old-channel");
  })()`;
}

async function exerciseStop(cdp: CdpSession): Promise<{
  readonly stopRemovedPresentation: boolean;
  readonly stopRemovedPlayer: boolean;
}> {
  if ((await cdp.evaluate(CLICK_STOP_EXPRESSION)) !== true) {
    return { stopRemovedPresentation: false, stopRemovedPlayer: false };
  }
  const stopRemovedPresentation = await waitForBoolean(
    cdp,
    PRESENTATION_ABSENT_EXPRESSION,
    TRANSITION_TIMEOUT_MS,
  );
  const stopRemovedPlayer = await waitForBoolean(
    cdp,
    PLAYER_REMOVED_EXPRESSION,
    TRANSITION_TIMEOUT_MS,
  );
  return { stopRemovedPresentation, stopRemovedPlayer };
}

async function enforceSilenceWhenMounted(cdp: CdpSession): Promise<void> {
  const deadline = performance.now() + 5_000;
  while (performance.now() < deadline) {
    if ((await cdp.evaluate(ENFORCE_PROCESS_SILENCE_EXPRESSION)) === true) return;
    await delay(25);
  }
  throw new HarnessFailure("silent-sink", "the app-owned playback sink could not be silenced");
}

async function waitForBoolean(
  cdp: CdpSession,
  expression: string,
  timeoutMs: number,
): Promise<boolean> {
  const deadline = performance.now() + timeoutMs;
  while (performance.now() < deadline) {
    if ((await cdp.evaluate(expression)) === true) return true;
    await delay(100);
  }
  return false;
}

async function requireTargetDevice(adb: Adb): Promise<void> {
  if ((await adb.run(["get-state"], "adb-state")).stdout.trim() !== "device") {
    throw new HarnessFailure("device-unavailable", "the explicit adb device is not ready");
  }
  unwrap(
    verifyDeviceIdentity({
      manufacturer: await adb.property("ro.product.manufacturer"),
      model: await adb.property("ro.product.model"),
      apiLevel: await adb.property("ro.build.version.sdk"),
      primaryAbi: await adb.property("ro.product.cpu.abi"),
      androidRelease: await adb.property("ro.build.version.release"),
      kernelQemu: await adb.property("ro.kernel.qemu"),
      hardware: await adb.property("ro.hardware"),
      productName: await adb.property("ro.product.name"),
    }),
    "wrong-device",
  );
}

async function forceStop(adb: Adb): Promise<void> {
  await adb.run(["shell", "am", "force-stop", PACKAGE_NAME], "force-stop");
  const deadline = performance.now() + 5_000;
  while (performance.now() < deadline) {
    const result = await adb.execute(["shell", "pidof", "-s", PACKAGE_NAME], "pid-check");
    if (isProvenAbsent(result)) return;
    if (result.exitCode !== 0 || !/^\d+$/u.test(result.stdout.trim())) {
      throw new HarnessFailure("pid-check", "the app process state could not be proven");
    }
    await delay(50);
  }
  throw new HarnessFailure("force-stop", "the prior Sparrow process did not stop");
}

async function waitForPid(adb: Adb, deadline: number): Promise<string> {
  while (performance.now() < deadline) {
    const result = await adb.execute(["shell", "pidof", "-s", PACKAGE_NAME], "pid-check");
    const pid = result.stdout.trim();
    if (result.exitCode === 0 && /^\d+$/u.test(pid)) return pid;
    if (!isProvenAbsent(result)) {
      throw new HarnessFailure("pid-check", "the app process state could not be read");
    }
    await delay(25);
  }
  throw new HarnessFailure("process-timeout", "the Sparrow main process did not start");
}

function isProvenAbsent(result: CommandResult): boolean {
  return (
    result.signal === null &&
    result.exitCode === 1 &&
    result.stdout.trim() === "" &&
    result.stderr.trim() === ""
  );
}

async function connectCdp(port: string, deadline: number): Promise<CdpSession> {
  while (performance.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/list`, {
        signal: AbortSignal.timeout(1_000),
      });
      const targets: unknown = await response.json();
      if (Array.isArray(targets)) {
        for (const target of targets) {
          if (
            isRecord(target) &&
            target.type === "page" &&
            typeof target.webSocketDebuggerUrl === "string" &&
            isLocalDebuggerUrl(target.webSocketDebuggerUrl, port)
          ) {
            return await CdpSession.connect(target.webSocketDebuggerUrl);
          }
        }
      }
    } catch {
      // The process appears before the debuggable WebView socket.
    }
    await delay(50);
  }
  throw new HarnessFailure("cdp-unavailable", "the debuggable Sparrow WebView was unavailable");
}

async function requireWebviewForward(adb: Adb, pid: string, port: string): Promise<void> {
  const expected = `localabstract:webview_devtools_remote_${pid}`;
  const matches = await matchingForwards(adb, expected);
  if (matches.length !== 1 || matches[0] !== `tcp:${port}`) {
    throw new HarnessFailure("webview-forward", "the WebView debugger forward was not exact");
  }
}

async function removeWebviewForwards(adb: Adb, pid: string): Promise<void> {
  const expected = `localabstract:webview_devtools_remote_${pid}`;
  for (const host of await matchingForwards(adb, expected)) {
    await adb.run(["forward", "--remove", host], "remove-webview-forward");
  }
  if ((await matchingForwards(adb, expected)).length !== 0) {
    throw new HarnessFailure(
      "remove-webview-forward",
      "the WebView debugger forward remained active",
    );
  }
}

async function matchingForwards(adb: Adb, target: string): Promise<readonly string[]> {
  const output = (await adb.run(["forward", "--list"], "list-webview-forwards")).stdout;
  const matches: string[] = [];
  for (const line of output.split(/\r?\n/u)) {
    if (line.trim() === "") continue;
    const fields = line.trim().split(/\s+/u);
    if (fields.length !== 3) {
      throw new HarnessFailure("list-webview-forwards", "adb returned an invalid forward record");
    }
    const [serial, host, deviceTarget] = fields;
    if (serial === adb.serialValue() && deviceTarget === target) {
      if (host === undefined || !/^tcp:\d{2,5}$/u.test(host)) {
        throw new HarnessFailure("list-webview-forwards", "adb returned an unsafe forward record");
      }
      matches.push(host);
    }
  }
  return matches;
}

async function stageApk(inputPath: string): Promise<StagedApk> {
  const input = await stat(inputPath).catch(() => null);
  if (input === null || !input.isFile()) {
    throw new HarnessFailure("apk-unavailable", "the explicit APK is not a regular file");
  }
  const root = await mkdtemp(join(tmpdir(), "sparrow-android-playback-acceptance-"));
  await chmod(root, 0o700);
  const path = join(root, "candidate.apk");
  try {
    await copyFile(inputPath, path);
    await chmod(path, 0o400);
    const staged = await stat(path);
    if (!staged.isFile() || staged.nlink !== 1 || staged.size !== input.size) {
      throw new HarnessFailure("apk-stage", "the private staged APK is not an exact regular copy");
    }
    return { root, path };
  } catch (error) {
    await rm(root, { recursive: true, force: true });
    throw error;
  }
}

async function sha256File(path: string): Promise<string> {
  const hash = createHash("sha256");
  await new Promise<void>((resolveHash, rejectHash) => {
    const input = createReadStream(path);
    input.on("data", (chunk) => hash.update(chunk));
    input.on("end", resolveHash);
    input.on("error", () => rejectHash(new HarnessFailure("apk-hash", "the APK could not be hashed")));
  });
  return hash.digest("hex");
}

async function writeNewEvidence(path: string, evidence: unknown): Promise<void> {
  await mkdir(dirname(resolve(path)), { recursive: true, mode: 0o700 });
  const file = await open(path, "wx", 0o600);
  try {
    await file.writeFile(`${JSON.stringify(evidence, null, 2)}\n`, "utf8");
    await file.sync();
  } finally {
    await file.close();
  }
}

function executeCommand(
  command: string,
  args: readonly string[],
  label: string,
  timeoutMs = 30_000,
): Promise<CommandResult> {
  return new Promise<CommandResult>((resolveCommand, rejectCommand) => {
    const child = spawn(command, [...args], { stdio: ["ignore", "pipe", "pipe"], timeout: timeoutMs });
    let stdout = "";
    let stderr = "";
    let exceeded = false;
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
      if (stdout.length + stderr.length > COMMAND_OUTPUT_LIMIT) {
        exceeded = true;
        child.kill();
      }
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
      if (stdout.length + stderr.length > COMMAND_OUTPUT_LIMIT) {
        exceeded = true;
        child.kill();
      }
    });
    child.once("error", () => rejectCommand(new HarnessFailure(label, `${label} could not start`)));
    child.once("close", (code, signal) => {
      if (exceeded) {
        rejectCommand(new HarnessFailure(label, `${label} returned too much output`));
        return;
      }
      resolveCommand({ exitCode: code ?? 1, signal, stdout, stderr });
    });
  });
}

async function requireCommand(
  command: string,
  args: readonly string[],
  label: string,
  timeoutMs = 30_000,
): Promise<CommandResult> {
  const result = await executeCommand(command, args, label, timeoutMs);
  if (result.signal !== null || result.exitCode !== 0) {
    throw new HarnessFailure(label, `${label} failed`);
  }
  return result;
}

function requireExpectedExit(result: CommandResult, allowed: readonly number[], label: string): void {
  if (result.signal !== null || !allowed.includes(result.exitCode)) {
    throw new HarnessFailure(label, `${label} did not complete with an expected result`);
  }
}

function unwrap<Value>(
  result:
    | { readonly ok: true; readonly value: Value }
    | { readonly ok: false; readonly reason: string },
  code: string,
): Value {
  if (!result.ok) throw new HarnessFailure(code, result.reason);
  return result.value;
}

function safeFailure(error: unknown): SafeFailureEvidence["failure"] {
  return error instanceof HarnessFailure
    ? { code: error.code, detail: error.message }
    : { code: "harness-defect", detail: "the acceptance harness stopped unexpectedly" };
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isLocalDebuggerUrl(value: string, expectedPort: string): boolean {
  try {
    const url = new URL(value);
    return (
      url.protocol === "ws:" &&
      (url.hostname === "127.0.0.1" || url.hostname === "localhost") &&
      url.port === expectedPort &&
      url.pathname.startsWith("/devtools/page/")
    );
  } catch {
    return false;
  }
}

function buttonExpression(label: string): string {
  return `(() => {
    const button = Array.from(document.querySelectorAll(".hosted-player__controls button"))
      .find((candidate) => candidate.textContent?.trim() === ${JSON.stringify(label)});
    if (!(button instanceof HTMLButtonElement)) return false;
    button.click();
    return true;
  })()`;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}

const invokedPath = process.argv[1];
if (invokedPath !== undefined && resolve(invokedPath) === fileURLToPath(import.meta.url)) {
  await main();
}
