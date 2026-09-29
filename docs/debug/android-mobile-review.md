# Android mobile review

Browser resizing can check the shared guide layout, but Android verification
must use the installed APK: Media3 draws a native surface above the WebView.
Check rotation, fullscreen controls, system bars, the soft keyboard, and
background/resume on Android. Emulator frame timing is not a physical-device
performance result; retain the [physical playback acceptance](../acceptance/android-playback-performance.md)
for performance claims.

## Local toolchain

Point `ANDROID_HOME` at the installed SDK and `NDK_HOME` at its
`ndk/29.0.14206865` directory. The build also needs Android platform 36,
build-tools 35.0.0, the repository's pinned Java, and the Rust Android targets.
On the current development host the SDK is `/usr/lib/android-sdk`.
The ignored `mise.local.toml` sets these two paths for this checkout.

Use an isolated AVD so testing never replaces a personal Source Configuration:

```sh
export ANDROID_HOME=/usr/lib/android-sdk
export NDK_HOME="$ANDROID_HOME/ndk/29.0.14206865"
emulator -accel-check
avdmanager create avd --name sparrow-mobile-review \
  --package 'system-images;android-36;google_apis;x86_64' --device pixel_6
emulator -avd sparrow-mobile-review -no-window -no-audio \
  -no-boot-anim -no-snapshot -gpu swiftshader_indirect -memory 2048 -cores 2
```

Use `adb devices` to select this emulator explicitly. Build from `app/`:

```sh
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_STRIP=symbols \
  mise exec -- bun run tauri android build --apk --debug --ci \
  --split-per-abi --target aarch64 x86_64
```

Do not run `just check-app` concurrently with an APK build: both write
`app/dist`, and the hosted and installed builds use different asset paths.
Run the frontend checks before building the APK.

## Review procedure

Configure only generated M3U/XMLTV fixtures, with paced H.264/AAC MPEG-TS
test video and two Audio Tracks. An `adb reverse` mapping can expose a local
fixture server to the emulator. Keep fixture locations and all private
configuration out of review screenshots and logs.

- Check unconfigured setup, then the populated guide in portrait and landscape.
- Start a Playback Session; verify increasing Media3 frame counters.
- Open Channel search, Channel Groups, and Feeds during playback. The native
  picture must not cover any panel controls.
- Open the soft keyboard and confirm the input, close action, and results remain
  usable. Check both orientations.
- Scroll the guide horizontally and vertically. The time ruler and playhead
  must remain aligned with Programme cells.
- Fail a source refresh while playing, then restore it and refresh again. The
  native picture must follow the video slot when the retained-catalog banner
  appears and disappears, even when the slot's size stays unchanged.
- Enter fullscreen in landscape and wait for the control-hide timer. The
  native picture must fill the display with no reserved control rows or system
  bars. Tap the picture to restore controls; pause and verify Resume stays
  available. Check Back and the exit button while playing and paused, then
  verify system bars and guide insets return.
- Change Audio Track, switch Channel, pause/resume, and background/foreground
  the Activity. Check recovery and stop without overlapping Playback Sessions.

For debug APKs, connect `agent-browser` to the app's forwarded WebView DevTools
socket and use DOM controls. Capture the device with `adb exec-out screencap -p`:
a WebView-only screenshot cannot show the native video surface. Use only
allowlisted playback counters in diagnostic output, never Playback Sources.

The native content view owns system-bar, display-cutout, and IME insets using
[Android's inset APIs](https://developer.android.com/develop/ui/views/layout/edge-to-edge).
Media3 geometry remains relative to the inset WebView, so CSS should not apply
those Android insets a second time.

Fullscreen follows the document's lifetime, independently of native Playback
Session pause/replacement. An origin-restricted WebView message listener owns
Android's [immersive system bars](https://developer.android.com/develop/ui/views/layout/immersive)
and Back handling. While fullscreen, system bars can be revealed transiently
by swiping from the edge; the picture keeps its full viewport.
