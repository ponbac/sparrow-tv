# Shield development APK

The Shield development app opens the installed Channel Catalog and provides the Now and Timeline Guide, Channel and Programme Search, Channel Group visibility, and Source Configuration. Media3 presents Playback Sessions in the Guide or fullscreen. The development APK uses its own Android package and data directory.

With Java 17, `ANDROID_HOME`, NDK 29 and the `aarch64-linux-android` Rust target available, run from the workspace root.

```sh
just check-shield-host
just build-shield-host
SPARROW_SHIELD_FIXTURE=1 just build-shield-host
```

The default build omits fixture commands. The fixture-enabled build takes the same development application ID, `xyz.ponbac.sparrow.shield`. The ARM64 APK is non-debuggable and development-signed at `app/src-shield/android/app/build/outputs/apk/release/app-release.apk`. The shared Gradle wrapper supplies the pinned build and Kotlin dependencies. The GPUI backend requires Android API 26 for `libnativewindow`. Cargo resolves the pinned `gpui-mobile` Git revision from the shared workspace lock, including when building the hosted server and Tauri.

Set `ANDROID_SERIAL` on the ADB host to the intended Shield. The following commands address only the development APK. Serve a synthetic `catalog.m3u` and `guide.xml` on the ADB host's loopback port 18108 before seeding. The HTTPS command separately tests platform certificate verification at a fixed public endpoint.

```sh
adb install -r app/src-shield/android/app/build/outputs/apk/release/app-release.apk
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
app/src-tauri/gen/android/gradlew -p app/src-shield/android --write-locks :buildEnvironment :app:resolveAndLockBuildClasspaths
```
