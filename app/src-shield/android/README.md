# Shield development APK

The Shield development app opens the installed Channel Catalog and provides the Now and Timeline Guide, Channel and Programme Search, Channel Group visibility, and Source Configuration. Media3 presents Playback Sessions in the Guide or fullscreen. The development APK uses its own Android package and data directory.

With Java 17, `ANDROID_HOME`, NDK 29 and the `aarch64-linux-android` Rust target available, run from the workspace root.

```sh
just check-shield-host
just build-shield-host
SPARROW_SHIELD_FIXTURE=1 just build-shield-host
```

The debug APK uses the development application ID `xyz.ponbac.sparrow.shield`,
the Gradle debug key, and an optimized Rust library. It uses `SPARROW_SHIELD_ABI`
(default `arm64-v8a`) and writes
`app/src-shield/android/app/build/outputs/apk/debug/app-debug.apk`.
`SPARROW_SHIELD_FIXTURE=1` enables the private fixture commands in development builds.

`just build-android-release` builds one universal APK for `xyz.ponbac.sparrow`
in `app/build/outputs/apk/release/app-release.apk` relative to this Gradle project.
It requires the four `SPARROW_ANDROID_*` signing values documented in the Gradle
build. The project reads its SDK and release ABIs from `release/toolchain.json`,
its product version from `app/package.json`, and its Rust dependencies from the
workspace lockfile. The GPUI backend requires Android API 26 for `libnativewindow`.

Set `ANDROID_SERIAL` on the ADB host to the intended Shield. The following commands address only the development APK. Serve a synthetic `catalog.m3u` and `guide.xml` on the ADB host's loopback port 18108 before seeding. The HTTPS command separately tests platform certificate verification at a fixed public endpoint.

```sh
adb install -r app/src-shield/android/app/build/outputs/apk/debug/app-debug.apk
adb reverse --no-rebind tcp:18108 tcp:18108
adb shell am start -S -W -n xyz.ponbac.sparrow.shield/.ShieldActivity
adb shell am broadcast -n xyz.ponbac.sparrow.shield/.FixtureReceiver \
  -a xyz.ponbac.sparrow.shield.SEED_FIXTURE
adb shell am broadcast -n xyz.ponbac.sparrow.shield/.FixtureReceiver \
  -a xyz.ponbac.sparrow.shield.CHECK_HTTPS
adb shell dumpsys activity xyz.ponbac.sparrow.shield/.ShieldActivity --sparrow-guide
```

The Activity dump reports catalog phase, process-local generation changes, lifecycle, probe state, graphics backend, live GPUI windows, catalog subscriptions and Guide focus. The fixture receiver requires Android’s `DUMP` permission. Launch the Activity before sending either command. Poll the dump until the seed and probe finish. A complete seed requires both a Channel Catalog and a validated EPG Source. Keep the fixture reachable while testing playback of its synthetic streams. Remove the reverse for offline catalog tests.

```sh
adb reverse --remove tcp:18108
```

`initial_window_ms` records the first successful window open. `windows_opened` counts all successful opens in the process. D-pad arrows move among the Toolbar, Channel Group lane, Now cards and Timeline cells. Center on the Now/Timeline control switches views. The Activity dump includes focus kind, Channel Number, first partially visible row, scroll offset in 1920-wide reference units and loaded family count. The `sparrow-shield-v3` snapshot also reports mode, focused Programme index and the Timeline’s pan in minutes. Home reattaches the host's surface, and Back followed by relaunch opens a new Activity over the same process catalog.

Center on a Channel starts playback. A fresh Center press on the playing Channel opens fullscreen. Channel keys browse the visible catalog in Channel Number order. Back closes the editor or panel first, then leaves fullscreen, then closes the Activity. Home suspends playback when Android hides the Activity. The app resumes only a visibility-owned pause.

The dump includes Search result counts, the hidden Channel Group count, panel state, Playback Session phase, Channel Number, fullscreen, and decoded frames. Source locations, Search terms and Channel names stay out of the dump.

## Android TV emulator

Install the SDK emulator, platform-tools, and `system-images;android-36;android-tv;x86_64` under `ANDROID_HOME`. The harness uses Java 17, the `x86_64-linux-android` Rust target, Python 3, FFmpeg with libx264 and drawtext, and ImageMagick. From the workspace root, build your worktree's APK and create the shared 1080p TV AVD once.

```sh
SPARROW_SHIELD_ABI=x86_64 SPARROW_SHIELD_FIXTURE=1 just build-shield-host
scripts/debug/shield-emulator.py create
```

Run `scripts/debug/shield-fixture.py` in a separate terminal. One loopback server on port 18108 serves all instances. It caches the synthetic media in `${XDG_CACHE_HOME:-$HOME/.cache}/sparrow/shield-fixture-v1`, loops it during playback, and generates current XMLTV schedules on request.

Choose a free even console port from 5554 through 5584 for each instance. Each headless emulator uses the same read-only AVD with a disposable data layer. Agents can use separate worktrees, build outputs and serials concurrently. The default GPU is `host`; `start --gpu swiftshader` selects software rendering.

```sh
emu() { scripts/debug/shield-emulator.py --serial emulator-5576 "$@"; }
emu start
emu install
emu seed
emu dump
emu shot guide.png
emu key CENTER                 # Start docked playback on the focused Channel.
emu shot docked.png
emu key CENTER                 # Enter fullscreen.
emu key BACK
emu record focus.mp4 5 &
record_pid=$!
sleep 1                        # screenrecord needs about a second to start.
emu key --delay 0.5 RIGHT RIGHT DOWN LEFT UP
wait "$record_pid"
emu frames focus.mp4 focus-frames 3
emu stop
```

`start` waits for boot and maps the fixture port. `seed` waits for the Channel Catalog and validated EPG Source. Use `dump` to confirm `playback=Playing` and decoded frames before capturing video. `frames` writes PNG frames and a contact sheet to a new or empty directory.

## Code map

| Behavior | Source |
| --- | --- |
| Guide focus, paging, Search and group selection | [sparrow-tv-guide](../../../crates/sparrow-tv-guide) |
| Source Configuration and durable Guide preferences | [sparrow-installed-catalog](../../../crates/sparrow-installed-catalog) |
| Playback Session timers, recovery and transport | [sparrow-installed-playback](../../../crates/sparrow-installed-playback) |
| Activity, GPUI layout and JNI glue | [sparrow-shield](../../../crates/sparrow-shield) |
| Shared Media3 player, transport reader and audio focus | [Android playback](../../android-playback) |
| Channel Icon fetch and decode bounds | [sparrow-channel-icons](../../../crates/sparrow-channel-icons) |

Update the Gradle locks after an intentional dependency change.

```sh
app/src-shield/android/gradlew -p app/src-shield/android --write-locks :buildEnvironment :app:resolveAndLockBuildClasspaths
```
