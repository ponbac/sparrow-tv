# Linux in-app startup buffering

Date: 2026-09-09. Follow-up to the [frame pacing diagnosis](linux-in-app-frame-pacing.md)
on the same Hyprland/Intel machine, using the packaged Tauri app and Rust stream
transport. The owner reported several seconds of erratic motion after tuning,
followed by smaller recurring interruptions. Their copied playback report stayed
`playing`, with no failure or recovery. That state does not count media seeks or
short buffering interruptions within the same playback session.

## Reproduction and policy

Two private listings match the reported event. They are tested separately as D
and E; their provider locations and catalog metadata remain private. Both
baseline runs, using the previously revised six-second limit and three-second
reserve, recorded 13 startup seeks. In seconds 20–90, D recorded three waiting
events; E recorded eight, with a maximum frame callback gap of 625 ms. The traces
show the initial burst being drained through repeated seeks, followed by
multi-second batches that sometimes exhausted the remaining reserve.

The installed adapter now disables mpegts.js's append-triggered latency chasing
and owns a small buffer watcher with the same lifetime as its player:

- Start playback immediately and observe the buffered timeline every 250 ms.
- Wait for a 1.5-second observation window whose incoming timeline grows no
  faster than 1.5 times real time. This avoids chasing every append in the initial
  fast download. Batched delivery can still resemble a settled window; this is a
  heuristic, not detection of an upstream protocol boundary.
- When forward buffering exceeds ten seconds, catch up to five seconds before
  the buffered end, only if that final buffered range contains the full reserve.
- Permit no further catch-up for 30 seconds. Pause, seeking, an empty buffer, or
  a regressing buffered timeline resets the observation window.
- Cancel observations before releasing the player. Use the mpegts.js seek API so
  its internal seek bookkeeping remains consistent.

A three-second-reserve intermediate build reduced D's startup seeks from 13 to
one but still recorded five steady waiting events. It therefore isolates the
startup improvement without resolving the reserve problem. The final five-second
reserve trades additional live delay for tolerance of the observed batched
arrival. Forward-buffer depth is not total delay from the broadcaster.

## Measurements

Runs use the same XWayland shared-memory transport, one provider connection at a
time, and isolated private single-channel profiles. The feature-gated probe
records numeric media events, presentation callbacks, buffer depth, and seek
writes. Startup is elapsed seconds 0–20; steady comparisons are seconds 20–90.
The probe's nominal seconds make rates approximate. Separate live runs do not
replay identical content or delivery conditions.

| Sample and policy | Startup seeks | Steady waiting | Approx. callbacks/s | Longest steady gap | Median forward buffer |
| --- | ---: | ---: | ---: | ---: | ---: |
| D, previous 6s / 3s | 13 | 3 | 49.70 | 183 ms | 2.21s |
| E, previous 6s / 3s | 13 | 8 | 47.96 | 625 ms | 2.01s |
| D, settled / 3s intermediate | 1 | 5 | 49.06 | 566 ms | 2.39s |
| D, settled / 5s final | 1 | 0 | 50.04 | 33 ms | 6.85s |
| E, settled / 5s final | 1 | 0 | 50.06 | 33 ms | 4.31s |
| C, settled / 5s regression | 1 | 0 | 49.90 | 42 ms | 5.37s |

D's final run continued to 180 seconds, still with zero steady seeks or waiting
events, approximately 50.02 callbacks/s, and a maximum gap of 34 ms. E's final
90-second run had no steady seeks or waiting events and a maximum gap of 33 ms.
The earlier interlaced sample C also completed 90 seconds with one startup seek,
no steady seeks or waiting events, and a maximum steady gap of 42 ms.

The final D/E runs first reported playing at second four, versus second five
in the baselines. This coarse single-run observation establishes no extra
multi-second startup hold, not a precise tune-time improvement. Their single
catch-up caused maximum startup frame gaps of 245/242 ms and jumped forward
about 36/38 seconds of previously buffered video. Startup is therefore still
visibly discontinuous once, rather than completely seamless.

All six runs completed normally with no failed playback-state samples or
remaining mpv sockets. Raw samples and artifact hashes are retained below.


## Validation and limits

The frontend suite passes 315 tests, including startup bursts, delayed bursts,
cooldown, paused playback, insufficient final buffer ranges, and timer disposal both directly and through adapter stop/error paths.
ESLint and the TypeScript/Vite build pass, and the feature-enabled AppImage builds
successfully. No Rust production logic changes in this follow-up.

This removes the repeated startup seek storm; it does not promise a seamless
startup. One larger jump can remain when the player catches up after the initial
burst, including a brief decoder interruption. A delivery gap longer than the
available reserve can still stall playback. The cooldown limits how often catch-up
can interrupt playback and may temporarily allow more than ten seconds buffered.
Browser frame callbacks measure presentation activity, not physical display
scanout. A 50-frame-per-second video on the 120 Hz panel also cannot occupy an
identical number of refreshes per frame; small regular timing variation is distinct
from the long buffer stalls measured here.

The [numeric artifacts](linux-in-app-startup-buffering-artifacts/comparison.json)
include raw samples, AppImage hashes, and process completion/cleanup records.
Logs, screenshots, source configuration, and private catalog data are excluded.
