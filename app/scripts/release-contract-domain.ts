import { z } from "zod";

const STABLE_SEMVER =
  /^(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})$/u;
const SHA256 = /^[0-9a-f]{64}$/u;
const ACTION_PIN = /^\s*(?:-\s*)?uses:\s+([^\s@]+)@([0-9a-f]{40})(?:\s+#.*)?$/u;
const STRICT_GRADLE_LOCK_MODE = "lockMode.set(LockMode.STRICT)";
const APP_GRADLE_CLASSPATHS = listGradleClasspaths(
  ["arm64", "arm", "x86", "x86_64", "universal"],
  ["Debug", "Release"],
);
const BUILD_SRC_GRADLE_CLASSPATHS = [
  "compileClasspath",
  "runtimeClasspath",
  "testCompileClasspath",
  "testRuntimeClasspath",
] as const;
const APPIMAGE_TOOL_CACHE_NAMES = [
  "AppRun-x86_64",
  "linuxdeploy-x86_64.AppImage",
  "linuxdeploy-plugin-gtk.sh",
  "linuxdeploy-plugin-gstreamer.sh",
  "linuxdeploy-plugin-appimage.AppImage",
] as const;
const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10] as const;

/** The application identity shared by Tauri and Android packaging. */
export const ANDROID_APPLICATION_ID = "xyz.ponbac.sparrow";
/** The minimum Android API accepted by the release contract. */
export const ANDROID_MIN_SDK = 24;
/** The Android API targeted by the release contract. */
export const ANDROID_TARGET_SDK = 36;
/** The complete ABI set required in Sparrow's one universal APK. */
export const ANDROID_ABIS = [
  "arm64-v8a",
  "armeabi-v7a",
  "x86",
  "x86_64",
] as const;

/** A boundary result that carries either parsed domain data or a safe reason. */
export type ParseResult<Value> =
  | { readonly ok: true; readonly value: Value }
  | { readonly ok: false; readonly reason: string };

/** One stable product version and every deterministic projection derived from it. */
export interface ProductVersion {
  readonly text: string;
  readonly major: number;
  readonly minor: number;
  readonly patch: number;
  readonly tag: string;
  readonly androidVersionCode: number;
  readonly appImageName: string;
  readonly apkName: string;
}

/** The release preflight modes have deliberately different publication authority. */
export type PreflightInput =
  | {
      readonly mode: "tag";
      readonly productVersion: unknown;
      readonly refName: unknown;
      readonly commit: unknown;
      readonly masterCommit: unknown;
      readonly existingTags: readonly string[];
    }
  | {
      readonly mode: "rehearsal";
      readonly productVersion: unknown;
      readonly requestedVersion: unknown;
      readonly refName: unknown;
      readonly commit: unknown;
      readonly masterCommit: unknown;
    };

/** Safe, serializable evidence produced before any candidate build starts. */
export interface ReleasePreflight {
  readonly schemaVersion: 1;
  readonly mode: "tag" | "rehearsal";
  readonly publishable: boolean;
  readonly version: string;
  readonly tag: string;
  readonly commit: string;
  readonly androidVersionCode: number;
  readonly appImageName: string;
  readonly apkName: string;
}

/** Parsed package claims from Android's pinned `aapt2 dump badging` output. */
export interface ApkPackageIdentity {
  readonly applicationId: typeof ANDROID_APPLICATION_ID;
  readonly versionName: string;
  readonly versionCode: number;
  readonly minSdk: typeof ANDROID_MIN_SDK;
  readonly targetSdk: typeof ANDROID_TARGET_SDK;
  readonly abis: typeof ANDROID_ABIS;
  readonly debuggable: false;
}

/** The two immutable binary subjects collected before the publication gate. */
export interface CandidateDigests {
  readonly appImage: { readonly name: string; readonly sha256: string };
  readonly apk: { readonly name: string; readonly sha256: string };
}

/** Repository-generated metadata that binds candidate bytes to their source run. */
export interface CandidateManifest {
  readonly schemaVersion: 1;
  readonly version: string;
  readonly tag: string;
  readonly commit: string;
  readonly repository: string;
  readonly workflowRunId: string;
  readonly workflowRunAttempt: number;
  readonly publishable: true;
  readonly artifacts: CandidateDigests;
  readonly android: {
    readonly applicationId: typeof ANDROID_APPLICATION_ID;
    readonly versionName: string;
    readonly versionCode: number;
    readonly minSdk: typeof ANDROID_MIN_SDK;
    readonly targetSdk: typeof ANDROID_TARGET_SDK;
    readonly abis: typeof ANDROID_ABIS;
    readonly certificateSha256: string;
  };
}

/** Repository-owned Gradle inputs that must remain locked in strict mode. */
export interface GradleDependencyLockContractInput {
  readonly androidRootBuild: unknown;
  readonly appBuild: unknown;
  readonly buildSrcBuild: unknown;
  readonly androidBuildscriptLock: unknown;
  readonly appLock: unknown;
  readonly buildSrcLock: unknown;
}

const appImageReleaseContractSchema = z
  .object({
    schemaVersion: z.literal(1),
    architecture: z.literal("x86_64"),
    icon: z
      .object({
        path: z.string().regex(/^icons\/[A-Za-z0-9._+-]+\.png$/u),
        width: z.number().int().positive().max(4096),
        height: z.number().int().positive().max(4096),
        sha256: z.string().regex(SHA256),
      })
      .strict(),
    tools: z
      .array(
        z
          .object({
            cacheName: z.enum(APPIMAGE_TOOL_CACHE_NAMES),
            url: z
              .string()
              .max(2048)
              .regex(/^https:\/\/[^\s]+$/u),
            sha256: z.string().regex(SHA256),
          })
          .strict(),
      )
      .length(APPIMAGE_TOOL_CACHE_NAMES.length),
  })
  .strict();

/** Repository-owned icon and helper-byte inputs for one x86_64 AppImage build. */
export interface AppImageReleaseContract {
  readonly schemaVersion: 1;
  readonly architecture: "x86_64";
  readonly icon: {
    readonly path: string;
    readonly width: number;
    readonly height: number;
    readonly sha256: string;
  };
  readonly tools: readonly {
    readonly cacheName: (typeof APPIMAGE_TOOL_CACHE_NAMES)[number];
    readonly url: string;
    readonly sha256: string;
  }[];
}

const candidateManifestSchema = z
  .object({
    schemaVersion: z.literal(1),
    version: z.string(),
    tag: z.string(),
    commit: z.string().regex(/^[0-9a-f]{40}$/u),
    repository: z.string().regex(/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/u),
    workflowRunId: z.string().regex(/^[1-9][0-9]*$/u),
    workflowRunAttempt: z.number().int().positive(),
    publishable: z.literal(true),
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
        versionName: z.string(),
        versionCode: z.number().int().positive(),
        minSdk: z.literal(ANDROID_MIN_SDK),
        targetSdk: z.literal(ANDROID_TARGET_SDK),
        abis: z.tuple([
          z.literal("arm64-v8a"),
          z.literal("armeabi-v7a"),
          z.literal("x86"),
          z.literal("x86_64"),
        ]),
        certificateSha256: z
          .string()
          .regex(SHA256)
          .refine((digest) => !/^0{64}$/u.test(digest)),
      })
      .strict(),
  })
  .strict();

const environmentSchema = z
  .object({
    name: z.string(),
    can_admins_bypass: z.literal(false),
    protection_rules: z.array(z.unknown()),
    deployment_branch_policy: z
      .object({
        protected_branches: z.literal(false),
        custom_branch_policies: z.literal(true),
      })
      .strict(),
  })
  .strip();

const requiredReviewersRuleSchema = z
  .object({
    type: z.literal("required_reviewers"),
    prevent_self_review: z.literal(false),
    reviewers: z.tuple([
      z
        .object({
          type: z.literal("User"),
          reviewer: z
            .object({
              id: z.number().int().positive(),
              login: z.string().min(1),
            })
            .strip(),
        })
        .strip(),
    ]),
  })
  .strip();

const deploymentBranchPoliciesSchema = z
  .object({
    total_count: z.number().int().nonnegative(),
    branch_policies: z.array(
      z
        .object({
          name: z.string().min(1),
          type: z.enum(["branch", "tag"]),
        })
        .strip(),
    ),
  })
  .strip();

/** Parses the complete repository-owned AppImage packaging input contract. */
export function parseAppImageReleaseContract(
  input: unknown,
): ParseResult<AppImageReleaseContract> {
  const parsed = appImageReleaseContractSchema.safeParse(input);
  if (!parsed.success || parsed.data.icon.width !== parsed.data.icon.height) {
    return reject("the AppImage release contract is invalid");
  }
  const cacheNames = new Set(parsed.data.tools.map((tool) => tool.cacheName));
  const urls = new Set(parsed.data.tools.map((tool) => tool.url));
  if (
    cacheNames.size !== APPIMAGE_TOOL_CACHE_NAMES.length ||
    APPIMAGE_TOOL_CACHE_NAMES.some((name) => !cacheNames.has(name)) ||
    urls.size !== parsed.data.tools.length
  ) {
    return reject(
      "the AppImage release contract must pin the exact helper cache entries",
    );
  }
  return accept(parsed.data);
}

/** Verifies Tauri's configured bundle icon against the pinned square PNG bytes. */
export function verifyAppImageBundleIcon(
  configuredIcons: unknown,
  icon: AppImageReleaseContract["icon"],
  bytes: unknown,
  digest: unknown,
): ParseResult<true> {
  if (!z.tuple([z.literal(icon.path)]).safeParse(configuredIcons).success) {
    return reject("Tauri must configure the one pinned AppImage bundle icon");
  }
  if (!(bytes instanceof Uint8Array) || bytes.byteLength < 24) {
    return reject("the pinned AppImage bundle icon is not a PNG");
  }
  if (
    PNG_SIGNATURE.some((byte, index) => bytes[index] !== byte) ||
    bytes[8] !== 0 ||
    bytes[9] !== 0 ||
    bytes[10] !== 0 ||
    bytes[11] !== 13 ||
    bytes[12] !== 73 ||
    bytes[13] !== 72 ||
    bytes[14] !== 68 ||
    bytes[15] !== 82
  ) {
    return reject("the pinned AppImage bundle icon has an invalid PNG header");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const width = view.getUint32(16);
  const height = view.getUint32(20);
  if (width !== icon.width || height !== icon.height || width !== height) {
    return reject(
      "the pinned AppImage bundle icon is not the required square size",
    );
  }
  if (digest !== icon.sha256) {
    return reject(
      "the AppImage bundle icon bytes do not match the repository pin",
    );
  }
  return accept(true);
}

/** Parses stable SemVer and derives filenames and Android's deterministic code. */
export function parseProductVersion(
  input: unknown,
): ParseResult<ProductVersion> {
  if (typeof input !== "string") {
    return reject("the product version must be a stable SemVer string");
  }
  const match = STABLE_SEMVER.exec(input);
  if (match === null) {
    return reject(
      "the product version must be MAJOR.MINOR.PATCH with components below 1000",
    );
  }
  const major = Number(match[1]);
  const minor = Number(match[2]);
  const patch = Number(match[3]);
  if (![major, minor, patch].every(Number.isSafeInteger)) {
    return reject(
      "the product version components are outside the supported range",
    );
  }
  if (major === 0 && minor === 0 && patch === 0) {
    return reject(
      "the product version must produce a positive Android version code",
    );
  }
  return accept({
    text: input,
    major,
    minor,
    patch,
    tag: `v${input}`,
    androidVersionCode: major * 1_000_000 + minor * 1_000 + patch,
    appImageName: `Sparrow_${input}_x86_64.AppImage`,
    apkName: `Sparrow_${input}_universal.apk`,
  });
}

/** Proves the stable tag or build-only rehearsal is bound to the package version. */
export function verifyReleasePreflight(
  input: PreflightInput,
): ParseResult<ReleasePreflight> {
  const product = parseProductVersion(input.productVersion);
  if (!product.ok) return product;
  const commit = parseCommit(input.commit);
  if (!commit.ok) return commit;

  if (input.mode === "rehearsal") {
    const requested = parseProductVersion(input.requestedVersion);
    if (!requested.ok) return requested;
    if (requested.value.text !== product.value.text) {
      return reject("the rehearsal version does not match app/package.json");
    }
    if (input.refName !== "master") {
      return reject("the rehearsal must run from the master branch");
    }
    const masterCommit = parseCommit(input.masterCommit);
    if (!masterCommit.ok) return masterCommit;
    if (masterCommit.value !== commit.value) {
      return reject("the rehearsal is not on the current origin/master commit");
    }
    return accept(
      projectPreflight("rehearsal", false, product.value, commit.value),
    );
  }

  if (input.refName !== product.value.tag) {
    return reject("the pushed tag does not match app/package.json");
  }
  const masterCommit = parseCommit(input.masterCommit);
  if (!masterCommit.ok) return masterCommit;
  if (masterCommit.value !== commit.value) {
    return reject("the release tag is not on the current origin/master commit");
  }

  for (const rawTag of input.existingTags) {
    if (rawTag === product.value.tag) continue;
    if (!rawTag.startsWith("v")) continue;
    const prior = parseProductVersion(rawTag.slice(1));
    if (prior.ok && compareVersions(product.value, prior.value) <= 0) {
      return reject(
        "the release version is not newer than every prior stable tag",
      );
    }
  }
  return accept(projectPreflight("tag", true, product.value, commit.value));
}

/** Parses a repository signing identity into a normalized lowercase digest. */
export function parseCertificateSha256(input: unknown): ParseResult<string> {
  if (typeof input !== "string") {
    return reject("the Android release certificate SHA-256 is not configured");
  }
  const normalized = input.trim().replaceAll(":", "").toLowerCase();
  if (!SHA256.test(normalized) || /^0{64}$/u.test(normalized)) {
    return reject("the Android release certificate SHA-256 is not configured");
  }
  return accept(normalized);
}

/** Parses Android package metadata and requires one non-debuggable universal APK. */
export function parseApkBadging(
  output: unknown,
  productVersion: ProductVersion,
): ParseResult<ApkPackageIdentity> {
  if (typeof output !== "string" || output.length > 4 * 1024 * 1024) {
    return reject("aapt2 returned invalid APK metadata");
  }
  const packageLine = lineStartingWith(output, "package:");
  const minSdkLine = lineStartingWith(output, "sdkVersion:");
  const targetSdkLine = lineStartingWith(output, "targetSdkVersion:");
  const nativeCodeLine = lineStartingWith(output, "native-code:");
  if (
    packageLine === undefined ||
    minSdkLine === undefined ||
    targetSdkLine === undefined ||
    nativeCodeLine === undefined
  ) {
    return reject("aapt2 did not report the complete Sparrow package identity");
  }
  const applicationId = quotedAttribute(packageLine, "name");
  const versionCodeText = quotedAttribute(packageLine, "versionCode");
  const versionName = quotedAttribute(packageLine, "versionName");
  const minSdk = quotedScalar(minSdkLine);
  const targetSdk = quotedScalar(targetSdkLine);
  const versionCode = Number(versionCodeText);
  const abis = Array.from(
    nativeCodeLine.matchAll(/'([^']+)'/gu),
    (match) => match[1],
  );
  const sortedAbis = [...abis].sort();
  const expectedAbis = [...ANDROID_ABIS].sort();
  if (
    applicationId !== ANDROID_APPLICATION_ID ||
    versionName !== productVersion.text ||
    versionCode !== productVersion.androidVersionCode ||
    minSdk !== String(ANDROID_MIN_SDK) ||
    targetSdk !== String(ANDROID_TARGET_SDK) ||
    sortedAbis.length !== expectedAbis.length ||
    !sortedAbis.every((abi, index) => abi === expectedAbis[index]) ||
    /application-debuggable/iu.test(output)
  ) {
    return reject(
      "the APK package identity does not match the release contract",
    );
  }
  return accept({
    applicationId: ANDROID_APPLICATION_ID,
    versionName: productVersion.text,
    versionCode: productVersion.androidVersionCode,
    minSdk: ANDROID_MIN_SDK,
    targetSdk: ANDROID_TARGET_SDK,
    abis: ANDROID_ABIS,
    debuggable: false,
  });
}

/** Parses `apksigner --print-certs` without retaining certificate subject data. */
export function parseApkSignerCertificate(
  output: unknown,
): ParseResult<string> {
  if (typeof output !== "string" || output.length > 1024 * 1024) {
    return reject("apksigner returned invalid certificate metadata");
  }
  const matches = Array.from(
    output.matchAll(/certificate SHA-256 digest:\s*([0-9a-f:]{64,95})/giu),
    (match) => match[1],
  );
  if (matches.length !== 1) {
    return reject("the APK must have exactly one signing certificate");
  }
  return parseCertificateSha256(matches[0]);
}

/** Formats the only accepted release checksum manifest, in deterministic order. */
export function formatChecksums(
  digests: CandidateDigests,
): ParseResult<string> {
  const appImageDigest = parseSha256(digests.appImage.sha256);
  const apkDigest = parseSha256(digests.apk.sha256);
  if (!appImageDigest.ok || !apkDigest.ok) {
    return reject("candidate checksums must be lowercase SHA-256 values");
  }
  if (!safeArtifactName(digests.appImage.name, ".AppImage")) {
    return reject("the AppImage candidate name is unsafe");
  }
  if (!safeArtifactName(digests.apk.name, ".apk")) {
    return reject("the APK candidate name is unsafe");
  }
  return accept(
    `${appImageDigest.value}  ${digests.appImage.name}\n${apkDigest.value}  ${digests.apk.name}\n`,
  );
}

/** Parses a checksum manifest and requires only the two versioned candidates. */
export function parseChecksums(
  input: unknown,
  version: ProductVersion,
): ParseResult<CandidateDigests> {
  if (typeof input !== "string") return reject("SHA256SUMS is not text");
  const lines = input.endsWith("\n") ? input.slice(0, -1).split("\n") : [];
  if (lines.length !== 2) {
    return reject("SHA256SUMS must contain exactly the AppImage and APK");
  }
  const entries = new Map<string, string>();
  for (const line of lines) {
    const match = /^([0-9a-f]{64}) {2}([A-Za-z0-9._+-]+)$/u.exec(line);
    if (match === null || entries.has(match[2])) {
      return reject("SHA256SUMS contains an invalid or duplicate entry");
    }
    entries.set(match[2], match[1]);
  }
  const appImage = entries.get(version.appImageName);
  const apk = entries.get(version.apkName);
  if (appImage === undefined || apk === undefined) {
    return reject("SHA256SUMS does not name the expected release candidates");
  }
  return accept({
    appImage: { name: version.appImageName, sha256: appImage },
    apk: { name: version.apkName, sha256: apk },
  });
}

/** Requires every external action reference in workflow YAML to be a full commit SHA. */
export function verifyActionPins(
  workflows: Readonly<Record<string, string>>,
): ParseResult<true> {
  for (const [name, contents] of Object.entries(workflows)) {
    for (const line of contents.split(/\r?\n/u)) {
      if (!/^\s*(?:-\s*)?uses:/u.test(line)) continue;
      const match = ACTION_PIN.exec(line);
      if (match === null || match[1]?.startsWith("./")) {
        return reject(
          `${name} contains an action that is not pinned to a full commit SHA`,
        );
      }
    }
  }
  return accept(true);
}

/** Requires workflow preflight values to cross into shell only through environment variables. */
export function verifyReleaseWorkflowPreflightBoundary(
  input: unknown,
): ParseResult<true> {
  if (
    typeof input !== "string" ||
    input.length === 0 ||
    input.length > 1024 * 1024
  ) {
    return reject("the release workflow is invalid");
  }
  if (/\bjust\s+release-preflight\b/u.test(input)) {
    return reject(
      "the release workflow must not pass preflight values through Just interpolation",
    );
  }

  let runIndent: number | undefined;
  for (const line of input.split(/\r?\n/u)) {
    const indentation = /^ */u.exec(line)?.[0].length ?? 0;
    const trimmed = line.trim();
    if (
      runIndent !== undefined &&
      trimmed.length > 0 &&
      indentation <= runIndent
    ) {
      runIndent = undefined;
    }
    const run = /^( *)run:\s*(.*)$/u.exec(line);
    if (run !== null) {
      runIndent = run[1].length;
      if (/\$\{\{\s*inputs(?:\.|\[)/u.test(run[2])) {
        return reject(
          "workflow inputs must enter preflight through the environment",
        );
      }
      continue;
    }
    if (runIndent !== undefined && /\$\{\{\s*inputs(?:\.|\[)/u.test(line)) {
      return reject(
        "workflow inputs must enter preflight through the environment",
      );
    }
  }
  return accept(true);
}

/** Requires every recipe to have fixed shell source and receive values outside Just templating. */
export function verifyJustBoundaryRecipes(input: unknown): ParseResult<true> {
  if (
    typeof input !== "string" ||
    input.length === 0 ||
    input.length > 1024 * 1024
  ) {
    return reject("the Just recipe file is invalid");
  }
  if (input.includes("{{")) {
    return reject(
      "the Just recipe file interpolates a value into generated shell source",
    );
  }
  for (const line of input.split(/\r?\n/u)) {
    if (/^\s/u.test(line) || /^(?:set|alias|export)\b/u.test(line)) continue;
    const recipe = /^@?([A-Za-z_][A-Za-z0-9_-]*)([^:]*):/u.exec(line);
    if (recipe !== null && (recipe[2] ?? "").trim().length > 0) {
      return reject(
        `${recipe[1] ?? "recipe"} must receive values through environment variables`,
      );
    }
  }
  return accept(true);
}

/** Requires exact lock coverage for repository-owned Android build classpaths. */
export function verifyGradleDependencyLocking(
  input: GradleDependencyLockContractInput,
): ParseResult<true> {
  const scripts = [input.androidRootBuild, input.appBuild, input.buildSrcBuild];
  if (
    scripts.some(
      (script) => typeof script !== "string" || script.length > 1024 * 1024,
    )
  ) {
    return reject("the Gradle dependency-locking build scripts are invalid");
  }
  const [androidRootBuild, appBuild, buildSrcBuild] = scripts as [
    string,
    string,
    string,
  ];
  if (
    occurrences(androidRootBuild, STRICT_GRADLE_LOCK_MODE) !== 2 ||
    occurrences(appBuild, STRICT_GRADLE_LOCK_MODE) !== 1 ||
    occurrences(buildSrcBuild, STRICT_GRADLE_LOCK_MODE) !== 1 ||
    !androidRootBuild.includes(
      "resolutionStrategy.activateDependencyLocking()",
    ) ||
    !appBuild.includes("resolutionStrategy.activateDependencyLocking()") ||
    !buildSrcBuild.includes("resolutionStrategy.activateDependencyLocking()") ||
    scripts.some(
      (script) =>
        (script as string).includes("lockAllConfigurations()") ||
        (script as string).includes("ignoredDependencies"),
    )
  ) {
    return reject(
      "Gradle dependency locking is not strict and explicitly scoped",
    );
  }

  const expectedLocks: ReadonlyArray<readonly [unknown, readonly string[]]> = [
    [input.androidBuildscriptLock, ["classpath"]],
    [input.appLock, APP_GRADLE_CLASSPATHS],
    [input.buildSrcLock, BUILD_SRC_GRADLE_CLASSPATHS],
  ];
  for (const [contents, expectedClasspaths] of expectedLocks) {
    const parsed = parseGradleLockClasspaths(contents);
    if (!parsed.ok) return parsed;
    if (
      parsed.value.size !== expectedClasspaths.length ||
      expectedClasspaths.some((classpath) => !parsed.value.has(classpath))
    ) {
      return reject(
        "a Gradle dependency lock does not cover the exact build classpaths",
      );
    }
  }
  return accept(true);
}

/** Proves the remote release tag and default branch still resolve to the accepted commit. */
export function verifyRemoteReleaseRefs(
  input: unknown,
  expectedTag: string,
  expectedCommit: string,
): ParseResult<true> {
  if (
    typeof input !== "string" ||
    !/^v(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})$/u.test(
      expectedTag,
    ) ||
    !/^[0-9a-f]{40}$/u.test(expectedCommit)
  ) {
    return reject("the remote release reference expectation is invalid");
  }
  const references = new Map<string, string>();
  const lines = input.endsWith("\n") ? input.slice(0, -1).split("\n") : [];
  for (const line of lines) {
    const match = /^([0-9a-f]{40})\t(.+)$/u.exec(line);
    if (match === null || references.has(match[2])) {
      return reject(
        "git returned invalid or duplicate remote release references",
      );
    }
    references.set(match[2], match[1]);
  }
  const expectedNames = new Set([
    "refs/heads/master",
    `refs/tags/${expectedTag}`,
    `refs/tags/${expectedTag}^{}`,
  ]);
  if ([...references.keys()].some((name) => !expectedNames.has(name))) {
    return reject("git returned an unexpected remote release reference");
  }
  const master = references.get("refs/heads/master");
  const tag = references.get(`refs/tags/${expectedTag}`);
  const peeledTag = references.get(`refs/tags/${expectedTag}^{}`);
  if (
    master !== expectedCommit ||
    tag === undefined ||
    (peeledTag ?? tag) !== expectedCommit
  ) {
    return reject(
      "the remote release tag or master moved after candidate verification",
    );
  }
  return accept(true);
}

/** Requires exact owner review, non-bypassable custom refs, and no extra deployment policies. */
export function verifyEnvironmentProtection(
  environmentInput: unknown,
  policyInput: unknown,
  expectedEnvironment: string,
  repositoryOwner: string,
): ParseResult<true> {
  if (
    (expectedEnvironment !== "release-signing" &&
      expectedEnvironment !== "release-publish") ||
    !/^[A-Za-z0-9_.-]+$/u.test(repositoryOwner)
  ) {
    return reject("the release environment name is invalid");
  }
  const environment = environmentSchema.safeParse(environmentInput);
  if (!environment.success || environment.data.name !== expectedEnvironment) {
    return reject("the GitHub release environment response is invalid");
  }
  const reviewerRules = environment.data.protection_rules.filter(
    (rule) =>
      z
        .object({ type: z.literal("required_reviewers") })
        .passthrough()
        .safeParse(rule).success,
  );
  if (reviewerRules.length !== 1) {
    return reject(
      "the GitHub release environment must have one required-reviewer rule",
    );
  }
  const reviewerRule = requiredReviewersRuleSchema.safeParse(reviewerRules[0]);
  if (
    !reviewerRule.success ||
    reviewerRule.data.reviewers[0].reviewer.login.toLowerCase() !==
      repositoryOwner.toLowerCase()
  ) {
    return reject(
      "the GitHub release environment reviewer must be the repository owner",
    );
  }

  const policies = deploymentBranchPoliciesSchema.safeParse(policyInput);
  if (
    !policies.success ||
    policies.data.total_count !== policies.data.branch_policies.length
  ) {
    return reject("the GitHub deployment branch policy response is invalid");
  }
  const expectedPolicies =
    expectedEnvironment === "release-signing"
      ? ["branch:master", "tag:v*"]
      : ["tag:v*"];
  const actualPolicies = policies.data.branch_policies
    .map((policy) => `${policy.type}:${policy.name}`)
    .sort();
  if (
    actualPolicies.length !== expectedPolicies.length ||
    actualPolicies.some((policy, index) => policy !== expectedPolicies[index])
  ) {
    return reject(
      "the GitHub release environment has unexpected deployment policies",
    );
  }
  return accept(true);
}

/** Parses and cross-checks the candidate manifest before publication. */
export function parseCandidateManifest(
  input: unknown,
  expectedVersion: ProductVersion,
  expectedTag: string,
  expectedCommit: string,
): ParseResult<CandidateManifest> {
  const parsed = candidateManifestSchema.safeParse(input);
  if (!parsed.success) return reject("the candidate manifest is invalid");
  const manifest = parsed.data;
  if (
    manifest.version !== expectedVersion.text ||
    expectedTag !== expectedVersion.tag ||
    manifest.tag !== expectedTag ||
    manifest.commit !== expectedCommit ||
    manifest.artifacts.appImage.name !== expectedVersion.appImageName ||
    manifest.artifacts.apk.name !== expectedVersion.apkName ||
    manifest.android.versionName !== expectedVersion.text ||
    manifest.android.versionCode !== expectedVersion.androidVersionCode
  ) {
    return reject(
      "the candidate manifest does not match the release invocation",
    );
  }
  return accept(manifest);
}

/** Formats GitHub's supported workflow-run artifact collection endpoint. */
export function formatWorkflowRunArtifactsEndpoint(
  repository: string,
  workflowRunId: string,
): ParseResult<string> {
  if (
    !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/u.test(repository) ||
    !/^[1-9][0-9]*$/u.test(workflowRunId) ||
    !Number.isSafeInteger(Number(workflowRunId))
  ) {
    return reject("the workflow-run artifact endpoint identity is invalid");
  }
  return accept(
    `repos/${repository}/actions/runs/${workflowRunId}/artifacts?per_page=100`,
  );
}

/** Renders the exact-byte manual gates shown alongside the waiting environment job. */
export function formatAcceptanceManifest(manifest: CandidateManifest): string {
  return (
    `# Sparrow ${manifest.version} candidate acceptance\n\n` +
    `Tag: \`${manifest.tag}\`  \nCommit: \`${manifest.commit}\`  \n` +
    `Workflow run: \`${manifest.workflowRunId}\` (attempt ${manifest.workflowRunAttempt})\n\n` +
    `- AppImage: \`${manifest.artifacts.appImage.name}\` — \`${manifest.artifacts.appImage.sha256}\`\n` +
    `- APK: \`${manifest.artifacts.apk.name}\` — \`${manifest.artifacts.apk.sha256}\`\n\n` +
    `Android package: \`${manifest.android.applicationId}\` \`${manifest.android.versionName}\` ` +
    `(version code \`${manifest.android.versionCode}\`)  \n` +
    `Android certificate SHA-256: \`${manifest.android.certificateSha256}\`  \n` +
    `Universal APK ABIs: \`${manifest.android.abis.join("`, `")}\`\n\n` +
    `Approval is valid only for these hashes and this workflow attempt. A rerun requires new acceptance.\n\n` +
    `Do not edit this candidate bundle or this file. From the tagged checkout, set ` +
    `\`RELEASE_CANDIDATE\` and \`RELEASE_ACCEPTANCE_OUTPUT\`, then run ` +
    `\`just release-acceptance-prepare\` to create separate ` +
    `private, attempt-bound evidence forms.\n\n` +
    `## Target Arch / Wayland\n\n` +
    `- [ ] Restore the downloaded AppImage executable bit without changing its bytes\n` +
    `- [ ] Startup, version, catalog, primary A/V, Channel and Audio Track changes\n` +
    `- [ ] Pause/resume, stop/restart, fullscreen, volume/mute, recovery and resource release\n` +
    `- [ ] Explicit mpv fallback after primary release\n\n` +
    `## Physical Android\n\n` +
    `- [ ] Signature/package values above match the installed universal APK\n` +
    `- [ ] Clean install or upgrade, startup/catalog, primary A/V and Audio Track behavior\n` +
    `- [ ] Rotation, background/foreground, manual lock, wake lock and resource release\n\n` +
    `## Android key continuity\n\n` +
    `- [ ] The physical Realme accepted the older signed APK followed by this APK as an in-place update\n` +
    `- [ ] Application ID, signing certificate, UID, first-install identity, and retained app state match\n\n` +
    `Seal the completed evidence with \`just release-acceptance-seal\`. A blank or ordinary UI ` +
    `approval is rejected. Submit only the generated receipt to the protected ` +
    `\`release-publish\` environment with ` +
    `\`just release-acceptance-approve\`; it authorizes this attempt and candidate artifact only.\n`
  );
}

function projectPreflight(
  mode: "tag" | "rehearsal",
  publishable: boolean,
  version: ProductVersion,
  commit: string,
): ReleasePreflight {
  return {
    schemaVersion: 1,
    mode,
    publishable,
    version: version.text,
    tag: version.tag,
    commit,
    androidVersionCode: version.androidVersionCode,
    appImageName: version.appImageName,
    apkName: version.apkName,
  };
}

function parseCommit(input: unknown): ParseResult<string> {
  if (typeof input !== "string" || !/^[0-9a-f]{40}$/u.test(input)) {
    return reject("the release commit must be a full lowercase Git SHA");
  }
  return accept(input);
}

function compareVersions(left: ProductVersion, right: ProductVersion): number {
  if (left.major !== right.major) return left.major - right.major;
  if (left.minor !== right.minor) return left.minor - right.minor;
  return left.patch - right.patch;
}

function lineStartingWith(output: string, prefix: string): string | undefined {
  return output.split(/\r?\n/u).find((line) => line.startsWith(prefix));
}

function quotedAttribute(line: string, name: string): string | undefined {
  const match = new RegExp(`(?:^|\\s)${name}='([^']*)'`, "u").exec(line);
  return match?.[1];
}

function quotedScalar(line: string): string | undefined {
  return /:'([^']+)'/u.exec(line)?.[1];
}

function parseSha256(input: unknown): ParseResult<string> {
  if (typeof input !== "string" || !SHA256.test(input)) {
    return reject("the value is not a lowercase SHA-256 digest");
  }
  return accept(input);
}

function safeArtifactName(name: string, suffix: string): boolean {
  return (
    /^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$/u.test(name) && name.endsWith(suffix)
  );
}

function listGradleClasspaths(
  flavors: readonly string[],
  buildTypes: readonly string[],
): readonly string[] {
  return flavors.flatMap((flavor) =>
    buildTypes.flatMap((buildType) =>
      ["Compile", "Runtime"].map(
        (usage) => `${flavor}${buildType}${usage}Classpath`,
      ),
    ),
  );
}

function parseGradleLockClasspaths(
  input: unknown,
): ParseResult<ReadonlySet<string>> {
  if (
    typeof input !== "string" ||
    input.length === 0 ||
    input.length > 4 * 1024 * 1024
  ) {
    return reject("a required Gradle dependency lock is invalid");
  }
  const classpaths = new Set<string>();
  let dependencyCount = 0;
  for (const line of input.split(/\r?\n/u)) {
    if (line.length === 0 || line.startsWith("#")) continue;
    const separator = line.lastIndexOf("=");
    if (separator <= 0)
      return reject("a required Gradle dependency lock is malformed");
    const dependency = line.slice(0, separator);
    const lockedClasspaths = line.slice(separator + 1);
    if (dependency === "empty") {
      if (lockedClasspaths.length > 0) {
        return reject(
          "a required Gradle dependency lock has malformed empty state",
        );
      }
      continue;
    }
    dependencyCount += 1;
    for (const classpath of lockedClasspaths.split(",")) {
      if (!/^[A-Za-z0-9_.-]+$/u.test(classpath)) {
        return reject(
          "a required Gradle dependency lock has an invalid classpath",
        );
      }
      classpaths.add(classpath);
    }
  }
  if (dependencyCount === 0) {
    return reject("a required Gradle dependency lock contains no dependencies");
  }
  return accept(classpaths);
}

function occurrences(input: string, fragment: string): number {
  return input.split(fragment).length - 1;
}

function accept<Value>(value: Value): ParseResult<Value> {
  return { ok: true, value };
}

function reject(reason: string): ParseResult<never> {
  return { ok: false, reason };
}
