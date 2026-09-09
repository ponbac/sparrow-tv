# Linux in-app frame pacing diagnosis

Date: 2026-09-09. Host and private sample aliases are described in the
[Hyprland report](linux-in-app-playback-hyprland.md). The Linux in-app player
remains the default; external mpv remains an explicit user choice.

## Cause under test

The installed adapter enables `liveBufferLatencyChasing` but originally used
mpegts.js 1.7.3's implicit thresholds: catch up above 1.5 seconds of forward
buffer, seeking to only 0.5 seconds before the buffered end. The library applies
that check on SourceBuffer updates. See its pinned [defaults](https://github.com/xqq/mpegts.js/blob/v1.7.3/src/config.js)
and [seek implementation](https://github.com/xqq/mpegts.js/blob/v1.7.3/src/player/mse-player.js).

The provider sends an initial burst containing tens of seconds of video. The
old policy repeatedly advances through it in approximately one-second jumps.
Private sample C recorded 36 seeks in the first 12 seconds. During seconds
20–90, it recorded six more seeks and 36 additional `waiting` events, with a
maximum frame callback gap of 1,042 ms. Browser animation callbacks remained
responsive (maximum 19 ms gap in that interval).

Every recorded startup seek targeted the buffered end minus 0.5 seconds. This
identifies the application's catch-up setting as the source of those jumps.
The small remaining reserve also leaves playback exposed to uneven delivery.
The measurements do not identify the upstream reason for each delivery gap.

## Method

All runs use packaged AppImages on the same XWayland/SHM path and the same private
sample aliases. Only one playback test runs at a time. Provider locations stay
inside Rust; no source URLs, credentials, or catalog data enter the shared
measurement artifacts.

The feature-gated installed probe records one-second summaries of
`requestVideoFrameCallback` intervals, media timestamp intervals,
`requestAnimationFrame` intervals, forward buffered duration, `currentTime`
setter calls, and cumulative `waiting`/`seeking`/`seeked` counts. It wraps the
native setter without changing its result. The hook and callback loops live
only in the bounded diagnostic app process and are absent from normal builds.

Steady comparisons use seconds 20–90. Callback rates use the probe's nominal
elapsed seconds and are approximate. Frame callback intervals observe browser
presentation activity, not physical scanout. The desktop was observed locked during the later sample A
check, so these runs are instrumented timing checks rather than a final visual
motion review. A `waiting` event is a buffering
signal, not necessarily a full one-second stall. Separate live runs do not
replay identical content or network delivery; the explicit seek targets and
configuration differentials provide the causal evidence.

## Confirmed differential on sample C

| Policy | Startup seeks (0–20s) | Steady seeks | Steady waiting events | Approx. frame callbacks/s | Longest steady frame gap | Median forward buffer |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Original, seek above 1.5s to a 0.5s reserve | 36 | 6 | 36 | 40.11 | 1,042 ms | 0.85s |
| Catch-up disabled (diagnostic control) | 0 | 0 | 0 | 49.81 | 42 ms | 33.27s |
| Revised, seek above 6s to a 3s reserve | 12 | 0 | 0 | 49.94 | 43 ms | 5.31s |

Disabling catch-up removes both the decoder-counter resets and the large
frame gaps on the same sample without replacing the decoder, byte bridge,
transmuxer, or compositor. It leaves over 30 seconds of forward buffer, so it
is a diagnostic control rather than the chosen live-viewing policy.

The revised policy changes only the installed adapter's two latency thresholds.
All recorded seeks in that candidate targeted the buffered end minus three
seconds. It retains enough buffered video to absorb the observed delivery
variation while continuing to catch up if forward buffering exceeds six
seconds. This is forward-buffer depth, not a measurement of total delay from
the broadcaster's live event.

The long stalls in these runs are explained by repeated application-induced
seeks and inadequate buffer reserve. The experiment provides no evidence that
GPU decode performance or JavaScript main-thread saturation caused those long
stalls. It does not prove that every frame on every Channel is perfectly paced.

## Cross-sample check and validation

The same revised AppImage completed all three 90-second sample runs. Each
recorded zero seeks, zero waiting events, and zero frame callback gaps above
100 ms during seconds 20–90:

| Sample | Approx. callbacks/s | Longest frame gap | Median forward buffer |
| --- | ---: | ---: | ---: |
| A, 720p50 | 49.97 | 34 ms | 5.26s |
| B, interlaced 1080 | 50.01 | 34 ms | 3.55s |
| C, interlaced 1080 | 49.94 | 43 ms | 5.31s |

Sample A's original-policy baseline recorded three steady seeks, four waiting
events, 47.84 callbacks/s, and a maximum gap of 283 ms. Sample B has a revised
run here but no new instrumented original-policy baseline; its earlier
comparison remains in the Hyprland report.

All six runs exited normally with no failed playback-state samples and no
remaining mpv sockets. The frontend's 307 tests, ESLint, production build,
99 installed Rust tests with the diagnostic feature, and Clippy with warnings
denied passed. The revised candidate is retained at
`~/.local/share/sparrow-playback-lab/Sparrow-lab.AppImage`; the local
`sparrow-playback-lab` launcher opens it with the existing private profile.

## Remaining limits

Startup still performs catch-up seeks while draining the provider's initial
burst (12 in sample C's revised run). The new thresholds reduce their frequency
but do not eliminate startup jumps. A future startup policy could wait for the
initial burst to settle and catch up once; that requires separate tune-time and
cancellation validation.

The runs are 90 seconds each, with a 70-second steady comparison interval.
Longer sessions, unusually variable networks, and other channel formats may
need further investigation. No monitor timing or hardware decode configuration
was changed. External mpv and the in-app-default preference remain intact.

Raw numeric observations, per-run AppImage hashes, completion/cleanup summaries,
and the aggregate [comparison](linux-in-app-frame-pacing-artifacts/comparison.json)
are retained under `linux-in-app-frame-pacing-artifacts/`. Raw provider data and
screenshots remain private.
