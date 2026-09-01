import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import {
  chmod,
  copyFile,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rename,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import { downloadPinnedAppImageTool } from "./appimage-tool-download.ts";
import {
  readReleaseRegularFile,
  snapshotReleaseFiles,
  ReleaseFilesystemFailure,
  type ReleaseDirectorySnapshot,
} from "./release-filesystem.ts";
import { HOSTED_REHEARSAL_FIXTURE_SHA256 } from "./hosted-cutover-domain.ts";
import {
  projectAcceptanceCandidate,
  verifyAcceptanceApprovalHistory,
} from "./release-acceptance-domain.ts";
import {
  ANDROID_ABIS,
  ANDROID_APPLICATION_ID,
  ANDROID_MIN_SDK,
  ANDROID_TARGET_SDK,
  formatAcceptanceManifest,
  formatChecksums,
  formatWorkflowRunArtifactsEndpoint,
  parseApkBadging,
  parseApkSignerCertificate,
  parseAppImageReleaseContract,
  parseCandidateManifest,
  parseCertificateSha256,
  parseChecksums,
  parseProductVersion,
  verifyActionPins,
  verifyAppImageBundleIcon,
  verifyEnvironmentProtection,
  verifyGradleDependencyLocking,
  verifyJustBoundaryRecipes,
  verifyRemoteReleaseRefs,
  verifyReleasePreflight,
  verifyReleaseWorkflowPreflightBoundary,
  type AppImageReleaseContract,
  type ProductVersion,
} from "./release-contract-domain.ts";

const REPOSITORY_ROOT = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const IDENTITY_PATH = join(
  REPOSITORY_ROOT,
  "release/android-signing-identity.json",
);
const APPIMAGE_RELEASE_PATH = join(REPOSITORY_ROOT, "release/appimage.json");
const TOOLCHAIN_PATH = join(REPOSITORY_ROOT, "release/toolchain.json");
const COMMAND_OUTPUT_LIMIT = 4 * 1024 * 1024;
const APPIMAGE_TOOL_SIZE_LIMIT = 64 * 1024 * 1024;
const APPIMAGE_TOOL_DOWNLOAD_TIMEOUT_MS = 60_000;
const APPIMAGE_SMOKE_MS = 8_000;
const LINUX_EXECUTABLE_NAME = "sparrow-installed";
const SHA256 = /^[0-9a-f]{64}$/u;

const packageSchema = z.object({ version: z.string() }).passthrough();
const identitySchema = z
  .object({
    schemaVersion: z.literal(1),
    certificateSha256: z.string().nullable(),
  })
  .strict();
const toolchainSchema = z
  .object({
    schemaVersion: z.literal(1),
    android: z
      .object({
        commandLineTools: z.literal("19.0"),
        platform: z.literal("android-36"),
        platformRevision: z.literal("2"),
        buildTools: z.literal("35.0.0"),
        ndk: z.literal("29.0.14206865"),
        minSdk: z.literal(24),
        targetSdk: z.literal(36),
        abis: z.tuple([
          z.literal("arm64-v8a"),
          z.literal("armeabi-v7a"),
          z.literal("x86"),
          z.literal("x86_64"),
        ]),
      })
      .strict(),
  })
  .strict();
const candidateArtifactResponseSchema = z
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
const workflowRunResponseSchema = z
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

interface CommandResult {
  readonly stdout: string;
  readonly stderr: string;
}

class ContractFailure extends Error {
  readonly _tag = "ContractFailure";
}

async function main(): Promise<void> {
  const arguments_ = parseArguments(process.argv.slice(2));
  switch (arguments_.command) {
    case "preflight":
      await preflight(arguments_.values);
      return;
    case "static-check":
      await staticCheck();
      return;
    case "prepare-appimage-tools":
      await prepareAppImageTools();
      return;
    case "stage":
      await stageCandidate(arguments_.values);
      return;
    case "verify-appimage":
      await verifyAppImage(arguments_.values);
      return;
    case "smoke-appimage":
      await smokeAppImage(arguments_.values);
      return;
    case "verify-apk":
      await verifyApk(arguments_.values);
      return;
    case "verify-signing-identity":
      await readSigningDigest();
      return;
    case "verify-environment-protection":
      verifyProtectedEnvironment(arguments_.values);
      return;
    case "verify-android-toolchain":
      await verifyAndroidToolchain();
      return;
    case "assemble":
      await assembleCandidate(arguments_.values);
      return;
    case "verify-candidate":
      await verifyCandidate(arguments_.values);
      return;
    case "verify-attestations":
      await verifyAttestations(arguments_.values);
      return;
    case "publish":
      await publish(arguments_.values);
      return;
  }
  throw new ContractFailure("unknown release-contract command");
}

async function preflight(values: ReadonlyMap<string, string>): Promise<void> {
  const mode = required(values, "--mode");
  const output = resolve(required(values, "--output"));
  const packageVersion = await readPackageVersion();
  const commit = required(values, "--commit");
  const result =
    mode === "tag"
      ? verifyReleasePreflight({
          mode: "tag",
          productVersion: packageVersion,
          refName: required(values, "--ref-name"),
          commit,
          masterCommit: required(values, "--master-commit"),
          existingTags: optional(values, "--existing-tags")?.split("\n") ?? [],
        })
      : mode === "rehearsal"
        ? verifyReleasePreflight({
            mode: "rehearsal",
            productVersion: packageVersion,
            requestedVersion: required(values, "--requested-version"),
            refName: required(values, "--ref-name"),
            commit,
            masterCommit: required(values, "--master-commit"),
          })
        : {
            ok: false as const,
            reason: "the release mode must be tag or rehearsal",
          };
  if (!result.ok) throw new ContractFailure(result.reason);
  await writeJson(output, result.value);
  await appendGithubOutput(values, {
    version: result.value.version,
    tag: result.value.tag,
    publishable: String(result.value.publishable),
    appimage_name: result.value.appImageName,
    apk_name: result.value.apkName,
  });
}

async function staticCheck(): Promise<void> {
  const workflowDirectory = join(REPOSITORY_ROOT, ".github/workflows");
  const workflowNames = (await readdir(workflowDirectory)).filter(
    (name) => name.endsWith(".yml") || name.endsWith(".yaml"),
  );
  const workflows: Record<string, string> = {};
  for (const name of workflowNames) {
    workflows[name] = await readFile(join(workflowDirectory, name), "utf8");
  }
  const actionPins = verifyActionPins(workflows);
  if (!actionPins.ok) throw new ContractFailure(actionPins.reason);
  const ci = workflows["ci.yml"];
  const release = workflows["release.yml"];
  if (ci === undefined || release === undefined) {
    throw new ContractFailure("both repository workflows must be present");
  }
  const preflightBoundary = verifyReleaseWorkflowPreflightBoundary(release);
  if (!preflightBoundary.ok)
    throw new ContractFailure(preflightBoundary.reason);
  if (
    /pull_request_target:/u.test(ci) ||
    /\$\{\{\s*secrets(?:\.|\[)/u.test(ci) ||
    /^\s*environment:/mu.test(ci) ||
    /:\s*write\s*$/mu.test(ci)
  ) {
    throw new ContractFailure(
      "ordinary CI must be unprivileged and secret-free",
    );
  }
  const justfile = await readFile(join(REPOSITORY_ROOT, "justfile"), "utf8");
  const justBoundary = verifyJustBoundaryRecipes(justfile);
  if (!justBoundary.ok) throw new ContractFailure(justBoundary.reason);
  const hostedCutoverFiles = [
    "deployment/hosted-contract.json",
    "app/scripts/hosted-cutover-domain.ts",
    "app/scripts/hosted-cutover.ts",
    "app/scripts/hosted-cutover-cli.test.ts",
    "scripts/verify-hosted-endpoint.sh",
    "scripts/rehearse-hosted-cutover.sh",
    "scripts/accept-hosted-candidate.sh",
    "scripts/hosted-rehearsal-fixture.py",
    "docs/deployment/hosted-hard-cutover.md",
  ];
  for (const path of hostedCutoverFiles) {
    const file = await stat(join(REPOSITORY_ROOT, path)).catch(() => undefined);
    if (file === undefined || !file.isFile() || file.size === 0) {
      throw new ContractFailure("the hosted hard-cutover contract is incomplete");
    }
  }
  const hostedAcceptanceRunner = await readFile(join(REPOSITORY_ROOT, "scripts/accept-hosted-candidate.sh"), "utf8");
  if (!hostedAcceptanceRunner.includes("docker_cmd network create --internal") ||
    hostedAcceptanceRunner.includes("host.docker.internal") || hostedAcceptanceRunner.includes(".env.local") ||
    hostedAcceptanceRunner.includes("--publish") || !hostedAcceptanceRunner.includes("SPARROW_HOSTED_RESOLVE_IP") ||
    !hostedAcceptanceRunner.includes('docker_cmd() { /usr/bin/docker --host "$docker_endpoint" "$@"; }') ||
    !hostedAcceptanceRunner.includes('with index .NetworkSettings.Networks \\"$network\\"') ||
    !hostedAcceptanceRunner.includes("the synthetic fixture did not become ready") ||
    !hostedAcceptanceRunner.includes('--user "$fixture_user"') ||
    !hostedAcceptanceRunner.includes("verify-hosted-endpoint.sh") || !hostedAcceptanceRunner.includes("--volumes") ||
    !hostedAcceptanceRunner.includes("--memory") || !hostedAcceptanceRunner.includes("Config.Volumes") ||
    !hostedAcceptanceRunner.includes("--env PASSWORD")) {
    throw new ContractFailure("hosted acceptance must stay synthetic, internal, and verifier-backed");
  }
  const cutoverRunner = await readFile(join(REPOSITORY_ROOT, "scripts/rehearse-hosted-cutover.sh"), "utf8");
  const endpointVerifier = await readFile(join(REPOSITORY_ROOT, "scripts/verify-hosted-endpoint.sh"), "utf8");
  if ([hostedAcceptanceRunner, cutoverRunner, endpointVerifier].some((runner) => runner.includes("iso-8601=milliseconds")) ||
    ![hostedAcceptanceRunner, cutoverRunner, endpointVerifier].every((runner) => runner.includes("new Date().toISOString()"))) {
    throw new ContractFailure("hosted evidence timestamps must use the pinned cross-platform runtime");
  }
  if (![hostedAcceptanceRunner, cutoverRunner].every((runner) => runner.includes('bun_path="$(mise which bun)"')) ||
    !endpointVerifier.includes('bun_path="${SPARROW_PINNED_BUN:-}"') || !endpointVerifier.includes('bun_path="$(mise which bun)"') ||
    !hostedAcceptanceRunner.includes('SPARROW_PINNED_BUN="$bun_path"') || !cutoverRunner.includes('SPARROW_PINNED_BUN="$bun_path"') ||
    !cutoverRunner.includes("docker_cmd network create --internal") || cutoverRunner.includes("host.docker.internal") ||
    !cutoverRunner.includes('docker_cmd() { /usr/bin/docker --host "$context_host" "$@"; }') ||
    !cutoverRunner.includes('with index .NetworkSettings.Networks \\"$network_name\\"') ||
    cutoverRunner.includes("--publish") || !cutoverRunner.includes("SPARROW_HOSTED_RESOLVE_IP") || !endpointVerifier.includes("--resolve") ||
    !cutoverRunner.includes("the synthetic fixture did not become ready") ||
    !cutoverRunner.includes('--user "$fixture_user"') ||
    !cutoverRunner.includes("--volumes") || !cutoverRunner.includes("Config.Volumes") || !cutoverRunner.includes("--memory") ||
    !endpointVerifier.includes("curl -q") || !endpointVerifier.includes("--noproxy '*'") || !endpointVerifier.includes("--max-redirs 0") ||
    !endpointVerifier.includes("--max-filesize") || !endpointVerifier.includes("https://tv.ponbac.xyz")) {
    throw new ContractFailure("hosted rehearsal and endpoint privacy boundaries are incomplete");
  }
  const hostedContract = await readFile(join(REPOSITORY_ROOT, "deployment/hosted-contract.json"), "utf8");
  const hostedFixture = await readFile(join(REPOSITORY_ROOT, "scripts/hosted-rehearsal-fixture.py"));
  if (!hostedContract.includes("docker.io/library/python:3.13.15-alpine3.24@sha256:540c7d91f98ff6880174c40e99067bf5941eb54d818a7a5e094d188b196a934d") ||
    !hostedContract.includes(HOSTED_REHEARSAL_FIXTURE_SHA256) ||
    createHash("sha256").update(hostedFixture).digest("hex") !== HOSTED_REHEARSAL_FIXTURE_SHA256 ||
    justfile.includes(".env.local") || cutoverRunner.includes("--network host") || hostedAcceptanceRunner.includes("--network host")) {
    throw new ContractFailure("the hosted fixture and network pins are incomplete");
  }
  for (const recipe of [
    "hosted-cutover-prepare:",
    "hosted-candidate-accept:",
    "hosted-shell-check:",
    "hosted-cutover-rehearse:",
    "hosted-cutover-readiness:",
    "hosted-cutover-seal:",
    "hosted-cutover-bind-route:",
    "hosted-cutover-observe-start:",
    "hosted-cutover-observe-finish:",
  ]) {
    if (!justfile.includes(recipe)) {
      throw new ContractFailure("the hosted hard-cutover Just interface is incomplete");
    }
  }
  if (
    !release.includes(`existing_release_tags="$(git tag --list 'v*')"`) ||
    !release.includes("bun run release:contract preflight")
  ) {
    throw new ContractFailure(
      "release preflight does not receive the fetched stable tags",
    );
  }
  const releaseInterface = `${release}\n${justfile}`;
  const requiredReleaseFragments = [
    "environment: release-signing",
    "environment: release-publish",
    "verify-environment-protection",
    "--environment release-signing",
    "--environment release-publish",
    "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')",
    "tauri build --bundles appimage",
    "tauri android build --apk --ci",
    "actions/attest-build-provenance@",
    "release-verify-candidate",
    "verify-attestations",
    "--run-attempt",
    "--artifact-id",
    "--artifact-digest",
    "release-acceptance-seal",
    "release-acceptance-approve",
    "artifact-digest: ${{ steps.upload.outputs.artifact-digest }}",
    '--artifact-id "${{ needs.candidate.outputs.artifact-id }}"',
    '--artifact-digest "${{ needs.candidate.outputs.artifact-digest }}"',
  ];
  if (
    requiredReleaseFragments.some(
      (fragment) => !releaseInterface.includes(fragment),
    )
  ) {
    throw new ContractFailure(
      "the release workflow is missing a required trust boundary",
    );
  }
  if (/--aab|updater/iu.test(releaseInterface)) {
    throw new ContractFailure(
      "the release workflow must not produce AAB or updater artifacts",
    );
  }
  const publishMarker = "\n  publish:";
  const publishOffset = release.indexOf(publishMarker);
  const publishSection = publishOffset < 0 ? "" : release.slice(publishOffset);
  if (
    publishSection.length === 0 ||
    /\b(?:just\s+build-|tauri(?:\s+android)?\s+build)\b/u.test(
      publishSection,
    ) ||
    (release.match(/^\s*contents:\s*write\s*$/gmu) ?? []).length !== 1 ||
    !/^\s*contents:\s*write\s*$/mu.test(publishSection)
  ) {
    throw new ContractFailure(
      "publication must be the only write-capable job and must not rebuild",
    );
  }
  const releaseContractSource = await readFile(
    fileURLToPath(import.meta.url),
    "utf8",
  );
  const releaseAcceptanceSource = await readFile(
    join(REPOSITORY_ROOT, "app/scripts/release-acceptance.ts"),
    "utf8",
  );
  const unsupportedAttemptPathSegment = ["/attempts", "/"].join("");
  if (
    `${releaseContractSource}\n${releaseAcceptanceSource}`.includes(
      unsupportedAttemptPathSegment,
    )
  ) {
    throw new ContractFailure(
      "release artifact lookup uses an unsupported attempt-scoped route",
    );
  }
  const publishFunctionOffset = releaseContractSource.indexOf(
    "async function publish(",
  );
  const approvalOffset = releaseContractSource.indexOf(
    "await verifyPublicationApproval(",
    publishFunctionOffset,
  );
  const releaseCreationOffset = releaseContractSource.indexOf(
    '"publish immutable GitHub Release"',
    approvalOffset,
  );
  if (
    publishFunctionOffset < 0 ||
    approvalOffset < publishFunctionOffset ||
    releaseCreationOffset < approvalOffset ||
    !releaseContractSource.includes("/approvals`")
  ) {
    throw new ContractFailure(
      "publication is not fail-closed on authenticated acceptance",
    );
  }

  for (const lockPath of [
    "Cargo.lock",
    "app/bun.lockb",
    "mise.lock",
    "app/src-tauri/gen/android/buildscript-gradle.lockfile",
    "app/src-tauri/gen/android/app/gradle.lockfile",
    "app/src-tauri/gen/android/buildSrc/gradle.lockfile",
  ]) {
    const lock = await stat(join(REPOSITORY_ROOT, lockPath)).catch(
      () => undefined,
    );
    if (lock === undefined || !lock.isFile() || lock.size === 0) {
      throw new ContractFailure(
        "a required repository dependency lock is missing",
      );
    }
  }
  const rustToolchain = await readFile(
    join(REPOSITORY_ROOT, "rust-toolchain.toml"),
    "utf8",
  );
  const requiredRustFragments = [
    'channel = "1.98.0"',
    '"aarch64-linux-android"',
    '"armv7-linux-androideabi"',
    '"i686-linux-android"',
    '"x86_64-linux-android"',
  ];
  if (
    requiredRustFragments.some((fragment) => !rustToolchain.includes(fragment))
  ) {
    throw new ContractFailure("the Rust release toolchain is not fully pinned");
  }

  const tauriConfig = z
    .object({
      version: z.literal("../package.json"),
      bundle: z
        .object({
          active: z.literal(true),
          icon: z.unknown(),
          linux: z.object({
            appimage: z.object({ bundleMediaFramework: z.literal(true) }),
          }),
        })
        .passthrough(),
    })
    .passthrough()
    .safeParse(
      await readJson(join(REPOSITORY_ROOT, "app/src-tauri/tauri.conf.json")),
    );
  if (!tauriConfig.success) {
    throw new ContractFailure(
      "Tauri packaging is not bound to the product version and media contract",
    );
  }
  const appImageRelease = await readAppImageReleaseContract();
  const iconPath = join(
    REPOSITORY_ROOT,
    "app/src-tauri",
    appImageRelease.icon.path,
  );
  const iconFile = await lstat(iconPath).catch(() => undefined);
  const iconBytes =
    iconFile?.isFile() === true
      ? await readFile(iconPath).catch(() => undefined)
      : undefined;
  const iconDigest =
    iconBytes === undefined ? undefined : await sha256(iconPath);
  const icon = verifyAppImageBundleIcon(
    tauriConfig.data.bundle.icon,
    appImageRelease.icon,
    iconBytes,
    iconDigest,
  );
  if (!icon.ok) throw new ContractFailure(icon.reason);
  const prepareOffset = justfile.indexOf(
    "bun run release:contract prepare-appimage-tools",
  );
  const bundleOffset = justfile.indexOf(
    "bun run tauri build --bundles appimage",
  );
  if (prepareOffset < 0 || bundleOffset < 0 || prepareOffset > bundleOffset) {
    throw new ContractFailure(
      "the AppImage helper cache is not verified before Tauri bundling",
    );
  }
  if (
    !release.includes(
      "XDG_CACHE_HOME: ${{ runner.temp }}/sparrow-appimage-cache",
    )
  ) {
    throw new ContractFailure(
      "the release AppImage helper cache is not job-private",
    );
  }
  parseToolchain(await readJson(TOOLCHAIN_PATH));
  identitySchema.parse(await readJson(IDENTITY_PATH));
  const androidBuild = await readFile(
    join(REPOSITORY_ROOT, "app/src-tauri/gen/android/app/build.gradle.kts"),
    "utf8",
  );
  const requiredAndroidFragments = [
    "compileSdk = 36",
    'buildToolsVersion = "35.0.0"',
    'ndkVersion = "29.0.14206865"',
    "minSdk = 24",
    "targetSdk = 36",
    "gradle.startParameter.taskNames",
    "requestedTask.substringAfterLast(':')",
  ];
  if (
    requiredAndroidFragments.some(
      (fragment) => !androidBuild.includes(fragment),
    )
  ) {
    throw new ContractFailure(
      "the Android build does not match release/toolchain.json",
    );
  }
  const gradleLocking = verifyGradleDependencyLocking({
    androidRootBuild: await readFile(
      join(REPOSITORY_ROOT, "app/src-tauri/gen/android/build.gradle.kts"),
      "utf8",
    ),
    appBuild: androidBuild,
    buildSrcBuild: await readFile(
      join(
        REPOSITORY_ROOT,
        "app/src-tauri/gen/android/buildSrc/build.gradle.kts",
      ),
      "utf8",
    ),
    androidBuildscriptLock: await readFile(
      join(
        REPOSITORY_ROOT,
        "app/src-tauri/gen/android/buildscript-gradle.lockfile",
      ),
      "utf8",
    ),
    appLock: await readFile(
      join(REPOSITORY_ROOT, "app/src-tauri/gen/android/app/gradle.lockfile"),
      "utf8",
    ),
    buildSrcLock: await readFile(
      join(
        REPOSITORY_ROOT,
        "app/src-tauri/gen/android/buildSrc/gradle.lockfile",
      ),
      "utf8",
    ),
  });
  if (!gradleLocking.ok) throw new ContractFailure(gradleLocking.reason);
  const wrapper = await readFile(
    join(
      REPOSITORY_ROOT,
      "app/src-tauri/gen/android/gradle/wrapper/gradle-wrapper.properties",
    ),
    "utf8",
  );
  if (
    !wrapper.includes("gradle-8.14.3-bin.zip") ||
    !wrapper.includes(
      "distributionSha256Sum=bd71102213493060956ec229d946beee57158dbd89d0e62b91bca0fa2c5f3531",
    )
  ) {
    throw new ContractFailure(
      "the Gradle wrapper distribution is not checksum-pinned",
    );
  }
  const product = parseProductVersion(await readPackageVersion());
  if (!product.ok) throw new ContractFailure(product.reason);
}

async function prepareAppImageTools(): Promise<void> {
  const contract = await readAppImageReleaseContract();
  const cacheDirectory = appImageToolCacheDirectory();
  await mkdir(cacheDirectory, { recursive: true });
  for (const tool of contract.tools) {
    await prepareAppImageTool(cacheDirectory, tool);
  }
}

async function prepareAppImageTool(
  cacheDirectory: string,
  tool: AppImageReleaseContract["tools"][number],
): Promise<void> {
  const destination = join(cacheDirectory, tool.cacheName);
  const existing = await lstat(destination).catch((error: unknown) => {
    if (errorHasCode(error, "ENOENT")) return undefined;
    throw new ContractFailure("the AppImage helper cache cannot be inspected");
  });
  if (existing !== undefined && !existing.isFile()) {
    throw new ContractFailure(
      "an AppImage helper cache entry is not a regular file",
    );
  }
  if (existing !== undefined && (await sha256(destination)) === tool.sha256) {
    await chmod(destination, 0o770);
    return;
  }

  const temporaryDirectory = await mkdtemp(
    join(cacheDirectory, ".sparrow-appimage-tool-"),
  );
  const temporary = join(temporaryDirectory, tool.cacheName);
  try {
    const download = await downloadPinnedAppImageTool({
      url: tool.url,
      temporaryPath: temporary,
      expectedSha256: tool.sha256,
      maximumBytes: APPIMAGE_TOOL_SIZE_LIMIT,
      timeoutMs: APPIMAGE_TOOL_DOWNLOAD_TIMEOUT_MS,
    });
    if (!download.ok) {
      switch (download.reason) {
        case "download-failed":
          throw new ContractFailure(
            "a pinned AppImage helper could not be downloaded",
          );
        case "invalid-size":
          throw new ContractFailure(
            "a pinned AppImage helper has an invalid size",
          );
        case "digest-mismatch":
          throw new ContractFailure(
            "a downloaded AppImage helper does not match its SHA-256 pin",
          );
      }
    }
    if ((await sha256(temporary)) !== tool.sha256) {
      throw new ContractFailure(
        "an AppImage helper changed while being written to disk",
      );
    }
    await chmod(temporary, 0o770);
    await rename(temporary, destination);
    if ((await sha256(destination)) !== tool.sha256) {
      throw new ContractFailure(
        "an AppImage helper changed while populating the cache",
      );
    }
  } finally {
    await rm(temporaryDirectory, { recursive: true, force: true });
  }
}

function appImageToolCacheDirectory(): string {
  const configured = process.env.XDG_CACHE_HOME;
  const cacheRoot =
    configured === undefined || configured.length === 0
      ? join(homedir(), ".cache")
      : configured;
  if (!isAbsolute(cacheRoot)) {
    throw new ContractFailure(
      "XDG_CACHE_HOME must be absolute for AppImage packaging",
    );
  }
  return join(cacheRoot, "tauri");
}

function verifyProtectedEnvironment(values: ReadonlyMap<string, string>): void {
  const repository = parseRepository(required(values, "--repository"));
  const environment = parseReleaseEnvironment(
    required(values, "--environment"),
  );
  const environmentResponse = run(
    "gh",
    [
      "api",
      "--method",
      "GET",
      "--header",
      "Accept: application/vnd.github+json",
      "--header",
      "X-GitHub-Api-Version: 2026-03-10",
      `repos/${repository}/environments/${environment}`,
    ],
    "read protected GitHub release environment",
  );
  const policyResponse = run(
    "gh",
    [
      "api",
      "--method",
      "GET",
      "--header",
      "Accept: application/vnd.github+json",
      "--header",
      "X-GitHub-Api-Version: 2026-03-10",
      `repos/${repository}/environments/${environment}/deployment-branch-policies?per_page=100`,
    ],
    "read GitHub release environment deployment policies",
  );
  const protection = verifyEnvironmentProtection(
    parseJsonText(environmentResponse.stdout),
    parseJsonText(policyResponse.stdout),
    environment,
    repository.split("/")[0] ?? "",
  );
  if (!protection.ok) throw new ContractFailure(protection.reason);
}

async function stageCandidate(
  values: ReadonlyMap<string, string>,
): Promise<void> {
  const kind = required(values, "--kind");
  if (kind !== "appimage" && kind !== "apk") {
    throw new ContractFailure("candidate kind must be appimage or apk");
  }
  const version = parseVersion(required(values, "--version"));
  const input = resolve(required(values, "--input"));
  const output = resolve(required(values, "--output"));
  const files = await walkFiles(input);
  if (files.some((path) => path.endsWith(".aab"))) {
    throw new ContractFailure(
      "AAB output is forbidden by the release contract",
    );
  }
  const matches = files.filter((path) =>
    kind === "appimage" ? path.endsWith(".AppImage") : path.endsWith(".apk"),
  );
  if (matches.length !== 1) {
    throw new ContractFailure(
      `the ${kind} build must produce exactly one candidate`,
    );
  }
  const source = matches[0];
  if (source === undefined)
    throw new ContractFailure("candidate output disappeared");
  const sourceName = basename(source).toLowerCase();
  if (kind === "appimage" && !sourceName.includes(version.text.toLowerCase())) {
    throw new ContractFailure(
      "the build output filename does not contain the product version",
    );
  }
  if (
    kind === "apk" &&
    (!sourceName.includes("universal") || !sourceName.includes("release"))
  ) {
    throw new ContractFailure(
      "the Android build output is not a universal release APK",
    );
  }
  if (kind === "apk" && sourceName.includes("unsigned")) {
    throw new ContractFailure("the Android release APK is unsigned");
  }
  await mkdir(output, { recursive: true });
  const destination = join(
    output,
    kind === "appimage" ? version.appImageName : version.apkName,
  );
  await copyFile(source, destination);
  if (kind === "appimage") await chmod(destination, 0o755);
  const digest = await sha256(destination);
  await writeFile(
    `${destination}.sha256`,
    `${digest}  ${basename(destination)}\n`,
    "utf8",
  );
}

async function verifyAppImage(
  values: ReadonlyMap<string, string>,
): Promise<void> {
  const version = parseVersion(required(values, "--version"));
  const artifact = resolve(required(values, "--artifact"));
  if (basename(artifact) !== version.appImageName) {
    throw new ContractFailure(
      "the AppImage filename does not match the product version",
    );
  }
  const snapshot = await snapshotReleaseFiles(
    dirname(artifact),
    [basename(artifact), `${basename(artifact)}.sha256`],
    { exact: false },
  );
  try {
    await verifyAppImageFile(join(snapshot.boundDirectory, basename(artifact)));
  } finally {
    await snapshot.close();
  }
}

async function verifyAppImageFile(artifact: string): Promise<void> {
  const file = run(
    "file",
    ["--brief", artifact],
    "inspect AppImage architecture",
  );
  if (!/ELF 64-bit LSB.*x86-64/iu.test(file.stdout)) {
    throw new ContractFailure("the AppImage is not an x86_64 ELF executable");
  }
  await verifySidecar(artifact);
  await chmod(artifact, 0o755);
  const extraction = await mkdtemp(join(tmpdir(), "sparrow-appimage-"));
  try {
    run(artifact, ["--appimage-extract"], "extract AppImage", {
      cwd: extraction,
      timeout: 60_000,
    });
    const root = join(extraction, "squashfs-root");
    const desktopFiles = (await walkFiles(root)).filter((path) =>
      path.endsWith(".desktop"),
    );
    if (desktopFiles.length !== 1) {
      throw new ContractFailure(
        "the AppImage must contain exactly one desktop entry",
      );
    }
    const desktop = await readFile(desktopFiles[0] ?? "", "utf8");
    if (
      !/^Name=Sparrow$/mu.test(desktop) ||
      !new RegExp(`^Exec=${LINUX_EXECUTABLE_NAME}(?:\\s.*)?$`, "mu").test(
        desktop,
      )
    ) {
      throw new ContractFailure(
        "the AppImage desktop identity does not match Sparrow",
      );
    }
    const appRun = await stat(join(root, "AppRun"));
    if (!appRun.isFile() || (appRun.mode & 0o111) === 0) {
      throw new ContractFailure(
        "the AppImage has no executable AppRun entrypoint",
      );
    }
  } finally {
    await rm(extraction, { recursive: true, force: true });
  }
}

async function smokeAppImage(
  values: ReadonlyMap<string, string>,
): Promise<void> {
  const version = parseVersion(required(values, "--version"));
  const artifact = resolve(required(values, "--artifact"));
  if (basename(artifact) !== version.appImageName) {
    throw new ContractFailure(
      "the AppImage filename does not match the product version",
    );
  }
  await chmod(artifact, 0o755);
  const privateRoot = await mkdtemp(join(tmpdir(), "sparrow-appimage-smoke-"));
  const launchEnvironment = { ...process.env };
  delete launchEnvironment.WEBKIT_DISABLE_DMABUF_RENDERER;
  delete launchEnvironment.WEBKIT_DMABUF_RENDERER_FORCE_SHM;
  const child = spawn(artifact, [], {
    env: {
      ...launchEnvironment,
      APPIMAGE_EXTRACT_AND_RUN: "1",
      XDG_CACHE_HOME: join(privateRoot, "cache"),
      XDG_CONFIG_HOME: join(privateRoot, "config"),
      XDG_DATA_HOME: join(privateRoot, "data"),
    },
    stdio: "ignore",
  });
  try {
    await waitForLaunchWindow(child, APPIMAGE_SMOKE_MS);
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new ContractFailure(
        "the AppImage exited during the bounded launch smoke check",
      );
    }
  } finally {
    child.kill("SIGTERM");
    await waitForExitWindow(child, 2_000);
    if (child.exitCode === null && child.signalCode === null)
      child.kill("SIGKILL");
    await rm(privateRoot, { recursive: true, force: true });
  }
}

async function verifyApk(values: ReadonlyMap<string, string>): Promise<void> {
  const version = parseVersion(required(values, "--version"));
  const artifact = resolve(required(values, "--artifact"));
  if (basename(artifact) !== version.apkName) {
    throw new ContractFailure(
      "the APK filename does not match the product version",
    );
  }
  const snapshot = await snapshotReleaseFiles(
    dirname(artifact),
    [basename(artifact), `${basename(artifact)}.sha256`],
    { exact: false },
  );
  try {
    await verifyApkFile(
      join(snapshot.boundDirectory, basename(artifact)),
      version,
    );
  } finally {
    await snapshot.close();
  }
}

async function verifyApkFile(
  artifact: string,
  version: ProductVersion,
): Promise<void> {
  await verifySidecar(artifact);
  const digest = await readSigningDigest();
  const tools = await androidTools();
  const badging = run(
    tools.aapt2,
    ["dump", "badging", artifact],
    "inspect APK package",
  );
  const identity = parseApkBadging(badging.stdout, version);
  if (!identity.ok) throw new ContractFailure(identity.reason);
  const signature = run(
    tools.apksigner,
    [
      "verify",
      "--verbose",
      "--print-certs",
      "--min-sdk-version",
      "24",
      artifact,
    ],
    "verify APK signature",
  );
  const certificate = parseApkSignerCertificate(
    `${signature.stdout}\n${signature.stderr}`,
  );
  if (!certificate.ok) throw new ContractFailure(certificate.reason);
  if (certificate.value !== digest) {
    throw new ContractFailure(
      "the APK signing certificate does not match the recorded identity",
    );
  }
}

async function verifyAndroidToolchain(): Promise<void> {
  const config = parseToolchain(await readJson(TOOLCHAIN_PATH));
  const androidHome = androidHomePath();
  const packageFiles = [
    [
      join(
        androidHome,
        `cmdline-tools/${config.android.commandLineTools}/package.xml`,
      ),
      `cmdline-tools;${config.android.commandLineTools}`,
      config.android.commandLineTools,
    ],
    [
      join(androidHome, `platforms/${config.android.platform}/package.xml`),
      `platforms;${config.android.platform}`,
      config.android.platformRevision,
    ],
    [
      join(androidHome, `build-tools/${config.android.buildTools}/package.xml`),
      `build-tools;${config.android.buildTools}`,
      config.android.buildTools,
    ],
    [
      join(androidHome, `ndk/${config.android.ndk}/package.xml`),
      `ndk;${config.android.ndk}`,
      config.android.ndk,
    ],
  ] as const;
  for (const [path, packageId, revision] of packageFiles) {
    const contents = await readFile(path, "utf8").catch(() => "");
    if (
      !contents.includes(`localPackage path="${packageId}"`) ||
      parseAndroidRevision(contents) !== normalizeAndroidRevision(revision)
    ) {
      throw new ContractFailure(
        "the installed Android SDK does not match release/toolchain.json",
      );
    }
  }
}

async function assembleCandidate(
  values: ReadonlyMap<string, string>,
): Promise<void> {
  const version = parseVersion(required(values, "--version"));
  const directory = resolve(required(values, "--directory"));
  const commit = parseCommit(required(values, "--commit"));
  const tag = parseReleaseTag(required(values, "--tag"), version);
  const repository = parseRepository(required(values, "--repository"));
  const workflowRunId = parseWorkflowRunId(required(values, "--run-id"));
  const workflowRunAttempt = parseWorkflowRunAttempt(
    required(values, "--run-attempt"),
  );
  const certificateSha256 = await readSigningDigest();
  const appImage = join(directory, version.appImageName);
  const apk = join(directory, version.apkName);
  await verifySidecar(appImage);
  await verifySidecar(apk);
  const digests = {
    appImage: { name: version.appImageName, sha256: await sha256(appImage) },
    apk: { name: version.apkName, sha256: await sha256(apk) },
  };
  const checksums = formatChecksums(digests);
  if (!checksums.ok) throw new ContractFailure(checksums.reason);
  await writeFile(join(directory, "SHA256SUMS"), checksums.value, "utf8");
  const rawManifest = {
    schemaVersion: 1,
    version: version.text,
    tag,
    commit,
    repository,
    workflowRunId,
    workflowRunAttempt,
    publishable: true,
    artifacts: digests,
    android: {
      applicationId: ANDROID_APPLICATION_ID,
      versionName: version.text,
      versionCode: version.androidVersionCode,
      minSdk: ANDROID_MIN_SDK,
      targetSdk: ANDROID_TARGET_SDK,
      abis: ANDROID_ABIS,
      certificateSha256,
    },
  };
  const manifest = parseCandidateManifest(rawManifest, version, tag, commit);
  if (!manifest.ok) throw new ContractFailure(manifest.reason);
  await writeJson(join(directory, "candidate-manifest.json"), manifest.value);
  await writeFile(
    join(directory, "CANDIDATE-ACCEPTANCE.md"),
    formatAcceptanceManifest(manifest.value),
    "utf8",
  );
}

async function verifyCandidate(
  values: ReadonlyMap<string, string>,
): Promise<void> {
  const version = parseVersion(required(values, "--version"));
  const requestedDirectory = resolve(required(values, "--directory"));
  const repository = parseRepository(required(values, "--repository"));
  const tag = parseReleaseTag(required(values, "--tag"), version);
  const commit = parseCommit(required(values, "--commit"));
  const workflowRunId = parseWorkflowRunId(required(values, "--run-id"));
  const workflowRunAttempt = parseWorkflowRunAttempt(
    required(values, "--run-attempt"),
  );
  const snapshot = await snapshotCandidate(requestedDirectory, version);
  try {
    await verifyCandidateDirectory(snapshot.boundDirectory, {
      version,
      repository,
      tag,
      commit,
      workflowRunId,
      workflowRunAttempt,
    });
  } finally {
    await snapshot.close();
  }
}

function candidateEntryNames(version: ProductVersion): readonly string[] {
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

function snapshotCandidate(
  directory: string,
  version: ProductVersion,
): Promise<ReleaseDirectorySnapshot> {
  return snapshotReleaseFiles(directory, candidateEntryNames(version), {
    exact: true,
  });
}

async function verifyCandidateDirectory(
  directory: string,
  expected: {
    readonly version: ProductVersion;
    readonly repository: string;
    readonly tag: string;
    readonly commit: string;
    readonly workflowRunId: string;
    readonly workflowRunAttempt: number;
  },
): Promise<void> {
  const {
    version,
    repository,
    tag,
    commit,
    workflowRunId,
    workflowRunAttempt,
  } = expected;
  const appImage = join(directory, version.appImageName);
  const apk = join(directory, version.apkName);
  await verifySidecar(appImage);
  await verifySidecar(apk);
  const checksums = parseChecksums(
    (await readReleaseRegularFile(join(directory, "SHA256SUMS"))).toString(
      "utf8",
    ),
    version,
  );
  if (!checksums.ok) throw new ContractFailure(checksums.reason);
  if (
    checksums.value.appImage.sha256 !== (await sha256(appImage)) ||
    checksums.value.apk.sha256 !== (await sha256(apk))
  ) {
    throw new ContractFailure("candidate bytes do not match SHA256SUMS");
  }
  const manifest = parseCandidateManifest(
    parseJsonText(
      (
        await readReleaseRegularFile(join(directory, "candidate-manifest.json"))
      ).toString("utf8"),
    ),
    version,
    tag,
    commit,
  );
  if (!manifest.ok) throw new ContractFailure(manifest.reason);
  if (
    manifest.value.repository !== repository ||
    manifest.value.workflowRunId !== workflowRunId ||
    manifest.value.workflowRunAttempt !== workflowRunAttempt
  ) {
    throw new ContractFailure(
      "the candidate manifest does not match the release invocation",
    );
  }
  if (
    manifest.value.artifacts.appImage.sha256 !==
      checksums.value.appImage.sha256 ||
    manifest.value.artifacts.apk.sha256 !== checksums.value.apk.sha256
  ) {
    throw new ContractFailure("the candidate manifest and SHA256SUMS disagree");
  }
  if (
    manifest.value.android.certificateSha256 !== (await readSigningDigest())
  ) {
    throw new ContractFailure(
      "the candidate manifest has the wrong Android signing identity",
    );
  }
  const acceptance = (
    await readReleaseRegularFile(join(directory, "CANDIDATE-ACCEPTANCE.md"))
  ).toString("utf8");
  if (acceptance !== formatAcceptanceManifest(manifest.value)) {
    throw new ContractFailure(
      "the candidate acceptance instructions do not match the manifest",
    );
  }
}

async function verifyAttestations(
  values: ReadonlyMap<string, string>,
): Promise<void> {
  const version = parseVersion(required(values, "--version"));
  const requestedDirectory = resolve(required(values, "--directory"));
  const repository = parseRepository(required(values, "--repository"));
  const commit = parseCommit(required(values, "--commit"));
  const tag = parseReleaseTag(required(values, "--tag"), version);
  const snapshot = await snapshotReleaseFiles(
    requestedDirectory,
    [version.appImageName, version.apkName],
    { exact: false },
  );
  try {
    for (const name of [version.appImageName, version.apkName]) {
      run(
        "gh",
        [
          "attestation",
          "verify",
          join(snapshot.boundDirectory, name),
          "--repo",
          repository,
          "--signer-workflow",
          `${repository}/.github/workflows/release.yml`,
          "--signer-digest",
          commit,
          "--source-digest",
          commit,
          "--source-ref",
          `refs/tags/${tag}`,
          "--deny-self-hosted-runners",
        ],
        "verify candidate provenance",
      );
    }
  } finally {
    await snapshot.close();
  }
}

async function publish(values: ReadonlyMap<string, string>): Promise<void> {
  const version = parseVersion(required(values, "--version"));
  const requestedDirectory = resolve(required(values, "--directory"));
  const repository = parseRepository(required(values, "--repository"));
  const tag = parseReleaseTag(required(values, "--tag"), version);
  const commit = parseCommit(required(values, "--commit"));
  const workflowRunId = parseWorkflowRunId(required(values, "--run-id"));
  const workflowRunAttempt = parseWorkflowRunAttempt(
    required(values, "--run-attempt"),
  );
  const artifactId = parseWorkflowRunId(required(values, "--artifact-id"));
  const artifactSha256 = parseArtifactDigest(
    required(values, "--artifact-digest"),
  );
  const snapshot = await snapshotCandidate(requestedDirectory, version);
  try {
    const directory = snapshot.boundDirectory;
    await verifyCandidateDirectory(directory, {
      version,
      repository,
      tag,
      commit,
      workflowRunId,
      workflowRunAttempt,
    });
    await verifyPublicationApproval({
      directory,
      repository,
      version,
      tag,
      commit,
      workflowRunId,
      workflowRunAttempt,
      artifactId,
      artifactSha256,
    });
    const remoteReferences = run(
      "git",
      [
        "ls-remote",
        "--exit-code",
        "origin",
        "refs/heads/master",
        `refs/tags/${tag}`,
        `refs/tags/${tag}^{}`,
      ],
      "verify immutable remote release references",
      { cwd: REPOSITORY_ROOT },
    );
    const remoteProof = verifyRemoteReleaseRefs(
      remoteReferences.stdout,
      tag,
      commit,
    );
    if (!remoteProof.ok) throw new ContractFailure(remoteProof.reason);
    const existing = spawnSync(
      "gh",
      ["release", "view", tag, "--repo", repository],
      {
        encoding: "utf8",
        maxBuffer: COMMAND_OUTPUT_LIMIT,
      },
    );
    if (existing.status === 0) {
      throw new ContractFailure(
        "a GitHub Release already exists for this immutable tag",
      );
    }
    run(
      "gh",
      [
        "release",
        "create",
        tag,
        join(directory, version.appImageName),
        join(directory, version.apkName),
        join(directory, "SHA256SUMS"),
        "--repo",
        repository,
        "--verify-tag",
        "--title",
        `Sparrow ${version.text}`,
        "--generate-notes",
        "--fail-on-no-commits",
      ],
      "publish immutable GitHub Release",
    );
  } finally {
    await snapshot.close();
  }
}

async function verifyPublicationApproval(input: {
  readonly directory: string;
  readonly repository: string;
  readonly version: ProductVersion;
  readonly tag: string;
  readonly commit: string;
  readonly workflowRunId: string;
  readonly workflowRunAttempt: number;
  readonly artifactId: string;
  readonly artifactSha256: string;
}): Promise<void> {
  const runResponse = run(
    "gh",
    [
      "api",
      "--method",
      "GET",
      "--header",
      "Accept: application/vnd.github+json",
      "--header",
      "X-GitHub-Api-Version: 2026-03-10",
      `repos/${input.repository}/actions/runs/${input.workflowRunId}`,
    ],
    "read current release workflow run",
  );
  const currentRun = workflowRunResponseSchema.safeParse(
    parseJsonText(runResponse.stdout),
  );
  if (
    !currentRun.success ||
    String(currentRun.data.id) !== input.workflowRunId ||
    currentRun.data.run_attempt !== input.workflowRunAttempt ||
    currentRun.data.head_sha !== input.commit ||
    currentRun.data.event !== "push" ||
    currentRun.data.status === "completed"
  ) {
    throw new ContractFailure(
      "publication is not running in the accepted workflow attempt",
    );
  }
  const artifactEndpoint = formatWorkflowRunArtifactsEndpoint(
    input.repository,
    input.workflowRunId,
  );
  if (!artifactEndpoint.ok) throw new ContractFailure(artifactEndpoint.reason);
  const artifactResponse = run(
    "gh",
    [
      "api",
      "--method",
      "GET",
      "--header",
      "Accept: application/vnd.github+json",
      "--header",
      "X-GitHub-Api-Version: 2026-03-10",
      artifactEndpoint.value,
    ],
    "read exact release candidate artifact",
  );
  const artifacts = candidateArtifactResponseSchema.safeParse(
    parseJsonText(artifactResponse.stdout),
  );
  if (!artifacts.success)
    throw new ContractFailure(
      "GitHub returned invalid candidate artifact data",
    );
  const expectedName = `release-candidate-${input.workflowRunId}-${input.workflowRunAttempt}`;
  const matchingArtifacts = artifacts.data.artifacts.filter(
    (artifact) =>
      String(artifact.id) === input.artifactId &&
      artifact.name === expectedName &&
      !artifact.expired &&
      parseOptionalArtifactDigest(artifact.digest) === input.artifactSha256 &&
      String(artifact.workflow_run.id) === input.workflowRunId &&
      artifact.workflow_run.head_sha === input.commit,
  );
  if (matchingArtifacts.length !== 1) {
    throw new ContractFailure(
      "the approved candidate artifact identity is invalid",
    );
  }

  const manifestPath = join(input.directory, "candidate-manifest.json");
  const manifest = parseCandidateManifest(
    await readJson(manifestPath),
    input.version,
    input.tag,
    input.commit,
  );
  if (!manifest.ok) throw new ContractFailure(manifest.reason);
  const reviewHistory = run(
    "gh",
    [
      "api",
      "--method",
      "GET",
      "--header",
      "Accept: application/vnd.github+json",
      "--header",
      "X-GitHub-Api-Version: 2026-03-10",
      `repos/${input.repository}/actions/runs/${input.workflowRunId}/approvals`,
    ],
    "read release publication approval",
  );
  const approval = verifyAcceptanceApprovalHistory(
    parseJsonText(reviewHistory.stdout),
    {
      candidate: projectAcceptanceCandidate(manifest.value),
      artifactId: input.artifactId,
      artifactSha256: input.artifactSha256,
      manifestSha256: await sha256(manifestPath),
    },
  );
  if (!approval.ok) throw new ContractFailure(approval.reason);
}

function parseArguments(argv: readonly string[]): CliArguments {
  const command = argv[0];
  if (command === undefined)
    throw new ContractFailure("a release-contract command is required");
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
      throw new ContractFailure(
        "release-contract arguments must be unique flag/value pairs",
      );
    }
    values.set(flag, value);
  }
  return { command, values };
}

function required(values: ReadonlyMap<string, string>, name: string): string {
  const value = values.get(name);
  if (value === undefined || value.length === 0)
    throw new ContractFailure(`${name} is required`);
  return value;
}

function optional(
  values: ReadonlyMap<string, string>,
  name: string,
): string | undefined {
  return values.get(name);
}

function parseVersion(input: string): ProductVersion {
  const parsed = parseProductVersion(input);
  if (!parsed.ok) throw new ContractFailure(parsed.reason);
  return parsed.value;
}

function parseReleaseTag(input: string, version: ProductVersion): string {
  if (input !== version.tag) {
    throw new ContractFailure(
      "the release tag does not match the product version",
    );
  }
  return input;
}

function parseCommit(input: string): string {
  if (!/^[0-9a-f]{40}$/u.test(input)) {
    throw new ContractFailure(
      "the release commit must be a full lowercase Git SHA",
    );
  }
  return input;
}

function parseRepository(input: string): string {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/u.test(input)) {
    throw new ContractFailure("the release repository identity is invalid");
  }
  return input;
}

function parseReleaseEnvironment(
  input: string,
): "release-signing" | "release-publish" {
  if (input !== "release-signing" && input !== "release-publish") {
    throw new ContractFailure("the release environment name is invalid");
  }
  return input;
}

function parseWorkflowRunId(input: string): string {
  if (!/^[1-9][0-9]*$/u.test(input)) {
    throw new ContractFailure("the workflow run ID is invalid");
  }
  return input;
}

function parseWorkflowRunAttempt(input: string): number {
  if (!/^[1-9][0-9]*$/u.test(input)) {
    throw new ContractFailure("the workflow run attempt is invalid");
  }
  const attempt = Number(input);
  if (!Number.isSafeInteger(attempt)) {
    throw new ContractFailure("the workflow run attempt is invalid");
  }
  return attempt;
}

function parseArtifactDigest(input: string): string {
  const parsed = parseOptionalArtifactDigest(input);
  if (parsed === undefined)
    throw new ContractFailure("the candidate artifact digest is invalid");
  return parsed;
}

function parseOptionalArtifactDigest(input: string | null): string | undefined {
  if (input === null) return undefined;
  const normalized = input.startsWith("sha256:")
    ? input.slice("sha256:".length)
    : input;
  return SHA256.test(normalized) ? normalized : undefined;
}

function parseAndroidRevision(input: string): string | undefined {
  const revision = /<revision>([\s\S]*?)<\/revision>/u.exec(input)?.[1];
  if (revision === undefined) return undefined;
  const major = /<major>([0-9]+)<\/major>/u.exec(revision)?.[1];
  const minor = /<minor>([0-9]+)<\/minor>/u.exec(revision)?.[1] ?? "0";
  const micro = /<micro>([0-9]+)<\/micro>/u.exec(revision)?.[1] ?? "0";
  return major === undefined ? undefined : `${major}.${minor}.${micro}`;
}

function normalizeAndroidRevision(input: string): string | undefined {
  if (!/^[0-9]+(?:\.[0-9]+){0,2}$/u.test(input)) return undefined;
  const [major, minor = "0", micro = "0"] = input.split(".");
  return `${major}.${minor}.${micro}`;
}

async function readPackageVersion(): Promise<string> {
  const parsed = packageSchema.safeParse(
    await readJson(join(REPOSITORY_ROOT, "app/package.json")),
  );
  if (!parsed.success)
    throw new ContractFailure("app/package.json has no product version");
  return parsed.data.version;
}

function parseToolchain(input: unknown): z.infer<typeof toolchainSchema> {
  const parsed = toolchainSchema.safeParse(input);
  if (
    !parsed.success ||
    parsed.data.android.abis.join(",") !== ANDROID_ABIS.join(",")
  ) {
    throw new ContractFailure("release/toolchain.json is invalid");
  }
  return parsed.data;
}

async function readAppImageReleaseContract(): Promise<AppImageReleaseContract> {
  const parsed = parseAppImageReleaseContract(
    await readJson(APPIMAGE_RELEASE_PATH),
  );
  if (!parsed.ok) throw new ContractFailure(parsed.reason);
  return parsed.value;
}

async function readSigningDigest(): Promise<string> {
  const identity = identitySchema.safeParse(await readJson(IDENTITY_PATH));
  if (!identity.success)
    throw new ContractFailure("the Android signing identity file is invalid");
  const digest = parseCertificateSha256(identity.data.certificateSha256);
  if (!digest.ok) throw new ContractFailure(digest.reason);
  return digest.value;
}

async function androidTools(): Promise<{
  readonly aapt2: string;
  readonly apksigner: string;
}> {
  const config = parseToolchain(await readJson(TOOLCHAIN_PATH));
  const directory = join(
    androidHomePath(),
    `build-tools/${config.android.buildTools}`,
  );
  return {
    aapt2: join(directory, "aapt2"),
    apksigner: join(directory, "apksigner"),
  };
}

function androidHomePath(): string {
  const value = process.env.ANDROID_HOME ?? process.env.ANDROID_SDK_ROOT;
  if (value === undefined || value.length === 0) {
    throw new ContractFailure(
      "ANDROID_HOME is required for release verification",
    );
  }
  return resolve(value);
}

function errorHasCode(error: unknown, code: string): boolean {
  return error instanceof Error && "code" in error && error.code === code;
}

async function verifySidecar(artifact: string): Promise<void> {
  const expected = `${await sha256(artifact)}  ${basename(artifact)}\n`;
  const actual = await readFile(`${artifact}.sha256`, "utf8").catch(() => "");
  if (actual !== expected)
    throw new ContractFailure("the candidate checksum sidecar is invalid");
}

async function sha256(path: string): Promise<string> {
  const hash = createHash("sha256");
  const stream = createReadStream(path);
  for await (const chunk of stream) hash.update(chunk);
  return hash.digest("hex");
}

async function walkFiles(root: string): Promise<string[]> {
  const entries = await readdir(root, { withFileTypes: true }).catch(() => []);
  const files: string[] = [];
  for (const entry of entries) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) files.push(...(await walkFiles(path)));
    if (entry.isFile()) files.push(path);
  }
  return files;
}

function run(
  command: string,
  arguments_: readonly string[],
  label: string,
  options: {
    readonly cwd?: string;
    readonly env?: NodeJS.ProcessEnv;
    readonly timeout?: number;
  } = {},
): CommandResult {
  const result = spawnSync(command, arguments_, {
    cwd: options.cwd,
    env: options.env,
    encoding: "utf8",
    maxBuffer: COMMAND_OUTPUT_LIMIT,
    timeout: options.timeout,
  });
  if (result.status !== 0) throw new ContractFailure(`failed to ${label}`);
  return { stdout: result.stdout, stderr: result.stderr };
}

function waitForLaunchWindow(
  child: ChildProcess,
  durationMs: number,
): Promise<void> {
  return new Promise((resolveDelay, reject) => {
    const onError = (): void => {
      clearTimeout(timer);
      reject(new ContractFailure("the AppImage could not start"));
    };
    const timer = setTimeout(() => {
      child.off("error", onError);
      resolveDelay();
    }, durationMs);
    child.once("error", onError);
  });
}

function waitForExitWindow(
  child: ChildProcess,
  durationMs: number,
): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null)
    return Promise.resolve();
  return new Promise((resolveExit) => {
    const onExit = (): void => {
      clearTimeout(timer);
      resolveExit();
    };
    const timer = setTimeout(() => {
      child.off("exit", onExit);
      resolveExit();
    }, durationMs);
    child.once("exit", onExit);
  });
}

async function readJson(path: string): Promise<unknown> {
  try {
    return JSON.parse(await readFile(path, "utf8")) as unknown;
  } catch {
    throw new ContractFailure("a release contract JSON file is invalid");
  }
}

function parseJsonText(input: string): unknown {
  try {
    return JSON.parse(input) as unknown;
  } catch {
    throw new ContractFailure("GitHub returned invalid release metadata JSON");
  }
}

async function writeJson(path: string, value: unknown): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

async function appendGithubOutput(
  values: ReadonlyMap<string, string>,
  output: Readonly<Record<string, string>>,
): Promise<void> {
  const path = optional(values, "--github-output");
  if (path === undefined || path.length === 0) return;
  const lines = Object.entries(output)
    .map(([name, value]) => `${name}=${value}\n`)
    .join("");
  await writeFile(path, lines, { encoding: "utf8", flag: "a" });
}

void main().catch((error: unknown) => {
  const message =
    error instanceof ContractFailure ||
    error instanceof ReleaseFilesystemFailure
      ? error.message
      : "unexpected release contract failure";
  process.stderr.write(`release contract rejected: ${message}\n`);
  process.exitCode = 1;
});
