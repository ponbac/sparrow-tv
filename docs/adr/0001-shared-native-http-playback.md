# Use shared native-HTTP playback with manual mpv failover

Sparrow TV will use `mpegts.js` over Tauri's native HTTP stream as the Primary Playback Engine on Linux and Android. This keeps one proxy-free player and control model across both targets while avoiding browser CORS; the prototype passed visible Linux playback and the physical Android cold-start, channel-switch, and lifecycle checks. The existing hosted web application remains independent and unchanged.

## Boundaries

- Shared TypeScript owns the Playback Session state machine, bounded recovery, telemetry, and controls: start, stop, pause, resume, restart, mute, volume, fullscreen, and Audio Track selection.
- The React UI starts playback with a Channel identifier. Rust resolves the ephemeral Playback Source from the on-device Channel Catalog, opens and cancels native requests, and keeps provider URLs out of React state and routine logs.
- Rust parses the MPEG-TS program map, exposes Audio Tracks, and rewrites/filters the native stream so only the selected audio PID is presented to unmodified upstream `mpegts.js`. Changing Audio Track fully releases and recreates playback; a brief interruption is accepted. The Audio Track Preference is remembered per Channel and falls back visibly when unavailable.
- Linux provides mpv in its own Wayland window as a Fallback Playback Engine. Playback Failover is always user-authorized and starts only after the primary connection is released. The target system's installed mpv is a declared dependency and is not bundled into the AppImage.

## Rejected alternatives

- Direct WebView `<video>` is rejected because the representative MPEG-TS source was unsupported on the target Android WebView.
- Browser-transport `mpegts.js` remains diagnostic only: it shares the same MSE/decoder envelope while adding browser CORS variability.
- Android Media3 is deferred because native-HTTP `mpegts.js` passed the actual-device tests; adding Media3 now would duplicate the surface, controls, and lifecycle model without addressing a demonstrated failure.
- Forking the stale, unmerged upstream multi-audio implementation is rejected in favor of selecting a single audio PID at the native stream boundary.
- Automatic mpv failover is rejected to prevent hidden or overlapping provider connections and rate-limit surprises.

## Revisit gates

Reopen this decision if an important in-scope Channel fails the primary engine on three clean attempts while a native engine plays it; if the primary engine produces two unrecovered stalls or crashes during ordinary viewing after bounded recovery; if the required Audio Track cannot be enumerated or selected; or if the packaged Linux application can no longer render with the validated WebKit workaround. Crossing an Android gate triggers a focused Media3 prototype against the exact failure. Repeated Linux failures may promote mpv from fallback to primary.
