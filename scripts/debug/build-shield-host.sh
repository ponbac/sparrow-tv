#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/../.."
mode=${1:-build}
case "$mode" in
    check|build|release) ;;
    *) printf 'Usage: %s [build|check|release]\n' "$0" >&2; exit 2 ;;
esac

android_sdk="${ANDROID_HOME:?Set ANDROID_HOME to the Android SDK}"
config=$(bun -e 'import { android } from "./release/toolchain.json"; console.log(android.ndk, android.minSdk, ...android.abis)')
read -r ndk_version min_sdk supported_abis <<< "$config"
export ANDROID_NDK_HOME="$android_sdk/ndk/$ndk_version"
toolchain="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
export CARGO_TARGET_DIR
CARGO_TARGET_DIR="$(realpath -m -- "${CARGO_TARGET_DIR:-target}")"

features=()
if [[ "${SPARROW_SHIELD_FIXTURE:-0}" == 1 ]]; then
    if [[ "$mode" == release ]]; then
        echo "Android release builds exclude fixture commands" >&2
        exit 2
    fi
    features=(--features fixture)
fi

android_project=app/src-shield/android
gradle=("$android_project/gradlew" -p "$android_project" --no-daemon --max-workers=2)
abis=("${SPARROW_SHIELD_ABI:-arm64-v8a}")
gradle_task=:app:assembleDebug
if [[ "$mode" == release ]]; then
    read -r -a abis <<< "$supported_abis"
    unset SPARROW_SHIELD_ABI
    gradle_task=:app:assembleRelease
    # Check the production signing guard before compiling Rust for every ABI.
    "${gradle[@]}" "$gradle_task" --dry-run
fi

if [[ "$mode" != check ]]; then
    export SPARROW_SHIELD_JNI_LIBS
    SPARROW_SHIELD_JNI_LIBS=$(mktemp -d)
    trap 'rm -rf -- "$SPARROW_SHIELD_JNI_LIBS"' EXIT
fi

for abi in "${abis[@]}"; do
    case "$abi" in
        arm64-v8a) target=aarch64-linux-android; clang_target=$target ;;
        armeabi-v7a) target=armv7-linux-androideabi; clang_target=armv7a-linux-androideabi ;;
        x86) target=i686-linux-android; clang_target=$target ;;
        x86_64) target=x86_64-linux-android; clang_target=$target ;;
        *) echo "Unsupported Shield ABI: $abi" >&2; exit 2 ;;
    esac
    target_env=${target//-/_}
    clang="$toolchain/$clang_target$min_sdk-clang"
    export "CARGO_TARGET_${target_env^^}_LINKER=$clang"
    export "CC_${target_env}=$clang" "CXX_${target_env}=$clang++" "AR_${target_env}=$toolchain/llvm-ar"
    build_args=(--locked --package sparrow-shield --target "$target" "${features[@]}")
    if [[ "$mode" == check ]]; then
        cargo check "${build_args[@]}"
        cargo clippy "${build_args[@]}" -- -D warnings -D clippy::debug_assert_with_mut_call
    else
        cargo build --release "${build_args[@]}"
        install -D -m 644 "$CARGO_TARGET_DIR/$target/release/libsparrow_shield.so" \
            "$SPARROW_SHIELD_JNI_LIBS/$abi/libsparrow_shield.so"
    fi
done

if [[ "$mode" != check ]]; then
    "${gradle[@]}" "$gradle_task"
fi
