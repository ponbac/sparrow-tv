# AppImage and signed APK release path

Researched 2026-08-06 for [Wayfinder issue #5](https://github.com/ponbac/sparrow-tv/issues/5).

## Decision

Use one tag-triggered GitHub Actions workflow with three jobs:

1. Build and smoke-check one x86_64 AppImage on `ubuntu-22.04`.
2. Build and cryptographically verify one signed universal release APK on `ubuntu-22.04`.
3. Only after both builds succeed, download their workflow artifacts, create a tag-backed GitHub Release, and upload the AppImage, APK, and a SHA-256 checksum manifest.

Invoke the repository-pinned Tauri CLI directly for both builds. Use `tauri build --bundles appimage` for Linux and `tauri android build --apk --ci` for Android. Do not request an AAB: app stores are out of scope, and Tauri builds both APK and AAB when neither `--apk` nor `--aab` is specified. The CLI's source also confirms that `--ci` skips prompts and that `android build` is a build path rather than a device/emulator run path ([Tauri Android build options](https://github.com/tauri-apps/tauri/blob/c0bd0d5a61eedba5c4783add24455c5028c6f390/crates/tauri-cli/src/mobile/android/build.rs#L32-L82), [default bundle selection](https://github.com/tauri-apps/tauri/blob/c0bd0d5a61eedba5c4783add24455c5028c6f390/crates/tauri-cli/src/mobile/android/build.rs#L253-L276)).

The current repository has not yet been converted to Tauri, so `<tauri-project>` below means the directory that eventually contains the pinned JavaScript Tauri CLI dependency, lockfile, `src-tauri`, and `tauri.conf.json`. The workflow should set one `working-directory` to that directory rather than encode today's web/server layout.

## Release workflow contract

### Trigger and permissions

- A pushed, pre-existing version tag such as `v1.2.3` is the only publishing trigger. Add `workflow_dispatch` for build-only rehearsal; a manual run uploads workflow artifacts but does not create a public Release.
- Assert that the tag version equals the Tauri application version before building. Keep the Android application identifier stable. Let Tauri derive `versionCode` from the application SemVer unless the implementation needs an explicit override; Android requires every successive release to use a greater `versionCode` and prevents installation of a lower one ([Android versioning](https://developer.android.com/studio/publish/versioning#versioningsettings), [Tauri Android configuration schema](https://github.com/tauri-apps/tauri/blob/c0bd0d5a61eedba5c4783add24455c5028c6f390/crates/tauri-cli/config.schema.json)).
- Give the two build jobs `contents: read`. Give only the final tag-only publication job `contents: write` for `GITHUB_TOKEN`. GitHub Releases are based on tags and are the durable public home for binary assets ([GitHub Releases](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases)).
- Pin the Tauri CLI/Rust dependencies in repository lockfiles and pin every action to a reviewed full commit SHA. This matters particularly in the privileged job that can read signing secrets or write a Release.

The workflow shape should be:

```text
tag or manual dispatch
  |-- build_appimage --upload-artifact--\
  |-- build_apk ------upload-artifact----+--> publish_release (tag only)
```

GitHub documents `upload-artifact`/`download-artifact` as the supported way to pass build outputs between jobs and validates downloaded artifacts against the upload SHA-256 digest ([workflow artifacts](https://docs.github.com/en/actions/tutorials/store-and-share-data)). The final job should additionally generate a user-visible `SHA256SUMS` file, then use `gh release create "$GITHUB_REF_NAME" ... --verify-tag --generate-notes`; `--verify-tag` prevents an accidental release from silently creating a tag at the default branch tip ([GitHub CLI release creation](https://cli.github.com/manual/gh_release_create)). Never rebuild an existing tag and replace its binaries.

### Linux/AppImage job

Use `ubuntu-22.04`, not `ubuntu-latest`. Tauri recommends building on the oldest base system intended for support because the AppImage still depends on the build system's glibc baseline; it names Ubuntu 22.04 and Debian 12 as suitable Tauri v2 baselines ([Tauri AppImage limitations](https://v2.tauri.app/distribute/appimage/#limitations)). On the runner:

1. Check out the tag.
2. Install the Tauri-documented Ubuntu packages: `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `patchelf`, and `xdg-utils`; retain any additional general Linux prerequisites required by the pinned Tauri version. Tauri's maintained pipeline example uses this package set on Ubuntu 22.04 ([Tauri GitHub pipeline](https://v2.tauri.app/distribute/pipelines/github/#example-workflow)).
3. Install the repository-pinned frontend toolchain and Rust toolchain, then perform a frozen/locked dependency install.
4. Run `<runner> tauri build --bundles appimage`.
5. Assert that exactly one `src-tauri/target/release/bundle/appimage/*.AppImage` exists, mark it executable, run the runtime's cheap metadata/version check if the application exposes one, and upload it as a workflow artifact.

The eventual Tauri configuration must make bundling active and restrict the desktop target to AppImage, either with `bundle.targets: ["appimage"]` or the CLI flag above. Because Sparrow TV plays audio/video, set `bundle.linux.appimage.bundleMediaFramework: true` if the chosen Linux playback route uses WebKit/GStreamer. Tauri says this flag bundles the extra GStreamer files, is currently fully supported only on Ubuntu build systems, increases bundle size, and requires the build host to contain every plugin needed at runtime ([Tauri AppImage multimedia support](https://v2.tauri.app/distribute/appimage/#multimedia-support-via-gstreamer)). The current bundler source confirms that the flag adds the `linuxdeploy` GStreamer plugin ([Tauri bundler source](https://github.com/tauri-apps/tauri/blob/c0bd0d5a61eedba5c4783add24455c5028c6f390/crates/tauri-bundler/src/bundle/linux/appimage/linuxdeploy.rs#L172-L189)).

Do not guess the final GStreamer codec package list before the playback-engine ticket is resolved. Derive it from the supported stream corpus, install it explicitly on the runner, and run a real playback smoke test on the target Arch machine before release. In particular, Tauri warns that GStreamer's `ugly` plugin set can be difficult to redistribute; including it is a licensing decision, not merely a CI convenience ([same Tauri multimedia guidance](https://v2.tauri.app/distribute/appimage/#multimedia-support-via-gstreamer)). If Linux playback no longer uses WebKit/GStreamer, leave `bundleMediaFramework` off and document that player-specific runtime instead.

### Android/APK job

An emulator is not required. Tauri's build command compiles native targets and asks Gradle to generate APK output; device discovery belongs to the separate `dev`/`run` commands ([Tauri Android build implementation](https://github.com/tauri-apps/tauri/blob/c0bd0d5a61eedba5c4783add24455c5028c6f390/crates/tauri-cli/src/mobile/android/build.rs#L105-L214)). A GitHub-hosted Ubuntu runner already carries Android command-line tools, SDK platforms/build-tools, multiple NDKs, Java, Rust, and the relevant Android environment variables, but the image changes over time ([current Ubuntu 22.04 runner inventory](https://github.com/actions/runner-images/blob/f9e16a05492f0a757004dfc8465a397df086254b/images/ubuntu/Ubuntu2204-Readme.md#android)). Therefore the job should still make its inputs explicit:

1. Check out the tag.
2. Select JDK 17 with `actions/setup-java` (current Android Gradle Plugin 8.x requires JDK 17) and enable its Gradle dependency cache ([Android Gradle Plugin/JDK compatibility](https://developer.android.com/build/jdks#jdk-config-in-agp), [setup-java Gradle cache](https://github.com/actions/setup-java#caching-packages-dependencies)).
3. Install/select the exact SDK platform, build-tools, and side-by-side NDK expected by the committed Tauri Android scaffold, rather than relying on the runner's moving defaults. Set `ANDROID_HOME` and `NDK_HOME` to those paths and add the four Tauri-documented Rust Android targets ([Tauri Android prerequisites](https://v2.tauri.app/start/prerequisites/#android)).
4. Restore frontend and Rust caches, perform frozen/locked installs, and materialize the keystore only in `$RUNNER_TEMP` as described below.
5. Run `<runner> tauri android build --apk --ci` without `--split-per-abi` or a narrowed `--target`. Tauri builds all Android ABIs by default, yielding one larger universal APK that is the least fragile choice until the one physical device's ABI is recorded. After that fact is known, `--target aarch64` is a viable size optimization for an ARM64-only device; it is not required for release correctness ([Tauri target handling](https://github.com/tauri-apps/tauri/blob/c0bd0d5a61eedba5c4783add24455c5028c6f390/crates/tauri-cli/src/mobile/android/build.rs#L253-L313)).
6. Assert exactly one release APK output, then run `apksigner verify --verbose --print-certs <apk>`. Android documents `apksigner verify` as the check that a signature verifies on every Android platform supported by that APK ([apksigner](https://developer.android.com/tools/apksigner#usage-verify)). Upload only the verified APK.

Commit the generated `src-tauri/gen/android` scaffold, including the non-secret release signing block in `app/build.gradle.kts`, so CI does not need Android Studio or interactive project generation. Keep `src-tauri/gen/android/keystore.properties` ignored. Tauri's signing guide specifies this properties file and the required Gradle `signingConfigs.release` wiring ([Tauri Android code signing](https://v2.tauri.app/distribute/sign/android/#configure-the-signing-key)).

## Signing-key handling

Generate one long-lived release keystore once, offline, with `keytool`; Tauri's official example uses RSA 2048, a 10,000-day validity, and a named alias ([Tauri key generation](https://v2.tauri.app/distribute/sign/android/#creating-a-keystore-and-upload-key)). For this direct-distribution application, that key is the app signing key, not merely a disposable CI credential. Android requires APKs to be signed before installation or update, expects the app signing key to remain unchanged over the application's lifetime, and warns that losing a self-managed key prevents future versions from updating the installed app ([Android app signing](https://developer.android.com/studio/publish/app-signing#app-signing-key), [key security](https://developer.android.com/studio/publish/app-signing#secure-key)). Keep an encrypted offline backup separate from GitHub.

Create these repository or `release`-environment secrets:

- `ANDROID_KEY_BASE64`: base64 of the complete `.jks` file.
- `ANDROID_KEY_ALIAS`: the alias.
- `ANDROID_KEY_PASSWORD`: the key/store password used by the Tauri Gradle signing configuration.

GitHub explicitly supports base64-encoding small binary blobs into encrypted secrets while warning that base64 itself is not encryption ([GitHub binary secrets](https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/use-secrets#storing-base64-binary-blobs-as-secrets)). Tauri publishes a matching GitHub Actions step that decodes `ANDROID_KEY_BASE64` into `$RUNNER_TEMP` and writes `keystore.properties` from secrets ([Tauri CI signing example](https://v2.tauri.app/distribute/sign/android/#configure-the-signing-key)). Use `umask 077`, quote every expansion, never enable shell tracing, never print the properties file, and never upload or cache the keystore, properties file, runner temp directory, or signing passwords. GitHub also warns that cache contents must be treated as untrusted and must never contain credentials ([GitHub cache security](https://docs.github.com/en/actions/concepts/workflows-and-actions/dependency-caching#cache-security)).

Restrict the signing job to trusted tag pushes and explicit maintainer dispatches. Do not expose these secrets to pull-request workflows or use a `pull_request_target` path to release. The hosted runner is ephemeral, but the offline backup remains the recovery source of truth.

## Caching choices

Caching affects build time, not correctness. Key every cache from the relevant lockfile/toolchain inputs and keep platform/build target in the key.

| Cache | Recommended mechanism | Key inputs | Constraint |
|---|---|---|---|
| Frontend package downloads | Package-manager setup cache or `actions/cache` | OS, package-manager version, frontend lockfile | Cache the download store, not installed `node_modules`; install frozen/locked every run. |
| Rust registry/git data and `target` | `Swatinem/rust-cache` as in Tauri's maintained pipeline, pinned to a reviewed SHA | OS, Rust toolchain, `Cargo.lock`, target triple, relevant feature set | Separate desktop and Android target keys; never share incompatible target outputs. |
| Gradle wrapper/dependencies | `actions/setup-java` with `cache: gradle` | Gradle files, wrapper properties, lockfiles | Do not include `keystore.properties` or runner temp. |
| Android SDK/NDK | Prefer the runner image plus explicit `sdkmanager` selection | Exact SDK/build-tools/NDK versions | Avoid a custom large cache unless measurements justify it; the hosted image already includes common versions. |

GitHub-hosted jobs start on clean images and GitHub's cache is intended for reused downloaded dependencies; caches are distinct from workflow artifacts and can be evicted ([GitHub dependency caching](https://docs.github.com/en/actions/concepts/workflows-and-actions/dependency-caching)). The release must never depend on a cache surviving.

## Arch/Wayland runtime constraints

- The AppImage is x86_64 and must be made executable before launch. On Arch, AppImage's official troubleshooting guide requires `fuse2` (`sudo pacman -S fuse2`). If FUSE cannot be used, a type-2 AppImage can run with `--appimage-extract-and-run` or `APPIMAGE_EXTRACT_AND_RUN=1`, at a performance cost ([AppImage FUSE/Arch guidance](https://docs.appimage.org/user-guide/troubleshooting/fuse.html#setting-up-fuse-on-arch-linux), [extract-and-run fallback](https://docs.appimage.org/user-guide/troubleshooting/fuse.html#extract-and-run-type-2-appimages)).
- AppImage does not eliminate glibc compatibility constraints; that is why the Linux artifact is built on Ubuntu 22.04 rather than the rolling Arch host ([Tauri AppImage limitations](https://v2.tauri.app/distribute/appimage/#limitations)).
- Tauri renders through WebKitGTK on Linux. On some Wayland/NVIDIA combinations, WebKitGTK's DMABUF renderer can produce blank windows, flicker, or protocol errors. Test native Wayland first; only apply `__NV_DISABLE_EXPLICIT_SYNC=1`, `WEBKIT_DISABLE_DMABUF_RENDERER=1`, or the more expensive `WEBKIT_DISABLE_COMPOSITING_MODE=1` when the documented symptoms reproduce. Tauri cautions against shipping an unconditional override because it disables faster paths for working systems ([Tauri Linux graphics guidance](https://v2.tauri.app/develop/debug/linux-graphics/)).
- AppImage build success cannot establish multimedia compatibility. Acceptance requires launching the exact release artifact on the intended Arch/Wayland machine and playing representative live streams with audio, seeking/channel switching as applicable, and accelerated rendering observed.

## Alternatives considered

### Let `tauri-action` create the Release from each platform job

This is viable for desktop-only releases and is Tauri's documented simplest route. It can also change its command to `android build`, but its own README labels mobile support experimental ([tauri-action mobile input](https://github.com/tauri-apps/tauri-action/blob/abbd19ad15b37f89667db9cc46ec1cb5419f22be/README.md#L243-L261)). More importantly here, allowing parallel jobs to create/upload the Release exposes users to a partially populated release when Android signing or Linux packaging fails. The build-artifacts-then-publish topology is slightly more YAML but provides an atomic release gate.

### Build the AppImage locally on Arch

Rejected for releases. It raises the glibc baseline to a rolling distribution and conflicts with Tauri's oldest-supported-build-host guidance. Local Arch builds remain useful for development only.

### Publish an AAB or use Play App Signing

Rejected for this scope. An AAB is a store delivery format, while the requested artifact is directly installable. The workflow explicitly requests `--apk` and self-manages the long-lived signing key.

### Split the APK per ABI

Deferred until the target device ABI is recorded. One universal APK is larger but avoids publishing/installing the wrong architecture. For a confirmed ARM64-only device, a single `aarch64` APK is a straightforward later optimization.

## Release acceptance checklist

- The release workflow is tied to an existing tag whose version matches `tauri.conf.json`.
- AppImage and APK jobs use pinned runner/tool/action/dependency inputs and frozen lockfiles.
- Exactly one AppImage and one APK are produced; no AAB, updater metadata, store upload, or automatic-update artifact is produced.
- `apksigner verify --verbose --print-certs` succeeds and the certificate digest matches the recorded release certificate.
- The final job runs only after both build jobs succeed and uploads `*.AppImage`, `*.apk`, and `SHA256SUMS` to one GitHub Release.
- The exact AppImage runs on the intended Arch/Wayland host with `fuse2` (and with the extraction fallback documented), and representative streams play with audio.
- The exact APK installs on the physical device. A subsequent higher-`versionCode` rehearsal APK signed by the same key installs as an update, proving key continuity before the first public release.
- The keystore exists in an encrypted offline backup; neither the keystore nor generated signing properties appears in Git history, caches, workflow artifacts, logs, or Release assets.

## Residual risks

- Codec/container support and redistribution licensing remain coupled to the playback-engine decision and must be closed by a target-machine stream test.
- Tauri's current AppImage bundler downloads some packaging helpers from mutable `master`/`continuous` endpoints ([bundler tool download source](https://github.com/tauri-apps/tauri/blob/c0bd0d5a61eedba5c4783add24455c5028c6f390/crates/tauri-bundler/src/bundle/linux/appimage/linuxdeploy.rs#L207-L247)). Pinning Tauri is therefore necessary but does not make AppImage output bit-for-bit reproducible. Treat the artifact and checksum produced by the first successful tag run as canonical; do not rebuild and overwrite it.
- CI can prove compilation, packaging, and APK signature validity without an emulator. It cannot prove device-specific playback, WebView/media behavior, or Android installation/update UX; those remain physical-device release checks.
