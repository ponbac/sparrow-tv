import { describe, expect, it } from "vitest";
import {
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
} from "./release-contract-domain.ts";

const COMMIT = "a".repeat(40);
const MASTER = COMMIT;
const APPIMAGE_DIGEST = "b".repeat(64);
const APK_DIGEST = "c".repeat(64);
const ICON_DIGEST = "d".repeat(64);

const APPIMAGE_TOOL_NAMES = [
  "AppRun-x86_64",
  "linuxdeploy-x86_64.AppImage",
  "linuxdeploy-plugin-gtk.sh",
  "linuxdeploy-plugin-gstreamer.sh",
  "linuxdeploy-plugin-appimage.AppImage",
] as const;

describe("release contract", () => {
  it("uses the supported run artifact collection without an attempt-scoped route", () => {
    const endpoint = formatWorkflowRunArtifactsEndpoint(
      "ponbac/sparrow-tv",
      "33311303581",
    );
    expect(endpoint).toEqual({
      ok: true,
      value:
        "repos/ponbac/sparrow-tv/actions/runs/33311303581/artifacts?per_page=100",
    });
    if (endpoint.ok) expect(endpoint.value).not.toContain("/attempts/");
    expect(
      formatWorkflowRunArtifactsEndpoint("invalid", "33311303581").ok,
    ).toBe(false);
    expect(
      formatWorkflowRunArtifactsEndpoint(
        "ponbac/sparrow-tv",
        "9007199254740992",
      ).ok,
    ).toBe(false);
  });

  it("derives every candidate identity from one stable SemVer", () => {
    expect(parseProductVersion("0.11.4")).toEqual({
      ok: true,
      value: {
        text: "0.11.4",
        major: 0,
        minor: 11,
        patch: 4,
        tag: "v0.11.4",
        androidVersionCode: 11_004,
        appImageName: "Sparrow_0.11.4_x86_64.AppImage",
        apkName: "Sparrow_0.11.4_universal.apk",
      },
    });
    for (const invalid of [
      "0.0.0",
      "1.2",
      "1.2.3-rc.1",
      "01.2.3",
      "1.1000.0",
      "latest",
    ]) {
      expect(parseProductVersion(invalid).ok).toBe(false);
    }
  });

  it("gives only a current-master stable tag publication authority", () => {
    expect(
      verifyReleasePreflight({
        mode: "tag",
        productVersion: "1.2.3",
        refName: "v1.2.3",
        commit: COMMIT,
        masterCommit: MASTER,
        existingTags: ["v1.2.3", "v1.2.2", "not-a-release"],
      }),
    ).toMatchObject({ ok: true, value: { publishable: true, mode: "tag" } });
    expect(
      verifyReleasePreflight({
        mode: "tag",
        productVersion: "1.2.3",
        refName: "v1.2.3",
        commit: COMMIT,
        masterCommit: "d".repeat(40),
        existingTags: [],
      }),
    ).toEqual({
      ok: false,
      reason: "the release tag is not on the current origin/master commit",
    });
    expect(
      verifyReleasePreflight({
        mode: "tag",
        productVersion: "1.2.3",
        refName: "v1.2.3",
        commit: COMMIT,
        masterCommit: MASTER,
        existingTags: ["v1.3.0"],
      }),
    ).toEqual({
      ok: false,
      reason: "the release version is not newer than every prior stable tag",
    });
  });

  it("allows manual dispatch to build only the exact package version", () => {
    expect(
      verifyReleasePreflight({
        mode: "rehearsal",
        productVersion: "1.2.3",
        requestedVersion: "1.2.3",
        refName: "master",
        commit: COMMIT,
        masterCommit: MASTER,
      }),
    ).toMatchObject({
      ok: true,
      value: { publishable: false, mode: "rehearsal" },
    });
    expect(
      verifyReleasePreflight({
        mode: "rehearsal",
        productVersion: "1.2.3",
        requestedVersion: "1.2.4",
        refName: "master",
        commit: COMMIT,
        masterCommit: MASTER,
      }),
    ).toEqual({
      ok: false,
      reason: "the rehearsal version does not match app/package.json",
    });
    expect(
      verifyReleasePreflight({
        mode: "rehearsal",
        productVersion: "1.2.3",
        requestedVersion: "1.2.3",
        refName: "feature/release",
        commit: COMMIT,
        masterCommit: MASTER,
      }),
    ).toEqual({
      ok: false,
      reason: "the rehearsal must run from the master branch",
    });
    expect(
      verifyReleasePreflight({
        mode: "rehearsal",
        productVersion: "1.2.3",
        requestedVersion: "1.2.3",
        refName: "master",
        commit: COMMIT,
        masterCommit: "d".repeat(40),
      }),
    ).toEqual({
      ok: false,
      reason: "the rehearsal is not on the current origin/master commit",
    });
  });

  it("requires the exact universal non-debuggable Android package", () => {
    const version = successfulVersion("0.11.4");
    const badging = [
      "package: name='xyz.ponbac.sparrow' versionCode='11004' versionName='0.11.4'",
      "sdkVersion:'24'",
      "targetSdkVersion:'36'",
      "native-code: 'arm64-v8a' 'armeabi-v7a' 'x86' 'x86_64'",
    ].join("\n");
    expect(parseApkBadging(badging, version)).toMatchObject({
      ok: true,
      value: { versionCode: 11_004, debuggable: false },
    });
    expect(
      parseApkBadging(`${badging}\napplication-debuggable`, version).ok,
    ).toBe(false);
    expect(parseApkBadging(badging.replace(" 'x86'", ""), version).ok).toBe(
      false,
    );
  });

  it("normalizes one configured signing certificate without accepting a sentinel", () => {
    const digest = "AB:".repeat(31) + "AB";
    expect(parseCertificateSha256(digest)).toEqual({
      ok: true,
      value: "ab".repeat(32),
    });
    expect(parseCertificateSha256("")).toEqual({
      ok: false,
      reason: "the Android release certificate SHA-256 is not configured",
    });
    expect(
      parseApkSignerCertificate(
        `Signer #1 certificate SHA-256 digest: ${"ab".repeat(32)}`,
      ),
    ).toEqual({ ok: true, value: "ab".repeat(32) });
    expect(
      parseApkSignerCertificate(
        `certificate SHA-256 digest: ${"ab".repeat(32)}\n` +
          `certificate SHA-256 digest: ${"cd".repeat(32)}`,
      ).ok,
    ).toBe(false);
  });

  it("round-trips only the two deterministic checksum entries", () => {
    const version = successfulVersion("0.11.4");
    const digests = {
      appImage: { name: version.appImageName, sha256: APPIMAGE_DIGEST },
      apk: { name: version.apkName, sha256: APK_DIGEST },
    };
    const formatted = formatChecksums(digests);
    expect(formatted.ok).toBe(true);
    if (!formatted.ok) return;
    expect(parseChecksums(formatted.value, version)).toEqual({
      ok: true,
      value: digests,
    });
    expect(parseChecksums(`${formatted.value}extra\n`, version).ok).toBe(false);
  });

  it("rejects every mutable or local action reference", () => {
    expect(
      verifyActionPins({
        "ci.yml": `steps:\n  - uses: actions/checkout@${COMMIT} # reviewed`,
      }),
    ).toEqual({ ok: true, value: true });
    expect(
      verifyActionPins({ "ci.yml": "- uses: actions/checkout@v7" }).ok,
    ).toBe(false);
    expect(verifyActionPins({ "ci.yml": "- uses: ./local-action" }).ok).toBe(
      false,
    );
  });

  it("keeps untrusted workflow preflight values out of generated shell source", () => {
    const safe = [
      "env:",
      "  REQUESTED_VERSION: ${{ inputs.version }}",
      "run: |",
      '  bun run release:contract preflight --requested-version "$REQUESTED_VERSION"',
    ].join("\n");
    expect(verifyReleaseWorkflowPreflightBoundary(safe)).toEqual({
      ok: true,
      value: true,
    });
    expect(
      verifyReleaseWorkflowPreflightBoundary(
        [
          "run: |",
          "  bun run release:contract preflight --requested-version '${{ inputs.version }}'",
        ].join("\n"),
      ).ok,
    ).toBe(false);
    expect(
      verifyReleaseWorkflowPreflightBoundary(
        'run: just release-preflight rehearsal "$REQUESTED_VERSION"',
      ).ok,
    ).toBe(false);
  });

  it("keeps boundary recipe values out of Just-generated shell source", () => {
    const safe = [
      'set shell := ["bash", "-c"]',
      'output := "artifacts"',
      "release-stage:",
      '    tool --input "${RELEASE_INPUT:?RELEASE_INPUT is required}"',
      "check: check-rust check-app",
      "    tool fixed",
      "release-acceptance-seal:",
      '    tool --evidence "${ACCEPTANCE_EVIDENCE:?ACCEPTANCE_EVIDENCE is required}"',
    ].join("\n");
    expect(verifyJustBoundaryRecipes(safe)).toEqual({ ok: true, value: true });
    expect(
      verifyJustBoundaryRecipes('release-stage input:\n    tool "{{input}}"')
        .ok,
    ).toBe(false);
    expect(
      verifyJustBoundaryRecipes('stage-release input:\n    tool "$input"').ok,
    ).toBe(false);
    expect(
      verifyJustBoundaryRecipes('@release-stage input:\n    tool "$input"').ok,
    ).toBe(false);
    expect(
      verifyJustBoundaryRecipes('ordinary:\n    tool "{{value}}"').ok,
    ).toBe(false);
  });

  it("requires one pinned square PNG before AppImage bundling", () => {
    const contract = successfulAppImageContract();
    expect(
      verifyAppImageBundleIcon(
        ["icons/icon.png"],
        contract.icon,
        pngHeader(512, 512),
        ICON_DIGEST,
      ),
    ).toEqual({ ok: true, value: true });
    expect(
      verifyAppImageBundleIcon(
        [],
        contract.icon,
        pngHeader(512, 512),
        ICON_DIGEST,
      ).ok,
    ).toBe(false);
    expect(
      verifyAppImageBundleIcon(
        ["icons/icon.png"],
        contract.icon,
        undefined,
        undefined,
      ).ok,
    ).toBe(false);
    expect(
      verifyAppImageBundleIcon(
        ["icons/icon.png"],
        contract.icon,
        pngHeader(512, 256),
        ICON_DIGEST,
      ).ok,
    ).toBe(false);
  });

  it("requires the exact five digest-pinned AppImage helper cache entries", () => {
    const raw = appImageContractFixture();
    expect(parseAppImageReleaseContract(raw)).toMatchObject({ ok: true });
    expect(
      parseAppImageReleaseContract({ ...raw, tools: raw.tools.slice(1) }).ok,
    ).toBe(false);
    expect(
      parseAppImageReleaseContract({
        ...raw,
        tools: raw.tools.map((tool, index) =>
          index === 0 ? { ...tool, url: "http://example.invalid/tool" } : tool,
        ),
      }).ok,
    ).toBe(false);
    expect(
      parseAppImageReleaseContract({
        ...raw,
        tools: raw.tools.map((tool, index) =>
          index === 0 ? { ...tool, sha256: "unpinned" } : tool,
        ),
      }).ok,
    ).toBe(false);
  });

  it("requires exact strict Gradle lock coverage", () => {
    const strict = "lockMode.set(LockMode.STRICT)";
    const activation = "resolutionStrategy.activateDependencyLocking()";
    const appClasspaths = [
      "arm64",
      "arm",
      "x86",
      "x86_64",
      "universal",
    ].flatMap((abi) =>
      ["Debug", "Release"].flatMap((buildType) =>
        ["Compile", "Runtime"].map(
          (usage) => `${abi}${buildType}${usage}Classpath`,
        ),
      ),
    );
    const buildSrcClasspaths = [
      "compileClasspath",
      "runtimeClasspath",
      "testCompileClasspath",
      "testRuntimeClasspath",
    ];
    const lock = (classpaths: readonly string[]) =>
      `# This is a Gradle generated file for dependency locking.\n` +
      `example:dependency:1.0=${classpaths.join(",")}\nempty=\n`;
    const valid = {
      androidRootBuild: `${strict}\n${strict}\n${activation}`,
      appBuild: `${strict}\n${activation}`,
      buildSrcBuild: `${strict}\n${activation}`,
      androidBuildscriptLock: lock(["classpath"]),
      appLock: lock(appClasspaths),
      buildSrcLock: lock(buildSrcClasspaths),
    };
    expect(verifyGradleDependencyLocking(valid)).toEqual({
      ok: true,
      value: true,
    });
    expect(
      verifyGradleDependencyLocking({
        ...valid,
        appLock: lock(appClasspaths.slice(1)),
      }).ok,
    ).toBe(false);
    expect(
      verifyGradleDependencyLocking({
        ...valid,
        buildSrcBuild: `${activation}\nlockAllConfigurations()`,
      }).ok,
    ).toBe(false);
  });

  it("rejects a release if the remote tag or master moved during approval", () => {
    const tagObject = "e".repeat(40);
    const stableRefs =
      `${COMMIT}\trefs/heads/master\n` +
      `${tagObject}\trefs/tags/v1.2.3\n` +
      `${COMMIT}\trefs/tags/v1.2.3^{}\n`;
    expect(verifyRemoteReleaseRefs(stableRefs, "v1.2.3", COMMIT)).toEqual({
      ok: true,
      value: true,
    });
    expect(
      verifyRemoteReleaseRefs(
        stableRefs.replace(COMMIT, "f".repeat(40)),
        "v1.2.3",
        COMMIT,
      ),
    ).toEqual({
      ok: false,
      reason:
        "the remote release tag or master moved after candidate verification",
    });
  });

  it("requires exact owner review and tag-only publication policies", () => {
    const protectedEnvironment = {
      name: "release-publish",
      can_admins_bypass: false,
      deployment_branch_policy: {
        protected_branches: false,
        custom_branch_policies: true,
      },
      protection_rules: [
        { type: "wait_timer", wait_timer: 5 },
        {
          type: "required_reviewers",
          prevent_self_review: false,
          reviewers: [{ type: "User", reviewer: { id: 42, login: "ponbac" } }],
        },
      ],
      url: "ignored",
    };
    const publishPolicies = {
      total_count: 1,
      branch_policies: [{ name: "v*", type: "tag" }],
    };
    expect(
      verifyEnvironmentProtection(
        protectedEnvironment,
        publishPolicies,
        "release-publish",
        "ponbac",
      ),
    ).toEqual({ ok: true, value: true });
    expect(
      verifyEnvironmentProtection(
        { ...protectedEnvironment, can_admins_bypass: true },
        publishPolicies,
        "release-publish",
        "ponbac",
      ).ok,
    ).toBe(false);
    expect(
      verifyEnvironmentProtection(
        protectedEnvironment,
        {
          total_count: 2,
          branch_policies: [
            { name: "v*", type: "tag" },
            { name: "master", type: "branch" },
          ],
        },
        "release-publish",
        "ponbac",
      ).ok,
    ).toBe(false);
    expect(
      verifyEnvironmentProtection(
        {
          ...protectedEnvironment,
          protection_rules: [
            {
              type: "required_reviewers",
              prevent_self_review: true,
              reviewers: [
                { type: "User", reviewer: { id: 42, login: "someone-else" } },
              ],
            },
          ],
        },
        publishPolicies,
        "release-publish",
        "ponbac",
      ).ok,
    ).toBe(false);
  });

  it("allows only master rehearsals and stable tags to use release signing", () => {
    const signingEnvironment = {
      name: "release-signing",
      can_admins_bypass: false,
      deployment_branch_policy: {
        protected_branches: false,
        custom_branch_policies: true,
      },
      protection_rules: [
        {
          type: "required_reviewers",
          prevent_self_review: false,
          reviewers: [{ type: "User", reviewer: { id: 42, login: "ponbac" } }],
        },
      ],
    };
    const policies = {
      total_count: 2,
      branch_policies: [
        { name: "master", type: "branch" },
        { name: "v*", type: "tag" },
      ],
    };
    expect(
      verifyEnvironmentProtection(
        signingEnvironment,
        policies,
        "release-signing",
        "ponbac",
      ),
    ).toEqual({ ok: true, value: true });
    expect(
      verifyEnvironmentProtection(
        signingEnvironment,
        {
          ...policies,
          branch_policies: policies.branch_policies.slice(1),
          total_count: 1,
        },
        "release-signing",
        "ponbac",
      ).ok,
    ).toBe(false);
  });

  it("binds the acceptance checklist to one workflow attempt and exact bytes", () => {
    const version = successfulVersion("0.11.4");
    const raw = {
      schemaVersion: 1,
      version: version.text,
      tag: version.tag,
      commit: COMMIT,
      repository: "ponbac/sparrow-tv",
      workflowRunId: "123",
      workflowRunAttempt: 2,
      publishable: true,
      artifacts: {
        appImage: { name: version.appImageName, sha256: APPIMAGE_DIGEST },
        apk: { name: version.apkName, sha256: APK_DIGEST },
      },
      android: {
        applicationId: "xyz.ponbac.sparrow",
        versionName: version.text,
        versionCode: version.androidVersionCode,
        minSdk: 24,
        targetSdk: 36,
        abis: ["arm64-v8a", "armeabi-v7a", "x86", "x86_64"],
        certificateSha256: "d".repeat(64),
      },
    } as const;
    const parsed = parseCandidateManifest(raw, version, version.tag, COMMIT);
    expect(parsed).toEqual({ ok: true, value: raw });
    expect(parseCandidateManifest(raw, version, "v0.11.5", COMMIT).ok).toBe(
      false,
    );
    expect(
      parseCandidateManifest(
        { ...raw, android: { ...raw.android, versionCode: 11_005 } },
        version,
        version.tag,
        COMMIT,
      ).ok,
    ).toBe(false);
    if (!parsed.ok) return;
    const checklist = formatAcceptanceManifest(parsed.value);
    expect(checklist).toContain(APPIMAGE_DIGEST);
    expect(checklist).toContain(APK_DIGEST);
    expect(checklist).toContain("Android certificate SHA-256");
    expect(checklist).toContain("attempt 2");
    expect(checklist).toContain("release-publish");
    expect(checklist).toContain("release-acceptance-seal");
    expect(checklist).toContain("blank or ordinary UI approval is rejected");
    expect(checklist).toContain(
      "Primary Linux mpv A/V, controls/fullscreen, Channel switching, any applicable direct mpv Audio Track choice (not Sparrow-persisted), one provider connection, and process/socket cleanup",
    );
    expect(checklist).not.toContain(
      "Startup, version, catalog, and Audio Track behavior",
    );
    expect(checklist).not.toContain("mpv fallback");
  });
});

function successfulVersion(input: string) {
  const parsed = parseProductVersion(input);
  if (!parsed.ok) throw new Error(parsed.reason);
  return parsed.value;
}

function successfulAppImageContract() {
  const parsed = parseAppImageReleaseContract(appImageContractFixture());
  if (!parsed.ok) throw new Error(parsed.reason);
  return parsed.value;
}

function appImageContractFixture() {
  return {
    schemaVersion: 1,
    architecture: "x86_64",
    icon: {
      path: "icons/icon.png",
      width: 512,
      height: 512,
      sha256: ICON_DIGEST,
    },
    tools: APPIMAGE_TOOL_NAMES.map((cacheName, index) => ({
      cacheName,
      url: `https://example.invalid/tool-${index}`,
      sha256: String(index).repeat(64),
    })),
  } as const;
}

function pngHeader(width: number, height: number): Uint8Array {
  const bytes = new Uint8Array(24);
  bytes.set([137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82]);
  const view = new DataView(bytes.buffer);
  view.setUint32(16, width);
  view.setUint32(20, height);
  return bytes;
}
