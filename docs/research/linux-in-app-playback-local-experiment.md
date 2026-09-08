# Local experiments: WebKit/MSE and embedded libmpv

Date: 2026-09-08

## Result

Both approaches produced video locally. Embedded libmpv rendered inside a GTK
window underneath a transparent WebKit interface on X11 and native Wayland.
WebKit/MSE played the same synthetic MPEG-TS fixture on both backends, with
both tested WebKit renderer policies. This establishes local feasibility;
it does not reproduce or overturn Sparrow's earlier IPTV-stream measurements.

The user's challenge was correct: a headless host still allowed useful real
experiments. Xvfb and Weston were already installed. There was no exposed
`/dev/dri` device, so the trials used Mesa llvmpipe software rendering.

Continue on another host using the [testing handoff](linux-in-app-playback-testing-handoff.md).
The [runnable diagnostic harness](../../scripts/debug/linux-playback-lab/README.md)
and [raw artifacts](linux-in-app-playback-local-artifacts/summary.json) accompany
this report. No production playback code or default was changed.

## Setup

- Ubuntu 26.04, GTK 3.24.52, WebKitGTK 2.52.6, Weston 14.0.2, mpv/libmpv 0.41.0,
  FFmpeg 8.0.1, GStreamer GL plugin 1.28.2.
- A generated 45-second 1280×720 progressive H.264/AAC transport stream at
  30 fps, served at live pace over loopback HTTP. No provider URLs or credentials.
- `mpegts.js 1.7.3`, stash enabled, worker disabled; a direct HTTP loader.
- Native engine: libmpv's OpenGL render API in GTK3 GLArea, below a transparent
  WebKit view in GtkOverlay. Software decode, null audio output.
- X11 runs used Xvfb. The recorded Wayland runs used native Wayland clients
  talking to Weston, whose output was nested into Xvfb for screenshot capture.
  An earlier exploratory embedded run also worked with headless Weston.
  This is not a test of XWayland clients or Hyprland.
- A 28-second scripted run per variant. Resize at 6 s; fullscreen requested at
  10 s and left at 12 s; release at 14 s; resume at 16 s; release before
  replacement at 19 s; replacement at 20 s; final stop at 26 s; teardown at 28 s.

Before the main matrix, WebKit reported missing GL video-sink dependencies.
Installing `gstreamer1.0-gl` removed that missing-dependency condition.
The preliminary MSE run already played through a different rendering path;
this was not a demonstrated fix for Sparrow's original bug.

MSE runs logged unavailable ALSA devices, and the environment lacked an
accessibility bus. Video still advanced, but these runs make no claim about
audible playback or accessibility. Those warnings remain in the raw logs.

## Recorded matrix

Each row links its raw log. Media counters below refer to the first playback
generation before release, not totals across reconnects. Render rates are
short-run observations, not a throughput benchmark.

| Variant | Observed video / counters | Outcome |
| --- | --- | --- |
| [X11 MSE, renderer disabled](linux-in-app-playback-local-artifacts/x11-mse-disable.log) | 369 total video frames at final active sample; 0 dropped; frame callbacks stayed at 0 | Picture visible; media time advanced; clean exit |
| [X11 MSE, SHM](linux-in-app-playback-local-artifacts/x11-mse-shm.log) | 368 total frames; 0 dropped; 368 callbacks at the same sample | Picture visible; callbacks tracked playback; clean exit |
| [X11 embedded libmpv, WebKit SHM](linux-in-app-playback-local-artifacts/x11-embed-shm.log) | 300 additional render calls from seconds 3–13, all sampled pixels changed; 0 decoder drops; output drops stayed at 3 after startup | In-window video beneath web UI; clean teardown |
| [Wayland MSE, renderer disabled](linux-in-app-playback-local-artifacts/wayland-mse-disable.log) | 402 total frames; 0 dropped; frame callbacks stayed at 0 | Picture visible; media time advanced; clean exit |
| [Wayland MSE, SHM](linux-in-app-playback-local-artifacts/wayland-mse-shm.log) | 339 total frames; 0 dropped; 338 callbacks at the same sample | Picture visible; callbacks tracked playback; clean exit |
| [Wayland embedded libmpv, WebKit renderer disabled](linux-in-app-playback-local-artifacts/wayland-embed-disable.log) | 300 additional render calls from seconds 3–13, all sampled pixels changed; 0 decoder drops; output drops stayed at 6 after startup | In-window video beneath web UI; clean teardown |

The differing totals are affected by startup and sampling time; do not rank
engines using them. MSE active samples generally advanced about 30 frames per
second. mpv's decoder and output drop counts are distinct. Startup output
drops occurred even though decoder drops were zero.

Video-frame callbacks staying at zero in disabled-renderer variants did not
mean there was no picture. Screenshots and other media counters contradicted
that interpretation. Similarly, a screenshot proves composition at one moment,
not sustained display cadence. mpv's sampled framebuffer readback provides
additional evidence of changing pixels, but also adds diagnostic overhead.

The final local-server count returned to zero between the scripted replacement
generations, with an observed peak of one in the matrix's
[server log](linux-in-app-playback-local-artifacts/runner-server.log).
This is not a proof of Sparrow's provider-ownership invariant: the harness uses
a deliberate one-second gap and server handlers detect closed clients on their
next write. Early exploratory runs exposed a harness bug where FFmpeg could
remain blocked on its stdout pipe during cleanup; the server now closes that
pipe and bounds process termination. Those initial handler counts were not
valid application connection measurements.

## Control checks and visual evidence

### Follow-up: web controls and fullscreen

The final harness wires the web page's buttons to native start/stop through a
WebKit message handler, and exercises them programmatically. Three additional
runs covered X11 embedded mpv, Wayland MSE/SHM and Wayland embedded mpv.
All exited cleanly; the native logs show `control:stop` / `control:start`
messages and matching playback events, with an idle player after final stop.
Both Wayland runs reported actual `fullscreen=1` then `fullscreen=0` window
state transitions. See the [follow-up results](linux-in-app-playback-local-artifacts/controls/summary.json),
[embedded Wayland log](linux-in-app-playback-local-artifacts/controls/wayland-embed-disable.log)
and [MSE Wayland log](linux-in-app-playback-local-artifacts/controls/wayland-mse-shm.log).
This verifies the diagnostic bridge and Weston window transitions, not the
production Tauri bridge, pointer hit testing, or Hyprland behavior.

Screenshots were inspected for embedded libmpv as a native Wayland client with
the WebKit interface above it, and for WebKit/MSE on native Wayland with SHM.
They showed the test pattern within the window and the web overlays above it.
Review-only screenshots are kept outside Git. The runner recreates
`wayland-embed-disable.png` and `wayland-mse-shm.png` in its local output
directory; retain those files with any new machine's observations.

## What this changes

The GTK/libmpv arrangement is now more than an upstream source precedent:
it renders locally with Sparrow's GTK/WebKit generation, including on a
native Wayland client. This supports building the focused Tauri integration.

MSE also remains plausible. The generic claim that this WebKitGTK stack cannot
play live MPEG-TS is not supported by these trials. The harder question is
why the representative provider streams failed in the installed app. This
fixture bypasses Tauri's Rust byte bridge and uses a simple progressive
H.264/AAC stream, so it cannot settle that question.

Keep the [proposed investigation order](linux-in-app-playback-options.md):
a bounded packaged MSE differential on the original streams, followed by
an embedded libmpv candidate if MSE still fails. These trials do not justify
changing the production primary yet.

## Remaining limits

No AppImage or Tauri integration was built here. No original failing stream,
HEVC/AC-3 or interlaced matrix, real audio output/A/V-sync check, long soak,
GPU decode/zero-copy test, target NVIDIA/AMD driver test, fractional-scaling
matrix, accessibility check, or production cancellation/privacy audit was run.
Plain Xvfb has no window manager, so fullscreen requests there cannot validate
fullscreen behavior. Hyprland remains the final target.
