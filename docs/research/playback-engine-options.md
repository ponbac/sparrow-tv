# Playback engine options for Linux and Android

Research date: 2026-08-06

Decision scope: shortlist hands-on prototypes; do **not** select the production architecture yet.

## Recommendation

Advance these four paths to a direct-stream prototype, in this order:

1. **Ordinary WebView `<video>` playback** on Linux and Android, as the near-zero-code baseline.
2. **`mpegts.js` with Tauri's native HTTP streaming transport** on Linux and Android. This is the strongest shared-player candidate and should receive the most test time.
3. **Android Media3/ExoPlayer with a native `PlayerView`** as the Android reliability fallback.
4. **mpv in its own Wayland window, controlled over JSON IPC**, as the Linux engine fallback. This prototype tests playback reliability, not final in-app embedding.

Do not prototype native GStreamer or LibVLC in the first round. Both are credible engines, but they add native-surface and packaging work on both platforms without answering a question that the smaller Media3/mpv fallback pair cannot answer first.

This is deliberately a candidate set, not a production choice. Reliability on the actual provider and devices must decide whether one shared WebView player is sufficient or platform-native fallbacks are justified.

## The important new option: native streaming without a proxy

Sparrow does not need a localhost or hosted relay to try `mpegts.js` with a native network stack. Tauri's official HTTP plugin:

- is a Rust-backed `fetch` API and is listed as fully supported on Linux and Android in the plugin's own package metadata and documentation ([Tauri HTTP client guide](https://v2.tauri.app/plugin/http-client/), [plugin manifest](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/http/Cargo.toml));
- issues the request with `reqwest`, then exposes response headers and a resource ID ([Rust command implementation](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/http/src/commands.rs)); and
- reads response chunks on demand and constructs a JavaScript `ReadableStream`, with abort and body cleanup support ([JavaScript implementation](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/http/guest-js/index.ts#L197-L285)).

`mpegts.js` already selects a configurable loader class: its v1.8.1 configuration contains `customLoader`, and the I/O controller instantiates that class ([configuration](https://github.com/xqq/mpegts.js/blob/v1.8.1/src/config.js#L27-L63), [loader selection](https://github.com/xqq/mpegts.js/blob/v1.8.1/src/io/io-controller.js#L238-L256)). A thin loader adapter can therefore feed Tauri's `ReadableStream` into the existing transmuxer while preserving the React `<video>` element, controls, and stall/reconnect policy.

This path should not use a Tauri custom URI protocol. The current custom-protocol responder accepts a complete in-memory byte body, not an open-ended stream ([Tauri `Builder` API](https://docs.rs/tauri/latest/tauri/struct.Builder.html#method.register_asynchronous_uri_scheme_protocol), [Wry responder API](https://docs.rs/wry/latest/wry/struct.RequestAsyncResponder.html#method.respond)); buffering a live channel to completion is impossible.

The open risk is throughput: each native response chunk crosses an IPC invocation before entering the JavaScript `ReadableStream`. No upstream documentation proves that this remains stable for a multi-megabit stream over hours. The prototype must measure it.

## Current Sparrow baseline

The existing `TvPlayer` already has useful shared behavior: an `mpegts.js` player, React controls, exponential reconnect, a hard-stall watchdog, fullscreen/orientation behavior, and teardown. The package declares `mpegts.js ^1.7.3`; the prototype should pin the current v1.8.1 release rather than silently float versions. v1.8.1 was released on 2026-08-06, so the project is actively maintained ([v1.8.1 release](https://github.com/xqq/mpegts.js/releases/tag/v1.8.1)).

## Comparison

| Path | Targets | MPEG-TS and codec coverage | Reuse and controls | Tauri / surface complexity | HTTP behavior | Packaging, license, health | Reliability expectation before testing |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Ordinary `<video src>` | Linux + Android | Entirely host-WebView dependent. Linux Tauri uses WebKitGTK, whose media stack is GStreamer; Android Tauri uses the device's Chromium-based System WebView ([Tauri WebView versions](https://v2.tauri.app/reference/webview-versions/), [WebKitGTK multimedia](https://docs.webkit.org/Ports/WebKitGTK%20and%20WPE%20WebKit/Multimedia.html)). Raw TS, HLS, and codec support must be probed on the exact installs. | Reuses React UI and native HTML media methods; least code. | No native view or bridge. | Direct provider request. CSP must allow the media host. Android cleartext needs explicit policy. | No added player library. Linux AppImage still needs the WebKit/GStreamer media framework bundled. | Lowest confidence, but cheap enough that it is the mandatory baseline. A successful result could eliminate most player integration work. |
| `mpegts.js` + browser `fetch` | Linux + Android | Transmuxes MPEG-TS to fragmented MP4 for MSE. Upstream documents H.264/H.265 + AAC, AC-3/Opus additions, and no MPEG-2 video ([README](https://github.com/xqq/mpegts.js#features)). Final decode still depends on the WebView codec stack. | Maximum reuse: current component, controls, reconnect logic, and statistics events. | No native view. | Provider must satisfy browser CORS, including redirects; upstream documents this explicitly ([CORS guide](https://xqq.im/mpegts.js/docs/cors.html)). | Apache-2.0; v1.8.1 is current and active. Same WebKit/GStreamer AppImage requirement. | Useful control case only. Provider CORS may make it fail even when media is valid. |
| `mpegts.js` + Tauri HTTP `ReadableStream` | Linux + Android | Same transmux and decoder envelope as above. Native transport changes network behavior, not codecs. | Maximum UI/player reuse; only the loader is new. | No native video surface. A custom `BaseLoader` adapter is required. | Direct native provider request, no CORS enforcement and no localhost server. Tauri still adds an `Origin` unless configured otherwise; URL scopes and header behavior must be tested. | Apache-2.0/MIT transport plus Apache-2.0 player. No second media engine. | Best shared candidate. Main risks are per-chunk IPC overhead, long-run MSE buffers, WebView codec gaps, and background lifecycle behavior. |
| Media3/ExoPlayer | Android | Explicit support for progressive MPEG-TS and MPEG-TS inside HLS. Audio/video sample decoding normally uses Android platform decoders, so device capability still matters ([supported formats](https://developer.android.com/media/media3/exoplayer/supported-formats)). | Catalog and selection UI remain React. Playback controls can be native `PlayerView` for the prototype; sharing the current React controls would require a command/event facade and surface coordination. | A Tauri mobile plugin can run Kotlin and expose commands to Rust/JavaScript ([Tauri mobile plugins](https://v2.tauri.app/develop/plugins/develop-mobile/)). `PlayerView` adds a native surface to the activity. | Direct provider request. Media3 permits custom headers, network stacks, and retry policy ([customization](https://developer.android.com/media/media3/exoplayer/customization), [network stacks](https://developer.android.com/media/media3/exoplayer/network-stacks)). Android cleartext policy applies. | Apache-2.0 ([AndroidX Media repository](https://github.com/androidx/media)); official AndroidX releases and current 1.10.1 docs indicate strong maintenance. Normal Gradle/AAR packaging. | Highest Android confidence on normal H.264/AAC streams, with good errors and retry instrumentation. Odd TS and device codec variation remain empirical. |
| mpv / libmpv | Linux (first round) | Broad FFmpeg-backed demux/decode and native Wayland output; exact provider coverage remains a test result. | React can issue play/pause/volume/stop over JSON IPC, but the prototype window uses mpv's surface and controls. Catalog/UI reuse is high; player-view reuse is low. | A separate Wayland window is straightforward. `--wid` embedding is documented for X11, Windows, and Android surfaces, not Wayland; true in-window use would require libmpv's render API ([mpv embedding options](https://mpv.io/manual/stable/#using-mpv-from-other-programs-or-scripts), [`--wid`](https://mpv.io/manual/stable/#options)). | Direct provider request. | Active upstream; latest stable is v0.41.0 ([release](https://github.com/mpv-player/mpv/releases/tag/v0.41.0)). Default build is GPLv2+; an LGPL-oriented build is possible but dependency licenses still matter ([mpv copyright/licensing](https://github.com/mpv-player/mpv/blob/master/Copyright)). A self-contained AppImage must carry mpv/libmpv and codec dependencies. | Strong Linux engine comparator, but packaging and a coherent single-window UX are material risks. |
| LibVLC | Linux + Android | Broad multimedia-file and stream engine, with native bindings on both platforms ([LibVLC introduction](https://videolan.videolan.me/vlc/master/libvlc.html)). | Catalog stays shared; player view and much of control integration become platform-specific. | Requires GTK/Wayland drawable work on Linux and an Android native surface/plugin. | Direct provider request. | Core LibVLC is LGPLv2+ but some plugins are more restrictive ([license](https://videolan.videolan.me/vlc/master/libvlc.html#license)). Native libraries and codec modules enlarge both bundles. | Credible revisit if Media3 or mpv cannot handle the feed corpus. Too much surface/packaging work for round one. |
| Native GStreamer | Linux + Android | Highly configurable, cross-platform pipeline with MPEG-TS/HLS plugins. The Android SDK's MPEG-TS demux and several common codecs are in its restricted plugin set ([Android installation/plugin list](https://gstreamer.freedesktop.org/documentation/installing/for-android-development.html)). | Catalog stays shared; player implementation and controls need native adapters. | Natural on GTK/Linux, but Android guidance still joins Java UI to C through JNI. This duplicates media infrastructure already under WebKitGTK. | Direct provider request. | Core is LGPL; plugin and codec licensing must be audited ([licensing FAQ](https://gstreamer.freedesktop.org/documentation/frequently-asked-questions/licensing.html)). The project is very active, with stable 1.28.5 current in July 2026 ([releases](https://gstreamer.freedesktop.org/releases/)). | Technically strong but the largest integration surface. Revisit only if the smaller candidates reveal a specific pipeline-level need. |

## Candidate details

### 1. Ordinary WebView playback

Prototype both raw channel URLs and HLS URLs by assigning them directly to `<video src>`. Do not infer support from a desktop browser: Tauri deliberately uses the installed platform WebView, and Android support changes with the selected System WebView provider ([Tauri WebView versions](https://v2.tauri.app/reference/webview-versions/)). On Linux, WebKitGTK media playback, MSE, and codecs are provided through GStreamer ([WebKitGTK multimedia architecture](https://docs.webkit.org/Ports/WebKitGTK%20and%20WPE%20WebKit/Multimedia.html)).

This is a baseline, not the expected winner. It has the smallest code and control surface, so a pass across the actual feed corpus is valuable even if documentation cannot promise it.

### 2. `mpegts.js` through Tauri native HTTP streaming

Implement the smallest possible `mpegts.BaseLoader` using `@tauri-apps/plugin-http.fetch` and its response reader. Do not change global `window.fetch`; isolate native transport to playback. Preserve abort on channel switch and component unmount.

Important test settings:

- Compare v1.8.1's default stash buffer with `enableStashBuffer: false`; upstream says disabling it reduces latency but increases stall risk under jitter ([API configuration](https://xqq.im/mpegts.js/docs/api.html)).
- Enable and measure automatic SourceBuffer cleanup. The existing player currently does not set it, while long live sessions are a primary requirement.
- Record `MEDIA_INFO`, `STATISTICS_INFO`, error type/detail, dropped frames, decoded codec, buffer duration, and every reconnect.
- Test the plugin's default `Origin` header and, if the provider rejects it, the documented `unsafe-headers` build with an explicitly removed Origin. Keep the HTTP capability scoped as narrowly as the stored provider configuration permits ([Tauri HTTP security](https://v2.tauri.app/reference/javascript/http/)).

This path bypasses browser CORS but does not bypass MSE or the WebView's decoders. `mpegts.js` is a transmuxer, not a software video decoder ([design overview](https://github.com/xqq/mpegts.js#overview)).

### 3. Android Media3

Use current stable Media3 1.10.1 and a Tauri Android plugin with a native `PlayerView`. Use `SurfaceView` first: Media3 recommends it over `TextureView` for lower power, smoother frame timing, HDR, and secure output ([surface guidance](https://developer.android.com/media/media3/ui/surface)). For round one, use Media3's own controls; that keeps the prototype focused on playback and lifecycle reliability. A later architecture can decide whether to retain native controls or drive the player from React.

Media3 supports both continuous MPEG-TS and MPEG-TS HLS, but its default sample decoders are device decoders ([format table](https://developer.android.com/media/media3/exoplayer/supported-formats#progressive-container-formats)). It offers player-state/error events, explicit retry after failure, detailed analytics, injectable HTTP stacks and custom retry policies ([player events](https://developer.android.com/media/media3/exoplayer/listening-to-player-events), [customization](https://developer.android.com/media/media3/exoplayer/customization)). Those features make it a good reliability reference even if the shared player ultimately wins.

The surface is the integration cost. A `SurfaceView` can coexist with overlay controls in an Android view hierarchy, but coordinating its bounds, z-order, orientation, fullscreen, and lifecycle with DOM content is native UI work. The prototype should use a full-screen native player layer rather than attempt pixel-perfect DOM/native composition.

### 4. mpv on Linux

Run an installed mpv 0.41 process with `--no-config --input-ipc-server=...` and its own native Wayland window. Use JSON IPC for start/stop/pause/volume and gather log/events. mpv explicitly recommends JSON IPC for controlling a subprocess and libmpv when used as an embedded backend ([official manual](https://mpv.io/manual/stable/#using-mpv-from-other-programs-or-scripts)).

Do not spend this research prototype embedding mpv inside Tauri. The common `--wid` mechanism has X11 semantics and does not provide a Wayland child-window path. If mpv clearly wins playback reliability, the production decision must then compare a managed second window with the substantially deeper libmpv render-API route.

## Cleartext HTTP and security

Many IPTV URLs are plain HTTP. Android apps targeting API 28 or newer default to `cleartextTrafficPermitted="false"`; a network-security configuration must opt in ([Android network security configuration](https://developer.android.com/privacy-and-security/security-config#CleartextTrafficPermitted)). Because the user's configured provider host is not known at build time, a domain-only rule cannot be generated into a static manifest. The prototype should explicitly enable cleartext for its test build and record which paths actually honor the policy.

Treat that opt-in as an accepted product constraint, not as TLS. Do not disable certificate or hostname verification for HTTPS. Tauri CSP and HTTP plugin capabilities should still be restricted: Tauri recommends a narrowly tailored CSP, and its HTTP plugin denies URLs outside configured scopes ([Tauri CSP](https://v2.tauri.app/security/csp/), [HTTP plugin security](https://v2.tauri.app/reference/javascript/http/)).

On Linux, permit the selected provider URL in the Tauri `media-src`/`connect-src` policy used by the prototype. Browser-fetch `mpegts.js` still needs provider CORS; Tauri-native fetch does not.

## Packaging consequences

- **WebView paths:** smallest application delta, but not dependency-free. Tauri says an audio/video AppImage must set `bundle.linux.appimage.bundleMediaFramework`, which bundles extra GStreamer files; this is currently fully supported only on Ubuntu build systems, and plugin licensing must be checked ([Tauri AppImage multimedia guidance](https://v2.tauri.app/distribute/appimage/#multimedia-support-via-gstreamer)).
- **Media3:** ordinary Android Gradle dependencies, no native ABI matrix for the base engine, and Apache-2.0 licensing. This is the cleanest native packaging story.
- **mpv:** either require a system player (not self-contained) or bundle mpv/libmpv, FFmpeg, and their runtime dependencies. The selected build's GPL/LGPL and linked-library licenses must be preserved and documented.
- **LibVLC/GStreamer:** carry native libraries per target ABI plus codec/plugin choices. Their plugin-based licensing means “the core is LGPL” is not a complete distribution audit.

Packaging is a comparison criterion here, not a reason to skip direct playback validation; the separate release-path research should prove the final AppImage and APK artifacts.

## Explicit prototype matrix

Use no hosted or localhost proxy. All candidates request the provider URL directly.

| ID | Linux Arch/Wayland | Android emulator, then physical device | Purpose |
| --- | --- | --- | --- |
| W0 | Direct `<video src>` | Direct `<video src>` | Establish the WebView-native floor at almost zero integration cost. |
| W1 | `mpegts.js` v1.8.1 + browser fetch | Same | Identify CORS-only failures and provide a control for native-transport overhead. Skip a cell only when provider CORS makes it categorically impossible, and record that result. |
| N1 | `mpegts.js` v1.8.1 + Tauri HTTP custom loader | Same | Test the maximum-reuse, proxy-free native transport path and sustained IPC/MSE behavior. |
| A1 | — | Media3 1.10.1 + native full-screen `PlayerView` | Establish Android native reliability, codec, lifecycle, and power baseline. |
| L1 | mpv 0.41 own Wayland window + JSON IPC | — | Establish a native Linux reliability/codec baseline without prematurely solving embedding. |

### Feed corpus

Before playback, collect container/video/audio codec metadata with `ffprobe` for at least six representative channels, without committing credentials or URLs:

- the most-watched SD and HD channels;
- at least one high-bitrate channel;
- every distinct combination found among H.264, HEVC, AAC, AC-3/E-AC-3, MPEG-TS, and HLS;
- one known-problem or slow-starting channel; and
- both HTTP and HTTPS if the provider supplies both.

If MPEG-2 video is present, mark it as an expected `mpegts.js` miss because upstream explicitly lists MPEG-2 video as unsupported; still run Media3 and mpv.

### Identical exercise for every applicable cell

1. Cold start each feed three times; record time to first frame and first audio.
2. Switch channels 30 times on a fixed rotation; record failed starts, leaked players/surfaces, and time to release the old connection.
3. Drop networking for 30 seconds and restore it three times; record automatic recovery time and whether manual restart is required.
4. Run pause/resume, Linux fullscreen, Android rotation, Android background/foreground, and screen lock/unlock ten times each where applicable.
5. Run a 90-minute soak on the same representative HD channel. Sample memory, CPU, dropped frames, buffered duration, A/V continuity, stalls, and reconnects every minute.
6. Build and run the resulting AppImage/APK-shaped prototype, not only a development server build.

### Advancement gates for the architecture decision

A candidate remains viable only if it:

- plays every feed whose container and codec are inside its documented envelope, or explains a device codec limitation precisely;
- has no crash, unrecovered stall, or monotonic unbounded memory growth in the 90-minute soak;
- returns to playback within 30 seconds after the forced network outage without restarting the app;
- releases the previous connection and decoder on all 30 channel switches;
- survives every applicable lifecycle/fullscreen/orientation cycle; and
- can be packaged without an undeclared system dependency or unresolved license obligation.

Use relative evidence to choose later: if N1 matches the native platform baselines on feed success, recovery, and soak behavior, prefer its much greater UI reuse. If Android Media3 materially outperforms N1, accept a native Android player. If only mpv covers important Linux feeds or remains stable, carry the Linux-native fallback into the architecture decision. A successful W0 result is allowed to win; complexity should be earned by measured failures.

## Decision gist

Prototype direct `<video>` and `mpegts.js` with Tauri's proxy-free native HTTP `ReadableStream` on both targets, plus Media3 on Android and an IPC-controlled mpv Wayland window on Linux; defer LibVLC/GStreamer unless those smaller native baselines expose a concrete gap.
