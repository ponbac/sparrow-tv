# Build tagged release candidates once and publish after device acceptance

Sparrow TV will use repository-pinned local tooling and two GitHub Actions workflows: an unprivileged deterministic CI workflow, and a tag-triggered release workflow that builds one x86_64 AppImage and one signed universal APK. A release tag produces candidate artifacts once; GitHub publication waits at a manual environment gate until those exact candidates pass the target Arch/Wayland and physical Android checks. This keeps CI reproducible enough for a personal project without pretending that an Ubuntu runner can validate the actual playback devices.

## Repository-owned build interface

- `mise.toml` pins Bun, Temurin JDK 17, and `just`; `rust-toolchain.toml` pins Rust, `rustfmt`, `clippy`, and the four Android targets. The initial Android contract remains API 36, Build Tools 35.0.0, and NDK 29.0.14206865 until an intentional toolchain-update change proves a replacement locally and in CI.
- The generated Android scaffold and Gradle wrapper are committed. Cargo, Bun, and Gradle lockfiles are authoritative; Tauri CLI and JavaScript dependencies are repository dependencies rather than globally installed tools. Frozen or locked installation is mandatory in CI and release builds. AppImage helper executables remain upstream artifacts, but their cache filenames, source URLs, and SHA-256 values are repository-owned in `release/appimage.json`; a job-private cache is populated and verified before the pinned Tauri CLI may execute them.
- A small `just` interface is shared by humans and workflows: `just check` runs formatting, linting, type checks, and deterministic tests; target-specific recipes build the hosted application, AppImage, debug APK, and release APK; artifact-verification and device-smoke recipes consume already-built files. Workflow YAML supplies runners, system packages, caches, permissions, and secrets but does not reimplement the checks.
- The exact tool versions live in those repository files, not in prose or runner defaults. Toolchain changes update the pins and lockfiles together and must pass the same gates as application changes. Every third-party action is pinned to a reviewed full commit SHA and updated deliberately, with Dependabot allowed to propose—not merge—action updates.
- The target Arch machine may build and run debug applications, the local API 36 emulator may exercise debug APK startup, and the physical phone may install debug or candidate APKs. Neither a local Arch AppImage nor an APK signed with the debug key is a releasable artifact.

## Version and tag contract

- `app/package.json` owns the product SemVer. Tauri reads that file through its `version` configuration; Rust packages remain unpublished implementation packages and do not create competing product versions.
- Releases use stable tags only, exactly `vMAJOR.MINOR.PATCH`, on the current `master` commit. The release preflight proves that the tag, product version, AppImage metadata, Android `versionName`, and release filenames agree.
- Android uses Tauri's deterministic SemVer mapping, `major * 1,000,000 + minor * 1,000 + patch`, with each component below 1,000. Stateful auto-incrementing is disabled. The workflow rejects a version that is not newer than the latest published Android version.
- A version bump is committed before tagging. The pushed tag must already exist when publication runs; publication uses `--verify-tag` and never creates or moves it. Published tags and assets are immutable. A fixed build receives a new version rather than replacing artifacts attached to an existing release.

## CI workflow

`.github/workflows/ci.yml` runs on pull requests and pushes to the active rewrite branch and `master`. It has `contents: read`, receives no signing or provider secrets, and runs `just check` with locked dependencies. It also builds the hosted application and performs adapter/package checks that do not need a display or device. Deterministic sanitized and generated fixtures are the only catalog inputs; ordinary CI never contacts the private M3U or EPG sources.

Caches may hold Bun downloads, Cargo registry/git data and target outputs, and Gradle dependencies. Keys include the runner, toolchain, target, and relevant lockfiles. Caches are disposable accelerators, never inputs required for correctness, and may not contain source configuration, provider data, keystores, signing properties, or passwords.

## Release workflow

`.github/workflows/release.yml` runs for a pushed release tag and by manual `workflow_dispatch`. Manual dispatch is a build-only rehearsal, is accepted only from the current `master` commit, and cannot publish. A tag run has five stages:

1. `check` verifies the tag/version/master contract and runs the ordinary deterministic checks.
2. `appimage` uses `ubuntu-22.04`, installs the explicitly listed Tauri/WebKit packaging and tested media dependencies, and runs the pinned CLI with `tauri build --bundles appimage`. Media-framework bundling remains enabled because the primary Linux engine renders through WebKit; the committed package list is limited to what the representative H.264/AAC corpus proves necessary.
3. `apk` selects the pinned JDK, Android SDK, Build Tools, NDK, and Rust targets, materializes the release keystore only below `$RUNNER_TEMP`, and runs `tauri android build --apk --ci` for one universal APK. No AAB, split-per-ABI output, store metadata, or updater artifact is built.
4. `candidate` downloads both outputs by immutable artifact ID, reverifies package identities and provenance, creates `SHA256SUMS` plus the attempt-bound acceptance manifest, and uploads one exact-byte acceptance bundle.
5. `publish` downloads that bundle without rebuilding, waits in a manually approved `release-publish` environment, reverifies its bytes, provenance, source refs, workflow attempt, and environment protection, then publishes one existing-tag GitHub Release containing exactly the AppImage, APK, and checksum manifest.

The two build jobs assert exactly one expected output and give it a deterministic versioned name. They upload candidates as workflow artifacts, generate GitHub build-provenance attestations for the candidate files, and expose their SHA-256 digests for acceptance. Only the final job has `contents: write`; build jobs have the minimum read, artifact, OIDC, and attestation permissions they need. Repository release immutability is enabled so a published tag or asset cannot be modified.

The hosted OCI image is not a GitHub Release asset and the Android keystore has no role in it. Its hosted-web smoke check and immutable image identity remain part of the replacement checkpoint and final Compose cutover in ADR 0004; binary publication does not deploy or restart production.

## Android signing

- One long-lived release keystore and alias are generated offline. An encrypted offline backup is the recovery authority; its restoration and certificate digest are tested before the first release.
- The base64-encoded keystore, store password, key alias, and key password are separate secrets in a `release-signing` GitHub environment restricted to exactly the `master` branch and stable `v*` tags. This admits current-master rehearsals and tagged candidates without a pull-request or arbitrary-branch path. The repository owner is the sole required reviewer, owner self-review is allowed, and administrator bypass is disabled. `release-publish` has the same reviewer and bypass controls but admits only stable `v*` tags. The workflow verifies these exact typed policies before using either environment. The signing job uses `umask 077`, never enables shell tracing, never prints generated properties, and never uploads or caches signing material.
- The expected signing-certificate SHA-256 digest is non-secret repository configuration. Every candidate runs `apksigner verify --verbose --print-certs` and fails unless the signature, certificate digest, application identifier, version name/code, minimum SDK, and universal ABI set match the release contract.
- Losing the offline key blocks updates to installed copies and therefore blocks release. Key rotation means a new Android application identity and is not an ordinary workflow operation.

## Candidate verification and publication gate

CI mechanically extracts the AppImage, verifies its product version/resources and performs a bounded headless launch smoke check. It inspects the APK with `apksigner` and Android package tools. Both artifacts are checksum-verified after transfer between jobs, and their GitHub attestations remain independently verifiable.

Before approving `release-publish`, the owner downloads that run's exact candidates and:

- launches the AppImage on the target Arch/Wayland system with the validated WebKit renderer setting, loads the real on-device catalog, plays representative video and audio, changes Channel and Audio Track, and invokes mpv failover once;
- installs the APK on the physical Realme device over the preceding release when one exists, loads cached and refreshed catalog data, plays representative video and audio, changes Channel and Audio Track, and checks background/resume plus screen-lock behavior; and
- confirms the displayed version and candidate SHA-256 values match the waiting workflow run.

The local emulator is optional pre-tag feedback. If it is unavailable or flaky, the release does not acquire a CI emulator requirement: build, signature, and package checks remain automated, while the physical-device check remains mandatory. Rejecting or rerunning a candidate invalidates any earlier manual acceptance; the final successful files must be exercised before approval.

Before the first public APK release, two successively versioned candidates signed by the release key prove Android update continuity on the phone. The first may remain unpublished; the later accepted candidate becomes the first public release.

## Reproducibility limits

Pinned sources, tools, actions, lockfiles, frozen installs, artifact digests, and attestations make a build attributable and repeatable in process. They do not promise bit-for-bit reproduction: GitHub runner images, Ubuntu repositories, and AppImage helper downloads can change. The canonical binaries are therefore the first accepted outputs of a tag run; they are transferred to publication without rebuilding and are never overwritten.

## Rejected alternatives

- Publishing directly from parallel platform jobs is rejected because it can expose a partial release and bypass target-device acceptance.
- Rebuilding after manual device checks is rejected because the published bytes would not be the tested bytes.
- Building release AppImages on Arch is rejected because a rolling glibc baseline is unsuitable even for the intended target; Arch remains the runtime acceptance host.
- Running an Android emulator in release CI is rejected because it adds cost without proving the physical device's WebView, codecs, hardware decoding, lifecycle, or update behavior.
- Storing the Android key only in GitHub, committing signing properties, using a debug key, publishing an AAB, splitting ABIs, app-store delivery, automatic updates, and broad platform matrices are out of scope.

## Revisit gates

Revisit this workflow if GitHub removes the pinned Ubuntu baseline or artifact/attestation features, if the target phone ceases to be covered by the universal APK, if another distributor imposes signing or provenance requirements, or if releases become frequent enough that the manual two-device gate is unsustainable. Until then, release speed does not justify weakening exact-artifact acceptance or key continuity.
