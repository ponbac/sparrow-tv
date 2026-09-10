# Upstream implementations of an in-app Linux IPTV player

Date: 2026-09-08. Source inspection only; no upstream application was built or benchmarked for this note.

## Finding

An in-app picture does not require returning to browser decoding. Existing Tauri applications put a native libmpv renderer underneath a transparent WebKitGTK interface. Cathode supplies the closest concrete implementation for Sparrow: GTK manages the video widget, so application code does not separately implement Wayland and X11 windows. This establishes feasibility, not acceptance on Sparrow's representative streams or packaged graphics stack. [Cathode Linux implementation](https://github.com/kaiserbh/cathode/blob/064ac3911b95277e551e0c367bbe1f0b7486e5c1/src-tauri/src/playback/linux.rs)

The smallest useful native experiment is a GTK `GLArea` displaying libmpv video inside Sparrow's existing Tauri window, with controls remaining in React. Keep the external player as the measured comparison. Do not equate successful native decode with successful native composition: rendering, overlays, driver interoperability, and teardown each need evidence.

## What comparable applications actually implement

| Application | Inspected implementation | Implication for Sparrow |
|---|---|---|
| Cathode | Tauri v2, GTK3 `Overlay` containing `GLArea` and transparent webview; libmpv OpenGL render API. | Strongest architectural reference for one GTK implementation covering both display backends. |
| MaxVideoPlayer | Custom Tauri plugin; current Linux renderer requires Wayland and owns EGL/subsurface resources. | Useful reference for explicit compositor control, but materially more machinery. README's X11 claim does not match inspected implementation. |
| Hypnotix | GTK3 drawing area, libmpv `wid` set to an X11 window ID; startup clears `WAYLAND_DISPLAY`. | Evidence for in-app native playback, not evidence of native Wayland embedding. |

Pinned code snapshots: Cathode `064ac3911b95277e551e0c367bbe1f0b7486e5c1`, MaxVideoPlayer `903aa0d2c30e41f7f321cb8242efe926ba99ea9d`, Hypnotix `0e0fa1c7596f7925c715c36efb0e4be53a3bde43`. Sources are linked in the sections below.

### Cathode: use GTK's video surface

`attach` replaces Tauri's default vertical box with an overlay directly under the GTK window. The GL area is the base child and the webview an overlay child. Its comments explain a critical integration constraint: the webview must remain two parent hops from the window for Tauri's borderless resize handler. This is native widget integration, not a DOM `<video>` replacement.

An mpv update callback signals a channel; a GTK main-loop task queues rendering. Realize creates the render context. Render obtains GTK's currently bound framebuffer and uses allocated dimensions multiplied by scale factor. This is actual code, including the framebuffer handling that a superficial “embed mpv” proposal omits. [Linux surface source](https://github.com/kaiserbh/cathode/blob/064ac3911b95277e551e0c367bbe1f0b7486e5c1/src-tauri/src/playback/linux.rs)

Cathode creates its mpv instance with `vo=libmpv` and `hwdec=auto-safe`, then deliberately leaks it to provide a static lifetime for its render context. That lifetime choice is not an appropriate ready-made proof of Sparrow's cleanup guarantees; Sparrow should own its resources explicitly. [Playback owner source](https://github.com/kaiserbh/cathode/blob/064ac3911b95277e551e0c367bbe1f0b7486e5c1/src-tauri/src/playback/mod.rs)

Linux builds link system libmpv; the documented packages include libmpv, GTK3 and WebKitGTK development libraries. This adds a shared-library packaging requirement even when no mpv executable is launched. [Build and distribution documentation](https://github.com/kaiserbh/cathode/blob/064ac3911b95277e551e0c367bbe1f0b7486e5c1/README.md)

### MaxVideoPlayer: own the Wayland surface

The current renderer matches only Wayland raw window/display handles, and rejects other handles. It creates a child surface, places its subsurface below the parent, and owns EGL resources. It dispatches rendering to the GLib main thread, handles decoration offsets, hides the surface by moving it off-screen, and recreates EGL surfaces during resizing. This is substantially more protocol and resource-lifetime code than the GTK widget approach.

Its embedded options explicitly choose `hwdec=auto-copy`: the source comments report colour corruption with direct VAAPI texture interop on some drivers. Therefore “hardware accelerated” does not establish zero-copy decoding. [Linux renderer and options](https://github.com/MaxMB15/MaxVideoPlayer/blob/903aa0d2c30e41f7f321cb8242efe926ba99ea9d/crates/tauri-plugin-mpv/src/linux.rs)

On renderer initialization/attachment failure, the coordinator recreates its libmpv engine with fallback output options and loads the stream. The separate window is created through libmpv; this inspected path does **not** spawn the mpv executable. The previous Sparrow research note's “spawned mpv window” wording should not be read as process-level equivalence. [Load and fallback coordinator](https://github.com/MaxMB15/MaxVideoPlayer/blob/903aa0d2c30e41f7f321cb8242efe926ba99ea9d/crates/tauri-plugin-mpv/src/mpv.rs)

### Hypnotix: X11 embedding

Startup clears `WAYLAND_DISPLAY` when present. `reinit_mpv` instantiates Python's libmpv wrapper with `wid` obtained from the GTK drawing area's `get_xid()`. The README separately recommends `vo=x11` and `GDK_BACKEND=x11` for Wayland sessions. It supports the product idea of embedded native IPTV playback, while illustrating why window-ID embedding is not the first choice for Sparrow's native Wayland target. [Application source](https://github.com/linuxmint/hypnotix/blob/0e0fa1c7596f7925c715c36efb0e4be53a3bde43/usr/lib/hypnotix/hypnotix.py), [README](https://github.com/linuxmint/hypnotix/blob/0e0fa1c7596f7925c715c36efb0e4be53a3bde43/README.md)

## Constraints that the prototype must respect

**mpv render API ownership.** Upstream recommends its render API over `wid` because toolkit/platform embedding can cause problems. Create the render context before video starts and destroy it before the mpv core. Rendering requires the same current GL context; callbacks must schedule work rather than call render functions themselves. Avoid synchronous waits or locks between the renderer and unsafe ordinary mpv API calls: upstream documents deadlocks and degraded playback from that pattern. GTK can own the rendering thread while a separate controller owns normal commands and events. [mpv render API contract](https://github.com/mpv-player/mpv/blob/master/include/mpv/render.h)

**Hardware decode is a separate acceptance item.** mpv supports hardware decode through the render API, but direct Intel/Linux interoperability requires EGL plus a native X11 or Wayland display parameter. The inspected Cathode context supplies OpenGL initialization parameters but no native display parameter. Consequently its source cannot establish direct hardware decode parity for Sparrow. Measure the selected decoder and frame-copy mode; compare correctness and CPU/GPU load with the external player. [mpv OpenGL API contract](https://github.com/mpv-player/mpv/blob/master/include/mpv/render_gl.h), [Cathode context creation](https://github.com/kaiserbh/cathode/blob/064ac3911b95277e551e0c367bbe1f0b7486e5c1/src-tauri/src/playback/linux.rs)

**GTK and Tauri integration.** GTK's GLArea owns a GL context and framebuffer, makes them available during its render signal, and supports explicit queued redraws. Tauri exposes the underlying platform webview on the main thread through `with_webview`; its documentation recommends pinning at least the Tauri minor version because native binding dependencies may change. Native container rearrangement still couples an adapter to Tauri/wry implementation details, so test actual bundled windows, decorations and resize behavior. [GTK GLArea](https://docs.gtk.org/gtk3/class.GLArea.html), [Tauri webview access](https://docs.rs/tauri/2.11.5/tauri/webview/struct.Webview.html#method.with_webview)

## Proposed bounded experiment

This is a recommendation inferred from the implementations, not a claim that upstream has validated Sparrow:

1. Integrate one GTK-owned libmpv surface with a transparent video region and existing app controls. First prove an in-window picture from a local fixture; then use the representative stream with the source confined to Rust.
2. Exercise Wayland and X11/XWayland, resize, maximize/fullscreen, scale changes, control overlays, menus and channel-list scrolling during playback. A full-window native surface behind web content must not show through unintended transparent areas.
3. Match external mpv's stream quality: sustained displayed cadence, decoder/output drops, audio sync and resource use. Record actual hardware decode mode; do not infer it from an option setting.
4. Prove channel switching, stop, resource-releasing pause/resume, close and renderer failure release the provider connection and native objects. Reject unbounded redraw notifications and avoid leaking the player to solve Rust lifetimes.
5. Build the intended package with its libmpv runtime dependency and repeat the same checks outside development mode.

A separate owned EGL/Wayland renderer is a second experiment if GTK composition demonstrably fails. Adopting MaxVideoPlayer's protocol machinery before establishing that failure would increase Sparrow's platform-specific maintenance without evidence it is necessary.
