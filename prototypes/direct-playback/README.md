# Sparrow direct playback prototype

> **THROWAWAY PROTOTYPE — never merge this directory into `master`.**

## Question

Which shortlisted path can play the owner's live IPTV feeds directly—without a hosted or localhost proxy—and survive channel changes, reconnects, lifecycle transitions, and a long soak on the target Arch/Wayland host and Android device?

The probe keeps channel URLs only in memory and shows a redacted host in its state panel. It compares ordinary WebView video, browser-transport `mpegts.js`, Tauri-native-transport `mpegts.js`, and an external mpv 0.41 Wayland baseline. The media surface requires a WebView, so the pure playback state reducer is driven by this deliberately plain Tauri shell instead of the Prototype skill's usual terminal shell.

The test build intentionally permits any HTTP/HTTPS playback host and Android cleartext traffic. Those broad permissions are acceptable only on this throwaway branch.

## Run on Linux

```sh
cd prototypes/direct-playback
bun install
bun run prototype
```

Paste a direct channel URL, select a candidate, and press **Start**. Use **Pause/resume**, **Restart**, **Fullscreen**, and **Stop** while watching the complete state panel. mpv is Linux-only and uses a private JSON IPC socket; the URL is sent over IPC rather than exposed in the process command line.

## Build and run on the Android emulator

Use the user-scoped toolchain prepared by the Wayfinder prerequisite:

```sh
export JAVA_HOME="$HOME/.local/share/mise/installs/java/temurin-17.0.20+8"
export ANDROID_HOME="$HOME/.local/share/android-sdk"
export NDK_HOME="$ANDROID_HOME/ndk/29.0.14206865"
export ANDROID_AVD_HOME="$XDG_CONFIG_HOME/.android/avd"
export PATH="$JAVA_HOME/bin:$ANDROID_HOME/platform-tools:$ANDROID_HOME/emulator:$ANDROID_HOME/cmdline-tools/latest/bin:$PATH"
cd prototypes/direct-playback
bun run tauri android build --debug --apk --target x86_64
```

The generated APK path is printed by Tauri. The emulator can be started with:

```sh
emulator @sparrow_api_36 -no-window -no-audio -no-boot-anim -gpu software -no-snapshot-save
```

For the physical device, build the ARM64 probe and install the printed APK with `adb install -r`:

```sh
bun run tauri android build --debug --apk --target aarch64
```

The currently verified ARM64 artifact is `src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`. It is a local build artifact and is intentionally excluded from the prototype branch.

## Evidence checklist

For each applicable candidate and representative feed:

1. Cold-start three times; record first-frame time and codec/media metadata.
2. Rotate among representative feeds 30 times; confirm each old connection is released.
3. Remove networking for 30 seconds three times; record whether playback recovers within 30 seconds.
4. Exercise pause/resume and fullscreen on Linux, and rotation/background/foreground/lock on Android ten times.
5. Soak the representative HD feed for 90 minutes. Use the one-minute samples in the state/event panel to inspect decoded/dropped frames, speed, stalls, errors, and reconnects.
6. Export the redacted JSON evidence. Add physical-device make/model, Android version, WebView version, and candidate verdict to the Wayfinder ticket; never add source URLs.

The Android Media3 native baseline is deliberately staged after these shared-path probes: it should be built if the WebView candidates fail or are materially unstable on the physical device. The emulator cannot establish actual-device codec or hardware-decoder reliability.
