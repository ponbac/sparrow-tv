import { createHash, randomBytes } from "node:crypto";
import { createReadStream } from "node:fs";
import {
  chmod,
  copyFile,
  mkdir,
  mkdtemp,
  open,
  realpath,
  rm,
  stat,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  evaluateAcceptanceGates,
  isUsableCatalog,
  parseApkIdentity,
  parseAdbSerial,
  parseCliArguments,
  parseInstalledPackageIdentity,
  parseProcessMemory,
  parseReadinessMarker,
  parseSnapshotManifest,
  parseSnapshotPointer,
  parseTotalPssKiB,
  verifyDeviceIdentity,
} from "./android-catalog-acceptance-domain.ts";

const HARNESS_VERSION = "1";
const PACKAGE_NAME = "xyz.ponbac.sparrow";
const MAIN_COMPONENT = `${PACKAGE_NAME}/.MainActivity`;
const PRIVATE_SNAPSHOT_ROOT = "private-v1/snapshots-v1";
const READY_TIMEOUT_MS = 30_000;
const BROWSE_TIMEOUT_MS = 5_000;
const MIN_M3U_BYTES = 64 * 1024 * 1024;
const MIN_EPG_BYTES = 24 * 1024 * 1024;
const COMMAND_OUTPUT_LIMIT = 4 * 1024 * 1024;
const PUBLIC_IPV4 = "1.1.1.1";
const PUBLIC_IPV6 = "2606:4700:4700::1111";

const READINESS_EXPRESSION = `(() => {
  const allowedStates = new Set(["fresh", "stale", "refreshing", "failed"]);
  const rawState = document.querySelector(".status-readout")?.getAttribute("data-state") ?? "unknown";
  const routineUrlCount = Array.from(document.querySelectorAll("[href], [src], input, textarea"))
    .filter((node) => {
      const liveValue = node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement
        ? node.value
        : "";
      return [liveValue, node.getAttribute("value"), node.getAttribute("href"), node.getAttribute("src")]
        .some((candidate) => /^https?:\\/\\//iu.test((candidate ?? "").trim()));
    }).length;
  return {
    nativeBridge: typeof window.__TAURI_INTERNALS__?.invoke === "function",
    catalogShell: document.querySelector(".catalog-shell") !== null,
    loading: document.querySelector(".catalog-loading") !== null,
    channelCount: document.querySelectorAll(".channel-card").length,
    groupButtonCount: document.querySelectorAll(".group-button").length,
    searchVisible: document.querySelector(".search-console") !== null,
    retainedCatalog: document.querySelector(".retained-banner") !== null,
    alertCount: document.querySelectorAll('[role="alert"]').length,
    routineUrlCount,
    statusState: allowedStates.has(rawState) ? rawState : "unknown",
    browseReady: document.querySelector(".inspector-details") !== null,
  };
})()`;

const SELECT_FIRST_CHANNEL_EXPRESSION = `(() => {
  const first = document.querySelector(".channel-card");
  if (!(first instanceof HTMLButtonElement)) return false;
  first.click();
  return true;
})()`;

interface CommandResult {
  readonly exitCode: number;
  readonly signal: NodeJS.Signals | null;
  readonly stdout: string;
  readonly stderr: string;
}

interface SafeFailureEvidence {
  readonly schemaVersion: 1;
  readonly harnessVersion: string;
  readonly recordedAt: string;
  readonly verdict: "rejected";
  readonly failure: {
    readonly code: string;
    readonly detail: string;
  };
}

interface PrivateSlot {
  readonly slot: "a" | "b";
  readonly payloadPath: string;
  readonly decodedBytes: number;
  readonly privateChecksum: string;
  readonly privateSourceKey: string;
  readonly identity: PrivateFileIdentity;
}

interface PreparedSource {
  readonly kind: "m3u" | "epg";
  readonly pointerPath: string;
  readonly active: PrivateSlot;
  readonly fallback: PrivateSlot;
}

interface RunEvidence {
  readonly kind: "baseline" | "corrupt-recovery" | "restored";
  readonly index: number;
  readonly processCold: true;
  readonly readyMs: number;
  readonly browseReadyMs: number;
  readonly channelCount: number;
  readonly groupButtonCount: number;
  readonly retainedCatalog: boolean;
  readonly statusState: string;
  readonly alertCount: number;
  readonly routineUrlCount: number;
  readonly vmHwmKiB: number;
  readonly peakVmRssKiB: number;
  readonly totalPssKiB: number | null;
  readonly cgroupCurrentKiB: number | null;
  readonly usable: boolean;
  readonly browseReady: boolean;
}

interface RecoveryEvidence {
  readonly corruptedKind: "m3u";
  readonly sameLengthCorruption: boolean;
  readonly contentChanged: boolean;
  readonly fallbackAdopted: boolean;
  readonly recoveredCatalogUsable: boolean;
  readonly payloadRestoredExactly: boolean;
  readonly pointerRestoredExactly: boolean;
  readonly restoredCatalogUsable: boolean;
  readonly prePointerSlot: "a" | "b";
  readonly postPointerSlot: "a" | "b";
  readonly pointerChangedToFallback: boolean;
  readonly activeFileIdentityRevalidated: boolean;
  readonly recoveryVmHwmKiB: number;
  readonly restoredVmHwmKiB: number;
}

interface PrivateFileIdentity {
  readonly canonicalPath: string;
  readonly mode: number;
  readonly links: number;
  readonly device: string;
  readonly inode: string;
  readonly size: number;
}

interface StagedApk {
  readonly root: string;
  readonly path: string;
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
  readonly socket: WebSocket;
  private nextId = 1;
  private readonly pending = new Map<
    number,
    {
      readonly resolve: (value: unknown) => void;
      readonly reject: (error: Error) => void;
      readonly timeout: ReturnType<typeof setTimeout>;
    }
  >();

  private constructor(socket: WebSocket) {
    this.socket = socket;
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
          rejectConnection(
            new HarnessFailure("cdp-unavailable", "the WebView debugger was unavailable"),
          );
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
      throw new HarnessFailure("cdp-invalid", "the WebView returned an invalid evaluation");
    }
    if (response.exceptionDetails !== undefined || response.result.value === undefined) {
      throw new HarnessFailure("cdp-evaluation", "the redacted WebView probe failed");
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
        rejectRequest(new HarnessFailure("cdp-timeout", "the WebView probe timed out"));
      }, 3_000);
      this.pending.set(id, {
        resolve: resolveRequest,
        reject: rejectRequest,
        timeout,
      });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  private receive(event: MessageEvent): void {
    if (typeof event.data !== "string") {
      return;
    }
    let message: unknown;
    try {
      message = JSON.parse(event.data);
    } catch {
      return;
    }
    if (!isRecord(message) || typeof message.id !== "number") {
      return;
    }
    const pending = this.pending.get(message.id);
    if (pending === undefined) {
      return;
    }
    clearTimeout(pending.timeout);
    this.pending.delete(message.id);
    if (message.error !== undefined) {
      pending.reject(new HarnessFailure("cdp-command", "the WebView rejected a probe"));
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

async function main(): Promise<void> {
  const parsedArguments = parseCliArguments(process.argv.slice(2));
  const parsedSerial = parseAdbSerial(process.env.ANDROID_SERIAL);
  if (!parsedArguments.ok || !parsedSerial.ok) {
    console.error(
      "usage: ANDROID_SERIAL=<private-adb-serial> bun scripts/android-catalog-acceptance.ts --apk <arm64-apk> --output <new-json-file>",
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
    if (evidence.verdict !== "accepted") {
      process.exitCode = 1;
    }
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

async function runAcceptance(serial: string, apk: string) {
  await requireTool("adb", ["version"]);
  await findExecutable("apkanalyzer");
  const staged = await stageApk(apk);
  try {
    return await runStagedAcceptance(serial, staged);
  } finally {
    await rm(staged.root, { recursive: true, force: false });
  }
}

async function runStagedAcceptance(serial: string, staged: StagedApk) {
  const apkFile = await stat(staged.path);

  const adb = new Adb(serial);
  const device = await requireTargetDevice(adb);

  const apkIdentity = unwrap(
    parseApkIdentity({
      summary: (await requireTool("apkanalyzer", ["apk", "summary", staged.path])).stdout,
      targetSdk: (await requireTool("apkanalyzer", ["manifest", "target-sdk", staged.path])).stdout,
      debuggable: (await requireTool("apkanalyzer", ["manifest", "debuggable", staged.path])).stdout,
      fileList: (await requireTool("apkanalyzer", ["files", "list", staged.path])).stdout,
    }),
    "invalid-apk",
  );
  const apkSha256 = await sha256File(staged.path);
  const install = await adb.run(
    ["install", "-r", "--no-streaming", staged.path],
    "apk-install",
    180_000,
  );
  if (!/Success/u.test(`${install.stdout}\n${install.stderr}`)) {
    throw new HarnessFailure("apk-install", "the candidate APK was not installed");
  }
  if ((await sha256File(staged.path)) !== apkSha256) {
    throw new HarnessFailure("apk-mutated", "the immutable staged APK changed during installation");
  }
  const installed = await verifyInstalledCandidate(adb, apkIdentity, apkSha256);
  await adb.runAs(["id"], "run-as-check");

  const m3u = await prepareSource(adb, "m3u", MIN_M3U_BYTES);
  const epg = await prepareSource(adb, "epg", MIN_EPG_BYTES);
  const baselineRuns: RunEvidence[] = [];
  for (let index = 1; index <= 3; index += 1) {
    baselineRuns.push(await runProcessCold(adb, "baseline", index));
  }

  const recovery = await exerciseCorruptRecovery(adb);
  const gateFailures = evaluateAcceptanceGates(baselineRuns, recovery);
  const adbVersionOutput = (await requireTool("adb", ["version"])).stdout;
  const adbVersion = /Version\s+([^\r\n]+)/u.exec(adbVersionOutput)?.[1]?.trim() ?? "unknown";
  const analyzerPath = await realpath(await findExecutable("apkanalyzer"));
  const analyzerVersion = /\/cmdline-tools\/([^/]+)\/bin\/apkanalyzer$/u.exec(analyzerPath)?.[1] ?? "unknown";

  return {
    schemaVersion: 1 as const,
    harnessVersion: HARNESS_VERSION,
    recordedAt: new Date().toISOString(),
    verdict: gateFailures.length === 0 ? ("accepted" as const) : ("rejected" as const),
    gateFailures,
    gates: {
      startupLimitMs: 3_000,
      vmHwmLimitKiB: 524_288,
      requiredBaselineRuns: 3,
      requiredFirstPageChannels: 24,
      representativeMinimumBytes: { m3u: MIN_M3U_BYTES, epg: MIN_EPG_BYTES },
    },
    tools: {
      adb: safeVersion(adbVersion),
      apkAnalyzer: safeVersion(analyzerVersion),
      bun: safeVersion(process.versions.bun ?? "unknown"),
    },
    build: {
      packageName: apkIdentity.packageName,
      versionCode: apkIdentity.versionCode,
      versionName: apkIdentity.versionName,
      targetSdk: apkIdentity.targetSdk,
      debuggable: apkIdentity.debuggable,
      arm64Runtime: apkIdentity.hasArm64Runtime,
      apkBytes: apkFile.size,
      apkSha256,
      installedBaseApkSha256: installed.baseApkSha256,
      installedIdentityVerified: true,
    },
    device: {
      manufacturer: device.manufacturer,
      model: device.model,
      apiLevel: device.apiLevel,
      primaryAbi: device.primaryAbi,
      androidRelease: safeVersion(device.androidRelease),
    },
    offline: {
      airplaneMode: true,
      wifiDisabled: true,
      publicIpv4Route: false,
      publicIpv6Route: false,
      boundedTcpIpv4: "unreachable" as const,
      boundedTcpIpv6: "unreachable" as const,
      checkedBeforeEveryLaunch: true,
    },
    snapshots: [safeSourceEvidence(m3u), safeSourceEvidence(epg)],
    runs: baselineRuns,
    recovery,
  };
}

async function stageApk(inputPath: string): Promise<StagedApk> {
  const input = await stat(inputPath).catch(() => null);
  if (input === null || !input.isFile()) {
    throw new HarnessFailure("apk-unavailable", "the explicit APK is not a regular file");
  }
  const root = await mkdtemp(join(tmpdir(), "sparrow-android-acceptance-"));
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

async function requireTargetDevice(adb: Adb) {
  const state = (await adb.run(["get-state"], "adb-state")).stdout.trim();
  if (state !== "device") {
    throw new HarnessFailure("device-unavailable", "the explicit adb device is not ready");
  }
  return unwrap(
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

async function verifyInstalledCandidate(
  adb: Adb,
  expected: {
    readonly versionCode: number;
    readonly versionName: string;
    readonly targetSdk: 36;
  },
  stagedSha256: string,
): Promise<{ readonly baseApkSha256: string | null }> {
  unwrap(
    parseInstalledPackageIdentity(
      (await adb.run(["shell", "dumpsys", "package", PACKAGE_NAME], "installed-package")).stdout,
      expected,
    ),
    "installed-package-mismatch",
  );
  const pathOutput = (
    await adb.run(["shell", "pm", "path", PACKAGE_NAME], "installed-package-path")
  ).stdout;
  const basePaths = pathOutput
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .filter((line) => line.startsWith("package:") && line.endsWith("/base.apk"))
    .map((line) => line.slice("package:".length));
  if (
    basePaths.length !== 1 ||
    !/^\/data\/app\/[A-Za-z0-9._=+~/-]+\/base\.apk$/u.test(basePaths[0] ?? "")
  ) {
    throw new HarnessFailure("installed-package-path", "the installed base APK path was invalid");
  }
  const basePath = basePaths[0];
  if (basePath === undefined) {
    throw new HarnessFailure("installed-package-path", "the installed base APK path was missing");
  }
  const readable = await adb.execute(
    ["shell", "toybox", "test", "-r", basePath],
    "installed-package-readable",
  );
  requireExpectedExit(readable, [0, 1], "installed-package-readable");
  if (readable.exitCode === 1) {
    return { baseApkSha256: null };
  }
  const digest = parseSha256(
    (await adb.run(["shell", "sha256sum", basePath], "installed-package-hash", 180_000)).stdout,
    "installed-package-hash",
  );
  if (digest !== stagedSha256) {
    throw new HarnessFailure("installed-package-hash", "the installed base APK differs from the staged APK");
  }
  return { baseApkSha256: digest };
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

  runAs(args: readonly string[], label: string): Promise<CommandResult> {
    return this.run(["shell", "run-as", PACKAGE_NAME, ...args], label);
  }

  async property(name: string): Promise<string> {
    return (await this.run(["shell", "getprop", name], "device-property")).stdout.trim();
  }
}

async function verifyOffline(adb: Adb): Promise<void> {
  const airplane = (
    await adb.run(["shell", "settings", "get", "global", "airplane_mode_on"], "airplane-mode")
  ).stdout.trim();
  const wifi = (await adb.run(["shell", "cmd", "wifi", "status"], "wifi-status")).stdout;
  if (airplane !== "1" || !/disabled/iu.test(wifi)) {
    throw new HarnessFailure(
      "device-online",
      "enable airplane mode and disable Wi-Fi before the offline acceptance run",
    );
  }
  const ncHelp = await adb.execute(["shell", "toybox", "nc", "--help"], "network-tool-help");
  requireExpectedExit(ncHelp, [0, 1], "network-tool-help");
  if (!/usage:\s+nc/iu.test(`${ncHelp.stdout}\n${ncHelp.stderr}`) || !/-w/iu.test(`${ncHelp.stdout}\n${ncHelp.stderr}`)) {
    throw new HarnessFailure("network-tool-help", "the bounded Android TCP probe is unavailable");
  }
  const ipv4Routes = (
    await adb.run(["shell", "ip", "-4", "route", "show", "table", "all"], "ipv4-routes")
  ).stdout;
  const ipv6Routes = (
    await adb.run(["shell", "ip", "-6", "route", "show", "table", "all"], "ipv6-routes")
  ).stdout;
  const routedDefault = /^(?!\s*(?:unreachable|prohibit|blackhole|throw)\s+)\s*default\b/mu;
  if (routedDefault.test(ipv4Routes) || routedDefault.test(ipv6Routes)) {
    throw new HarnessFailure("public-route", "the device still exposes a public default route");
  }
  await requireNoPublicRoute(
    adb,
    ["shell", "ip", "-4", "route", "get", PUBLIC_IPV4],
    "public-ipv4-route",
  );
  await requireNoPublicRoute(
    adb,
    ["shell", "ip", "-6", "route", "get", PUBLIC_IPV6],
    "public-ipv6-route",
  );
  await requireTcpUnreachable(
    adb,
    ["shell", "toybox", "nc", "-4", "-w", "2", PUBLIC_IPV4, "443"],
    "public-ipv4-tcp",
  );
  await requireTcpUnreachable(
    adb,
    ["shell", "toybox", "nc", "-6", "-w", "2", PUBLIC_IPV6, "443"],
    "public-ipv6-tcp",
  );
}

async function requireNoPublicRoute(
  adb: Adb,
  args: readonly string[],
  label: string,
): Promise<void> {
  const result = await adb.execute(args, label, 5_000);
  if (
    result.signal !== null ||
    result.exitCode !== 2 ||
    !/network is unreachable|unreachable/iu.test(`${result.stdout}\n${result.stderr}`)
  ) {
    throw new HarnessFailure(label, "the device public-route probe did not fail closed");
  }
}

async function requireTcpUnreachable(
  adb: Adb,
  args: readonly string[],
  label: string,
): Promise<void> {
  const result = await adb.execute(args, label, 5_000);
  if (
    result.signal !== null ||
    result.exitCode !== 1 ||
    /usage:\s+nc|unknown option|invalid option/iu.test(`${result.stdout}\n${result.stderr}`)
  ) {
    throw new HarnessFailure(label, "the bounded public TCP probe did not prove unreachability");
  }
}

async function prepareSource(
  adb: Adb,
  kind: "m3u" | "epg",
  minimumBytes: number,
): Promise<PreparedSource> {
  const directory = `${PRIVATE_SNAPSHOT_ROOT}/${kind}`;
  const pointerPath = `${directory}/active.json`;
  const pointer = unwrap(
    parseSnapshotPointer((await adb.runAs(["cat", pointerPath], "snapshot-pointer")).stdout),
    "invalid-snapshot-pointer",
  );
  const slots = await Promise.all(
    (["a", "b"] as const).map(async (slot): Promise<PrivateSlot> => {
      const manifestPath = `${directory}/slot-${slot}.manifest.json`;
      const payloadPath = `${directory}/slot-${slot}.payload`;
      const manifest = unwrap(
        parseSnapshotManifest(
          (await adb.runAs(["cat", manifestPath], "snapshot-manifest")).stdout,
          kind,
        ),
        "invalid-snapshot-manifest",
      );
      const identity = await privateFileIdentity(adb, payloadPath);
      const decodedBytes = identity.size;
      if (decodedBytes !== manifest.decodedBytes) {
        throw new HarnessFailure("snapshot-length", "a snapshot payload length does not match its manifest");
      }
      return {
        slot,
        payloadPath,
        decodedBytes,
        privateChecksum: manifest.privateChecksum,
        privateSourceKey: manifest.privateSourceKey,
        identity,
      };
    }),
  );
  const active = slots.find((slot) => slot.slot === pointer.slot);
  const fallback = slots.find((slot) => slot.slot !== pointer.slot);
  if (active === undefined || fallback === undefined) {
    throw new HarnessFailure("snapshot-slots", "both atomic snapshot slots are required");
  }
  if (
    active.privateChecksum !== pointer.privateChecksum ||
    active.privateSourceKey !== fallback.privateSourceKey
  ) {
    throw new HarnessFailure("snapshot-identity", "the two snapshot slots do not form one source lineage");
  }
  if (active.decodedBytes < minimumBytes || fallback.decodedBytes < minimumBytes) {
    throw new HarnessFailure(
      "source-too-small",
      "both active and fallback snapshots must contain the agreed representative source",
    );
  }
  return { kind, pointerPath, active, fallback };
}

async function runProcessCold(
  adb: Adb,
  kind: RunEvidence["kind"],
  index: number,
): Promise<RunEvidence> {
  await forceStop(adb);
  await requireTargetDevice(adb);
  await verifyOffline(adb);
  const started = performance.now();
  const launch = await adb.run(
    ["shell", "am", "start", "-W", "-n", MAIN_COMPONENT],
    "activity-launch",
    15_000,
  );
  if (/Error:/u.test(`${launch.stdout}\n${launch.stderr}`)) {
    throw new HarnessFailure("activity-launch", "the Sparrow activity did not launch");
  }
  const pid = await waitForPid(adb, started + 5_000);
  let peakVmHwmKiB = 0;
  let peakVmRssKiB = 0;
  const sample = async () => {
    const memory = unwrap(
      parseProcessMemory(
        (await adb.runAs(["cat", `/proc/${pid}/status`], "process-memory")).stdout,
      ),
      "process-memory",
    );
    peakVmHwmKiB = Math.max(peakVmHwmKiB, memory.vmHwmKiB);
    peakVmRssKiB = Math.max(peakVmRssKiB, memory.vmRssKiB);
  };
  await sample();

  const forward = await adb.execute(
    ["forward", "tcp:0", `localabstract:webview_devtools_remote_${pid}`],
    "webview-forward",
  );
  const port = forward.stdout.trim();
  let cdp: CdpSession | null = null;
  try {
    requireExpectedExit(forward, [0], "webview-forward");
    if (!/^\d{2,5}$/u.test(port)) {
      throw new HarnessFailure("webview-forward", "adb did not allocate a WebView debugger port");
    }
    await requireWebviewForward(adb, pid, port);
    cdp = await connectCdp(port, started + 10_000);
    let marker = await readMarker(cdp);
    while (!isUsableCatalog(marker)) {
      if (performance.now() - started > READY_TIMEOUT_MS) {
        throw new HarnessFailure("catalog-timeout", "the first catalog page did not become usable");
      }
      await sample();
      await delay(50);
      marker = await readMarker(cdp);
    }
    await sample();
    const readyMs = Math.round(performance.now() - started);
    const clicked = await cdp.evaluate(SELECT_FIRST_CHANNEL_EXPRESSION);
    if (clicked !== true) {
      throw new HarnessFailure("browse-probe", "the first catalog card could not be selected");
    }
    const browseDeadline = performance.now() + BROWSE_TIMEOUT_MS;
    marker = await readMarker(cdp);
    while (!marker.browseReady) {
      if (performance.now() > browseDeadline) {
        throw new HarnessFailure("browse-timeout", "the initial catalog browse did not resolve");
      }
      await sample();
      await delay(50);
      marker = await readMarker(cdp);
    }
    const browseReadyMs = Math.round(performance.now() - started);
    const settleDeadline = performance.now() + 500;
    while (performance.now() < settleDeadline) {
      await sample();
      await delay(50);
    }
    const totalPssKiB = parseTotalPssKiB(
      (await adb.run(["shell", "dumpsys", "meminfo", pid], "process-pss")).stdout,
    );
    return {
      kind,
      index,
      processCold: true,
      readyMs,
      browseReadyMs,
      channelCount: marker.channelCount,
      groupButtonCount: marker.groupButtonCount,
      retainedCatalog: marker.retainedCatalog,
      statusState: marker.statusState,
      alertCount: marker.alertCount,
      routineUrlCount: marker.routineUrlCount,
      vmHwmKiB: peakVmHwmKiB,
      peakVmRssKiB,
      totalPssKiB,
      cgroupCurrentKiB: await readCgroupCurrentKiB(adb, pid),
      usable: isUsableCatalog(marker),
      browseReady: marker.browseReady,
    };
  } finally {
    cdp?.close();
    await removeWebviewForwards(adb, pid);
  }
}

async function exerciseCorruptRecovery(adb: Adb): Promise<RecoveryEvidence> {
  await forceStop(adb);
  await requireTargetDevice(adb);
  await verifyOffline(adb);
  const source = await prepareSource(adb, "m3u", MIN_M3U_BYTES);
  const prePointer = {
    slot: source.active.slot,
    privateChecksum: source.active.privateChecksum,
  };
  const originalPayloadSha = await privateSha256(adb, source.active.payloadPath);
  const originalPointerSha = await privateSha256(adb, source.pointerPath);
  const token = randomBytes(8).toString("hex");
  const backupRoot = `private-v1/.catalog-acceptance-${token}`;
  const payloadBackup = `${backupRoot}/active.payload`;
  const pointerBackup = `${backupRoot}/active.json`;
  let backupReady = false;
  let payloadRestoredExactly = false;
  let pointerRestoredExactly = false;
  let activeFileIdentityRevalidated = false;
  let recoveryRun: RunEvidence | null = null;
  let postPointer: { readonly slot: "a" | "b"; readonly privateChecksum: string } | null = null;
  let primaryError: unknown = null;

  try {
    await adb.runAs(["mkdir", "-m", "700", backupRoot], "create-snapshot-backup");
    await adb.runAs(["cp", source.active.payloadPath, payloadBackup], "backup-snapshot-payload");
    await adb.runAs(["cp", source.pointerPath, pointerBackup], "backup-snapshot-pointer");
    await adb.runAs(["chmod", "600", payloadBackup, pointerBackup], "protect-snapshot-backup");
    await syncPrivateFile(adb, payloadBackup, "sync-snapshot-payload-backup");
    await syncPrivateFile(adb, pointerBackup, "sync-snapshot-pointer-backup");
    if (
      (await privateSha256(adb, payloadBackup)) !== originalPayloadSha ||
      (await privateSha256(adb, pointerBackup)) !== originalPointerSha
    ) {
      throw new HarnessFailure("snapshot-backup", "the private recovery backup was not exact");
    }
    backupReady = true;
    const offset = await findNonZeroOffset(adb, source.active);

    await requireTargetDevice(adb);
    await verifyOffline(adb);
    await assertQuiescent(adb);
    const currentIdentity = await privateFileIdentity(adb, source.active.payloadPath);
    const currentPointer = unwrap(
      parseSnapshotPointer(
        (await adb.runAs(["cat", source.pointerPath], "pre-mutation-snapshot-pointer")).stdout,
      ),
      "pre-mutation-snapshot-pointer",
    );
    activeFileIdentityRevalidated = samePrivateFile(source.active.identity, currentIdentity);
    if (
      !activeFileIdentityRevalidated ||
      currentPointer.slot !== prePointer.slot ||
      currentPointer.privateChecksum !== prePointer.privateChecksum
    ) {
      throw new HarnessFailure(
        "stale-active-snapshot",
        "the active snapshot changed before the corruption probe",
      );
    }

    await adb.runAs(
      [
        "dd",
        "if=/dev/zero",
        `of=${source.active.payloadPath}`,
        "bs=1",
        `seek=${offset}`,
        "count=1",
        "conv=notrunc",
      ],
      "corrupt-active-snapshot",
    );
    await syncPrivateFile(adb, source.active.payloadPath, "sync-corrupt-active-snapshot");
    const corruptedIdentity = await privateFileIdentity(adb, source.active.payloadPath);
    const corruptedSha = await privateSha256(adb, source.active.payloadPath);
    if (
      corruptedIdentity.size !== source.active.decodedBytes ||
      corruptedSha === originalPayloadSha
    ) {
      throw new HarnessFailure("corruption-probe", "the same-length corruption probe did not apply");
    }
    recoveryRun = await runProcessCold(adb, "corrupt-recovery", 1);
    await requireTargetDevice(adb);
    await forceStop(adb);
    postPointer = unwrap(
      parseSnapshotPointer(
        (await adb.runAs(["cat", source.pointerPath], "adopted-snapshot-pointer")).stdout,
      ),
      "invalid-adopted-pointer",
    );
  } catch (error) {
    primaryError = error;
  }

  try {
    await requireTargetDevice(adb);
    await forceStop(adb);
    await assertQuiescent(adb);
  } catch {
    throw new HarnessFailure(
      "restore-not-quiescent",
      "the app could not be proven quiescent; the private backup was retained",
    );
  }
  if (backupReady) {
    ({ payloadRestoredExactly, pointerRestoredExactly } = await restoreSnapshot(
      adb,
      source,
      payloadBackup,
      pointerBackup,
      originalPayloadSha,
      originalPointerSha,
      prePointer,
    ));
    if (payloadRestoredExactly && pointerRestoredExactly) {
      await adb.runAs(["rm", payloadBackup, pointerBackup], "remove-snapshot-backup");
      await adb.runAs(["rmdir", backupRoot], "remove-snapshot-backup-directory");
    }
  }
  if (primaryError !== null) {
    throw primaryError;
  }
  if (recoveryRun === null || postPointer === null) {
    throw new HarnessFailure("recovery-run", "the corrupt-slot recovery run did not complete");
  }
  if (!payloadRestoredExactly || !pointerRestoredExactly) {
    throw new HarnessFailure(
      "snapshot-restore",
      "the original snapshot state was not restored; the private backup was retained",
    );
  }
  const restoredRun = await runProcessCold(adb, "restored", 1);
  const pointerChangedToFallback =
    postPointer.slot !== prePointer.slot &&
    postPointer.slot === source.fallback.slot &&
    postPointer.privateChecksum === source.fallback.privateChecksum;
  return {
    corruptedKind: "m3u",
    sameLengthCorruption: true,
    contentChanged: true,
    fallbackAdopted: pointerChangedToFallback,
    recoveredCatalogUsable: recoveryRun.usable && recoveryRun.browseReady,
    payloadRestoredExactly,
    pointerRestoredExactly,
    restoredCatalogUsable: restoredRun.usable && restoredRun.browseReady,
    prePointerSlot: prePointer.slot,
    postPointerSlot: postPointer.slot,
    pointerChangedToFallback,
    activeFileIdentityRevalidated,
    recoveryVmHwmKiB: recoveryRun.vmHwmKiB,
    restoredVmHwmKiB: restoredRun.vmHwmKiB,
  };
}

async function restoreSnapshot(
  adb: Adb,
  source: PreparedSource,
  payloadBackup: string,
  pointerBackup: string,
  originalPayloadSha: string,
  originalPointerSha: string,
  prePointer: { readonly slot: "a" | "b"; readonly privateChecksum: string },
): Promise<{
  readonly payloadRestoredExactly: boolean;
  readonly pointerRestoredExactly: boolean;
}> {
  await privateFileIdentity(adb, source.active.payloadPath);
  await adb.runAs(["cp", payloadBackup, source.active.payloadPath], "restore-snapshot-payload");
  await adb.runAs(["chmod", "600", source.active.payloadPath], "protect-restored-payload");
  await syncPrivateFile(adb, source.active.payloadPath, "sync-restored-snapshot-payload");
  const payloadIdentity = await privateFileIdentity(adb, source.active.payloadPath);
  const payloadRestoredExactly =
    payloadIdentity.size === source.active.decodedBytes &&
    (await privateSha256(adb, source.active.payloadPath)) === originalPayloadSha;
  if (!payloadRestoredExactly) {
    throw new HarnessFailure(
      "snapshot-payload-restore",
      "the payload restore was not exact; the pointer and private backup were retained",
    );
  }

  await adb.runAs(["cp", pointerBackup, source.pointerPath], "restore-snapshot-pointer");
  await adb.runAs(["chmod", "600", source.pointerPath], "protect-restored-pointer");
  await syncPrivateFile(adb, source.pointerPath, "sync-restored-snapshot-pointer");
  const restoredPointer = unwrap(
    parseSnapshotPointer(
      (await adb.runAs(["cat", source.pointerPath], "restored-snapshot-pointer")).stdout,
    ),
    "restored-snapshot-pointer",
  );
  const pointerRestoredExactly =
    restoredPointer.slot === prePointer.slot &&
    restoredPointer.privateChecksum === prePointer.privateChecksum &&
    (await privateSha256(adb, source.pointerPath)) === originalPointerSha;
  if (!pointerRestoredExactly) {
    throw new HarnessFailure(
      "snapshot-pointer-restore",
      "the pointer restore was not exact; the private backup was retained",
    );
  }
  return { payloadRestoredExactly, pointerRestoredExactly };
}

async function findNonZeroOffset(adb: Adb, slot: PrivateSlot): Promise<number> {
  const first = Math.min(4_096, slot.decodedBytes - 1);
  for (let offset = first; offset >= Math.max(0, first - 64); offset -= 1) {
    const raw = (
      await adb.runAs(
        ["od", "-An", "-tu1", "-j", String(offset), "-N", "1", slot.payloadPath],
        "snapshot-byte-probe",
      )
    ).stdout.trim();
    const value = Number(raw);
    if (Number.isInteger(value) && value > 0 && value <= 255) {
      return offset;
    }
  }
  throw new HarnessFailure("corruption-offset", "no safe same-length corruption offset was found");
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
      // The WebView socket appears after the main process; retry within the bounded deadline.
    }
    await delay(50);
  }
  throw new HarnessFailure("cdp-unavailable", "the debuggable Sparrow WebView was unavailable");
}

async function requireWebviewForward(adb: Adb, pid: string, port: string): Promise<void> {
  const expectedTarget = `localabstract:webview_devtools_remote_${pid}`;
  const matches = await matchingForwards(adb, expectedTarget);
  if (matches.length !== 1 || matches[0] !== `tcp:${port}`) {
    throw new HarnessFailure("webview-forward", "the WebView debugger forward was not exact");
  }
}

async function removeWebviewForwards(adb: Adb, pid: string): Promise<void> {
  const expectedTarget = `localabstract:webview_devtools_remote_${pid}`;
  const forwards = await matchingForwards(adb, expectedTarget);
  for (const host of forwards) {
    await adb.run(["forward", "--remove", host], "remove-webview-forward");
  }
  if ((await matchingForwards(adb, expectedTarget)).length !== 0) {
    throw new HarnessFailure("remove-webview-forward", "the WebView debugger forward remained active");
  }
}

async function matchingForwards(adb: Adb, expectedTarget: string): Promise<readonly string[]> {
  const output = (await adb.run(["forward", "--list"], "list-webview-forwards")).stdout;
  const matches: string[] = [];
  for (const line of output.split(/\r?\n/u)) {
    if (line.trim() === "") {
      continue;
    }
    const fields = line.trim().split(/\s+/u);
    if (fields.length !== 3) {
      throw new HarnessFailure("list-webview-forwards", "adb returned an invalid forward record");
    }
    const [serial, host, target] = fields;
    if (serial === adb.serialValue() && target === expectedTarget) {
      if (host === undefined || !/^tcp:\d{2,5}$/u.test(host)) {
        throw new HarnessFailure("list-webview-forwards", "adb returned an unsafe forward record");
      }
      matches.push(host);
    }
  }
  return matches;
}

async function readMarker(cdp: CdpSession) {
  return unwrap(parseReadinessMarker(await cdp.evaluate(READINESS_EXPRESSION)), "invalid-readiness");
}

async function forceStop(adb: Adb): Promise<void> {
  await adb.run(["shell", "am", "force-stop", PACKAGE_NAME], "force-stop");
  const deadline = performance.now() + 5_000;
  while (performance.now() < deadline) {
    const result = await adb.execute(["shell", "pidof", "-s", PACKAGE_NAME], "pid-check");
    if (isProvenAbsent(result)) {
      return;
    }
    if (result.signal !== null || result.exitCode !== 0 || !/^\d+$/u.test(result.stdout.trim())) {
      throw new HarnessFailure("pid-check", "the app process state could not be proven");
    }
    await delay(50);
  }
  throw new HarnessFailure("force-stop", "the prior Sparrow process did not stop");
}

async function assertQuiescent(adb: Adb): Promise<void> {
  const result = await adb.execute(["shell", "pidof", "-s", PACKAGE_NAME], "quiescence-check");
  if (!isProvenAbsent(result)) {
    throw new HarnessFailure("quiescence-check", "the Sparrow process is not proven absent");
  }
}

function isProvenAbsent(result: CommandResult): boolean {
  return (
    result.signal === null &&
    result.exitCode === 1 &&
    result.stdout.trim() === "" &&
    result.stderr.trim() === ""
  );
}

async function waitForPid(adb: Adb, deadline: number): Promise<string> {
  while (performance.now() < deadline) {
    const result = await adb.execute(["shell", "pidof", "-s", PACKAGE_NAME], "pid-check");
    const pid = result.stdout.trim();
    if (result.exitCode === 0 && /^\d+$/u.test(pid)) {
      const command = (
        await adb.runAs(["cat", `/proc/${pid}/cmdline`], "process-identity")
      ).stdout.split(String.fromCharCode(0)).join("");
      if (command === PACKAGE_NAME) {
        return pid;
      }
    } else if (!isProvenAbsent(result)) {
      throw new HarnessFailure("pid-check", "the app process state could not be read");
    }
    await delay(25);
  }
  throw new HarnessFailure("process-timeout", "the Sparrow main process did not start");
}

async function readCgroupCurrentKiB(adb: Adb, pid: string): Promise<number | null> {
  const cgroup = await adb.execute(
    ["shell", "run-as", PACKAGE_NAME, "cat", `/proc/${pid}/cgroup`],
    "process-cgroup",
  );
  if (cgroup.exitCode !== 0) {
    return null;
  }
  const path = /^0::(\/apps\/uid_\d+\/pid_\d+)$/mu.exec(cgroup.stdout)?.[1];
  if (path === undefined) {
    return null;
  }
  const current = await adb.execute(
    ["shell", "run-as", PACKAGE_NAME, "cat", `/sys/fs/cgroup${path}/memory.current`],
    "cgroup-memory",
  );
  const bytes = Number(current.stdout.trim());
  return current.exitCode === 0 && Number.isSafeInteger(bytes) && bytes >= 0
    ? Math.ceil(bytes / 1024)
    : null;
}

async function privateFileIdentity(adb: Adb, path: string): Promise<PrivateFileIdentity> {
  if (!/^private-v1\/snapshots-v1\/(?:m3u|epg)\/slot-[ab]\.payload$/u.test(path)) {
    throw new HarnessFailure("private-path", "the snapshot payload path was outside the accepted layout");
  }
  const root = (await adb.runAs(["realpath", "."], "private-root-path")).stdout.trim();
  const canonicalPath = (await adb.runAs(["realpath", path], "private-file-path")).stdout.trim();
  if (
    !/^\/data\/(?:user\/\d+|data)\/xyz\.ponbac\.sparrow$/u.test(root) ||
    canonicalPath !== `${root}/${path}` ||
    !canonicalPath.startsWith(`${root}/${PRIVATE_SNAPSHOT_ROOT}/`)
  ) {
    throw new HarnessFailure("private-path", "the snapshot payload did not resolve inside app-private data");
  }
  const raw = (
    await adb.runAs(["stat", "-c", "%f|%h|%d|%i|%s", path], "private-file-identity")
  ).stdout.trim();
  const fields = raw.split("|");
  if (fields.length !== 5) {
    throw new HarnessFailure("private-file-identity", "the snapshot file identity was unavailable");
  }
  const [modeText, linksText, device, inode, sizeText] = fields;
  const mode = Number.parseInt(modeText ?? "", 16);
  const links = Number(linksText);
  const size = Number(sizeText);
  if (
    !Number.isSafeInteger(mode) ||
    (mode & 0xf000) !== 0x8000 ||
    (mode & 0o777) !== 0o600 ||
    links !== 1 ||
    device === undefined ||
    !/^\d+$/u.test(device) ||
    inode === undefined ||
    !/^\d+$/u.test(inode) ||
    !Number.isSafeInteger(size) ||
    size < 0
  ) {
    throw new HarnessFailure(
      "private-file-identity",
      "the snapshot must be a private regular single-link file",
    );
  }
  return { canonicalPath, mode, links, device, inode, size };
}

function samePrivateFile(left: PrivateFileIdentity, right: PrivateFileIdentity): boolean {
  return (
    left.canonicalPath === right.canonicalPath &&
    left.mode === right.mode &&
    left.links === right.links &&
    left.device === right.device &&
    left.inode === right.inode &&
    left.size === right.size
  );
}

async function syncPrivateFile(adb: Adb, path: string, label: string): Promise<void> {
  await adb.runAs(["sync", path], label);
}

async function privateSha256(adb: Adb, path: string): Promise<string> {
  return parseSha256(
    (await adb.runAs(["sha256sum", path], "private-file-hash")).stdout,
    "private-file-hash",
  );
}

function parseSha256(raw: string, label: string): string {
  const digest = /^([0-9a-f]{64})\s/u.exec(raw)?.[1];
  if (digest === undefined) {
    throw new HarnessFailure(label, `${label} was unavailable`);
  }
  return digest;
}

function safeSourceEvidence(source: PreparedSource) {
  return {
    kind: source.kind,
    representativeMinimumMet: true,
    redundantSlots: true,
  };
}

async function writeNewEvidence(path: string, evidence: unknown): Promise<void> {
  await mkdir(dirname(resolve(path)), { recursive: true, mode: 0o700 });
  const file = await open(path, "wx", 0o600);
  try {
    await file.writeFile(`${JSON.stringify(evidence, null, 2)}\n`, { encoding: "utf8" });
    await file.sync();
  } finally {
    await file.close();
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

async function findExecutable(command: string): Promise<string> {
  const result = await requireCommand("which", [command], "tool-location");
  const path = result.stdout.trim();
  if (path === "") {
    throw new HarnessFailure("tool-location", "a required tool location was unavailable");
  }
  return path;
}

function requireTool(command: string, args: readonly string[]): Promise<CommandResult> {
  return requireCommand(command, args, "required-tool", 60_000);
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

function requireExpectedExit(
  result: CommandResult,
  allowed: readonly number[],
  label: string,
): void {
  if (result.signal !== null || !allowed.includes(result.exitCode)) {
    throw new HarnessFailure(label, `${label} did not complete with an expected result`);
  }
}

function executeCommand(
  command: string,
  args: readonly string[],
  label: string,
  timeoutMs = 30_000,
): Promise<CommandResult> {
  return new Promise<CommandResult>((resolveCommand, rejectCommand) => {
    const child = spawn(command, [...args], {
      stdio: ["ignore", "pipe", "pipe"],
      timeout: timeoutMs,
    });
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
    child.once("error", () => {
      rejectCommand(new HarnessFailure(label, `${label} could not start`));
    });
    child.once("close", (code, signal) => {
      if (exceeded) {
        rejectCommand(new HarnessFailure(label, `${label} returned too much output`));
        return;
      }
      resolveCommand({ exitCode: code ?? 1, signal, stdout, stderr });
    });
  });
}

function unwrap<Value>(
  result:
    | { readonly ok: true; readonly value: Value }
    | { readonly ok: false; readonly reason: string },
  code: string,
): Value {
  if (!result.ok) {
    throw new HarnessFailure(code, result.reason);
  }
  return result.value;
}

function safeFailure(error: unknown): SafeFailureEvidence["failure"] {
  return error instanceof HarnessFailure
    ? { code: error.code, detail: error.message }
    : { code: "harness-defect", detail: "the acceptance harness stopped unexpectedly" };
}

function safeVersion(value: string): string {
  const trimmed = value.trim();
  return /^[A-Za-z0-9._+ -]{1,64}$/u.test(trimmed) ? trimmed : "unknown";
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

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}

const invokedPath = process.argv[1];
if (invokedPath !== undefined && resolve(invokedPath) === fileURLToPath(import.meta.url)) {
  await main();
}
