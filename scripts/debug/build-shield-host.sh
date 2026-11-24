#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/../.."
android_sdk="${ANDROID_HOME:?Set ANDROID_HOME to the Android SDK}"
ndk_version=29.0.14206865
export ANDROID_NDK_HOME="$android_sdk/ndk/$ndk_version"
toolchain="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$toolchain/aarch64-linux-android26-clang"
export CC_aarch64_linux_android="$CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"
export CXX_aarch64_linux_android="$toolchain/aarch64-linux-android26-clang++"
export AR_aarch64_linux_android="$toolchain/llvm-ar"
CARGO_TARGET_DIR="$(realpath -m -- "${CARGO_TARGET_DIR:-target}")"
export CARGO_TARGET_DIR

features=()
if [[ "${SPARROW_SHIELD_FIXTURE:-0}" == 1 ]]; then
    features=(--features fixture)
fi
build_args=(--locked --package sparrow-shield --target aarch64-linux-android "${features[@]}")

case "${1:-build}" in
    check)
        cargo check "${build_args[@]}"
        cargo clippy "${build_args[@]}" -- -D warnings -D clippy::debug_assert_with_mut_call
        ;;
    build)
        cargo build --release "${build_args[@]}"
        export SPARROW_SHIELD_JNI_LIBS="$CARGO_TARGET_DIR/shield-jni"
        install -D -m 644 \
            "$CARGO_TARGET_DIR/aarch64-linux-android/release/libsparrow_shield.so" \
            "$SPARROW_SHIELD_JNI_LIBS/arm64-v8a/libsparrow_shield.so"
        app/src-tauri/gen/android/gradlew -p app/src-shield/android --no-daemon --max-workers=2 :app:assembleRelease
        ;;
    *)
        printf 'Usage: %s [build|check]\n' "$0" >&2
        exit 2
        ;;
esac
