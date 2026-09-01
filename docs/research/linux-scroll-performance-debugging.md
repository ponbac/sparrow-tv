# Linux scroll-performance debugging

Date: 2026-09-01

## Conclusion

Profile Sparrow as a **WebKitGTK rendering problem first**, then split CSS
painting from JavaScript/React work. That workflow identified WebKitGTK buffer
transport as the defect: Sparrow set `WEBKIT_DISABLE_DMABUF_RENDERER=1`
unconditionally before Tauri created the webview, while the generated AppImage
GTK launcher also selected X11/XWayland.

On this computer Sparrow dynamically loads WebKitGTK 2.52.6 and GTK 3.24.52.
In WebKitGTK 2.52.6, the upstream implementation returns an empty renderer
transport mode when `WEBKIT_DISABLE_DMABUF_RENDERER` is nonzero; an empty mode
then prevents creation of the accelerated backing store. By contrast,
`WEBKIT_DMABUF_RENDERER_FORCE_SHM=1` adds shared-memory transport before it
returns. This behavior is explicit in the
[2.52.6 `AcceleratedBackingStore` source](https://github.com/WebKit/WebKit/blob/4fb33923db2f945803df49546f75867980365c08/Source/WebKit/UIProcess/gtk/AcceleratedBackingStore.cpp#L82-L117)
and its
[`create` guard](https://github.com/WebKit/WebKit/blob/4fb33923db2f945803df49546f75867980365c08/Source/WebKit/UIProcess/gtk/AcceleratedBackingStore.cpp#L195-L200).
The GTK port's
[`HardwareAccelerationManager`](https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebKit/UIProcess/gtk/HardwareAccelerationManager.cpp#L40-L55)
also turns hardware acceleration off when those backing-store requirements fail.
The resulting GTK preferences disable both accelerated compositing and threaded
scrolling
([2.52.6 `WebPreferencesGtk.cpp`](https://github.com/WebKit/WebKit/blob/4fb33923db2f945803df49546f75867980365c08/Source/WebKit/UIProcess/gtk/WebPreferencesGtk.cpp#L34-L49)).
That makes Sparrow's unconditional setting a much stronger initial hypothesis
than generic React memoization.

This does **not** mean that blindly enabling hardware DMA-BUF is safe on every
Linux GPU/driver combination. WebKitGTK introduced the disable switch as a way
to diagnose DMA-BUF regressions and asks graphics bug reports to include
`webkit://gpu` output
([WebKitGTK maintainer's accelerated-compositing notes](https://blogs.igalia.com/carlosgc/2023/04/03/webkitgtk-accelerated-compositing-rendering/)).
The measured fix selects `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1` for that packaged
X11/XWayland path. This retains the accelerated backing store without the
hardware DMA-BUF/GBM buffer failures seen on the target NVIDIA host. Native
Wayland retains the previous disable fallback, and an explicitly supplied
renderer variable always wins.

## Measured diagnosis on the target host

The same focused 1138×634 CSS-pixel window, loaded catalog, 160 Hz monitor, and
WebKit remote-inspector scroll loop were used for each warm differential. The
loop moved the document by a fixed amount on each `requestAnimationFrame` for
six to eight seconds and reported only aggregate frame timing.

| Variant | Mean fps | p95 frame | Frames over 33.3 ms | Result |
| --- | ---: | ---: | ---: | --- |
| Shipped AppImage: DMA-BUF renderer disabled | 28.95–29.11 | 37 ms | 60–65% | Reproduced the persistent jank |
| Rebuilt AppImage with explicit disable fallback, five warm runs | 17.67–18.29 | 72 ms | 75–76% | Confirmed the fallback remains severely janky |
| Animations disabled | 29.44 | 36 ms | 60% | No material change |
| Sticky rails disabled | 29.44 | 36 ms | 59% | No material change |
| `content-visibility` disabled | 28.97 | 37 ms | 67% | No improvement |
| All major paint effects flattened | 33.33 | 31 ms | 0% | Paint cost mattered slightly, but remained 30 Hz-class |
| Hardware DMA-BUF/GBM | N/A | N/A | N/A | Blank window or Wayland protocol error |
| Final AppImage, shared-memory transport, five warm runs | 60.09–60.20 | 17–18 ms | 0% | Smooth, stable result selected for the fix |

The baseline also recorded zero DOM mutations and zero long tasks during the
scroll interval. That falsified continuous React/query work as the cause. The
shared-memory result more than doubled delivered frames and eliminated every
multi-frame (>33.3 ms) stall in the measured run while preserving the rendered
catalog.

## Repository and machine facts

- The UI is React 18.3.1 built by Vite and hosted by Tauri 2.11 through Wry.
  On Linux the lockfile selects GTK 3 bindings and WebKitGTK. The direct release
  binary resolves the system GTK/WebKit libraries, while the tested AppImage
  bundles the same WebKitGTK 2.52.6 generation and its generated GTK hook sets
  `GDK_BACKEND=x11`.
- The original [`configure_platform_before_webview`](../../app/src-tauri/src/lib.rs)
  overwrote `WEBKIT_DISABLE_DMABUF_RENDERER` with `1` on every Linux launch.
  Supplying `WEBKIT_DISABLE_DMABUF_RENDERER=0` in the shell could not override
  it. The fixed policy preserves explicit configuration and selects the measured
  shared-memory mode for the AppImage's X11 backend.
- The host is Hyprland/Wayland with WebKitGTK 2.52.6, an NVIDIA RTX 4070 SUPER
  (610.57.04) plus an AMD integrated GPU. The focused display observed during
  research was 3840×2160 at 160 Hz and fractional scale 1.6667. That is a
  6.25 ms frame deadline, so the same defect may be less visible on a 60 Hz
  display.
- Normal Linux catalog scrolling has no application `scroll` or `wheel`
  listener. The one document-level scroll observer is confined to the Android
  Media3 playback adapter. React work should still be measured, but there is no
  repository evidence that ordinary Linux scrolling intentionally drives
  React state.
- The main catalog initially renders 24 channel cards and can append more pages.
  Cards use `content-visibility: auto`; the page also has large gradients,
  translucent sticky side rails with nested scrolling, and variable-font text
  in [`index.css`](../../app/src/index.css). These are useful isolation targets
  only after checking the renderer path.

## Actionable workflow

### 1. Establish a repeatable baseline

Use the same loaded catalog, window size, focused monitor, compositor session,
and 8–10 second scroll gesture for every run. Record at least five warm runs and
keep cold launch separate. Compare the shipped/release binary as well as an
instrumented build: Tauri and React development instrumentation can perturb the
result.

As a fast boundary test, serve the same frontend in a normal browser and repeat
the same scene. Smooth browser scrolling plus slow Tauri scrolling points toward
WebKitGTK/GTK/driver configuration; slowness in both points toward the document,
CSS, or React.

### 2. Inspect the real WebKit renderer before changing CSS

Open Tauri's Linux Web Inspector with `Ctrl+Shift+I` in a development or debug
build. Tauri documents that Linux uses WebKitGTK's inspector, that debug builds
enable it by default, and that release builds require the `tauri/devtools`
feature ([Tauri debugging guide](https://v2.tauri.app/develop/debug/)).

Capture `webkit://gpu` (or load `webkit://gpu/stdout` and retain the JSON from
the terminal). Record at minimum:

- display backend and DRM render node;
- GL vendor/renderer and whether Sparrow selected NVIDIA, AMD, or software;
- `Renderer` and supported buffer transports;
- whether 2D canvas/accelerated compositing is active.

WebKitGTK added richer DMA-BUF format/modifier data and the stdout form
specifically to make Linux graphics reports diagnosable
([WebKitGTK maintainer's 2.46 graphics notes](https://blogs.igalia.com/carlosgc/2024/09/27/graphics-improvements-in-webkitgtk-and-wpewebkit-2-46/)).

Then make three diagnostic builds, changing only the pre-webview environment
selection:

1. **Default/hardware-capable:** neither variable set (or disable set to `0`).
2. **Accelerated backing store with SHM transport:** only
   `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`.
3. **Current behavior:** only `WEBKIT_DISABLE_DMABUF_RENDERER=1`.

Do not set `DISABLE_DMABUF_RENDERER` and `FORCE_SHM` together: in the pinned
2.52.6 source the disable check returns first. For each variant capture
`webkit://gpu`, the same scroll runs, visual correctness, stderr, and any crash
or Wayland protocol error. A large improvement in variant 1 or 2 directly
identifies the renderer configuration; choose between them based on stability
on this NVIDIA/Wayland host.

If the default DMA-BUF path is unstable, compare `GDK_BACKEND=wayland` and
`GDK_BACKEND=x11` as a second, separately recorded A/B. GTK 3 documents this
backend selector and its other runtime diagnostics in the
[GTK 3 running guide](https://docs.gtk.org/gtk3/running.html). Do not apply GTK
4-only `GSK_RENDERER` advice to this GTK 3 build.

### 3. Use Web Inspector to classify each slow frame

In Web Inspector, record the Timelines **Layout and Rendering**, **JavaScript and
Events**, and CPU views while performing only the baseline scroll.
The [official Web Inspector Timelines reference](https://webkit.org/web-inspector/timelines-tab/)
describes the event, layout/rendering, CPU, and frames views.

- If JavaScript/Event work fills slow frames, inspect the triggering handler and
  use React's `<Profiler>` around the catalog/search/status subtrees. React's
  profiler reports commit `actualDuration` and `baseDuration`, but adds overhead
  and is disabled in normal production builds
  ([React Profiler reference](https://react.dev/reference/react/Profiler)).
- If layout/paint dominates with little script, enable Paint Flashing. WebKit's
  own guidance recommends the Layout and Rendering timeline and Paint Flashing
  to identify actual repaint work, including offscreen overdraw tiles
  ([WebKit performance guidance](https://webkit.org/blog/8970/how-web-content-can-affect-power-usage/)).
- If neither Web content category explains the missed frames, continue with a
  system trace: GTK/WebKit buffer transfer, the WebProcess paint workers, the
  compositor, or the GPU driver is the likely boundary.

For a noisy but useful diagnostic build, launch with
`WEBKIT_SHOW_COMPOSITING_DEBUG_VISUALS=1` to show WebKit's compositing
indicators, or `WEBKIT_DEBUG="Scrolling Compositing Tiling"` to enable selected
Linux log channels. Both controls come directly from the pinned 2.52.6 tree:
[`WebKitSettings.cpp`](https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebKit/UIProcess/API/glib/WebKitSettings.cpp#L341-L350)
and WebKit's
[logging instructions](https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Introduction.md#enabling-and-disabling-log-channels).

### 4. Isolate CSS one feature at a time

Apply temporary inspector overrides, repeat the identical trace, then revert
before testing the next one:

1. Change `.channel-card` from `content-visibility: auto` to `visible`. The CSS
   Containment specification says `auto` may skip offscreen layout/rendering,
   but revealing skipped contents is work on viewport entry
   ([CSS Containment Level 2](https://www.w3.org/TR/css-contain-2/#using-cv-auto)).
   An open WebKit report describes repeated layout and scroll jank with many
   `content-visibility: auto` blocks in Safari 26; it is a diagnostic lead, not
   proof that WebKitGTK has the same defect
   ([WebKit bug 318216](https://bugs.webkit.org/show_bug.cgi?id=318216)).
2. Replace `.catalog-shell`'s repeating and linear gradients with a solid color.
3. Make `.group-rail` and `.channel-inspector` non-sticky, opaque, and
   non-scrollable, testing each property family separately.
4. Disable all animations/transitions. Persistent loaded-state chrome is mostly
   static, so this should be neutral; a difference indicates an unexpectedly
   active loading/checking state.
5. Repeat with 24 cards and with several appended pages. A cost that scales with
   page count motivates list windowing or stricter containment; jank at 24 cards
   does not.

Avoid combining these overrides: the point is to attribute the improvement to
one rendering feature. WebKit recommends normal browser scrolling over
JavaScript-controlled scrolling and notes that continual animation, layout, and
painting consume the same CPU/GPU budget needed by interaction
([WebKit performance guidance](https://webkit.org/blog/8970/how-web-content-can-affect-power-usage/)).

### 5. Capture the whole Linux graphics pipeline when needed

WebKitGTK 2.46 and newer expose graphics trace points to Sysprof
([WebKitGTK 2.46 release notes](https://webkitgtk.org/2024/09/17/webkitgtk2.46.0-released.html)).
Install Sysprof if needed, then wrap the release process tree:

```sh
sysprof-cli -f /tmp/sparrow-scroll.syscap -- ./target/release/sparrow-installed
```

Inspect `sparrow-installed`, `WebKitWebProcess`, compositor activity, frame-clock
stalls, and graphics marks across exactly the baseline interval. Sysprof is a
whole-system sampling profiler and its callgraphs can distinguish process and
main-loop stalls ([GNOME Sysprof guide](https://developer.gnome.org/documentation/tools/sysprof.html)).
At research time neither `sysprof-cli` nor `perf` was installed on this host, so
Web Inspector and the renderer A/B are the available zero-install first steps.

## Evidence-to-fix decision table

| Evidence | Fix direction |
| --- | --- |
| Default or FORCE_SHM is smooth; current DISABLE variant is slow | Remove the unconditional disable. Prefer the stable accelerated option and keep a narrowly triggered compatibility fallback. |
| `webkit://gpu` shows software rendering or an unexpected DRM node | Fix pre-webview backend/GPU selection; retain the JSON in the regression report. |
| Slow frames are paint-heavy and solid-background override wins | Simplify or confine the full-document gradients/transparency. |
| Non-sticky/opaque rail override wins | Reduce sticky blended surfaces or give them their own cheaper opaque composition. |
| `content-visibility: visible` wins | Remove it for WebKitGTK or use a tested alternative; do not assume a nominal optimization is beneficial on this engine. |
| React commits occur continuously during otherwise idle scrolling | Find the subscription/state source, narrow the update boundary, then validate with React Profiler. |
| Cost appears only after many pages | Bound the rendered card count or window the channel list. |
| Web Inspector is quiet but Sysprof shows long WebProcess/GTK/compositor work | Treat it as WebKitGTK/driver/backend work, not a React optimization problem. |

## Minimum regression evidence

For the final fix, retain the before/after `webkit://gpu` JSON, system/WebKit
versions, monitor mode and scale, five warm scroll captures on the same catalog,
and a focused Web Inspector or Sysprof trace. Verify both visual correctness and
smoothness on the native release binary; a faster diagnostic build that crashes
or renders blank on NVIDIA/Wayland is not an acceptable result.
