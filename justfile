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

ci: check release-contract-check

release-contract-check:
    cd app && bun run release:contract static-check

build-hosted:
    cd app && bun install --frozen-lockfile
    cd app && bun run build

build-appimage:
    cd app && bun install --frozen-lockfile
    cd app && bun run release:contract prepare-appimage-tools
    cd app && bun run tauri build --bundles appimage

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

container-repro:
    bash scripts/verify-container-reproducibility.sh "${CONTAINER_REVISION:?CONTAINER_REVISION is required}" "${CONTAINER_OUTPUT:?CONTAINER_OUTPUT is required}"

container-rehearse:
    bash scripts/rehearse-hosted-container.sh "${CONTAINER_IMAGE:?CONTAINER_IMAGE is required}" "${CONTAINER_REVISION:?CONTAINER_REVISION is required}" "${CONTAINER_MANIFEST:?CONTAINER_MANIFEST is required}" "${CONTAINER_ENVIRONMENT_FILE:-.env.local}"

android-catalog-accept:
    cd app && bun run accept:android:catalog -- --apk "${ANDROID_ACCEPTANCE_APK:?ANDROID_ACCEPTANCE_APK is required}" --serial "${ANDROID_ACCEPTANCE_SERIAL:?ANDROID_ACCEPTANCE_SERIAL is required}" --output "${ANDROID_ACCEPTANCE_OUTPUT:?ANDROID_ACCEPTANCE_OUTPUT is required}"
