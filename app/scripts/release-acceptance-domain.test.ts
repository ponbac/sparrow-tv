import { describe, expect, it } from "vitest";
import {
  ANDROID_ACCEPTANCE_GATES,
  LINUX_ACCEPTANCE_GATES,
  createAcceptanceTemplates,
  formatAcceptanceApprovalReceipt,
  parseInstalledReleasePackage,
  parseSealedReleaseAcceptanceVerdict,
  projectAcceptanceCandidate,
  verifyAcceptanceApprovalHistory,
  verifyKeyContinuityEvidence,
  verifyReleaseAcceptance,
} from "./release-acceptance-domain.ts";
import type { CandidateManifest } from "./release-contract-domain.ts";

const TIMESTAMP = "2026-08-30T12:00:00.000Z";
const MANIFEST: CandidateManifest = {
  schemaVersion: 1,
  version: "0.11.5",
  tag: "v0.11.5",
  commit: "a".repeat(40),
  repository: "ponbac/sparrow-tv",
  workflowRunId: "12345",
  workflowRunAttempt: 2,
  publishable: true,
  artifacts: {
    appImage: {
      name: "Sparrow_0.11.5_x86_64.AppImage",
      sha256: "b".repeat(64),
    },
    apk: { name: "Sparrow_0.11.5_universal.apk", sha256: "c".repeat(64) },
  },
  android: {
    applicationId: "xyz.ponbac.sparrow",
    versionName: "0.11.5",
    versionCode: 11_005,
    minSdk: 24,
    targetSdk: 36,
    abis: ["arm64-v8a", "armeabi-v7a", "x86", "x86_64"],
    certificateSha256: "d".repeat(64),
  },
};

describe("personal release acceptance", () => {
  it("initializes attempt-bound forms that are unable to pass while pending", () => {
    const templates = createAcceptanceTemplates(MANIFEST);
    expect(templates.session.candidate).toEqual(
      projectAcceptanceCandidate(MANIFEST),
    );
    expect(templates.linux.gates.map((gate) => gate.id)).toEqual(
      [
        "startup-render-version",
        "browse-search-guide",
        "catalog-first-configuration",
        "catalog-offline-restart",
        "catalog-stale-manual-refresh",
        "primary-picture-audio",
        "primary-controls-channel-changes",
        "bounded-recovery-resource-release",
        "primary-mpv-playback-cleanup",
      ],
    );
    expect(templates.android.gates.map((gate) => gate.id)).toEqual(
      ANDROID_ACCEPTANCE_GATES,
    );
    expect(templates.android.gates.map((gate) => gate.id)).toContain(
      "audio-track-selection-preference-fallback",
    );
    expect(
      verifyReleaseAcceptance(
        {
          session: templates.session,
          linux: templates.linux,
          android: templates.android,
          keyContinuity: {},
        },
        MANIFEST,
      ),
    ).toEqual({
      ok: false,
      reason:
        "the Linux acceptance evidence is invalid or belongs to another attempt",
    });
  });

  it("accepts the exact complete Linux, Realme, and continuity records", () => {
    const evidence = completeEvidence(MANIFEST);
    expect(verifyReleaseAcceptance(evidence, MANIFEST)).toEqual({
      ok: true,
      value: {
        candidate: projectAcceptanceCandidate(MANIFEST),
        evidenceRecordedAt: {
          linux: TIMESTAMP,
          android: TIMESTAMP,
          keyContinuity: TIMESTAMP,
        },
      },
    });
  });

  it("strictly parses every field in a sealed v1 verdict", () => {
    const candidate = projectAcceptanceCandidate(MANIFEST);
    const verdict = {
      schemaVersion: 1,
      verdict: "evidence-complete",
      sealedAt: TIMESTAMP,
      candidate,
      candidateArtifact: { id: "9876", sha256: "1".repeat(64) },
      candidateManifestSha256: "2".repeat(64),
      evidenceSha256: {
        session: "3".repeat(64),
        linux: "4".repeat(64),
        android: "5".repeat(64),
        keyContinuity: "6".repeat(64),
      },
      evidenceRecordedAt: {
        linux: TIMESTAMP,
        android: TIMESTAMP,
        keyContinuity: TIMESTAMP,
      },
    };
    expect(parseSealedReleaseAcceptanceVerdict(verdict)).toEqual({
      ok: true,
      value: verdict,
    });
    expect(
      parseSealedReleaseAcceptanceVerdict({
        ...verdict,
        privateNote: "forbidden",
      }).ok,
    ).toBe(false);
    expect(
      parseSealedReleaseAcceptanceVerdict({
        ...verdict,
        candidateArtifact: { ...verdict.candidateArtifact, extra: true },
      }).ok,
    ).toBe(false);
    expect(
      parseSealedReleaseAcceptanceVerdict({
        ...verdict,
        evidenceSha256: { ...verdict.evidenceSha256, android: undefined },
      }).ok,
    ).toBe(false);
    expect(
      parseSealedReleaseAcceptanceVerdict({
        ...verdict,
        candidateArtifact: {
          ...verdict.candidateArtifact,
          id: "9007199254740992",
        },
      }).ok,
    ).toBe(false);
  });

  it("invalidates every receipt when the workflow attempt changes", () => {
    const evidence = completeEvidence(MANIFEST);
    const rerun = { ...MANIFEST, workflowRunAttempt: 3 };
    expect(verifyReleaseAcceptance(evidence, rerun)).toEqual({
      ok: false,
      reason: "the acceptance session belongs to a different candidate attempt",
    });
  });

  it("rejects omitted, reordered, failed, or commentary-bearing manual gates", () => {
    const evidence = completeEvidence(MANIFEST);
    expect(
      verifyReleaseAcceptance(
        {
          ...evidence,
          linux: { ...evidence.linux, gates: evidence.linux.gates.slice(1) },
        },
        MANIFEST,
      ),
    ).toEqual({
      ok: false,
      reason: "the Linux acceptance evidence does not pass every required gate",
    });
    expect(
      verifyReleaseAcceptance(
        {
          ...evidence,
          linux: {
            ...evidence.linux,
            gates: [
              ...evidence.linux.gates.slice(0, -1),
              {
                id: "audio-track-selection-preference-fallback",
                result: "passed",
              },
            ],
          },
        },
        MANIFEST,
      ),
    ).toEqual({
      ok: false,
      reason: "the Linux acceptance evidence does not pass every required gate",
    });
    expect(
      verifyReleaseAcceptance(
        {
          ...evidence,
          linux: {
            ...evidence.linux,
            gates: [
              ...evidence.linux.gates.slice(0, -1),
              { id: "mpv-fallback-cleanup", result: "passed" },
            ],
          },
        },
        MANIFEST,
      ),
    ).toEqual({
      ok: false,
      reason: "the Linux acceptance evidence does not pass every required gate",
    });
    expect(
      verifyReleaseAcceptance(
        {
          ...evidence,
          android: {
            ...evidence.android,
            gates: [...evidence.android.gates].reverse(),
          },
        },
        MANIFEST,
      ),
    ).toEqual({
      ok: false,
      reason:
        "the Android acceptance evidence does not pass every required gate",
    });
    const firstLinuxGate = evidence.linux.gates[0];
    expect(firstLinuxGate).toBeDefined();
    expect(
      verifyReleaseAcceptance(
        {
          ...evidence,
          linux: {
            ...evidence.linux,
            gates: [
              { ...firstLinuxGate, result: "failed" },
              ...evidence.linux.gates.slice(1),
            ],
          },
        },
        MANIFEST,
      ).ok,
    ).toBe(false);
    expect(
      verifyReleaseAcceptance(
        {
          ...evidence,
          linux: { ...evidence.linux, privateChannelNote: "forbidden" },
        },
        MANIFEST,
      ).ok,
    ).toBe(false);
  });

  it("requires the installed APK fields to identify the exact candidate bytes", () => {
    const evidence = completeEvidence(MANIFEST);
    expect(
      verifyReleaseAcceptance(
        {
          ...evidence,
          android: {
            ...evidence.android,
            installed: {
              ...evidence.android.installed,
              apkSha256: "e".repeat(64),
            },
          },
        },
        MANIFEST,
      ),
    ).toEqual({
      ok: false,
      reason:
        "the Android acceptance evidence does not identify the accepted APK bytes",
    });
  });

  it("requires an older release with the same application and signing identity", () => {
    const continuity = completeEvidence(MANIFEST).keyContinuity;
    expect(verifyKeyContinuityEvidence(continuity, MANIFEST).ok).toBe(true);
    expect(
      verifyKeyContinuityEvidence(
        {
          ...continuity,
          previous: {
            ...continuity.previous,
            certificateSha256: "e".repeat(64),
          },
        },
        MANIFEST,
      ),
    ).toEqual({
      ok: false,
      reason:
        "the Android update sequence does not match the accepted signing identity",
    });
    expect(
      verifyKeyContinuityEvidence(
        {
          ...continuity,
          previous: {
            ...continuity.previous,
            versionName: "0.11.6",
            versionCode: 11_006,
          },
        },
        MANIFEST,
      ),
    ).toEqual({
      ok: false,
      reason: "key continuity must start from an older stable Sparrow version",
    });
    expect(
      verifyKeyContinuityEvidence(
        {
          ...continuity,
          accepted: {
            ...continuity.accepted,
            uid: continuity.accepted.uid + 1,
          },
        },
        MANIFEST,
      ),
    ).toEqual({
      ok: false,
      reason:
        "the Android replace install did not retain the package data identity",
    });
  });

  it("parses exactly one installed release package identity", () => {
    const dump = `
      Package [xyz.ponbac.sparrow] (abc123):
        versionCode=11005 minSdk=24 targetSdk=36
        versionName=0.11.5
        userId=10234
        firstInstallTime=2026-08-30 11:55:00
    `;
    expect(
      parseInstalledReleasePackage(dump, {
        versionName: "0.11.5",
        versionCode: 11_005,
      }),
    ).toEqual({
      ok: true,
      value: {
        applicationId: "xyz.ponbac.sparrow",
        versionName: "0.11.5",
        versionCode: 11_005,
        minSdk: 24,
        targetSdk: 36,
        uid: 10_234,
        firstInstallTime: "2026-08-30 11:55:00",
      },
    });
    expect(
      parseInstalledReleasePackage(`${dump}${dump}`, {
        versionName: "0.11.5",
        versionCode: 11_005,
      }),
    ).toEqual({
      ok: false,
      reason: "the installed Sparrow package metadata is ambiguous",
    });
  });

  it("accepts only one authenticated exact-attempt environment receipt", () => {
    const candidate = projectAcceptanceCandidate(MANIFEST);
    const receipt = formatAcceptanceApprovalReceipt(candidate, {
      artifactId: "9876",
      artifactSha256: `sha256:${"1".repeat(64)}`,
      manifestSha256: "2".repeat(64),
      evidenceSha256: "3".repeat(64),
    });
    expect(receipt.ok).toBe(true);
    if (!receipt.ok) return;
    const review = {
      state: "approved",
      comment: receipt.value,
      environments: [{ id: 42, name: "release-publish", html_url: "ignored" }],
      user: { id: 7, login: "owner", extra: "ignored" },
    };
    const expected = {
      candidate,
      artifactId: "9876",
      artifactSha256: "1".repeat(64),
      manifestSha256: "2".repeat(64),
    };
    expect(verifyAcceptanceApprovalHistory([review], expected)).toEqual({
      ok: true,
      value: {
        comment: receipt.value,
        evidenceSha256: "3".repeat(64),
        reviewer: { id: 7, login: "owner" },
      },
    });
    expect(
      verifyAcceptanceApprovalHistory([{ ...review, comment: "" }], expected)
        .ok,
    ).toBe(false);
    expect(
      verifyAcceptanceApprovalHistory(
        [
          {
            ...review,
            comment: receipt.value.replace("run=12345/2", "run=12345/1"),
          },
        ],
        expected,
      ).ok,
    ).toBe(false);
    expect(
      verifyAcceptanceApprovalHistory(
        [{ ...review, environments: [{ id: 42, name: "release-signing" }] }],
        expected,
      ).ok,
    ).toBe(false);
    expect(
      verifyAcceptanceApprovalHistory(
        [{ ...review, state: "rejected" }],
        expected,
      ).ok,
    ).toBe(false);
    expect(
      verifyAcceptanceApprovalHistory([review], {
        ...expected,
        artifactSha256: "4".repeat(64),
      }).ok,
    ).toBe(false);
    expect(
      verifyAcceptanceApprovalHistory([review], {
        ...expected,
        manifestSha256: "5".repeat(64),
      }).ok,
    ).toBe(false);
    expect(verifyAcceptanceApprovalHistory([review, review], expected).ok).toBe(
      false,
    );
  });
});

function completeEvidence(manifest: CandidateManifest) {
  const candidate = projectAcceptanceCandidate(manifest);
  return {
    session: { schemaVersion: 1, candidate },
    linux: {
      schemaVersion: 1,
      candidate,
      recordedAt: TIMESTAMP,
      target: {
        distribution: "arch",
        sessionType: "wayland",
        compositor: "hyprland",
      },
      displayedVersion: manifest.version,
      gates: LINUX_ACCEPTANCE_GATES.map((id) => ({ id, result: "passed" })),
    },
    android: {
      schemaVersion: 1,
      candidate,
      recordedAt: TIMESTAMP,
      target: {
        manufacturer: "realme",
        model: "RMX5210",
        apiLevel: 36,
        primaryAbi: "arm64-v8a",
      },
      installed: {
        applicationId: manifest.android.applicationId,
        versionName: manifest.version,
        versionCode: manifest.android.versionCode,
        apkSha256: manifest.artifacts.apk.sha256,
        certificateSha256: manifest.android.certificateSha256,
        uid: 10_234,
        firstInstallTime: "2026-08-30 11:55:00",
      },
      gates: ANDROID_ACCEPTANCE_GATES.map((id) => ({ id, result: "passed" })),
    },
    keyContinuity: {
      schemaVersion: 1,
      candidate,
      recordedAt: TIMESTAMP,
      device: {
        manufacturer: "realme",
        model: "RMX5210",
        apiLevel: 36,
        primaryAbi: "arm64-v8a",
        androidRelease: "16",
      },
      previous: {
        applicationId: manifest.android.applicationId,
        versionName: "0.11.4",
        versionCode: 11_004,
        apkSha256: "f".repeat(64),
        certificateSha256: manifest.android.certificateSha256,
        uid: 10_234,
        firstInstallTime: "2026-08-30 11:55:00",
      },
      accepted: {
        applicationId: manifest.android.applicationId,
        versionName: manifest.version,
        versionCode: manifest.android.versionCode,
        apkSha256: manifest.artifacts.apk.sha256,
        certificateSha256: manifest.android.certificateSha256,
        uid: 10_234,
        firstInstallTime: "2026-08-30 11:55:00",
      },
      installOrder: ["previous", "accepted"],
      inPlaceUpgradeSucceeded: true,
      packageIdentityRetained: true,
    },
  };
}
