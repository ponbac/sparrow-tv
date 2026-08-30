import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { readdir } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import { verifyDeviceIdentity } from "./android-catalog-acceptance-domain.ts";
import {
  prepareReleaseOutput,
  readReleaseRegularFile,
  snapshotReleaseFiles,
  writeReleasePrivateDirectory,
  writeReleasePrivateFile,
  ReleaseFilesystemFailure,
} from "./release-filesystem.ts";
import {
  createAcceptanceTemplates,
  formatAcceptanceApprovalReceipt,
  parseInstalledReleasePackage,
  parseSealedReleaseAcceptanceVerdict,
  projectAcceptanceCandidate,
  verifyAcceptanceApprovalHistory,
  verifyKeyContinuityEvidence,
  verifyReleaseAcceptance,
  type VerifiedReleaseAcceptance,
} from "./release-acceptance-domain.ts";
import {
  formatWorkflowRunArtifactsEndpoint,
  parseCandidateManifest,
  parseProductVersion,
  type CandidateManifest,
  type ProductVersion,
} from "./release-contract-domain.ts";

const REPOSITORY_ROOT = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const RELEASE_CONTRACT = join(
  REPOSITORY_ROOT,
  "app/scripts/release-contract.ts",
);
const COMMAND_OUTPUT_LIMIT = 4 * 1024 * 1024;
const SHA256 = /^[0-9a-f]{64}$/u;
const PACKAGE_NAME = "xyz.ponbac.sparrow";

const manifestSeedSchema = z
  .object({
    version: z.string(),
    tag: z.string(),
    commit: z.string(),
    repository: z.string(),
    workflowRunId: z.string(),
    workflowRunAttempt: z.number(),
  })
  .passthrough();

const artifactResponseSchema = z
  .object({
    artifacts: z.array(
      z
        .object({
          id: z.number().int().positive(),
          name: z.string(),
          expired: z.boolean(),
          digest: z.string().nullable(),
          workflow_run: z
            .object({ id: z.number().int().positive(), head_sha: z.string() })
            .strip(),
        })
        .strip(),
    ),
  })
  .strip();

const pendingDeploymentsSchema = z.array(
  z
    .object({
      environment: z
        .object({ id: z.number().int().positive(), name: z.string() })
        .strip(),
      current_user_can_approve: z.boolean(),
    })
    .strip(),
);

const workflowRunSchema = z
  .object({
    id: z.number().int().positive(),
    run_attempt: z.number().int().positive(),
    head_sha: z.string(),
    event: z.string(),
    status: z.string(),
  })
  .strip();

interface CliArguments {
  readonly command: string;
  readonly values: ReadonlyMap<string, string>;
}

interface VerifiedCandidate {
  readonly directory: string;
  readonly sourceDirectory: string;
  readonly manifest: CandidateManifest;
  readonly manifestSha256: string;
  close(): Promise<void>;
}

interface CommandResult {
  readonly stdout: string;
  readonly stderr: string;
}

interface EvidencePaths {
  readonly session: string;
  readonly linux: string;
  readonly android: string;
  readonly keyContinuity: string;
}

interface VerifiedEvidenceSnapshot {
  readonly paths: EvidencePaths;
  readonly verified: VerifiedReleaseAcceptance;
  close(): Promise<void>;
}

class AcceptanceFailure extends Error {
  readonly _tag = "AcceptanceFailure";
}

async function main(): Promise<void> {
  const arguments_ = parseArguments(process.argv.slice(2));
  switch (arguments_.command) {
    case "prepare":
      await prepare(arguments_.values);
      return;
    case "prove-continuity":
      await proveContinuity(arguments_.values);
      return;
    case "seal":
      await seal(arguments_.values);
      return;
    case "approve":
      await approve(arguments_.values);
      return;
  }
  throw new AcceptanceFailure("unknown release-acceptance command");
}

async function prepare(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, ["--candidate", "--output"]);
  const candidate = await loadVerifiedCandidate(
    required(values, "--candidate"),
  );
  try {
    const output = await secureOutput(required(values, "--output"), candidate);
    try {
      const templates = createAcceptanceTemplates(candidate.manifest);
      await writeReleasePrivateDirectory(output, {
        "acceptance-session.json": jsonText(templates.session),
        "android-observations.json": jsonText(templates.android),
        "linux-observations.json": jsonText(templates.linux),
      });
      process.stdout.write(
        `prepared=${candidate.manifest.workflowRunId}/${candidate.manifest.workflowRunAttempt}\n`,
      );
    } finally {
      await output.close();
    }
  } finally {
    await candidate.close();
  }
}

async function proveContinuity(
  values: ReadonlyMap<string, string>,
): Promise<void> {
  requireExactFlags(values, [
    "--candidate",
    "--previous-apk",
    "--previous-version",
    "--serial",
    "--output",
  ]);
  const candidate = await loadVerifiedCandidate(
    required(values, "--candidate"),
  );
  try {
    const previousVersion = parseVersion(
      required(values, "--previous-version"),
    );
    if (
      previousVersion.androidVersionCode >=
      candidate.manifest.android.versionCode
    ) {
      throw new AcceptanceFailure(
        "the continuity predecessor must be an older stable version",
      );
    }
    const requestedPreviousApk = repositoryPath(
      required(values, "--previous-apk"),
    );
    if (basename(requestedPreviousApk) !== previousVersion.apkName) {
      throw new AcceptanceFailure(
        "the continuity predecessor APK has the wrong filename",
      );
    }
    const previousSnapshot = await snapshotReleaseFiles(
      dirname(requestedPreviousApk),
      [previousVersion.apkName, `${previousVersion.apkName}.sha256`],
      { exact: false },
    );
    try {
      const output = await secureOutput(
        required(values, "--output"),
        candidate,
      );
      try {
        const serial = parseAdbSerial(required(values, "--serial"));
        const previousContractApk = join(
          previousSnapshot.directory,
          previousVersion.apkName,
        );
        const previousApk = join(
          previousSnapshot.boundDirectory,
          previousVersion.apkName,
        );
        runReleaseContract([
          "verify-apk",
          "--version",
          previousVersion.text,
          "--artifact",
          previousContractApk,
        ]);
        const acceptedApk = join(
          candidate.directory,
          candidate.manifest.artifacts.apk.name,
        );
        const previousSha256 = await sha256(previousApk);
        const acceptedSha256 = await sha256(acceptedApk);
        if (
          acceptedSha256 !== candidate.manifest.artifacts.apk.sha256 ||
          previousSha256 === acceptedSha256
        ) {
          throw new AcceptanceFailure("the continuity APK bytes are invalid");
        }

        const device = await readTargetDevice(serial);
        installReleaseApk(
          serial,
          previousApk,
          "install continuity predecessor",
        );
        const previous = readInstalledPackage(serial, previousVersion);
        installReleaseApk(
          serial,
          acceptedApk,
          "install accepted candidate as an update",
        );
        const accepted = readInstalledPackage(
          serial,
          parseVersion(candidate.manifest.version),
        );
        if (
          (await sha256(previousApk)) !== previousSha256 ||
          (await sha256(acceptedApk)) !== acceptedSha256
        ) {
          throw new AcceptanceFailure(
            "an APK changed while proving key continuity",
          );
        }

        const rawEvidence = {
          schemaVersion: 1,
          candidate: projectAcceptanceCandidate(candidate.manifest),
          recordedAt: new Date().toISOString(),
          device,
          previous: {
            applicationId: previous.applicationId,
            versionName: previous.versionName,
            versionCode: previous.versionCode,
            apkSha256: previousSha256,
            certificateSha256: candidate.manifest.android.certificateSha256,
            uid: previous.uid,
            firstInstallTime: previous.firstInstallTime,
          },
          accepted: {
            applicationId: accepted.applicationId,
            versionName: accepted.versionName,
            versionCode: accepted.versionCode,
            apkSha256: acceptedSha256,
            certificateSha256: candidate.manifest.android.certificateSha256,
            uid: accepted.uid,
            firstInstallTime: accepted.firstInstallTime,
          },
          installOrder: ["previous", "accepted"],
          inPlaceUpgradeSucceeded: true,
          packageIdentityRetained: true,
        };
        const evidence = verifyKeyContinuityEvidence(
          rawEvidence,
          candidate.manifest,
        );
        if (!evidence.ok) throw new AcceptanceFailure(evidence.reason);
        await writeReleasePrivateFile(output, jsonText(evidence.value));
        process.stdout.write("continuity=proved\n");
      } finally {
        await output.close();
      }
    } finally {
      await previousSnapshot.close();
    }
  } finally {
    await candidate.close();
  }
}

async function seal(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, [
    "--candidate",
    "--evidence",
    "--artifact-id",
    "--artifact-digest",
    "--output",
  ]);
  const candidate = await loadVerifiedCandidate(
    required(values, "--candidate"),
  );
  try {
    const evidenceDirectory = repositoryPath(required(values, "--evidence"));
    const output = await secureOutput(required(values, "--output"), candidate);
    try {
      const artifactId = parsePositiveIdentifier(
        required(values, "--artifact-id"),
      );
      const artifactSha256 = parseArtifactDigest(
        required(values, "--artifact-digest"),
      );
      await verifyRemoteCandidateArtifact(
        candidate.manifest,
        artifactId,
        artifactSha256,
      );
      const evidence = await readVerifiedEvidence(
        candidate.manifest,
        evidenceDirectory,
      );
      try {
        const verdictText = await createVerdictText(
          candidate,
          evidence.verified,
          evidence.paths,
          artifactId,
          artifactSha256,
          new Date().toISOString(),
        );
        const verdictSha256 = sha256Text(verdictText);
        const receipt = formatAcceptanceApprovalReceipt(
          evidence.verified.candidate,
          {
            artifactId,
            artifactSha256,
            manifestSha256: candidate.manifestSha256,
            evidenceSha256: verdictSha256,
          },
        );
        if (!receipt.ok) throw new AcceptanceFailure(receipt.reason);
        await writeReleasePrivateDirectory(output, {
          "ACCEPTANCE-VERDICT.json": verdictText,
          "APPROVAL-COMMENT.txt": `${receipt.value}\n`,
        });
        process.stdout.write(`sealed=${verdictSha256}\n`);
      } finally {
        await evidence.close();
      }
    } finally {
      await output.close();
    }
  } finally {
    await candidate.close();
  }
}

async function approve(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, ["--candidate", "--evidence", "--sealed"]);
  const candidate = await loadVerifiedCandidate(
    required(values, "--candidate"),
  );
  try {
    const evidenceDirectory = repositoryPath(required(values, "--evidence"));
    const sealed = await snapshotReleaseFiles(
      repositoryPath(required(values, "--sealed")),
      ["ACCEPTANCE-VERDICT.json", "APPROVAL-COMMENT.txt"],
      { exact: true },
    );
    try {
      const verdictText = (
        await readReleaseRegularFile(
          join(sealed.boundDirectory, "ACCEPTANCE-VERDICT.json"),
        )
      ).toString("utf8");
      const verdict = parseSealedReleaseAcceptanceVerdict(
        parseJsonText(verdictText),
      );
      if (
        !verdict.ok ||
        verdict.value.candidateManifestSha256 !== candidate.manifestSha256
      ) {
        throw new AcceptanceFailure(
          "the sealed acceptance verdict does not match the candidate",
        );
      }
      await verifyRemoteCandidateArtifact(
        candidate.manifest,
        verdict.value.candidateArtifact.id,
        verdict.value.candidateArtifact.sha256,
      );
      const evidence = await readVerifiedEvidence(
        candidate.manifest,
        evidenceDirectory,
      );
      try {
        const expectedVerdict = await createVerdictText(
          candidate,
          evidence.verified,
          evidence.paths,
          verdict.value.candidateArtifact.id,
          verdict.value.candidateArtifact.sha256,
          verdict.value.sealedAt,
        );
        if (verdictText !== expectedVerdict) {
          throw new AcceptanceFailure(
            "the sealed verdict no longer matches the complete evidence",
          );
        }
        const expectedReceipt = formatAcceptanceApprovalReceipt(
          projectAcceptanceCandidate(candidate.manifest),
          {
            artifactId: verdict.value.candidateArtifact.id,
            artifactSha256: verdict.value.candidateArtifact.sha256,
            manifestSha256: candidate.manifestSha256,
            evidenceSha256: sha256Text(verdictText),
          },
        );
        if (!expectedReceipt.ok)
          throw new AcceptanceFailure(expectedReceipt.reason);
        const storedReceipt = (
          await readReleaseRegularFile(
            join(sealed.boundDirectory, "APPROVAL-COMMENT.txt"),
          )
        ).toString("utf8");
        if (storedReceipt !== `${expectedReceipt.value}\n`) {
          throw new AcceptanceFailure("the sealed approval comment is invalid");
        }

        const pending = pendingDeploymentsSchema.safeParse(
          ghApiJson(
            "GET",
            `repos/${candidate.manifest.repository}/actions/runs/${candidate.manifest.workflowRunId}/pending_deployments`,
          ),
        );
        if (!pending.success) {
          throw new AcceptanceFailure(
            "GitHub returned invalid pending deployment data",
          );
        }
        const targets = pending.data.filter(
          (deployment) =>
            deployment.environment.name === "release-publish" &&
            deployment.current_user_can_approve,
        );
        const target = targets.length === 1 ? targets[0] : undefined;
        if (target === undefined) {
          throw new AcceptanceFailure(
            "there is no uniquely approvable release-publish deployment",
          );
        }
        ghApi(
          "POST",
          `repos/${candidate.manifest.repository}/actions/runs/${candidate.manifest.workflowRunId}/pending_deployments`,
          {
            environment_ids: [target.environment.id],
            state: "approved",
            comment: expectedReceipt.value,
          },
        );
        const approvalExpectation = {
          candidate: projectAcceptanceCandidate(candidate.manifest),
          artifactId: verdict.value.candidateArtifact.id,
          artifactSha256: verdict.value.candidateArtifact.sha256,
          manifestSha256: candidate.manifestSha256,
        };
        let approvalReason =
          "GitHub did not expose the submitted acceptance receipt";
        let approved = false;
        for (let attempt = 0; attempt < 5; attempt += 1) {
          const approval = verifyAcceptanceApprovalHistory(
            ghApiJson(
              "GET",
              `repos/${candidate.manifest.repository}/actions/runs/${candidate.manifest.workflowRunId}/approvals`,
            ),
            approvalExpectation,
          );
          if (approval.ok) {
            approved = true;
            break;
          }
          approvalReason = approval.reason;
          await delay(1_000);
        }
        if (!approved) throw new AcceptanceFailure(approvalReason);
        process.stdout.write("approval=submitted\n");
      } finally {
        await evidence.close();
      }
    } finally {
      await sealed.close();
    }
  } finally {
    await candidate.close();
  }
}

async function readVerifiedEvidence(
  manifest: CandidateManifest,
  directory: string,
): Promise<VerifiedEvidenceSnapshot> {
  const snapshot = await snapshotReleaseFiles(
    directory,
    [
      "acceptance-session.json",
      "linux-observations.json",
      "android-observations.json",
      "android-key-continuity.json",
    ],
    { exact: true },
  );
  const paths = {
    session: join(snapshot.boundDirectory, "acceptance-session.json"),
    linux: join(snapshot.boundDirectory, "linux-observations.json"),
    android: join(snapshot.boundDirectory, "android-observations.json"),
    keyContinuity: join(snapshot.boundDirectory, "android-key-continuity.json"),
  };
  try {
    const verified = verifyReleaseAcceptance(
      {
        session: await readJson(paths.session),
        linux: await readJson(paths.linux),
        android: await readJson(paths.android),
        keyContinuity: await readJson(paths.keyContinuity),
      },
      manifest,
    );
    if (!verified.ok) throw new AcceptanceFailure(verified.reason);
    return { paths, verified: verified.value, close: () => snapshot.close() };
  } catch (error) {
    await snapshot.close();
    throw error;
  }
}

async function createVerdictText(
  candidate: VerifiedCandidate,
  evidence: VerifiedReleaseAcceptance,
  paths: EvidencePaths,
  artifactId: string,
  artifactSha256: string,
  sealedAt: string,
): Promise<string> {
  return jsonText({
    schemaVersion: 1,
    verdict: "evidence-complete",
    sealedAt,
    candidate: evidence.candidate,
    candidateArtifact: { id: artifactId, sha256: artifactSha256 },
    candidateManifestSha256: candidate.manifestSha256,
    evidenceSha256: {
      session: await sha256(paths.session),
      linux: await sha256(paths.linux),
      android: await sha256(paths.android),
      keyContinuity: await sha256(paths.keyContinuity),
    },
    evidenceRecordedAt: evidence.evidenceRecordedAt,
  });
}

async function loadVerifiedCandidate(
  input: string,
): Promise<VerifiedCandidate> {
  const requestedDirectory = repositoryPath(input);
  const discoveredNames = await readdir(requestedDirectory).catch(() => []);
  const snapshot = await snapshotReleaseFiles(
    requestedDirectory,
    discoveredNames,
    { exact: true },
  );
  try {
    const manifestPath = join(
      snapshot.boundDirectory,
      "candidate-manifest.json",
    );
    const raw = parseJsonText(
      (await readReleaseRegularFile(manifestPath)).toString("utf8"),
    );
    const seed = manifestSeedSchema.safeParse(raw);
    if (!seed.success)
      throw new AcceptanceFailure("the candidate manifest is invalid");
    const version = parseVersion(seed.data.version);
    const expectedNames = candidateEntryNames(version).sort();
    const actualNames = [...discoveredNames].sort();
    if (
      actualNames.length !== expectedNames.length ||
      !actualNames.every((name, index) => name === expectedNames[index])
    ) {
      throw new AcceptanceFailure(
        "the candidate bundle has unexpected or missing files",
      );
    }
    const manifest = parseCandidateManifest(
      raw,
      version,
      seed.data.tag,
      seed.data.commit,
    );
    if (!manifest.ok) throw new AcceptanceFailure(manifest.reason);
    if (
      manifest.value.repository !== seed.data.repository ||
      manifest.value.workflowRunId !== seed.data.workflowRunId ||
      manifest.value.workflowRunAttempt !== seed.data.workflowRunAttempt
    ) {
      throw new AcceptanceFailure(
        "the candidate manifest identity is inconsistent",
      );
    }
    const common = [
      "--version",
      version.text,
      "--directory",
      snapshot.directory,
      "--repository",
      manifest.value.repository,
      "--tag",
      manifest.value.tag,
      "--commit",
      manifest.value.commit,
    ];
    runReleaseContract([
      "verify-candidate",
      ...common,
      "--run-id",
      manifest.value.workflowRunId,
      "--run-attempt",
      String(manifest.value.workflowRunAttempt),
    ]);
    runReleaseContract(["verify-attestations", ...common]);
    runReleaseContract([
      "verify-appimage",
      "--version",
      version.text,
      "--artifact",
      join(snapshot.directory, manifest.value.artifacts.appImage.name),
    ]);
    runReleaseContract([
      "verify-apk",
      "--version",
      version.text,
      "--artifact",
      join(snapshot.directory, manifest.value.artifacts.apk.name),
    ]);
    return {
      directory: snapshot.boundDirectory,
      sourceDirectory: snapshot.sourceDirectory,
      manifest: manifest.value,
      manifestSha256: await sha256(manifestPath),
      close: () => snapshot.close(),
    };
  } catch (error) {
    await snapshot.close();
    throw error;
  }
}

function candidateEntryNames(version: ProductVersion): string[] {
  return [
    "CANDIDATE-ACCEPTANCE.md",
    "SHA256SUMS",
    "candidate-manifest.json",
    version.appImageName,
    `${version.appImageName}.sha256`,
    version.apkName,
    `${version.apkName}.sha256`,
  ];
}

async function verifyRemoteCandidateArtifact(
  manifest: CandidateManifest,
  artifactId: string,
  artifactSha256: string,
): Promise<void> {
  const currentRun = workflowRunSchema.safeParse(
    ghApiJson(
      "GET",
      `repos/${manifest.repository}/actions/runs/${manifest.workflowRunId}`,
    ),
  );
  if (
    !currentRun.success ||
    String(currentRun.data.id) !== manifest.workflowRunId ||
    currentRun.data.run_attempt !== manifest.workflowRunAttempt ||
    currentRun.data.head_sha !== manifest.commit ||
    currentRun.data.event !== "push" ||
    currentRun.data.status === "completed"
  ) {
    throw new AcceptanceFailure(
      "the candidate is not the current waiting workflow attempt",
    );
  }
  const artifactEndpoint = formatWorkflowRunArtifactsEndpoint(
    manifest.repository,
    manifest.workflowRunId,
  );
  if (!artifactEndpoint.ok)
    throw new AcceptanceFailure(artifactEndpoint.reason);
  const response = artifactResponseSchema.safeParse(
    ghApiJson("GET", artifactEndpoint.value),
  );
  if (!response.success)
    throw new AcceptanceFailure("GitHub returned invalid artifact data");
  const expectedName = `release-candidate-${manifest.workflowRunId}-${manifest.workflowRunAttempt}`;
  const matches = response.data.artifacts.filter(
    (artifact) =>
      String(artifact.id) === artifactId &&
      artifact.name === expectedName &&
      !artifact.expired &&
      parseArtifactDigestOptional(artifact.digest) === artifactSha256 &&
      String(artifact.workflow_run.id) === manifest.workflowRunId &&
      artifact.workflow_run.head_sha === manifest.commit,
  );
  if (matches.length !== 1) {
    throw new AcceptanceFailure(
      "the candidate artifact is not the exact live workflow output",
    );
  }
}

async function readTargetDevice(serial: string) {
  run("adb", ["-s", serial, "get-state"], "read connected Android device");
  const property = (name: string): string =>
    run(
      "adb",
      ["-s", serial, "shell", "getprop", name],
      "read Android device identity",
    ).stdout.trim();
  const device = verifyDeviceIdentity({
    manufacturer: property("ro.product.manufacturer"),
    model: property("ro.product.model"),
    apiLevel: property("ro.build.version.sdk"),
    primaryAbi: property("ro.product.cpu.abi"),
    androidRelease: property("ro.build.version.release"),
    kernelQemu: property("ro.kernel.qemu"),
    hardware: property("ro.hardware"),
    productName: property("ro.product.name"),
  });
  if (!device.ok) throw new AcceptanceFailure(device.reason);
  return device.value;
}

function installReleaseApk(serial: string, apk: string, label: string): void {
  const result = run(
    "adb",
    ["-s", serial, "install", "-r", "--no-streaming", apk],
    label,
    180_000,
  );
  if (!/Success/u.test(`${result.stdout}\n${result.stderr}`)) {
    throw new AcceptanceFailure(`${label} failed`);
  }
}

function readInstalledPackage(serial: string, version: ProductVersion) {
  const parsed = parseInstalledReleasePackage(
    run(
      "adb",
      ["-s", serial, "shell", "dumpsys", "package", PACKAGE_NAME],
      "read installed Sparrow identity",
    ).stdout,
    { versionName: version.text, versionCode: version.androidVersionCode },
  );
  if (!parsed.ok) throw new AcceptanceFailure(parsed.reason);
  return parsed.value;
}

function parseArguments(argv: readonly string[]): CliArguments {
  const command = argv[0];
  if (command === undefined)
    throw new AcceptanceFailure("a release-acceptance command is required");
  const values = new Map<string, string>();
  for (let index = 1; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (
      flag === undefined ||
      value === undefined ||
      !flag.startsWith("--") ||
      values.has(flag)
    ) {
      throw new AcceptanceFailure("arguments must be unique flag/value pairs");
    }
    values.set(flag, value);
  }
  return { command, values };
}

function requireExactFlags(
  values: ReadonlyMap<string, string>,
  flags: readonly string[],
): void {
  if (values.size !== flags.length || flags.some((flag) => !values.has(flag))) {
    throw new AcceptanceFailure(`expected exactly ${flags.join(", ")}`);
  }
}

function required(values: ReadonlyMap<string, string>, name: string): string {
  const value = values.get(name);
  if (value === undefined || value.length === 0)
    throw new AcceptanceFailure(`${name} is required`);
  return value;
}

function parseVersion(input: string): ProductVersion {
  const parsed = parseProductVersion(input);
  if (!parsed.ok) throw new AcceptanceFailure(parsed.reason);
  return parsed.value;
}

function parseAdbSerial(input: string): string {
  if (
    input.length === 0 ||
    input.length > 128 ||
    Array.from(input).some(
      (character) => /\s/u.test(character) || character.charCodeAt(0) <= 31,
    )
  ) {
    throw new AcceptanceFailure("the adb serial has an unsafe shape");
  }
  return input;
}

function parsePositiveIdentifier(input: string): string {
  if (!/^[1-9][0-9]*$/u.test(input) || !Number.isSafeInteger(Number(input))) {
    throw new AcceptanceFailure("the candidate artifact ID is invalid");
  }
  return input;
}

function parseArtifactDigest(input: string): string {
  const parsed = parseArtifactDigestOptional(input);
  if (parsed === undefined)
    throw new AcceptanceFailure("the candidate artifact digest is invalid");
  return parsed;
}

function parseArtifactDigestOptional(input: string | null): string | undefined {
  if (input === null) return undefined;
  const normalized = input.startsWith("sha256:")
    ? input.slice("sha256:".length)
    : input;
  return SHA256.test(normalized) ? normalized : undefined;
}

function repositoryPath(input: string): string {
  return isAbsolute(input) ? resolve(input) : resolve(REPOSITORY_ROOT, input);
}

function secureOutput(input: string, candidate: VerifiedCandidate) {
  return prepareReleaseOutput(repositoryPath(input), [
    candidate.sourceDirectory,
  ]);
}

function runReleaseContract(arguments_: readonly string[]): void {
  run(
    process.execPath,
    [RELEASE_CONTRACT, ...arguments_],
    "verify release candidate",
    180_000,
  );
}

function ghApiJson(
  method: "GET" | "POST",
  endpoint: string,
  body?: unknown,
): unknown {
  return parseJsonText(ghApi(method, endpoint, body));
}

function ghApi(
  method: "GET" | "POST",
  endpoint: string,
  body?: unknown,
): string {
  const arguments_ = [
    "api",
    "--method",
    method,
    "--header",
    "Accept: application/vnd.github+json",
    "--header",
    "X-GitHub-Api-Version: 2026-03-10",
    endpoint,
  ];
  if (body !== undefined) arguments_.push("--input", "-");
  const result = spawnSync("gh", arguments_, {
    cwd: REPOSITORY_ROOT,
    encoding: "utf8",
    input: body === undefined ? undefined : JSON.stringify(body),
    maxBuffer: COMMAND_OUTPUT_LIMIT,
    timeout: 60_000,
  });
  if (result.status !== 0)
    throw new AcceptanceFailure(
      "failed to read or review GitHub release state",
    );
  return result.stdout;
}

function run(
  command: string,
  arguments_: readonly string[],
  label: string,
  timeout = 60_000,
): CommandResult {
  const result = spawnSync(command, arguments_, {
    cwd: REPOSITORY_ROOT,
    encoding: "utf8",
    maxBuffer: COMMAND_OUTPUT_LIMIT,
    timeout,
  });
  if (result.status !== 0) throw new AcceptanceFailure(`failed to ${label}`);
  return { stdout: result.stdout, stderr: result.stderr };
}

async function sha256(path: string): Promise<string> {
  const hash = createHash("sha256");
  const stream = createReadStream(path);
  for await (const chunk of stream) hash.update(chunk);
  return hash.digest("hex");
}

function sha256Text(input: string): string {
  return createHash("sha256").update(input, "utf8").digest("hex");
}

async function readJson(path: string): Promise<unknown> {
  return parseJsonText((await readReleaseRegularFile(path)).toString("utf8"));
}

function parseJsonText(input: string): unknown {
  try {
    return JSON.parse(input) as unknown;
  } catch {
    throw new AcceptanceFailure("an acceptance JSON document is invalid");
  }
}

function jsonText(value: unknown): string {
  return `${JSON.stringify(value, null, 2)}\n`;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}

void main().catch((error: unknown) => {
  const message =
    error instanceof AcceptanceFailure ||
    error instanceof ReleaseFilesystemFailure
      ? error.message
      : "unexpected release acceptance failure";
  process.stderr.write(`release acceptance rejected: ${message}\n`);
  process.exitCode = 1;
});
