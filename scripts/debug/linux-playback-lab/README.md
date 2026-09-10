# Linux playback diagnostic harness

This is a deliberately small experiment, **not Sparrow's production player**.
It compares `mpegts.js 1.7.3 → WebKitGTK` with
`libmpv → GtkGLArea → GTK overlay + transparent WebKitGTK`.

For real desktop and packaged-app follow-up, use the
[cross-machine testing handoff](../../../docs/research/linux-in-app-playback-testing-handoff.md).

Run from the repository root:

```sh
python3 scripts/debug/linux-playback-lab/run.py
```

The default artifact directory is `/tmp/sparrow-playback-lab`. It contains the
generated 720p30 H.264/AAC MPEG-TS fixture, logs, screenshots, compiled harness
and `summary.json`. The runner downloads the pinned mpegts.js distribution if
absent. Nothing uses Sparrow's configuration or provider credentials.

Dependencies: Python 3, GCC, pkg-config, GTK3/WebKitGTK 4.1/libmpv/epoxy development
packages, GStreamer decoding and GL plugins, FFmpeg with libx264, Xvfb, Weston,
and ImageMagick's `import`. The runner does not install system packages.

Seven variants run sequentially, approximately 30 seconds each:

- X11 MSE with WebKit DMA-BUF rendering disabled.
- X11 MSE with accelerated shared-memory transport.
- X11 embedded libmpv with shared-memory WebKit transport.
- Native Wayland MSE with DMA-BUF rendering disabled.
- Native Wayland MSE with shared-memory WebKit transport.
- Native Wayland embedded libmpv with DMA-BUF rendering disabled.
- Native Wayland embedded libmpv with shared-memory WebKit transport.

For one variant:

```sh
python3 scripts/debug/linux-playback-lab/run.py --only wayland-embed-disable
```

X11 uses Xvfb. Native Wayland clients use Weston with its output nested inside
Xvfb so the final composed picture can be captured. This is **not XWayland
client testing**, Hyprland testing, or hardware-accelerated decoding. Mesa
software rendering is forced; embedded mpv uses software decode and a null
audio output. The first exploratory Wayland run also used headless Weston.

Each run requests resize, fullscreen/unfullscreen, release, resume, replacement
after a one-second release interval, and final stop. It then destroys the render
context before the mpv core. Under plain Xvfb there is no window manager to
honor fullscreen, so an action log is not proof of a fullscreen transition.
The page's Release/Restart buttons send commands through a WebKit script
message handler for embedded mpv. The final harness exercises those buttons
programmatically. This does not validate pointer hit testing, keyboard focus,
accessibility or production Tauri control wiring.

Read the logs and screenshots. `summary.json` reports exit, cleanup marker and
sample count only; it is **not a pass/fail playback or performance verdict**.
MSE logs media time, total/dropped frames and video-frame callback counts.
Embedded mpv logs time, decoder/output drops, render calls and changes to a
sample of framebuffer pixels. The framebuffer readback itself adds overhead.
Missing/zero callback counts need to be interpreted alongside the screenshot;
they are not proof of a blank or frozen picture.

The local server paces the same generated fixture through FFmpeg and logs
handler counts. A handler may notice a disconnected client on its next write;
these counts are not a rigorous network-ownership oracle. The one-second
replacement delay is a diagnostic convenience, **not** Sparrow's required
acknowledged stop-before-replace implementation. This harness cannot certify
source privacy or provider connection ownership in the installed application.

This bypasses Tauri, the Rust byte bridge and packaged AppImage dependencies.
It answers whether these rendering arrangements can work on this machine;
it cannot reproduce the earlier representative IPTV failure by itself.
The production follow-up must use Sparrow's actual adapters, representative
streams, packaged launcher, and the target Hyprland/GPU environment.

## Current desktop (Hyprland / XWayland)

```sh
python3 scripts/debug/linux-playback-lab/run.py --desktop \
  --output /tmp/sparrow-playback-desktop
```

This preserves the session's display sockets and runtime directory, removes
the forced software-rendering setting, and opens seven visible test windows
sequentially. It requires neither Xvfb nor Weston. On Hyprland, `hyprctl` and
`grim` record the actual backend and capture the test window. Other desktops
run without automatic screenshots. `GDK_BACKEND` and embedded `GL_RENDERER`
are also logged. Decode/audio settings remain `hwdec=no` / `ao=null`.

Use `--only wayland-mse-shm,wayland-embed-shm` for a subset, or
`--prepare-only` to compile and prepare assets without launching a window.
Unknown variant names fail explicitly. The fixture server selects a free
loopback port. Process failure or missing cleanup produces a nonzero runner
exit; successful exit alone still does not establish playback quality.

## Installed candidate with the actual Rust transport

Linux defaults to in-app MSE and offers an explicit **Open in mpv** button in
normal builds. The `linux-playback-lab` Cargo feature adds a bounded UI probe
and the `SPARROW_LINUX_PLAYBACK_ENGINE=mse|mpv` startup override for comparisons.
Without that override, feature builds also default to MSE. Explicit UI choices
take precedence and persist through resume.

Build a local AppImage with the same preparation as `just build-appimage`:

```sh
cd app
bun install --frozen-lockfile
bun run release:contract prepare-appimage-tools
NO_STRIP=1 bun run tauri build --features linux-playback-lab --bundles appimage
cd ..
```

Requires `patchelf` on PATH. This is an experimental candidate, not a release
artifact. If a moving upstream helper URL no longer matches its SHA-256 pin,
recover a matching cached helper from another trusted machine and verify its
digest; do not substitute an unchecked download.

Prepare a **private single-channel M3U** with an anonymous label such as
`Private sample A`, then run:

```sh
python3 scripts/debug/linux-playback-lab/installed-run.py \
  --app target/release/bundle/appimage/Sparrow_0.11.4_amd64.AppImage \
  --catalog /private/path/sample.m3u \
  --output /tmp/sparrow-private-comparison --seconds 65
```

The runner creates an isolated app-data directory, serves only the catalog
to Rust over loopback, and selects the first visible Channel through the real
UI. The three default runs compare XWayland MSE disabled/SHM with external
mpv. `--only wayland-mse-shm` selects another combination. Check window metadata
for the actual backend: an AppImage launcher may override the requested one.
External mpv retains access to the real Wayland display for its own window.

`--seconds` accepts 10–900; the probe samples once a second, then stops/exits.
MSE records media time, frame counters, dimensions and UI state. External mpv
is sampled over its private IPC socket; the unused video-element counters
are not mpv measurements. Loaded media/graphics library paths are recorded.
Raw logs and screenshots are private and must be reviewed before sharing.
The generated JSON contains only numeric/selected diagnostic properties.
These are sequential live inputs, not byte-identical replays. The probe does
not by itself verify A/V sync, pointer interactions, rapid switching or a
performance-equivalence tolerance.

Use `--only x11-default-shm` to test the real startup default without an engine
override. Add `--switch-players --seconds 90` to click **Open in mpv** at 20s
and **Play in app** at 50s, then inspect the `mpv`, `playing`, and `fullscreen`
samples alongside socket/process cleanup. Fullscreen still needs trusted desktop
input; the probe does not bypass browser user-gesture requirements.

The frame pacing probe also records per-interval frame/media/animation gap
summaries, explicit media-time writes, forward buffer duration, and cumulative
waiting/seeking events. Compare identical probe builds and renderer variants.
For a completed run, summarize the steady interval with:

```sh
python3 scripts/debug/linux-playback-lab/summarize-installed.py \
  /private/run/x11-default-shm-samples.json --start 20
```

See the [frame pacing diagnosis](../../../docs/research/linux-in-app-frame-pacing.md)
for the original latency-chasing policy, controlled comparisons, and limitations.
The [startup follow-up](../../../docs/research/linux-in-app-startup-buffering.md)
covers burst settling, a five-second reserve, and cooldown. Summaries also report
time to the first playing sample, startup seeks/waiting, and maximum startup
frame and media-timestamp gaps. The initial play request itself can produce a
waiting event; startup waiting counts are not all post-start stalls.
