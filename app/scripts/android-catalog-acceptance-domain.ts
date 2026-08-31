import { z } from "zod";

const HEX_DIGEST = /^[0-9a-f]{64}$/;
const SAFE_VERSION = /^[A-Za-z0-9._+-]{1,64}$/;

type ParseResult<Value> =
  | { readonly ok: true; readonly value: Value }
  | { readonly ok: false; readonly reason: string };

type Slot = "a" | "b";
type SourceKind = "m3u" | "epg";

interface CliArguments {
  readonly apk: string;
  readonly output: string;
}

interface DeviceIdentity {
  readonly manufacturer: "realme";
  readonly model: "RMX5210";
  readonly apiLevel: 36;
  readonly primaryAbi: "arm64-v8a";
  readonly androidRelease: string;
}

interface ApkIdentity {
  readonly packageName: "xyz.ponbac.sparrow";
  readonly versionCode: number;
  readonly versionName: string;
  readonly targetSdk: 36;
  readonly debuggable: true;
  readonly hasArm64Runtime: true;
}

interface InstalledPackageIdentity {
  readonly packageName: "xyz.ponbac.sparrow";
  readonly versionCode: number;
  readonly versionName: string;
  readonly targetSdk: 36;
  readonly debuggable: true;
}

interface SnapshotPointer {
  readonly slot: Slot;
  readonly privateChecksum: string;
}

interface SnapshotManifest {
  readonly kind: SourceKind;
  readonly decodedBytes: number;
  readonly privateChecksum: string;
  readonly privateSourceKey: string;
}

interface ReadinessMarker {
  readonly nativeBridge: boolean;
  readonly catalogShell: boolean;
  readonly loading: boolean;
  readonly channelCount: number;
  readonly groupButtonCount: number;
  readonly searchVisible: boolean;
  readonly retainedCatalog: boolean;
  readonly alertCount: number;
  readonly routineUrlCount: number;
  readonly statusState:
    | "unknown"
    | "fresh"
    | "stale"
    | "refreshing"
    | "failed";
  readonly browseReady: boolean;
}

interface ProcessMemory {
  readonly vmHwmKiB: number;
  readonly vmRssKiB: number;
}

interface GateRun {
  readonly readyMs: number;
  readonly vmHwmKiB: number;
  readonly usable: boolean;
  readonly browseReady: boolean;
  readonly retainedCatalog: boolean;
  readonly routineUrlCount: number;
}

interface RecoveryGate {
  readonly sameLengthCorruption: boolean;
  readonly contentChanged: boolean;
  readonly fallbackAdopted: boolean;
  readonly recoveredCatalogUsable: boolean;
  readonly payloadRestoredExactly: boolean;
  readonly pointerRestoredExactly: boolean;
  readonly restoredCatalogUsable: boolean;
  readonly pointerChangedToFallback: boolean;
  readonly activeFileIdentityRevalidated: boolean;
  readonly recoveryVmHwmKiB: number;
  readonly restoredVmHwmKiB: number;
}

const devicePropertiesSchema = z
  .object({
    manufacturer: z.string(),
    model: z.string(),
    apiLevel: z.string(),
    primaryAbi: z.string(),
    androidRelease: z.string().min(1).max(32),
    kernelQemu: z.string(),
    hardware: z.string(),
    productName: z.string(),
  })
  .strict();

const pointerSchema = z
  .object({
    version: z.literal(1),
    slot: z.enum(["a", "b"]),
    checksum: z.string().regex(HEX_DIGEST),
  })
  .strict();

const manifestSchema = z
  .object({
    version: z.literal(1),
    source_kind: z.enum(["m3u", "epg"]),
    source_key: z.string().regex(HEX_DIGEST),
    decoded_bytes: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
    checksum: z.string().regex(HEX_DIGEST),
    validated_at: z.string().min(1).max(64),
    etag: z.string().min(1).max(8 * 1024).optional(),
    last_modified: z.string().min(1).max(8 * 1024).optional(),
  })
  .strict();

const readinessSchema = z
  .object({
    nativeBridge: z.boolean(),
    catalogShell: z.boolean(),
    loading: z.boolean(),
    channelCount: z.number().int().nonnegative().max(10_000),
    groupButtonCount: z.number().int().nonnegative().max(10_000),
    searchVisible: z.boolean(),
    retainedCatalog: z.boolean(),
    alertCount: z.number().int().nonnegative().max(100),
    routineUrlCount: z.number().int().nonnegative().max(100),
    statusState: z.enum([
      "unknown",
      "fresh",
      "stale",
      "refreshing",
      "failed",
    ]),
    browseReady: z.boolean(),
  })
  .strict();

/** Parses only non-sensitive filesystem arguments from the process command line. */
export function parseCliArguments(argv: readonly string[]): ParseResult<CliArguments> {
  const values = new Map<string, string>();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (
      flag === undefined ||
      value === undefined ||
      !["--apk", "--output"].includes(flag) ||
      value.length === 0 ||
      values.has(flag)
    ) {
      return reject("expected one value for each required flag");
    }
    values.set(flag, value);
  }
  const apk = values.get("--apk");
  const output = values.get("--output");
  if (values.size !== 2 || apk === undefined || output === undefined) {
    return reject("--apk and --output are required");
  }
  return accept({ apk, output });
}

/** Validates the private standard adb selector read from ANDROID_SERIAL. */
export function parseAdbSerial(serial: unknown): ParseResult<string> {
  if (
    typeof serial !== "string" ||
    serial.length === 0 ||
    Array.from(serial).some(
      (character) => /\s/u.test(character) || character.charCodeAt(0) <= 31,
    )
  ) {
    return reject("ANDROID_SERIAL is missing or has an unsafe shape");
  }
  return accept(serial);
}

/** Accepts only a physical Realme GT8 Pro (RMX5210), API 36, arm64 device. */
export function verifyDeviceIdentity(input: unknown): ParseResult<DeviceIdentity> {
  const parsed = devicePropertiesSchema.safeParse(input);
  if (!parsed.success) {
    return reject("device properties were unavailable");
  }
  const device = parsed.data;
  const emulator =
    device.kernelQemu === "1" ||
    /goldfish|ranchu|emulator|qemu/iu.test(device.hardware) ||
    /sdk_gphone|generic|emulator/iu.test(device.productName);
  if (emulator) {
    return reject("emulators are not accepted for the physical-device gate");
  }
  if (device.manufacturer.toLowerCase() !== "realme" || device.model !== "RMX5210") {
    return reject("the connected device is not the required Realme GT8 Pro RMX5210");
  }
  if (device.apiLevel !== "36") {
    return reject("the connected device is not running Android API 36");
  }
  if (device.primaryAbi !== "arm64-v8a") {
    return reject("the connected device does not use the required arm64 ABI");
  }
  return accept({
    manufacturer: "realme",
    model: "RMX5210",
    apiLevel: 36,
    primaryAbi: "arm64-v8a",
    androidRelease: device.androidRelease,
  });
}

/** Parses apkAnalyzer output and accepts only the debuggable API-36 arm64 candidate. */
export function parseApkIdentity(input: {
  readonly summary: string;
  readonly targetSdk: string;
  readonly debuggable: string;
  readonly fileList: string;
}): ParseResult<ApkIdentity> {
  const fields = input.summary.trim().split(/\s+/u);
  if (fields.length !== 3) {
    return reject("apkAnalyzer returned an unexpected APK summary");
  }
  const [packageName, versionCodeText, versionName] = fields;
  const versionCode = Number(versionCodeText);
  if (
    packageName !== "xyz.ponbac.sparrow" ||
    !Number.isSafeInteger(versionCode) ||
    versionCode <= 0 ||
    versionName === undefined ||
    !SAFE_VERSION.test(versionName)
  ) {
    return reject("the APK identity does not match Sparrow");
  }
  if (input.targetSdk.trim() !== "36") {
    return reject("the APK does not target Android API 36");
  }
  if (input.debuggable.trim() !== "true") {
    return reject("the acceptance APK must be debuggable for redacted DOM timing");
  }
  const hasArm64Runtime = input.fileList
    .split(/\r?\n/u)
    .some((entry) => entry.trim() === "/lib/arm64-v8a/libsparrow_installed.so");
  if (!hasArm64Runtime) {
    return reject("the APK does not contain the Sparrow arm64 runtime");
  }
  return accept({
    packageName,
    versionCode,
    versionName,
    targetSdk: 36,
    debuggable: true,
    hasArm64Runtime: true,
  });
}

/** Verifies the installed package dump against the already parsed APK identity. */
export function parseInstalledPackageIdentity(
  raw: string,
  expected: Pick<ApkIdentity, "versionCode" | "versionName" | "targetSdk">,
): ParseResult<InstalledPackageIdentity> {
  const packageHeader = /^\s*Package \[xyz\.ponbac\.sparrow\] \([^\r\n]+\):$/mu.test(raw);
  const version = /^\s*versionCode=(\d+)\s+minSdk=\d+\s+targetSdk=(\d+)\s*$/mu.exec(raw);
  const versionName = /^\s*versionName=([^\s]+)\s*$/mu.exec(raw)?.[1];
  const debuggable = /^\s*(?:pkgFlags|flags)=\[[^\]\r\n]*\bDEBUGGABLE\b[^\]\r\n]*\]\s*$/mu.test(
    raw,
  );
  const versionCode = Number(version?.[1]);
  const targetSdk = Number(version?.[2]);
  if (
    !packageHeader ||
    !Number.isSafeInteger(versionCode) ||
    versionCode !== expected.versionCode ||
    targetSdk !== expected.targetSdk ||
    versionName !== expected.versionName ||
    !debuggable
  ) {
    return reject("the installed package does not match the staged APK identity");
  }
  return accept({
    packageName: "xyz.ponbac.sparrow",
    versionCode,
    versionName,
    targetSdk: 36,
    debuggable: true,
  });
}

/** Parses a private active-slot pointer without projecting its checksum to evidence. */
export function parseSnapshotPointer(raw: string): ParseResult<SnapshotPointer> {
  const document = parseJson(raw);
  if (!document.ok) {
    return document;
  }
  const parsed = pointerSchema.safeParse(document.value);
  return parsed.success
    ? accept({ slot: parsed.data.slot, privateChecksum: parsed.data.checksum })
    : reject("the active snapshot pointer is invalid");
}

/** Parses a private slot manifest and retains sensitive identity only for comparisons. */
export function parseSnapshotManifest(
  raw: string,
  expectedKind: SourceKind,
): ParseResult<SnapshotManifest> {
  const document = parseJson(raw);
  if (!document.ok) {
    return document;
  }
  const parsed = manifestSchema.safeParse(document.value);
  if (!parsed.success || parsed.data.source_kind !== expectedKind) {
    return reject("the snapshot manifest is invalid");
  }
  return accept({
    kind: parsed.data.source_kind,
    decodedBytes: parsed.data.decoded_bytes,
    privateChecksum: parsed.data.checksum,
    privateSourceKey: parsed.data.source_key,
  });
}

/** Parses the redacted, count-only DOM marker returned through CDP. */
export function parseReadinessMarker(input: unknown): ParseResult<ReadinessMarker> {
  const parsed = readinessSchema.safeParse(input);
  return parsed.success
    ? accept(parsed.data)
    : reject("the WebView readiness marker is invalid");
}

/** Returns whether a redacted DOM marker represents the complete first catalog page. */
export function isUsableCatalog(marker: ReadinessMarker): boolean {
  return (
    marker.nativeBridge &&
    marker.catalogShell &&
    !marker.loading &&
    marker.channelCount === 24 &&
    marker.groupButtonCount > 0 &&
    marker.searchVisible &&
    marker.retainedCatalog &&
    marker.alertCount === 0 &&
    marker.routineUrlCount === 0
  );
}

/** Parses Linux VmHWM and VmRSS values from the app-owned process status file. */
export function parseProcessMemory(raw: string): ParseResult<ProcessMemory> {
  const vmHwm = /^VmHWM:\s+(\d+)\s+kB$/mu.exec(raw)?.[1];
  const vmRss = /^VmRSS:\s+(\d+)\s+kB$/mu.exec(raw)?.[1];
  const vmHwmKiB = Number(vmHwm);
  const vmRssKiB = Number(vmRss);
  if (
    !Number.isSafeInteger(vmHwmKiB) ||
    vmHwmKiB < 0 ||
    !Number.isSafeInteger(vmRssKiB) ||
    vmRssKiB < 0
  ) {
    return reject("process memory counters were unavailable");
  }
  return accept({ vmHwmKiB, vmRssKiB });
}

/** Parses the supplemental TOTAL PSS counter from Android dumpsys output. */
export function parseTotalPssKiB(raw: string): number | null {
  const value = /TOTAL PSS:\s+(\d+)/u.exec(raw)?.[1];
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= 0 ? parsed : null;
}

/** Evaluates all repeatability, startup, memory, privacy, and recovery gates. */
export function evaluateAcceptanceGates(
  runs: readonly GateRun[],
  recovery: RecoveryGate,
): readonly string[] {
  const failures: string[] = [];
  if (runs.length !== 3) {
    failures.push("exactly-three-baseline-runs");
  }
  for (const [index, run] of runs.entries()) {
    const number = index + 1;
    if (!run.usable || !run.browseReady || !run.retainedCatalog) {
      failures.push(`run-${number}-catalog-not-usable`);
    }
    if (run.readyMs > 3_000) {
      failures.push(`run-${number}-startup-over-3000ms`);
    }
    if (run.vmHwmKiB > 524_288) {
      failures.push(`run-${number}-vmhwm-over-524288kib`);
    }
    if (run.routineUrlCount !== 0) {
      failures.push(`run-${number}-routine-url-in-dom`);
    }
  }
  if (!recovery.sameLengthCorruption || !recovery.contentChanged) {
    failures.push("corruption-probe-did-not-change-one-same-length-payload");
  }
  if (!recovery.fallbackAdopted || !recovery.recoveredCatalogUsable) {
    failures.push("corrupt-active-slot-did-not-recover-from-fallback");
  }
  if (!recovery.pointerChangedToFallback || !recovery.activeFileIdentityRevalidated) {
    failures.push("corrupt-active-slot-identity-was-not-proven");
  }
  if (recovery.recoveryVmHwmKiB > 524_288) {
    failures.push("recovery-run-vmhwm-over-524288kib");
  }
  if (recovery.restoredVmHwmKiB > 524_288) {
    failures.push("restored-run-vmhwm-over-524288kib");
  }
  if (
    !recovery.payloadRestoredExactly ||
    !recovery.pointerRestoredExactly ||
    !recovery.restoredCatalogUsable
  ) {
    failures.push("snapshot-state-was-not-restored-exactly");
  }
  return failures;
}

function parseJson(raw: string): ParseResult<unknown> {
  try {
    return accept(JSON.parse(raw));
  } catch {
    return reject("private snapshot metadata was not valid JSON");
  }
}

function accept<Value>(value: Value): ParseResult<Value> {
  return { ok: true, value };
}

function reject(reason: string): ParseResult<never> {
  return { ok: false, reason };
}
