# Continue Linux in-app playback testing on another machine

## Objective and current evidence

Determine whether Sparrow should regain an in-app picture through WebKit/MSE
or embedded libmpv. Production still launches external system mpv. This PR
contains research, a diagnostic harness and an opt-in installed MSE candidate,
not a production engine switch or a Tauri embedding implementation.

The [Hyprland laptop continuation](linux-in-app-playback-hyprland.md) records
target-desktop tests and the installed candidate. The harness now supports
`--desktop`; its [README](../../scripts/debug/linux-playback-lab/README.md)
also documents the private installed-app comparison runner.

Start with the [local results](linux-in-app-playback-local-experiment.md) and
[options](linux-in-app-playback-options.md). Nine recorded software-rendered
runs completed: six renderer variants, then three control-wiring checks.
Both approaches displayed a generated 720p30 H.264/AAC MPEG-TS fixture.
Those original virtual-display runs did not reproduce the provider-stream
failure. See the laptop continuation for the later real-stream results.

The main acceptance machine uses Hyprland. Distinguish its native Wayland
clients from XWayland clients: the currently documented AppImage launcher uses
X11/XWayland even inside a Hyprland session.

## 1. Any Linux development machine: reproduce the baseline

Check out this PR's branch. Install the prerequisites listed in the
[harness README](../../scripts/debug/linux-playback-lab/README.md). Package
names vary by distribution; verify the compilation dependencies with:

```sh
pkg-config --modversion gtk+-3.0 webkit2gtk-4.1 mpv epoxy
```

From the repo root, run:

```sh
python3 scripts/debug/linux-playback-lab/run.py --output /tmp/sparrow-playback-lab
```

This downloads pinned mpegts.js, generates a fixture, compiles the C harness
and runs seven sequential variants in about four minutes. It does not install
system packages or use Sparrow's configuration. Inspect `summary.json`, the
per-variant logs and screenshots, and `runner-server.log` in the output directory.

**This command always forces software rendering and virtual displays. Running
it on the Hyprland computer does not test Hyprland or GPU decoding.** Use it to
detect missing dependencies or establish that the same synthetic experiment
works there. Nonzero exits, missing video, stuck teardown or missing libraries
should be investigated before proceeding.

## 2. Hyprland machine: run directly in the desktop session

Run from a terminal inside the user's Hyprland session with its real
`WAYLAND_DISPLAY`, `DISPLAY` and `XDG_RUNTIME_DIR`. Do not reuse the runner's
private Weston socket/runtime directory. The following commands assume step 1
has generated `/tmp/sparrow-playback-lab/player` and its fixture.

In terminal A, from the repo root, start the local fixture server:

```sh
python3 scripts/debug/linux-playback-lab/server.py /tmp/sparrow-playback-lab 18765
```

In terminal B, run each of these sequentially, waiting for exit and the server
to return to `active=0`. They use only the generated fixture. Each finishes
after approximately 28 seconds.

Native Wayland MSE with the compatibility renderer policy:

```sh
env -u DISPLAY -u LIBGL_ALWAYS_SOFTWARE -u WEBKIT_DMABUF_RENDERER_FORCE_SHM \
  GDK_BACKEND=wayland WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  /tmp/sparrow-playback-lab/player mse \
  http://127.0.0.1:18765/ http://127.0.0.1:18765/stream.ts
```

Native Wayland MSE with accelerated shared-memory transport:

```sh
env -u DISPLAY -u LIBGL_ALWAYS_SOFTWARE -u WEBKIT_DISABLE_DMABUF_RENDERER \
  GDK_BACKEND=wayland WEBKIT_DMABUF_RENDERER_FORCE_SHM=1 \
  /tmp/sparrow-playback-lab/player mse \
  http://127.0.0.1:18765/ http://127.0.0.1:18765/stream.ts
```

Native Wayland embedded libmpv beneath the web UI:

```sh
env -u DISPLAY -u LIBGL_ALWAYS_SOFTWARE -u WEBKIT_DMABUF_RENDERER_FORCE_SHM \
  GDK_BACKEND=wayland WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  /tmp/sparrow-playback-lab/player embed \
  'http://127.0.0.1:18765/?embed' http://127.0.0.1:18765/stream.ts
```

Repeat the embedded command with the SHM policy used by the second command.
For the XWayland comparison, preserve the session's `DISPLAY`, unset
`WAYLAND_DISPLAY`, select `GDK_BACKEND=x11`, and use the SHM policy. Compare
MSE and embedded modes separately. This tests XWayland composition, but still
does not test the AppImage's bundled libraries or generated launcher.

These direct runs allow the desktop's GL implementation to be selected. Check
the embedded run's `GL_RENDERER` output: an actual GPU is not guaranteed merely
because `LIBGL_ALWAYS_SOFTWARE` is unset. **The C harness still explicitly sets
`hwdec=no` and `ao=null` for libmpv.** This stage tests native composition with
software decoding; it cannot establish GPU decode, zero-copy or audible audio.
MSE's decoder selection is independent and must be recorded rather than assumed.

Observe the picture and web overlays through resize, fullscreen/escape,
release/resume and replacement. The script drives button clicks and fullscreen
requests; manually inspect focus, menus, monitor placement and scale. Confirm
`WINDOW_STATE fullscreen=1` then `0` when supported, and final cleanup. Keep
screenshots/logs in a local output directory. Stop terminal A with Ctrl-C when
all runs are finished, after its active count has returned to zero.

If only one backend or renderer policy fails, retain that differential. Do not
change codecs, buffers and GPU options together in an attempt to get a pass.

## 3. Implementation machine: test the real Sparrow boundary

The `linux-playback-lab` Cargo feature enables a startup engine override
and bounded measurements through the real Rust byte transport. Follow the
harness README to build and run an isolated candidate. The embedded Tauri
adapter and broader acceptance work remain outstanding.

1. Build the opt-in `linux-playback-lab` candidate described in the README,
   retaining external mpv as the comparison. It routes MSE through Sparrow's
   existing native adapter and Rust byte stream.
2. On the target machine, compare external mpv, MSE with the previous disabled
   renderer, and MSE with the corrected packaged XWayland/SHM renderer. Keep
   the original representative streams and other settings fixed. Native
   Wayland is a separate variant; verify the actual backend after launch rather
   than assuming environment variables override an AppImage hook.
3. If MSE still fails, implement a minimal Tauri GTK GLArea/libmpv surface.
   Preserve Rust-only source resolution, session identity and acknowledged
   release-before-replace. The harness's one-second delay is not an ownership
   implementation. Do not copy Cathode's leaked player lifetime.
4. Compare embedded playback to Sparrow's actual CLI settings (`gpu-next`,
   Wayland), recording decoder and output modes. Enable and test hardware
   decoding as a separate deliberate change; add the required native display
   interop where applicable. The GL renderer may differ from the CLI renderer.
5. Build a local candidate with the README's feature-enabled equivalent of
   `just build-appimage` and inspect the actual resolved libmpv/WebKit/GStreamer
   dependencies. Repeat
   the target-host checks on that package. Development runs are not package
   acceptance. Follow [personal release acceptance](../release/personal-acceptance.md)
   only when testing an actual immutable release candidate; these experiments
   do not authorize a release or satisfy that checklist.

Android/Media3 and hosted playback do not need a new engine for this experiment.
Run their relevant regression checks if shared presentation contracts change.

## Evidence to return to the PR

Use one report per machine/backend/package combination. Keep provider URLs,
credentials and catalog names out of reports. Raw GStreamer or player logs
from real streams require private handling and review before sharing.

| Field | Record |
| --- | --- |
| Revision and artifact | Commit, harness vs installed candidate, package hash if applicable |
| Host | Distribution; compositor/version; GPU/driver; monitor refresh and scale |
| Actual rendering path | Native Wayland vs XWayland vs X11; renderer environment; GL renderer; decoder/copy mode |
| Runtime | GTK, WebKitGTK, GStreamer/plugin and mpv/libmpv versions actually loaded |
| Input | Synthetic fixture or private sample ID; codecs/profiles; resolution, frame rate, interlacing |
| Startup and steady playback | First-picture latency; run duration; frame counters with their definitions; stalls; A/V sync; CPU/GPU/memory |
| Interactions | Resize, fullscreen, overlays, guide scrolling, focus, monitor/scale changes, tracks, release/resume, rapid switching |
| Cleanup | Stop and exit; renderer failure; connection ownership; lingering process/surface checks |
| Verdict | Reproduced failure, successful limited experiment, or candidate meeting the agreed gate; unresolved limitations |

For candidate performance, propose three steady runs of at least 60 seconds
plus a ten-minute soak and an interaction pass, against the same external-mpv
baseline. Agree numerical equivalence tolerances before collecting the verdict.
The current 28-second harness is a smoke experiment and needs extension or a
candidate-specific driver for those longer measurements. Do not compare raw
counter names across engines without checking their meaning; missing counters
are not zero drops.

## Accepted default and remaining work

On 2026-09-09 the owner chose in-app Linux playback as the default despite the
remaining frame pacing issues, with an explicit button to open mpv. This
supersedes the previous performance-equivalence gate for the default. Follow
[ADR 0001](../adr/0001-shared-native-http-playback.md): improve the integrated
player while retaining source privacy and release-before-switch ownership.
Embedded libmpv still needs representative-stream performance and cleanup
validation before replacing the current in-app implementation.
