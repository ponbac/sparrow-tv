import { describe, expect, it } from "vitest";
import {
  evaluateAcceptanceGates,
  isUsableCatalog,
  parseApkIdentity,
  parseCliArguments,
  parseInstalledPackageIdentity,
  parseProcessMemory,
  parseReadinessMarker,
  parseSnapshotManifest,
  parseSnapshotPointer,
  parseTotalPssKiB,
  verifyDeviceIdentity,
} from "./android-catalog-acceptance-domain.ts";

const DIGEST_A = "a".repeat(64);
const DIGEST_B = "b".repeat(64);

describe("Android catalog acceptance boundaries", () => {
  it("requires one explicit adb serial, APK, and evidence output", () => {
    expect(
      parseCliArguments([
        "--serial",
        "physical-serial",
        "--apk",
        "candidate.apk",
        "--output",
        "evidence.json",
      ]),
    ).toEqual({
      ok: true,
      value: {
        serial: "physical-serial",
        apk: "candidate.apk",
        output: "evidence.json",
      },
    });
    expect(parseCliArguments(["--serial", "physical-serial"])).toEqual({
      ok: false,
      reason: "--serial, --apk, and --output are required",
    });
    expect(
      parseCliArguments([
        "--serial",
        "emulator 5554",
        "--apk",
        "candidate.apk",
        "--output",
        "evidence.json",
      ]),
    ).toEqual({ ok: false, reason: "the adb serial has an unsafe shape" });
  });

  it("hard-rejects emulators before accepting the exact physical target", () => {
    const target = {
      manufacturer: "realme",
      model: "RMX5210",
      apiLevel: "36",
      primaryAbi: "arm64-v8a",
      androidRelease: "16",
      kernelQemu: "0",
      hardware: "qcom",
      productName: "RMX5210",
    };
    expect(verifyDeviceIdentity(target)).toEqual({
      ok: true,
      value: {
        manufacturer: "realme",
        model: "RMX5210",
        apiLevel: 36,
        primaryAbi: "arm64-v8a",
        androidRelease: "16",
      },
    });
    expect(
      verifyDeviceIdentity({
        ...target,
        kernelQemu: "1",
        hardware: "ranchu",
        productName: "sdk_gphone64_x86_64",
      }),
    ).toEqual({
      ok: false,
      reason: "emulators are not accepted for the physical-device gate",
    });
    expect(verifyDeviceIdentity({ ...target, model: "RMX0000" })).toEqual({
      ok: false,
      reason: "the connected device is not the required Realme GT8 Pro RMX5210",
    });
  });

  it("accepts only the API-36 debuggable Sparrow arm64 APK", () => {
    const candidate = {
      summary: "xyz.ponbac.sparrow\t11004\t0.11.4\n",
      targetSdk: "36\n",
      debuggable: "true\n",
      fileList: "/lib/\n/lib/arm64-v8a/\n/lib/arm64-v8a/libsparrow_installed.so\n",
    };
    expect(parseApkIdentity(candidate)).toEqual({
      ok: true,
      value: {
        packageName: "xyz.ponbac.sparrow",
        versionCode: 11004,
        versionName: "0.11.4",
        targetSdk: 36,
        debuggable: true,
        hasArm64Runtime: true,
      },
    });
    expect(parseApkIdentity({ ...candidate, debuggable: "false" })).toEqual({
      ok: false,
      reason: "the acceptance APK must be debuggable for redacted DOM timing",
    });
    expect(parseApkIdentity({ ...candidate, fileList: "/lib/x86_64/runtime.so" })).toEqual({
      ok: false,
      reason: "the APK does not contain the Sparrow arm64 runtime",
    });
  });

  it("binds the installed package dump to the staged APK identity", () => {
    const installed = `
      Package [xyz.ponbac.sparrow] (abc123):
        versionCode=11004 minSdk=24 targetSdk=36
        versionName=0.11.4
        pkgFlags=[ DEBUGGABLE HAS_CODE ALLOW_CLEAR_USER_DATA ]
    `;
    expect(
      parseInstalledPackageIdentity(installed, {
        versionCode: 11004,
        versionName: "0.11.4",
        targetSdk: 36,
      }),
    ).toEqual({
      ok: true,
      value: {
        packageName: "xyz.ponbac.sparrow",
        versionCode: 11004,
        versionName: "0.11.4",
        targetSdk: 36,
        debuggable: true,
      },
    });
    expect(
      parseInstalledPackageIdentity(installed.replace("DEBUGGABLE ", ""), {
        versionCode: 11004,
        versionName: "0.11.4",
        targetSdk: 36,
      }),
    ).toEqual({
      ok: false,
      reason: "the installed package does not match the staged APK identity",
    });
  });

  it("parses private slot metadata without returning validators", () => {
    const pointer = parseSnapshotPointer(
      JSON.stringify({ version: 1, slot: "b", checksum: DIGEST_A }),
    );
    expect(pointer).toEqual({
      ok: true,
      value: { slot: "b", privateChecksum: DIGEST_A },
    });
    const manifest = parseSnapshotManifest(
      JSON.stringify({
        version: 1,
        source_kind: "m3u",
        source_key: DIGEST_B,
        decoded_bytes: 75_000_000,
        checksum: DIGEST_A,
        validated_at: "2026-08-30T00:00:00Z",
        etag: "private-validator-canary",
        last_modified: "private-validator-canary-2",
      }),
      "m3u",
    );
    expect(manifest).toEqual({
      ok: true,
      value: {
        kind: "m3u",
        decodedBytes: 75_000_000,
        privateChecksum: DIGEST_A,
        privateSourceKey: DIGEST_B,
      },
    });
    expect(JSON.stringify(manifest)).not.toContain("private-validator-canary");
  });

  it("accepts only a count-only complete retained first-page marker", () => {
    const marker = {
      nativeBridge: true,
      catalogShell: true,
      loading: false,
      channelCount: 24,
      groupButtonCount: 102,
      searchVisible: true,
      retainedCatalog: true,
      alertCount: 0,
      routineUrlCount: 0,
      statusState: "stale",
      browseReady: true,
    };
    const parsed = parseReadinessMarker(marker);
    expect(parsed).toEqual({ ok: true, value: marker });
    expect(parsed.ok && isUsableCatalog(parsed.value)).toBe(true);
    expect(parseReadinessMarker({ ...marker, channelName: "private-name-canary" })).toEqual({
      ok: false,
      reason: "the WebView readiness marker is invalid",
    });
    const incomplete = parseReadinessMarker({ ...marker, channelCount: 23 });
    expect(incomplete.ok && isUsableCatalog(incomplete.value)).toBe(false);
    const alerting = parseReadinessMarker({ ...marker, alertCount: 1 });
    expect(alerting.ok && isUsableCatalog(alerting.value)).toBe(false);
  });

  it("parses kernel and dumpsys memory counters", () => {
    expect(
      parseProcessMemory("Name:\tsparrow\nVmHWM:\t  523148 kB\nVmRSS:\t  521484 kB\n"),
    ).toEqual({ ok: true, value: { vmHwmKiB: 523148, vmRssKiB: 521484 } });
    expect(parseTotalPssKiB("TOTAL PSS:   492684 TOTAL RSS: 501016")).toBe(492684);
    expect(parseTotalPssKiB("unavailable")).toBeNull();
  });
});

describe("Android catalog acceptance gates", () => {
  const acceptedRun = {
    readyMs: 2_999,
    vmHwmKiB: 524_288,
    usable: true,
    browseReady: true,
    retainedCatalog: true,
    routineUrlCount: 0,
  };
  const acceptedRecovery = {
    sameLengthCorruption: true,
    contentChanged: true,
    fallbackAdopted: true,
    recoveredCatalogUsable: true,
    payloadRestoredExactly: true,
    pointerRestoredExactly: true,
    restoredCatalogUsable: true,
    pointerChangedToFallback: true,
    activeFileIdentityRevalidated: true,
    recoveryVmHwmKiB: 524_288,
    restoredVmHwmKiB: 524_288,
  };

  it("accepts three repeatable runs at the exact published boundaries", () => {
    expect(
      evaluateAcceptanceGates(
        [acceptedRun, acceptedRun, acceptedRun],
        acceptedRecovery,
      ),
    ).toEqual([]);
  });

  it("reports startup, VmHWM, privacy, and recovery failures as safe tags", () => {
    expect(
      evaluateAcceptanceGates(
        [
          { ...acceptedRun, readyMs: 3_001 },
          { ...acceptedRun, vmHwmKiB: 524_289 },
          { ...acceptedRun, routineUrlCount: 1 },
        ],
        { ...acceptedRecovery, fallbackAdopted: false },
      ),
    ).toEqual([
      "run-1-startup-over-3000ms",
      "run-2-vmhwm-over-524288kib",
      "run-3-routine-url-in-dom",
      "corrupt-active-slot-did-not-recover-from-fallback",
    ]);
  });

  it("gates recovery identity and both recovery-path VmHWM measurements", () => {
    expect(
      evaluateAcceptanceGates(
        [acceptedRun, acceptedRun, acceptedRun],
        {
          ...acceptedRecovery,
          pointerChangedToFallback: false,
          recoveryVmHwmKiB: 524_289,
          restoredVmHwmKiB: 524_289,
        },
      ),
    ).toEqual([
      "corrupt-active-slot-identity-was-not-proven",
      "recovery-run-vmhwm-over-524288kib",
      "restored-run-vmhwm-over-524288kib",
    ]);
  });
});
