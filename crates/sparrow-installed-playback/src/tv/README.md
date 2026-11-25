# Android TV Playback Sessions

The [TV owner](owner.rs) accepts commands and publishes typed state. It owns the selected Channel, Android host, transport generation, timers and recovery budget. [Transport admission](transport.rs) queues each open before a later Pause or Stop can retire it. The [Android presentation gate](../android/gate.rs) orders the transport change and the exact picture operation. The GPUI view renders snapshots and supplies measured bounds. Kotlin applies player and surface changes on Android's UI thread.

The Tauri reference is [installed-playback-runner.ts](../../../../app/src/features/playback/installed-playback-runner.ts) with Channel selection and Quality Variant fallback in [catalog-browser.tsx](../../../../app/src/features/catalog/catalog-browser.tsx). [ADR 0008](../../../../docs/adr/0008-keep-android-playback-while-the-picture-can-be-seen.md) defines which Activity lifecycle changes affect playback.

| Behavior | Coverage in [tests.rs](tests.rs) |
| --- | --- |
| End the old transport and release the exact picture before tuning. Ignore callbacks from retired presentations. | Retune, rejected replacement, and cleanup confirmation |
| A viewer pause survives Home. A visibility pause can reopen the same Playback Session. Dock and fullscreen reuse the retained picture. | Visibility pause, Activity handoff, finishing Activity, and paused viewport movement |
| Tune immediately from the Guide. Commit only the last Channel key after 350 ms. | Held Center and Channel-key tuning |
| Recover after 1, 5 and 15 seconds. Reset the budget after 60 seconds of stable playback. | Initial source timeout and decoder deadlines |
| Allow one lower Quality Variant only for the first transport's source timeout or early stream interruption, with no Channel-key tune pending. | Initial timeout fallback and exact presentation events |
| Stop playback and fence new tunes until Source Configuration replacement completes. | Source change and ordered admission |

Run the host tests with `cargo test --locked -p sparrow-installed-playback`. The runner tests control time and exercise presentation ordering. The Android host must also be checked with a real Media3 stream.
