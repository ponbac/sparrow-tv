set shell := ["bash", "-euo", "pipefail", "-c"]

check: check-rust check-app

check-rust:
    cargo fmt --all --check
    cargo check --workspace --all-targets --locked
    cargo clippy --workspace --all-targets --locked -- -D warnings -D clippy::debug_assert_with_mut_call
    cargo test --workspace --all-targets --locked

check-app:
    cd app && bun install --frozen-lockfile
    cd app && bun run lint
    cd app && bun run test
    cd app && bun run build

ci: check release-contract-check hosted-shell-check

release-contract-check:
    cd app && bun run release:contract static-check

build-hosted:
    cd app && bun install --frozen-lockfile
    cd app && bun run build

build-appimage:
    command -v patchelf >/dev/null || { echo "patchelf is required to bundle GStreamer (Arch: sudo pacman -S --needed patchelf)" >&2; exit 1; }
    cd app && bun install --frozen-lockfile
    cd app && bun run release:contract prepare-appimage-tools
    cd app && NO_STRIP=1 bun run tauri build --bundles appimage

build-android-debug:
    cd app && bun install --frozen-lockfile
    cd app && bun run tauri android build --apk --debug --ci

build-android-release:
    cd app && bun install --frozen-lockfile
    cd app && bun run tauri android build --apk --ci

write-android-dependency-locks:
    android_ndk_toolchain="${NDK_HOME:?NDK_HOME must point to the pinned Android NDK}/toolchains/llvm/prebuilt/linux-x86_64/bin" && test -x "$android_ndk_toolchain/aarch64-linux-android24-clang" && TAURI_CONFIG='{}' TAURI_ANDROID_PROJECT_PATH="$PWD/app/src-tauri/gen/android" ANDROID_NDK_HOME="$NDK_HOME" CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$android_ndk_toolchain/aarch64-linux-android24-clang" CC_aarch64_linux_android="$android_ndk_toolchain/aarch64-linux-android24-clang" CXX_aarch64_linux_android="$android_ndk_toolchain/aarch64-linux-android24-clang++" AR_aarch64_linux_android="$android_ndk_toolchain/llvm-ar" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo check --package sparrow-installed --target aarch64-linux-android --locked
    cd app/src-tauri/gen/android && ./gradlew :app:resolveAndLockBuildClasspaths --write-locks --max-workers=2
    cd app/src-tauri/gen/android && ./gradlew -p buildSrc resolveAndLockBuildClasspaths --write-locks --max-workers=2

release-preflight:
    existing_release_tags="$(git tag --list 'v*')" && cd app && bun run release:contract preflight --mode "${RELEASE_MODE:?RELEASE_MODE is required}" --commit "${RELEASE_COMMIT:?RELEASE_COMMIT is required}" --output "${RELEASE_OUTPUT:?RELEASE_OUTPUT is required}" --ref-name "${RELEASE_REF_NAME:-}" --master-commit "${RELEASE_MASTER_COMMIT:-}" --requested-version "${RELEASE_REQUESTED_VERSION:-}" --existing-tags "$existing_release_tags" --github-output "${RELEASE_GITHUB_OUTPUT:-}"

release-stage:
    cd app && bun run release:contract stage --kind "${RELEASE_KIND:?RELEASE_KIND is required}" --version "${RELEASE_VERSION:?RELEASE_VERSION is required}" --input "${RELEASE_INPUT:?RELEASE_INPUT is required}" --output "${RELEASE_OUTPUT:?RELEASE_OUTPUT is required}"

release-verify-appimage:
    cd app && bun run release:contract verify-appimage --version "${RELEASE_VERSION:?RELEASE_VERSION is required}" --artifact "${RELEASE_ARTIFACT:?RELEASE_ARTIFACT is required}"

release-smoke-appimage:
    cd app && bun run release:contract smoke-appimage --version "${RELEASE_VERSION:?RELEASE_VERSION is required}" --artifact "${RELEASE_ARTIFACT:?RELEASE_ARTIFACT is required}"

release-verify-apk:
    cd app && bun run release:contract verify-apk --version "${RELEASE_VERSION:?RELEASE_VERSION is required}" --artifact "${RELEASE_ARTIFACT:?RELEASE_ARTIFACT is required}"

release-verify-android-toolchain:
    cd app && bun run release:contract verify-android-toolchain

release-verify-candidate:
    cd app && bun run release:contract verify-candidate --version "${RELEASE_VERSION:?RELEASE_VERSION is required}" --directory "${RELEASE_DIRECTORY:?RELEASE_DIRECTORY is required}" --repository "${RELEASE_REPOSITORY:?RELEASE_REPOSITORY is required}" --tag "${RELEASE_TAG:?RELEASE_TAG is required}" --commit "${RELEASE_COMMIT:?RELEASE_COMMIT is required}" --run-id "${RELEASE_RUN_ID:?RELEASE_RUN_ID is required}" --run-attempt "${RELEASE_RUN_ATTEMPT:?RELEASE_RUN_ATTEMPT is required}"

release-acceptance-prepare:
    cd app && bun run release:acceptance prepare --candidate "${RELEASE_CANDIDATE:?RELEASE_CANDIDATE is required}" --output "${RELEASE_ACCEPTANCE_OUTPUT:?RELEASE_ACCEPTANCE_OUTPUT is required}"

release-acceptance-prove-continuity:
    cd app && test -n "${ANDROID_SERIAL:?ANDROID_SERIAL is required}" && bun run release:acceptance prove-continuity --candidate "${RELEASE_CANDIDATE:?RELEASE_CANDIDATE is required}" --previous-apk "${RELEASE_PREVIOUS_APK:?RELEASE_PREVIOUS_APK is required}" --previous-version "${RELEASE_PREVIOUS_VERSION:?RELEASE_PREVIOUS_VERSION is required}" --output "${RELEASE_ACCEPTANCE_OUTPUT:?RELEASE_ACCEPTANCE_OUTPUT is required}"

release-acceptance-seal:
    cd app && bun run release:acceptance seal --candidate "${RELEASE_CANDIDATE:?RELEASE_CANDIDATE is required}" --evidence "${RELEASE_ACCEPTANCE_EVIDENCE:?RELEASE_ACCEPTANCE_EVIDENCE is required}" --artifact-id "${RELEASE_ARTIFACT_ID:?RELEASE_ARTIFACT_ID is required}" --artifact-digest "${RELEASE_ARTIFACT_DIGEST:?RELEASE_ARTIFACT_DIGEST is required}" --output "${RELEASE_ACCEPTANCE_OUTPUT:?RELEASE_ACCEPTANCE_OUTPUT is required}"

release-acceptance-approve:
    cd app && bun run release:acceptance approve --candidate "${RELEASE_CANDIDATE:?RELEASE_CANDIDATE is required}" --evidence "${RELEASE_ACCEPTANCE_EVIDENCE:?RELEASE_ACCEPTANCE_EVIDENCE is required}" --sealed "${RELEASE_ACCEPTANCE_SEALED:?RELEASE_ACCEPTANCE_SEALED is required}"

container-repro:
    bash scripts/verify-container-reproducibility.sh "${CONTAINER_REVISION:?CONTAINER_REVISION is required}" "${CONTAINER_OUTPUT:?CONTAINER_OUTPUT is required}"

hosted-candidate-accept:
    bash scripts/accept-hosted-candidate.sh "${HOSTED_REPLACEMENT_IMAGE:?HOSTED_REPLACEMENT_IMAGE is required}" "${HOSTED_REPLACEMENT_REVISION:?HOSTED_REPLACEMENT_REVISION is required}" "${HOSTED_REPRODUCED_MANIFEST:?HOSTED_REPRODUCED_MANIFEST is required}" "${HOSTED_ACCEPTANCE_OUTPUT:?HOSTED_ACCEPTANCE_OUTPUT is required}"

hosted-shell-check:
    shellcheck scripts/accept-hosted-candidate.sh scripts/rehearse-hosted-cutover.sh scripts/verify-hosted-endpoint.sh

hosted-cutover-prepare:
    cd app && bun run hosted:cutover prepare --hosted-acceptance "${HOSTED_ACCEPTANCE:?HOSTED_ACCEPTANCE is required}" --baseline-compose "${HOSTED_BASELINE_COMPOSE:?HOSTED_BASELINE_COMPOSE is required}" --candidate-compose "${HOSTED_CANDIDATE_COMPOSE:?HOSTED_CANDIDATE_COMPOSE is required}" --environment-backup "${HOSTED_ENVIRONMENT_BACKUP:?HOSTED_ENVIRONMENT_BACKUP is required}" --caddy-backup "${HOSTED_CADDY_BACKUP:?HOSTED_CADDY_BACKUP is required}" --evidence-key "${HOSTED_EVIDENCE_KEY:?HOSTED_EVIDENCE_KEY is required}" --container "${HOSTED_CONTAINER:?HOSTED_CONTAINER is required}" --replacement-image "${HOSTED_REPLACEMENT_IMAGE:?HOSTED_REPLACEMENT_IMAGE is required}" --output "${HOSTED_OUTPUT:?HOSTED_OUTPUT is required}"

hosted-cutover-rehearse:
    bash scripts/rehearse-hosted-cutover.sh "${HOSTED_PLAN:?HOSTED_PLAN is required}" "${HOSTED_EVIDENCE_KEY:?HOSTED_EVIDENCE_KEY is required}" "${HOSTED_OUTPUT:?HOSTED_OUTPUT is required}"

hosted-cutover-readiness:
    cd app && bun run hosted:cutover verify-readiness --plan "${HOSTED_PLAN:?HOSTED_PLAN is required}" --rehearsal "${HOSTED_REHEARSAL:?HOSTED_REHEARSAL is required}" --hosted-acceptance "${HOSTED_ACCEPTANCE:?HOSTED_ACCEPTANCE is required}" --candidate "${RELEASE_CANDIDATE_MANIFEST:?RELEASE_CANDIDATE_MANIFEST is required}" --acceptance-verdict "${RELEASE_ACCEPTANCE_VERDICT:?RELEASE_ACCEPTANCE_VERDICT is required}" --baseline-compose "${HOSTED_BASELINE_COMPOSE:?HOSTED_BASELINE_COMPOSE is required}" --candidate-compose "${HOSTED_CANDIDATE_COMPOSE:?HOSTED_CANDIDATE_COMPOSE is required}" --environment-backup "${HOSTED_ENVIRONMENT_BACKUP:?HOSTED_ENVIRONMENT_BACKUP is required}" --caddy-backup "${HOSTED_CADDY_BACKUP:?HOSTED_CADDY_BACKUP is required}" --evidence-key "${HOSTED_EVIDENCE_KEY:?HOSTED_EVIDENCE_KEY is required}" --container "${HOSTED_CONTAINER:?HOSTED_CONTAINER is required}" --output "${HOSTED_OUTPUT:?HOSTED_OUTPUT is required}"

hosted-cutover-seal:
    cd app && bun run hosted:cutover seal-production-evidence --readiness "${HOSTED_READINESS:?HOSTED_READINESS is required}" --event "${HOSTED_PRODUCTION_EVENT:?HOSTED_PRODUCTION_EVENT is required}" --endpoint "${HOSTED_PRODUCTION_ENDPOINT:-}" --route-binding "${HOSTED_ROUTE_BINDING:-}" --baseline-compose "${HOSTED_BASELINE_COMPOSE:?HOSTED_BASELINE_COMPOSE is required}" --candidate-compose "${HOSTED_CANDIDATE_COMPOSE:?HOSTED_CANDIDATE_COMPOSE is required}" --environment-backup "${HOSTED_ENVIRONMENT_BACKUP:?HOSTED_ENVIRONMENT_BACKUP is required}" --caddy-backup "${HOSTED_CADDY_BACKUP:?HOSTED_CADDY_BACKUP is required}" --evidence-key "${HOSTED_EVIDENCE_KEY:?HOSTED_EVIDENCE_KEY is required}" --container "${HOSTED_CONTAINER:?HOSTED_CONTAINER is required}" --output "${HOSTED_OUTPUT:?HOSTED_OUTPUT is required}"

hosted-cutover-bind-route:
    cd app && bun run hosted:cutover record-route-binding --readiness "${HOSTED_READINESS:?HOSTED_READINESS is required}" --start "${HOSTED_OBSERVATION_START:?HOSTED_OBSERVATION_START is required}" --endpoint "${HOSTED_PRODUCTION_ENDPOINT:?HOSTED_PRODUCTION_ENDPOINT is required}" --caddy-backup "${HOSTED_CADDY_BACKUP:?HOSTED_CADDY_BACKUP is required}" --evidence-key "${HOSTED_EVIDENCE_KEY:?HOSTED_EVIDENCE_KEY is required}" --container "${HOSTED_CONTAINER:?HOSTED_CONTAINER is required}" --image-role "${HOSTED_IMAGE_ROLE:?HOSTED_IMAGE_ROLE is required}" --acknowledgement "${HOSTED_ROUTE_ACKNOWLEDGEMENT:?HOSTED_ROUTE_ACKNOWLEDGEMENT is required}" --output "${HOSTED_OUTPUT:?HOSTED_OUTPUT is required}"

hosted-cutover-observe-start:
    cd app && bun run hosted:cutover start-production-observation --readiness "${HOSTED_READINESS:?HOSTED_READINESS is required}" --evidence-key "${HOSTED_EVIDENCE_KEY:?HOSTED_EVIDENCE_KEY is required}" --output "${HOSTED_OUTPUT:?HOSTED_OUTPUT is required}"

hosted-cutover-observe-finish:
    cd app && bun run hosted:cutover finish-production-observation --start "${HOSTED_OBSERVATION_START:?HOSTED_OBSERVATION_START is required}" --route-binding "${HOSTED_ROUTE_BINDING:-}" --result "${HOSTED_RESULT:?HOSTED_RESULT is required}" --failure "${HOSTED_FAILURE:-}" --incident-reference "${HOSTED_INCIDENT_REFERENCE:-}" --evidence-key "${HOSTED_EVIDENCE_KEY:?HOSTED_EVIDENCE_KEY is required}" --output "${HOSTED_OUTPUT:?HOSTED_OUTPUT is required}"

android-catalog-accept:
    cd app && test -n "${ANDROID_SERIAL:?ANDROID_SERIAL is required}" && bun run accept:android:catalog -- --apk "${ANDROID_ACCEPTANCE_APK:?ANDROID_ACCEPTANCE_APK is required}" --output "${ANDROID_ACCEPTANCE_OUTPUT:?ANDROID_ACCEPTANCE_OUTPUT is required}"

android-playback-accept:
    cd app && test -n "${ANDROID_SERIAL:?ANDROID_SERIAL is required}" && bun run accept:android:playback -- --apk "${ANDROID_PLAYBACK_ACCEPTANCE_APK:?ANDROID_PLAYBACK_ACCEPTANCE_APK is required}" --output "${ANDROID_PLAYBACK_ACCEPTANCE_OUTPUT:?ANDROID_PLAYBACK_ACCEPTANCE_OUTPUT is required}"
