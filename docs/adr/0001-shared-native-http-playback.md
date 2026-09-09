# Default to in-app Linux playback with Rust-owned source resolution

Linux uses the in-app WebKit/MSE player by default. On 2026-09-09 the owner explicitly chose integrated viewing and fullscreen over the smoother external mpv default, accepting the measured frame pacing issues while they are investigated. Android continues to use Media3/ExoPlayer; hosted playback continues to use its same-origin `mpegts.js` path.

## Boundaries

- Rust owns Channel lookup, ephemeral Playback Source resolution, Playback Session lifecycle, and provider privacy. The in-app player receives an opaque native stream handle and bounded byte batches, never a provider URL.
- Linux exposes **Open in mpv** and **Play in app** as explicit session replacements. The shared runner waits for confirmed cleanup before creating the replacement. Selecting another Channel returns to the in-app default; pause/resume and recovery preserve the current session's player choice.
- mpv receives the source through a private Unix IPC socket with fixed, URL-free process arguments. Rust owns and reaps the process and socket. There is no automatic overlapping fallback.
- In-app fullscreen expands the entire player with controls over the picture. The button, double-click, and F toggle fullscreen; Escape returns to the guide. External mpv keeps its own window and correlated playback controls.
- Pause releases the provider connection; resume reconnects at the live edge. Audio Track selection for native streams remains at the Rust stream boundary. External mpv owns its own track presentation.
- Android retains its Rust-owned stream actor, JNI `DataSource`, and Media3 native `PlayerView`. Provider locations do not cross into Kotlin or WebView state.

## Consequences

The packaged Hyprland comparison found working in-app video after correcting audio codec wire names, but uneven delivery remains on interlaced samples. Choosing MSE as default is an accepted product tradeoff, not evidence of equivalent smoothness. See the [measurements](../research/linux-in-app-playback-hyprland.md). Improve frame pacing without changing the user's default to an external window. A subsequent [diagnosis](../research/linux-in-app-frame-pacing.md) isolated repeated latency-chasing seeks and buffer starvation; the installed adapter now retains three seconds after catch-up instead of half a second. Startup catch-up still warrants further work.

Linux mpv remains an optional system dependency for the explicit button and is not bundled into the AppImage. Embedding libmpv remains deferred until it demonstrates representative-stream performance, integrated controls, source privacy, and deterministic cleanup. Bridge batching or synthetic playback alone does not establish that result.

WebKit composition remains a separate constraint: packaged X11/XWayland uses shared-memory transport for its accelerated backing store; native Wayland retains the DMA-BUF-disable compatibility path. The bounded measurement probe and startup engine override are compiled only with `linux-playback-lab`; normal builds support both user-selectable Linux players without the probe.
