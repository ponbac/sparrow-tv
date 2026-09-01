# Use platform-native installed playback with Rust-owned source resolution

The measured Linux and Android failures crossed this ADR's original native-engine revisit gates: installed WebKit/MSE playback is no longer the Primary Playback Engine. Linux now uses system mpv and Android uses Media3/ExoPlayer, while Rust continues to own Channel lookup, ephemeral Playback Source resolution, Playback Session lifecycle, and privacy boundaries. The hosted web application remains independent and continues to use `mpegts.js` over same-origin HTTP.

## Boundaries

- Shared TypeScript owns the Playback Session state machine, bounded recovery, safe aggregate diagnostics, and user intent. A platform-selected presentation adapter translates that intent without placing a Playback Source in React state.
- On Android, Rust opens and cancels the single provider request, parses the MPEG-TS program map, and exposes only opaque Playback Session and stream-generation identifiers. A Media3 custom `DataSource` obtains bounded byte batches directly from the Rust stream actor over JNI; ExoPlayer and its native `PlayerView` own demux, decode, buffering, and video presentation. Provider locations do not cross into Kotlin, WebView state, or a Media3 URI.
- On Linux, Rust resolves the Playback Source and starts mpv with fixed, URL-free arguments. It sends the source only through a private Unix IPC socket, after mpv is running, and suppresses routine player output. The playback actor owns the mpv process and socket until confirmed termination, so there is at most one mpv process and one provider connection for Sparrow playback.
- The Linux video remains in mpv's separate Wayland window. Sparrow sends mute, volume, and fullscreen changes over the correlated private IPC connection; fullscreen therefore applies to the mpv window rather than the WebView surface. Channel switching stops and reaps the current process before launching its replacement, and final stop or application shutdown deterministically releases the process and socket.
- Linux Playback Session pause is resource-releasing, not a frozen-stream pause: Sparrow stops and reaps mpv, closing the provider connection while retaining session intent. Resume launches a new mpv process for that session at the current live edge. Visibility and lifecycle suspension use the same release-before-resume rule.
- Android Audio Track selection remains at the Rust stream boundary: changing the selected PID releases the old stream generation before opening its replacement. Linux delegates track presentation and any direct track choice to mpv's window because mpv receives the original source rather than Sparrow's WebKit stream projection.
- Linux WebKit rendering remains an independent UI-compositor constraint. The packaged X11/XWayland GTK path uses WebKit's accelerated backing store with shared-memory transport; native Wayland retains the DMA-BUF-disable compatibility path, and either choice can be overridden explicitly before startup. Removing WebKit/MSE from Linux playback does not remove this renderer boundary.

## Rejected alternatives

- A shared installed `mpegts.js`/MSE primary is rejected. The representative Linux stream sustained its expected frame rate with no decoder drops in mpv while WebKit repeatedly produced short decoded-frame runs and drops; Android ordinary UI remained responsive while the same installed media path was choppy.
- Increasing the native-read cap, coalescing reads, or enabling the `mpegts.js` stash buffer alone is rejected as the playback fix. Those differentials substantially reduced bridge calls without materially improving the measured Linux frame behavior.
- Passing the Playback Source to mpv in command-line arguments is rejected because process listings and routine diagnostics would expose provider locations or credentials. Automatic overlapping fallback is likewise rejected because it can create two provider connections.
- Embedding mpv into the WebView window is deferred. A separately owned system window is the smaller, proven Linux boundary and avoids coupling playback correctness to WebKit composition; the resulting fullscreen and control semantics are accepted explicitly above.
- Retaining Android WebView/MSE as primary is rejected in favor of the focused Media3 path. The JNI `DataSource` preserves native provider ownership while removing JavaScript invoke-per-read, MSE transmux, WebView decode, and WebView composition from the Android video-frame path.
- Forking the stale, unmerged upstream multi-audio implementation remains rejected in favor of selecting a single audio PID at the Rust stream boundary for native-stream presentations.

## Consequences

- Installed playback has one deep platform seam rather than one shared rendering technology: Linux returns an mpv-owned presentation, Android returns a Rust-stream/Media3 presentation, and installed WebKit/MSE is retained only as a non-primary compatibility path.
- Linux packages continue to depend on a supported system mpv and do not bundle it into the AppImage. Users see a separate player window, and pause/resume incurs a reconnect at the live edge in exchange for releasing the provider connection deterministically.
- Android carries Media3 and a small Kotlin/JNI adapter, but provider resolution, stream identity, restart ordering, and final cancellation remain Rust-owned. Instrumentation is limited to aggregate states and counters rather than sources or catalog contents.
- Hosted playback is unaffected: it continues to use its existing same-origin `mpegts.js` path and browser control model.

## Revisit gates

Reconsider the Linux presentation if the separate-window model cannot satisfy required fullscreen, switching, accessibility, or desktop-integration behavior; if supported system mpv is unavailable on a target distribution; or if an embedded native engine matches representative-stream performance without weakening source privacy or single-owner cleanup. Reconsider the Android presentation if Media3 still produces sustained stalls or unacceptable decoded/dropped-frame results on the representative physical device, cannot preserve lifecycle and Audio Track behavior, or cannot deterministically release its provider connection. Reconsider installed WebKit/MSE only after a measured, packaged-build differential demonstrates equivalent playback on the failing representative streams; bridge batching or stash-policy changes alone do not meet that gate.
