# Bringing Linux playback back inside Sparrow

Date: 2026-09-08

## Recommendation

Aim for an in-app picture backed by libmpv. First run a bounded WebKit/MSE differential on the corrected packaged renderer: there is new evidence that makes that experiment worth doing. If it still fails the representative streams, prototype libmpv's OpenGL render API in a GTK `GLArea`, using Cathode's integration as a reference. Do not restore MSE as default merely because the old adapter still exists.

This began as a source investigation and proposed experiment. The subsequent [local experiments](linux-in-app-playback-local-experiment.md) exercised both paths with synthetic MPEG-TS under virtual X11 and Wayland displays; neither has passed representative-stream or packaged-build acceptance. Sparrow's current primary remains unchanged. The historical note is [Linux mpv primary decision](linux-mpv-primary-decision.md); the detailed, pinned upstream implementation audit is [Linux in-app player upstream](linux-in-app-player-upstream.md).

The user confirmed that the eventual main machine uses Hyprland. This research environment appears headless: `DISPLAY`, `WAYLAND_DISPLAY` and session-type variables are unset, and no Hyprland, Xorg or XWayland process was found. The NVIDIA/Hyprland measurements cited below are historical repository evidence, not observations of this environment. Acceptance must run on the target Hyprland machine, distinguishing native Wayland from the AppImage's XWayland path inside that session. Local compilation or headless tests cannot establish graphical performance there.

## A new reason to retest MSE

The mpv promotion landed in `3c210e1`. That revision's `configure_platform_before_webview` unconditionally set `WEBKIT_DISABLE_DMABUF_RENDERER=1`. The later `5d78d2f` changed this after a packaged-build differential found severely impaired scrolling. On the measured X11/XWayland AppImage path, shared-memory transport retaining accelerated composition delivered about 60 fps versus about 29 fps for the original setting. Native Wayland still retains the disable compatibility policy. See the [recorded measurements](linux-scroll-performance-debugging.md) and [current renderer selection](../../app/src-tauri/src/lib.rs).

**Inference:** the original MSE verdict was made before this compositor correction, so it does not establish MSE performance under the corrected configuration. This is a reason to measure again, not evidence that decoding, buffering, or video presentation is now fixed. UI animation counters cannot substitute for media counters.

WebKitGTK uses GStreamer for media, including MSE. Its documentation provides GStreamer logging and pipeline graph diagnostics. Those can distinguish missing/incorrect decoder selection from starvation upstream of decoding. Its graphics documentation describes a separate video-frame path into composition. Codec decoding and the final composited picture therefore need separate investigation. Sources: [WebKit multimedia](https://docs.webkit.org/Ports/WebKitGTK%20and%20WPE%20WebKit/Multimedia.html), [WebKit graphics](https://docs.webkit.org/Ports/WebKitGTK%20and%20WPE%20WebKit/Graphics.html).

Also, MPEG-TS itself is not proof that MSE is unsuitable. `mpegts.js` explicitly targets live television and transmuxes TS into fragmented MP4 for MSE. Its supported combinations include H.264/H.265 with AAC; its README lists additional audio support and explicitly excludes MPEG-2 video. Parsing a codec does not guarantee that the installed browser can decode it. Record the actual video/audio codecs and profiles of every test stream. Source: [mpegts.js README](https://github.com/xqq/mpegts.js).

Sparrow declares `mpegts.js ^1.7.3` and carries a patch for that release in [package.json](../../app/package.json). Upstream documents worker-MSE improvements in 1.8, but its Safari/Chrome statements are not proof of support in Sparrow's WebKitGTK. Treat an upgrade and worker support as separate capability-tested differentials, including custom-loader compatibility and the existing prototype-freeze patch; do not mix them into the compositor baseline.

## Comparing the approaches

| Approach | In-app experience | Main cost or uncertainty | Assessment for Sparrow |
| --- | --- | --- | --- |
| Restore WebKit/MSE | Normal HTML video, CSS overlays and browser controls | Previous stream failures; exact codecs, GStreamer pipeline and composition remain relevant | Cheapest experiment because the adapter survives; promotion needs new evidence |
| libmpv render API + GTK GLArea | Native video behind the web UI, within Sparrow's window | GTK/GL lifetime, transparency, viewport alignment, frame pacing and packaging | Preferred native prototype; Cathode has concrete code using this arrangement |
| Custom EGL Wayland subsurface | Native video within the window, potentially separate render scheduling | Display-protocol ownership, stacking, input, scaling and backend-specific implementations | Reserve for an identified GLArea limitation; MaxVideoPlayer is a reference, not a ready cross-backend solution |
| Spawn mpv with an X11 parent window (`wid`) | Embedded child window on X11/XWayland | Ties embedding to X11 and native child geometry | Possible if deliberately accepting XWayland; poor basis for native Wayland support |
| Keep separate system mpv | Existing reliable playback, separate picture | User's integration problem remains | Benchmark and possible explicit fallback |

The upstream audit verifies Cathode's GTK overlay/GLArea implementation. Hypnotix's current code clears `WAYLAND_DISPLAY` and uses an XID. MaxVideoPlayer's audited Linux implementation rejects non-Wayland handles despite broader README claims; its separate-window fallback recreates libmpv with normal output rather than spawning the CLI executable. These are implementation precedents, not evidence of acceptance on Sparrow's GPU, AppImage or streams. See [pinned source findings](linux-in-app-player-upstream.md).

Switching the whole shell to Electron or replacing the playback engine with GStreamer/libVLC would enlarge the experiment. Neither is necessary to try the two credible paths already available here. Remuxing alone still leaves browser decode/composition; transcoding would add a new processing and latency cost. Those are reserve options if the narrower experiments reveal a specific need.

## What embedding libmpv would actually require

The proposed path is `Rust source resolution → libmpv → native GL framebuffer → GTK window`, with React supplying controls and the visible video rectangle. Frames do not pass through JavaScript, MSE, a canvas upload loop or a local HTTP proxy. The GTK widget and transparent WebView compose the final in-app picture.

The current [mpv adapter](../../app/src-tauri/src/playback/mpv.rs) launches `--vo=gpu-next --gpu-context=wayland`. A GLArea integration changes the rendering path even though it retains mpv's media engine. Match actual decoder configuration, measure presentation drops as well as decoder drops, and do not assume CLI results transfer automatically.

The libmpv API requires a current, consistent OpenGL context, correct framebuffer dimensions and ordered teardown. Rendering notifications must schedule GTK work instead of making unsafe calls from mpv callbacks; render-context destruction must precede mpv-core destruction. Direct hardware decode interop may need the native display handle and the appropriate GL/EGL setup. Sources: [render API](https://github.com/mpv-player/mpv/blob/master/include/mpv/render.h), [OpenGL API](https://github.com/mpv-player/mpv/blob/master/include/mpv/render_gl.h).

Cathode's inspected code leaks its mpv owner to obtain a static lifetime and does not pass native display parameters at render-context creation. MaxVideoPlayer chooses a hardware decode copy mode because of reported texture interop corruption. Borrow their rendering arrangement, not those choices without evaluation; neither source proves Sparrow's cleanup or hardware-decode requirements. See the [upstream audit](linux-in-app-player-upstream.md).

Concrete Sparrow work would include:

1. A Linux surface owner created through the Tauri/GTK main-thread boundary. Own realization, resize, redraw and destruction there; keep blocking playback work away from GTK callbacks.
2. A distinct embedded presentation descriptor and adapter. The existing [installed engine router](../../app/src/features/playback/installed-playback-engine.ts) already selects platform presentations. Its current Linux branch assumes a process; it is not a drop-in native-surface interface.
3. A transparent video region, with the surrounding UI opaque. Synchronize clipped bounds, scrolling, visibility and display scale. The [Android adapter](../../app/src/features/playback/android-media3-engine.ts) provides a local example of viewport synchronization, but Linux stacking and coordinate rules require their own implementation.
4. Window fullscreen with retained controls, keyboard focus, track selection and error overlays. The current [surface](../../app/src/features/playback/playback-surface.tsx) assumes an HTML video element; native embedding must provide real playback status rather than relying on its DOM media events.
5. Preserve session identity, stop-before-replace, pause releasing the provider connection, resume at live edge, and shutdown cleanup. A native library cannot use the current process kill/reap mechanism as its escape hatch. Prove callback cancellation and teardown under failure.
6. Package a supported libmpv runtime or declare it as a dependency. Finding the `mpv` executable does not verify the shared-library ABI or render support. Inspect the finished AppImage's resolved libraries on a clean target system. Embedding also places native-engine failures in the application's process; retaining process isolation would be a separate, larger surface-sharing design.

For the first prototype, preserve the current Linux ownership model: Rust gives the privately resolved source directly to the native engine, with no source in JS, argv, UI events or routine logs. An embedded engine does not need a Unix socket just to keep the source away from JavaScript. Keep configuration/script loading controlled and translate events into safe counters and error enums.

Rust-owned byte delivery is a later, independent option. libmpv has a custom stream callback API, but it is marked unstable, uses blocking read semantics and forbids callbacks into the same mpv instance from stream callbacks. Its lifetime rules require deliberate cancellation. Do not combine a new renderer and a new transport in the first differential. Source: [stream callback API](https://github.com/mpv-player/mpv/blob/master/include/mpv/stream_cb.h).

## Experiments and decision gates

Run these sequentially so there is only one provider connection. Use the previously failing representative channels, plus a small codec/frame-rate/interlacing matrix drawn from the real catalog. Keep identities private. A replay fixture, where available and permitted, helps hold input bytes constant; retain a live run to test real pacing and reconnection.

**First: bounded MSE retest.** Build a development-only engine selection into a packaged candidate. Compare the existing mpv path, retained MSE under the old disable setting, and retained MSE under the corrected X11/XWayland shared-memory policy. Test native Wayland separately. Keep the stream adapter and JS settings fixed initially. Record the actual package, WebKit/GStreamer versions, GPU/driver, backend and renderer mode.

Measure startup to first picture, expected content cadence, decoded and presented/dropped frames where available, buffering/stalls, A/V sync, CPU/GPU load and memory growth. Use repeated steady runs and a longer soak, including guide interaction, fullscreen, channel changes and pause/resume. Do not equate differently defined counters across engines or treat a zero/missing counter as a pass. Diagnostic logs/graphs must stay private and be reviewed before sharing because they may contain source data.

If MSE fails, classify it before changing another variable: data starvation or append timing; codec/decoder failure; or frames decoded but poorly presented. Then test only the relevant change, such as a verified decoder/plugin correction or a supported worker path. Stop this branch if it requires indefinite buffering/bridge tuning without improving delivered video.

**Second: libmpv surface spike.** Start with one visible rectangle and existing source resolution, using GLArea and a transparent WebView. Verify it in the actual packaged launcher on the target machine before adding all controls. Compare against the current `gpu-next` process baseline. Test native Wayland and X11/XWayland independently, particularly on the recorded NVIDIA/hybrid-GPU host.

Once frame delivery matches, exercise resize, fractional scaling and monitor moves, opaque dialogs over video, guide scrolling, fullscreen/escape, audio tracks, rapid switching, pause/resume, hide/show, stop and app exit. Inject renderer initialization failure and interruption during startup/teardown. Assert no lingering owner or overlapping connection; verify source privacy in emitted events, logs and UI.

**Promotion:** apply [ADR 0001's existing gate](../adr/0001-shared-native-http-playback.md): equivalent representative-stream playback plus privacy and single-owner cleanup. Define numerical tolerances before collecting the candidate result; do not invent a passing threshold afterward. If both pass, MSE offers simpler DOM integration, while libmpv offers continuity with the native playback engine; choose against measured stream coverage and desktop behavior. If only libmpv passes, promote the embedded presentation. Any external fallback should follow confirmed native cleanup and explicit user action.

Update ADR 0001 and reconcile the historical handoff only when an implementation decision is accepted. This investigation supplies a path to that decision, not a replacement architecture decree.
