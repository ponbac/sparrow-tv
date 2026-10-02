# Android mobile review

Android always uses the pocket layout ([ADR 0007](../adr/0007-use-a-pocket-layout-where-the-picture-cannot-be-covered.md)):
watch mode with the picture across the window, guide mode with the picture
docked to a band above a list of what each Channel has on.
A browser window narrower than 1051 px or shorter than 601 px shows the same
layout and can check the two modes, the list and the time chips, but Android
verification must use the installed APK: Media3 draws a native surface above
the WebView, and it follows the picture's box only while a presentation is
live. In a browser the page draws the picture, so it docks in every state:
the dock latch never holds, and the box is never held at its size.
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
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_OPT_LEVEL=3 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_STRIP=symbols \
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
  With nothing playing there is no picture box: the list fills the window
  under the masthead.
- Start a Playback Session from a list row; verify increasing Media3 frame
  counters. The app must enter watch mode: the picture across the window, the
  lower third, one row of controls, what follows, and the channel bar at the
  bottom. Nothing may lie over the picture. Open **More**: the menu must open
  downward, clear of it. In landscape it must open beside its button, towards
  the list, and still clear of the picture.
- Press **Guide** in the channel bar. The native picture must follow its box
  into the band at the top left, the playing row must be in the middle of the
  list, and no part of the guide may sit under the picture. Return with the
  button beside the band and again with a tap on the picture; the picture
  must fill the width each time.
- Check the dock latch. Pause in watch mode: the lower third must read
  "Paused". Press **Guide**: the picture must stay across the window with the
  guide below it. Still paused, tap the search field: the soft keyboard makes
  the window lower, and the picture must keep its size with the search field
  and **Sources** under it, not behind it. Open **Sources** there: it must
  open under the picture. Tap the picture to return, then **Resume**. Then,
  in guide mode, interrupt the fixture stream so that playback is
  reconnecting or has failed, and return to the picture: it must stay
  band-sized with the watch screen under it and the state named in the lower
  third, and fill the width once it plays again.
- Press previous and next in the channel bar. The lower third must change at
  once and the Channel must tune a moment later; the description and what
  follows appear once the Channel is tuned, and must not move after that.
  Press next a dozen times quickly: no press may be lost. Hide a large
  Channel Group, play the Channel just before it and press next: the bar
  must offer the first Channel after the group. The button is disabled only
  at either end of the Channel Catalog.
- Press the masthead's search button from watch mode. The guide must open with
  the search field focused and the soft keyboard up, and the picture must be
  in its band.
- Open the time chip at the start of the Channel Groups and choose a later
  hour. Rows must show what is on then with its start and end, and the chip
  must turn amber and show the time. Pressing a row tunes the Channel now.
  Choose **Now** to return.
- Open the full channel search, **Choose groups**, and **Sources** during
  playback. Each panel must open under the band, or under the full-width
  picture while the latch holds it. The native picture must not cover any
  panel controls. With a panel open, tap the band beside the picture, and
  then the picture: each tap must close the panel and leave the guide open.
- Open the soft keyboard and confirm the input, close action, and results remain
  usable. Check both orientations.
- Scroll the list, and the description under the controls in watch mode. The
  picture must not move with either. Scroll the Channel Group chips sideways.
- Fail a source refresh while playing, then restore it and refresh again. The
  native picture must follow the video slot when the retained-catalog banner
  appears and disappears, even when the slot's size stays unchanged.
- Rotate to landscape in each mode. The picture, the lower third and the
  controls must be at the left and the list at the right, with no channel bar
  and no band. Rotate back: the mode must be the one it was.
- Still in landscape, pause, resume, and choose **Copy diagnostics** from
  **More**. The picture's box must keep its height through all of it: the
  controls stay on one line, which scrolls sideways to the message. No part
  of the lower third may be covered by a picture left at its old size.
  Pause again and tap the search field: the keyboard takes most of the
  window, and the picture must keep its size and its column, with the lower
  third cut at the keyboard and nothing under the picture.
- Load **More channels** twice, scroll down the list, then open the time chip.
  The list must keep its rows and its place.
- Enter fullscreen in landscape and wait for the control-hide timer. The
  native picture must fill the display with no reserved control rows or system
  bars. Tap the picture to restore controls: they are the player's own bar,
  with **Restart** and **Copy diagnostics** as buttons in it and no **More**
  menu. Pause and verify Resume stays available. Check Back and the exit
  button while playing and paused, then verify system bars and guide insets
  return and the icon controls are back in their row.
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

For large-catalog startup and offline checks, see [Catalog startup](catalog-startup.md).
