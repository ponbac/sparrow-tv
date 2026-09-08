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

Six variants run sequentially, approximately 30 seconds each:

- X11 MSE with WebKit DMA-BUF rendering disabled.
- X11 MSE with accelerated shared-memory transport.
- X11 embedded libmpv with shared-memory WebKit transport.
- Native Wayland MSE with DMA-BUF rendering disabled.
- Native Wayland MSE with shared-memory WebKit transport.
- Native Wayland embedded libmpv with DMA-BUF rendering disabled.

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
