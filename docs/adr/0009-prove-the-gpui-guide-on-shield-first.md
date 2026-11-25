# Prove the GPUI guide on Shield first

Sparrow TV will develop the GPUI port first on NVIDIA Shield. During this milestone the native app is a development package with its own Android application ID and data directory. The current Android, desktop, and hosted packages remain the delivery path. This experiment extends [ADR 0003](0003-share-one-core-across-sibling-adapters.md) with a native adapter that shares the same Channel Catalog.

## Order and boundaries

1. Extract the installed catalog setup and Source Configuration storage from Tauri. Preserve its startup order, private data layout, and IPC behavior.
2. Render Now and Timeline on the physical Shield from the shared core and paged reads. Port the TV navigation rules, Quality Variant row folding, time windows, and paging from the existing behavior tests. Keep guide state testable without GPUI. The Activity owns lifecycle reporting and Android TLS initialization. Core startup, refresh, and queries run off the render thread with a Tokio runtime.
3. Port Playback Session behavior from the installed runner, including cancellation, recovery, and exclusive transport ownership. Reuse the Media3 path and preserve picture visibility semantics. Complete the remaining Shield workflows before taking up phone and desktop. Develop and test these changes off-device while the Shield is in use.

Pin GPUI and its Android backend to tested revisions, use the shared Cargo lockfile, and scope the native dependencies to Android. Seed the development package through Source Configuration replacement with a loopback-only fixture launch option. Keep the fixture generator and optical harness in the repository. Use D-pad input and an allowlisted Activity dump with stable focus IDs, Channel Number, generation, and page state to verify the native guide without exposing Channel names or source data.

## Shield gate

Compare optimized builds on the same physical Shield, using the same deterministic offline fixture, clock, and D-pad walk. The large fixture uses the [Android catalog acceptance](../acceptance/android-catalog-performance.md) snapshot floors of 64 MiB M3U and 24 MiB EPG per slot. Record resulting Channel, Channel Group, and Programme counts. Populate each build's processed cache once before measuring cold process starts, and activate refresh at the same point under the same offline conditions.

- Collect at least five alternating trials per renderer. Each 8 Hz trial must show all 120 moves at the expected focus positions and finish on the expected row. GPUI's p95 from input to visible focus must be at most 80% of Tauri's.
- GPUI's median cold launch to the first visible guide must be at most three seconds and at most Tauri's median.
- Peak total PSS must stay at or below 512 MiB and at or below Tauri's measured peak. Sample the app and its bound renderer processes on both builds over the same startup and browse interval.
- Record the share of 20 Hz input windows with a visible focus change and verify the end focus. This counts displayed changes within each window. Add the playback-on comparison and Media3 continuity evidence once the native Playback Session is in use.

Use the paired Shield results to decide whether the native package is ready to replace the installed Shield app. If the guide misses these targets, profile it on Shield before deciding on replacement.

## Replacement decisions

Set a native Channel Icon policy before adding image fetching. Decide accessibility parity, the long-term UI arrangement, APK signing and versioning, and rollback with the owner before replacing a shipping package. Preserve installed preferences and prove data continuity at that gate.
