import { z } from "zod";
import {
  ANDROID_APPLICATION_ID,
  ANDROID_MIN_SDK,
  ANDROID_TARGET_SDK,
  parseProductVersion,
  type CandidateManifest,
  type ParseResult,
} from "./release-contract-domain.ts";

const SHA256 = /^[0-9a-f]{64}$/u;

/** The complete manual Arch/Wayland gate for one exact AppImage candidate. */
export const LINUX_ACCEPTANCE_GATES = [
  "startup-render-version",
  "browse-search-guide",
  "catalog-first-configuration",
  "catalog-offline-restart",
  "catalog-stale-manual-refresh",
  "primary-picture-audio",
  "primary-controls-channel-changes",
  "audio-track-selection-preference-fallback",
  "bounded-recovery-resource-release",
  "mpv-fallback-cleanup",
] as const;

/** The complete manual physical-Realme gate for one exact signed APK candidate. */
export const ANDROID_ACCEPTANCE_GATES = [
  "package-identity-install",
  "catalog-cold-start-bounds",
  "catalog-offline-restart",
  "catalog-refresh-stale-status",
  "browse-search-guide",
  "primary-picture-audio",
  "primary-controls-channel-changes",
  "audio-track-selection-preference-fallback",
  "rotation-session-preservation",
  "background-foreground-release-resume",
  "manual-lock-wake-state",
  "bounded-recovery-resource-release",
] as const;

/** The minimal immutable identity copied into every local acceptance record. */
export interface AcceptanceCandidateBinding {
  readonly schemaVersion: 1;
  readonly repository: string;
  readonly version: string;
  readonly tag: string;
  readonly commit: string;
  readonly workflowRunId: string;
  readonly workflowRunAttempt: number;
  readonly artifacts: {
    readonly appImage: { readonly name: string; readonly sha256: string };
    readonly apk: { readonly name: string; readonly sha256: string };
  };
  readonly android: {
    readonly applicationId: typeof ANDROID_APPLICATION_ID;
    readonly versionCode: number;
    readonly certificateSha256: string;
  };
}

interface PendingGate<Id extends string> {
  readonly id: Id;
  readonly result: "pending";
}

/** Candidate-bound, deliberately incomplete forms emitted before target testing. */
export interface ReleaseAcceptanceTemplates {
  readonly session: {
    readonly schemaVersion: 1;
    readonly candidate: AcceptanceCandidateBinding;
  };
  readonly linux: {
    readonly schemaVersion: 1;
    readonly candidate: AcceptanceCandidateBinding;
    readonly recordedAt: null;
    readonly target: {
      readonly distribution: null;
      readonly sessionType: null;
      readonly compositor: null;
    };
    readonly displayedVersion: null;
    readonly gates: readonly PendingGate<
      (typeof LINUX_ACCEPTANCE_GATES)[number]
    >[];
  };
  readonly android: {
    readonly schemaVersion: 1;
    readonly candidate: AcceptanceCandidateBinding;
    readonly recordedAt: null;
    readonly target: {
      readonly manufacturer: null;
      readonly model: null;
      readonly apiLevel: null;
      readonly primaryAbi: null;
    };
    readonly installed: {
      readonly applicationId: null;
      readonly versionName: null;
      readonly versionCode: null;
      readonly apkSha256: null;
      readonly certificateSha256: null;
      readonly uid: null;
      readonly firstInstallTime: null;
    };
    readonly gates: readonly PendingGate<
      (typeof ANDROID_ACCEPTANCE_GATES)[number]
    >[];
  };
}

/** A release package identity parsed from `adb shell dumpsys package`. */
export interface InstalledReleasePackage {
  readonly applicationId: typeof ANDROID_APPLICATION_ID;
  readonly versionName: string;
  readonly versionCode: number;
  readonly minSdk: typeof ANDROID_MIN_SDK;
  readonly targetSdk: typeof ANDROID_TARGET_SDK;
  readonly uid: number;
  readonly firstInstallTime: string;
}

/** Machine-recorded proof that Android accepted an in-place release-key update. */
export interface KeyContinuityEvidence {
  readonly schemaVersion: 1;
  readonly candidate: AcceptanceCandidateBinding;
  readonly recordedAt: string;
  readonly device: {
    readonly manufacturer: "realme";
    readonly model: "RMX5210";
    readonly apiLevel: 36;
    readonly primaryAbi: "arm64-v8a";
    readonly androidRelease: string;
  };
  readonly previous: {
    readonly applicationId: typeof ANDROID_APPLICATION_ID;
    readonly versionName: string;
    readonly versionCode: number;
    readonly apkSha256: string;
    readonly certificateSha256: string;
    readonly uid: number;
    readonly firstInstallTime: string;
  };
  readonly accepted: {
    readonly applicationId: typeof ANDROID_APPLICATION_ID;
    readonly versionName: string;
    readonly versionCode: number;
    readonly apkSha256: string;
    readonly certificateSha256: string;
    readonly uid: number;
    readonly firstInstallTime: string;
  };
  readonly installOrder: readonly ["previous", "accepted"];
  readonly inPlaceUpgradeSucceeded: true;
  readonly packageIdentityRetained: true;
}

/** Immutable GitHub artifact and local evidence digests carried by an approval receipt. */
export interface AcceptanceReceiptDigests {
  readonly artifactId: string;
  readonly artifactSha256: string;
  readonly manifestSha256: string;
  readonly evidenceSha256: string;
}

/** An authenticated environment review that exactly authorizes one candidate attempt. */
export interface VerifiedAcceptanceApproval {
  readonly comment: string;
  readonly evidenceSha256: string;
  readonly reviewer: { readonly id: number; readonly login: string };
}

/** The safe projection returned only after every exact-candidate gate passes. */
export interface VerifiedReleaseAcceptance {
  readonly candidate: AcceptanceCandidateBinding;
  readonly evidenceRecordedAt: {
    readonly linux: string;
    readonly android: string;
    readonly keyContinuity: string;
  };
}

/** The complete immutable verdict whose file digest is carried by an approval receipt. */
export interface SealedReleaseAcceptanceVerdict {
  readonly schemaVersion: 1;
  readonly verdict: "evidence-complete";
  readonly sealedAt: string;
  readonly candidate: AcceptanceCandidateBinding;
  readonly candidateArtifact: {
    readonly id: string;
    readonly sha256: string;
  };
  readonly candidateManifestSha256: string;
  readonly evidenceSha256: {
    readonly session: string;
    readonly linux: string;
    readonly android: string;
    readonly keyContinuity: string;
  };
  readonly evidenceRecordedAt: {
    readonly linux: string;
    readonly android: string;
    readonly keyContinuity: string;
  };
}

const candidateBindingSchema = z
  .object({
    schemaVersion: z.literal(1),
    repository: z.string().regex(/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/u),
    version: z.string(),
    tag: z.string(),
    commit: z.string().regex(/^[0-9a-f]{40}$/u),
    workflowRunId: z.string().regex(/^[1-9][0-9]*$/u),
    workflowRunAttempt: z.number().int().positive(),
    artifacts: z
      .object({
        appImage: z
          .object({ name: z.string(), sha256: z.string().regex(SHA256) })
          .strict(),
        apk: z
          .object({ name: z.string(), sha256: z.string().regex(SHA256) })
          .strict(),
      })
      .strict(),
    android: z
      .object({
        applicationId: z.literal(ANDROID_APPLICATION_ID),
        versionCode: z.number().int().positive(),
        certificateSha256: z.string().regex(SHA256),
      })
      .strict(),
  })
  .strict();

const passedGateSchema = z
  .object({ id: z.string().min(1).max(96), result: z.literal("passed") })
  .strict();

const linuxEvidenceSchema = z
  .object({
    schemaVersion: z.literal(1),
    candidate: candidateBindingSchema,
    recordedAt: z.string().datetime({ offset: true }),
    target: z
      .object({
        distribution: z.literal("arch"),
        sessionType: z.literal("wayland"),
        compositor: z.literal("hyprland"),
      })
      .strict(),
    displayedVersion: z.string(),
    gates: z.array(passedGateSchema).max(LINUX_ACCEPTANCE_GATES.length),
  })
  .strict();

const androidEvidenceSchema = z
  .object({
    schemaVersion: z.literal(1),
    candidate: candidateBindingSchema,
    recordedAt: z.string().datetime({ offset: true }),
    target: z
      .object({
        manufacturer: z.literal("realme"),
        model: z.literal("RMX5210"),
        apiLevel: z.literal(36),
        primaryAbi: z.literal("arm64-v8a"),
      })
      .strict(),
    installed: z
      .object({
        applicationId: z.literal(ANDROID_APPLICATION_ID),
        versionName: z.string(),
        versionCode: z.number().int().positive(),
        apkSha256: z.string().regex(SHA256),
        certificateSha256: z.string().regex(SHA256),
        uid: z.number().int().positive(),
        firstInstallTime: z.string().min(1).max(64),
      })
      .strict(),
    gates: z.array(passedGateSchema).max(ANDROID_ACCEPTANCE_GATES.length),
  })
  .strict();

const continuityEvidenceSchema = z
  .object({
    schemaVersion: z.literal(1),
    candidate: candidateBindingSchema,
    recordedAt: z.string().datetime({ offset: true }),
    device: z
      .object({
        manufacturer: z.literal("realme"),
        model: z.literal("RMX5210"),
        apiLevel: z.literal(36),
        primaryAbi: z.literal("arm64-v8a"),
        androidRelease: z.string().min(1).max(32),
      })
      .strict(),
    previous: z
      .object({
        applicationId: z.literal(ANDROID_APPLICATION_ID),
        versionName: z.string(),
        versionCode: z.number().int().positive(),
        apkSha256: z.string().regex(SHA256),
        certificateSha256: z.string().regex(SHA256),
        uid: z.number().int().positive(),
        firstInstallTime: z.string().min(1).max(64),
      })
      .strict(),
    accepted: z
      .object({
        applicationId: z.literal(ANDROID_APPLICATION_ID),
        versionName: z.string(),
        versionCode: z.number().int().positive(),
        apkSha256: z.string().regex(SHA256),
        certificateSha256: z.string().regex(SHA256),
        uid: z.number().int().positive(),
        firstInstallTime: z.string().min(1).max(64),
      })
      .strict(),
    installOrder: z.tuple([z.literal("previous"), z.literal("accepted")]),
    inPlaceUpgradeSucceeded: z.literal(true),
    packageIdentityRetained: z.literal(true),
  })
  .strict();

const sessionSchema = z
  .object({ schemaVersion: z.literal(1), candidate: candidateBindingSchema })
  .strict();

const sha256Schema = z.string().regex(SHA256);
const timestampSchema = z.string().datetime({ offset: true });
const positiveIdentifierSchema = z
  .string()
  .regex(/^[1-9][0-9]*$/u)
  .refine((value) => Number.isSafeInteger(Number(value)));

const sealedReleaseAcceptanceVerdictSchema = z
  .object({
    schemaVersion: z.literal(1),
    verdict: z.literal("evidence-complete"),
    sealedAt: timestampSchema,
    candidate: candidateBindingSchema,
    candidateArtifact: z
      .object({ id: positiveIdentifierSchema, sha256: sha256Schema })
      .strict(),
    candidateManifestSha256: sha256Schema,
    evidenceSha256: z
      .object({
        session: sha256Schema,
        linux: sha256Schema,
        android: sha256Schema,
        keyContinuity: sha256Schema,
      })
      .strict(),
    evidenceRecordedAt: z
      .object({
        linux: timestampSchema,
        android: timestampSchema,
        keyContinuity: timestampSchema,
      })
      .strict(),
  })
  .strict();

const approvalHistorySchema = z.array(
  z
    .object({
      state: z.string(),
      comment: z.string().nullable(),
      environments: z.array(
        z.object({ id: z.number().int().positive(), name: z.string() }).strip(),
      ),
      user: z
        .object({ id: z.number().int().positive(), login: z.string().min(1) })
        .strip(),
    })
    .strip(),
);

/** Projects a candidate manifest into the identity required by every evidence file. */
export function projectAcceptanceCandidate(
  manifest: CandidateManifest,
): AcceptanceCandidateBinding {
  return {
    schemaVersion: 1,
    repository: manifest.repository,
    version: manifest.version,
    tag: manifest.tag,
    commit: manifest.commit,
    workflowRunId: manifest.workflowRunId,
    workflowRunAttempt: manifest.workflowRunAttempt,
    artifacts: manifest.artifacts,
    android: {
      applicationId: manifest.android.applicationId,
      versionCode: manifest.android.versionCode,
      certificateSha256: manifest.android.certificateSha256,
    },
  };
}

/** Creates fail-closed local forms whose pending values cannot pass final verification. */
export function createAcceptanceTemplates(
  manifest: CandidateManifest,
): ReleaseAcceptanceTemplates {
  const candidate = projectAcceptanceCandidate(manifest);
  return {
    session: { schemaVersion: 1, candidate },
    linux: {
      schemaVersion: 1,
      candidate,
      recordedAt: null,
      target: { distribution: null, sessionType: null, compositor: null },
      displayedVersion: null,
      gates: LINUX_ACCEPTANCE_GATES.map(pendingGate),
    },
    android: {
      schemaVersion: 1,
      candidate,
      recordedAt: null,
      target: {
        manufacturer: null,
        model: null,
        apiLevel: null,
        primaryAbi: null,
      },
      installed: {
        applicationId: null,
        versionName: null,
        versionCode: null,
        apkSha256: null,
        certificateSha256: null,
        uid: null,
        firstInstallTime: null,
      },
      gates: ANDROID_ACCEPTANCE_GATES.map(pendingGate),
    },
  };
}

/** Parses every v1 sealed-verdict field without stripping or accepting unknown fields. */
export function parseSealedReleaseAcceptanceVerdict(
  input: unknown,
): ParseResult<SealedReleaseAcceptanceVerdict> {
  const parsed = sealedReleaseAcceptanceVerdictSchema.safeParse(input);
  return parsed.success
    ? accept(parsed.data)
    : reject("the sealed release acceptance verdict is invalid");
}

/** Parses the installed non-debug release package and requires its exact expected identity. */
export function parseInstalledReleasePackage(
  input: unknown,
  expected: {
    readonly versionName: string;
    readonly versionCode: number;
  },
): ParseResult<InstalledReleasePackage> {
  if (typeof input !== "string" || input.length > 4 * 1024 * 1024) {
    return reject("the installed Sparrow package metadata is invalid");
  }
  const packageHeaders = input.match(
    /^\s*Package \[xyz\.ponbac\.sparrow\] \([^\r\n]+\):\s*$/gmu,
  );
  const versionLines = Array.from(
    input.matchAll(
      /^\s*versionCode=(\d+)\s+minSdk=(\d+)\s+targetSdk=(\d+).*$/gmu,
    ),
  );
  const versionNames = Array.from(
    input.matchAll(/^\s*versionName=([^\s]+)\s*$/gmu),
  );
  const userIds = Array.from(input.matchAll(/^\s*userId=(\d+)\s*$/gmu));
  const installTimes = Array.from(
    input.matchAll(/^\s*firstInstallTime=(.+?)\s*$/gmu),
  );
  if (
    packageHeaders?.length !== 1 ||
    versionLines.length !== 1 ||
    versionNames.length !== 1 ||
    userIds.length !== 1 ||
    installTimes.length !== 1
  ) {
    return reject("the installed Sparrow package metadata is ambiguous");
  }
  const versionCode = Number(versionLines[0]?.[1]);
  const minSdk = Number(versionLines[0]?.[2]);
  const targetSdk = Number(versionLines[0]?.[3]);
  const versionName = versionNames[0]?.[1];
  const uid = Number(userIds[0]?.[1]);
  const firstInstallTime = installTimes[0]?.[1];
  if (
    versionName !== expected.versionName ||
    versionCode !== expected.versionCode ||
    minSdk !== ANDROID_MIN_SDK ||
    targetSdk !== ANDROID_TARGET_SDK ||
    !Number.isSafeInteger(uid) ||
    uid <= 0 ||
    firstInstallTime === undefined ||
    firstInstallTime.length > 64
  ) {
    return reject(
      "the installed Sparrow package does not match the staged release APK",
    );
  }
  return accept({
    applicationId: ANDROID_APPLICATION_ID,
    versionName,
    versionCode,
    minSdk: ANDROID_MIN_SDK,
    targetSdk: ANDROID_TARGET_SDK,
    uid,
    firstInstallTime,
  });
}

/** Verifies machine-recorded key continuity against the exact accepted candidate. */
export function verifyKeyContinuityEvidence(
  input: unknown,
  manifest: CandidateManifest,
): ParseResult<KeyContinuityEvidence> {
  const parsed = continuityEvidenceSchema.safeParse(input);
  if (!parsed.success)
    return reject("the Android key-continuity evidence is invalid");
  const evidence = parsed.data;
  const candidate = projectAcceptanceCandidate(manifest);
  if (!sameCandidate(evidence.candidate, candidate)) {
    return reject(
      "the Android key-continuity evidence belongs to a different candidate attempt",
    );
  }
  const previousVersion = parseProductVersion(evidence.previous.versionName);
  if (
    !previousVersion.ok ||
    previousVersion.value.androidVersionCode !==
      evidence.previous.versionCode ||
    evidence.previous.versionCode >= manifest.android.versionCode
  ) {
    return reject(
      "key continuity must start from an older stable Sparrow version",
    );
  }
  if (
    evidence.previous.applicationId !== manifest.android.applicationId ||
    evidence.previous.certificateSha256 !==
      manifest.android.certificateSha256 ||
    evidence.accepted.applicationId !== manifest.android.applicationId ||
    evidence.accepted.versionName !== manifest.version ||
    evidence.accepted.versionCode !== manifest.android.versionCode ||
    evidence.accepted.apkSha256 !== manifest.artifacts.apk.sha256 ||
    evidence.accepted.certificateSha256 !==
      manifest.android.certificateSha256 ||
    evidence.previous.apkSha256 === evidence.accepted.apkSha256
  ) {
    return reject(
      "the Android update sequence does not match the accepted signing identity",
    );
  }
  if (
    evidence.previous.uid !== evidence.accepted.uid ||
    evidence.previous.firstInstallTime !== evidence.accepted.firstInstallTime
  ) {
    return reject(
      "the Android replace install did not retain the package data identity",
    );
  }
  return accept(evidence);
}

/** Formats the only approval comment that may authorize a waiting publication job. */
export function formatAcceptanceApprovalReceipt(
  candidate: AcceptanceCandidateBinding,
  digests: AcceptanceReceiptDigests,
): ParseResult<string> {
  const artifactId = parsePositiveIdentifier(digests.artifactId);
  const artifactSha256 = parseArtifactDigest(digests.artifactSha256);
  const manifestSha256 = parseSha256(digests.manifestSha256);
  const evidenceSha256 = parseSha256(digests.evidenceSha256);
  if (
    artifactId === undefined ||
    artifactSha256 === undefined ||
    manifestSha256 === undefined ||
    evidenceSha256 === undefined
  ) {
    return reject("the acceptance receipt digests are invalid");
  }
  return accept(
    `sparrow-acceptance/v1 repository=${candidate.repository} tag=${candidate.tag} ` +
      `commit=${candidate.commit} run=${candidate.workflowRunId}/${candidate.workflowRunAttempt} ` +
      `artifact=${artifactId} artifact-sha256=${artifactSha256} ` +
      `manifest-sha256=${manifestSha256} ` +
      `appimage-sha256=${candidate.artifacts.appImage.sha256} ` +
      `apk-sha256=${candidate.artifacts.apk.sha256} ` +
      `certificate-sha256=${candidate.android.certificateSha256} ` +
      `evidence-sha256=${evidenceSha256} gates=linux+android+continuity`,
  );
}

/** Requires one authenticated `release-publish` approval with the exact strict receipt. */
export function verifyAcceptanceApprovalHistory(
  input: unknown,
  expected: {
    readonly candidate: AcceptanceCandidateBinding;
    readonly artifactId: string;
    readonly artifactSha256: string;
    readonly manifestSha256: string;
  },
): ParseResult<VerifiedAcceptanceApproval> {
  const history = approvalHistorySchema.safeParse(input);
  if (!history.success)
    return reject("the GitHub environment review history is invalid");
  const artifactId = parsePositiveIdentifier(expected.artifactId);
  const artifactSha256 = parseArtifactDigest(expected.artifactSha256);
  const manifestSha256 = parseSha256(expected.manifestSha256);
  if (
    artifactId === undefined ||
    artifactSha256 === undefined ||
    manifestSha256 === undefined
  ) {
    return reject("the expected candidate artifact identity is invalid");
  }

  const matches: VerifiedAcceptanceApproval[] = [];
  for (const review of history.data) {
    if (
      review.state !== "approved" ||
      review.comment === null ||
      !review.environments.some(
        (environment) => environment.name === "release-publish",
      )
    ) {
      continue;
    }
    const receipt = parseApprovalReceipt(review.comment);
    if (
      receipt === undefined ||
      receipt.repository !== expected.candidate.repository ||
      receipt.tag !== expected.candidate.tag ||
      receipt.commit !== expected.candidate.commit ||
      receipt.runId !== expected.candidate.workflowRunId ||
      receipt.runAttempt !== expected.candidate.workflowRunAttempt ||
      receipt.artifactId !== artifactId ||
      receipt.artifactSha256 !== artifactSha256 ||
      receipt.manifestSha256 !== manifestSha256 ||
      receipt.appImageSha256 !== expected.candidate.artifacts.appImage.sha256 ||
      receipt.apkSha256 !== expected.candidate.artifacts.apk.sha256 ||
      receipt.certificateSha256 !== expected.candidate.android.certificateSha256
    ) {
      continue;
    }
    matches.push({
      comment: review.comment,
      evidenceSha256: receipt.evidenceSha256,
      reviewer: { id: review.user.id, login: review.user.login },
    });
  }
  const unique = matches.length === 1 ? matches[0] : undefined;
  return unique === undefined
    ? reject("publication has no unique exact-candidate acceptance approval")
    : accept(unique);
}

/** Accepts only complete Linux, Android, and key-continuity evidence for one run attempt. */
export function verifyReleaseAcceptance(
  input: {
    readonly session: unknown;
    readonly linux: unknown;
    readonly android: unknown;
    readonly keyContinuity: unknown;
  },
  manifest: CandidateManifest,
): ParseResult<VerifiedReleaseAcceptance> {
  const expected = projectAcceptanceCandidate(manifest);
  const session = sessionSchema.safeParse(input.session);
  if (!session.success || !sameCandidate(session.data.candidate, expected)) {
    return reject(
      "the acceptance session belongs to a different candidate attempt",
    );
  }

  const linux = linuxEvidenceSchema.safeParse(input.linux);
  if (!linux.success || !sameCandidate(linux.data.candidate, expected)) {
    return reject(
      "the Linux acceptance evidence is invalid or belongs to another attempt",
    );
  }
  if (linux.data.displayedVersion !== manifest.version) {
    return reject(
      "the Linux candidate did not display the accepted product version",
    );
  }
  if (!allGatesPassed(linux.data.gates, LINUX_ACCEPTANCE_GATES)) {
    return reject(
      "the Linux acceptance evidence does not pass every required gate",
    );
  }

  const android = androidEvidenceSchema.safeParse(input.android);
  if (!android.success || !sameCandidate(android.data.candidate, expected)) {
    return reject(
      "the Android acceptance evidence is invalid or belongs to another attempt",
    );
  }
  if (
    android.data.installed.applicationId !== manifest.android.applicationId ||
    android.data.installed.versionName !== manifest.version ||
    android.data.installed.versionCode !== manifest.android.versionCode ||
    android.data.installed.apkSha256 !== manifest.artifacts.apk.sha256 ||
    android.data.installed.certificateSha256 !==
      manifest.android.certificateSha256
  ) {
    return reject(
      "the Android acceptance evidence does not identify the accepted APK bytes",
    );
  }
  if (!allGatesPassed(android.data.gates, ANDROID_ACCEPTANCE_GATES)) {
    return reject(
      "the Android acceptance evidence does not pass every required gate",
    );
  }

  const continuity = verifyKeyContinuityEvidence(input.keyContinuity, manifest);
  if (!continuity.ok) return continuity;
  if (
    android.data.installed.uid !== continuity.value.accepted.uid ||
    android.data.installed.firstInstallTime !==
      continuity.value.accepted.firstInstallTime
  ) {
    return reject(
      "the Android observations do not match the continuity-tested installation",
    );
  }
  return accept({
    candidate: expected,
    evidenceRecordedAt: {
      linux: linux.data.recordedAt,
      android: android.data.recordedAt,
      keyContinuity: continuity.value.recordedAt,
    },
  });
}

function pendingGate<Id extends string>(id: Id): PendingGate<Id> {
  return { id, result: "pending" };
}

interface ParsedApprovalReceipt {
  readonly repository: string;
  readonly tag: string;
  readonly commit: string;
  readonly runId: string;
  readonly runAttempt: number;
  readonly artifactId: string;
  readonly artifactSha256: string;
  readonly manifestSha256: string;
  readonly appImageSha256: string;
  readonly apkSha256: string;
  readonly certificateSha256: string;
  readonly evidenceSha256: string;
}

function parseApprovalReceipt(
  input: string,
): ParsedApprovalReceipt | undefined {
  const match =
    /^sparrow-acceptance\/v1 repository=([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+) tag=(v[0-9]+\.[0-9]+\.[0-9]+) commit=([0-9a-f]{40}) run=([1-9][0-9]*)\/([1-9][0-9]*) artifact=([1-9][0-9]*) artifact-sha256=([0-9a-f]{64}) manifest-sha256=([0-9a-f]{64}) appimage-sha256=([0-9a-f]{64}) apk-sha256=([0-9a-f]{64}) certificate-sha256=([0-9a-f]{64}) evidence-sha256=([0-9a-f]{64}) gates=linux\+android\+continuity$/u.exec(
      input,
    );
  if (match === null) return undefined;
  const runAttempt = Number(match[5]);
  if (!Number.isSafeInteger(runAttempt)) return undefined;
  return {
    repository: match[1] ?? "",
    tag: match[2] ?? "",
    commit: match[3] ?? "",
    runId: match[4] ?? "",
    runAttempt,
    artifactId: match[6] ?? "",
    artifactSha256: match[7] ?? "",
    manifestSha256: match[8] ?? "",
    appImageSha256: match[9] ?? "",
    apkSha256: match[10] ?? "",
    certificateSha256: match[11] ?? "",
    evidenceSha256: match[12] ?? "",
  };
}

function parsePositiveIdentifier(input: string): string | undefined {
  if (!/^[1-9][0-9]*$/u.test(input)) return undefined;
  return Number.isSafeInteger(Number(input)) ? input : undefined;
}

function parseArtifactDigest(input: string): string | undefined {
  return parseSha256(
    input.startsWith("sha256:") ? input.slice("sha256:".length) : input,
  );
}

function parseSha256(input: string): string | undefined {
  return SHA256.test(input) ? input : undefined;
}

function allGatesPassed(
  actual: readonly { readonly id: string; readonly result: "passed" }[],
  expected: readonly string[],
): boolean {
  return (
    actual.length === expected.length &&
    actual.every(
      (gate, index) => gate.id === expected[index] && gate.result === "passed",
    )
  );
}

function sameCandidate(
  actual: AcceptanceCandidateBinding,
  expected: AcceptanceCandidateBinding,
): boolean {
  return (
    actual.schemaVersion === expected.schemaVersion &&
    actual.repository === expected.repository &&
    actual.version === expected.version &&
    actual.tag === expected.tag &&
    actual.commit === expected.commit &&
    actual.workflowRunId === expected.workflowRunId &&
    actual.workflowRunAttempt === expected.workflowRunAttempt &&
    actual.artifacts.appImage.name === expected.artifacts.appImage.name &&
    actual.artifacts.appImage.sha256 === expected.artifacts.appImage.sha256 &&
    actual.artifacts.apk.name === expected.artifacts.apk.name &&
    actual.artifacts.apk.sha256 === expected.artifacts.apk.sha256 &&
    actual.android.applicationId === expected.android.applicationId &&
    actual.android.versionCode === expected.android.versionCode &&
    actual.android.certificateSha256 === expected.android.certificateSha256
  );
}

function accept<Value>(value: Value): ParseResult<Value> {
  return { ok: true, value };
}

function reject(reason: string): ParseResult<never> {
  return { ok: false, reason };
}
