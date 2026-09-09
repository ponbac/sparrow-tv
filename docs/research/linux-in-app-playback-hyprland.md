# Hyprland laptop continuation

Date: 2026-09-09. Starting revision: `aa03304d` (PR #62).

Follow-up: the [frame pacing diagnosis](linux-in-app-frame-pacing.md) identifies
aggressive latency chasing and insufficient forward buffer as the cause of the
measured long stalls. The revised installed player retains a three-second
reserve after catch-up. The earlier observations below remain historical.

## Findings

The diagnostic in-app picture works on this computer with native Wayland
and XWayland, including embedded libmpv using the Intel GPU. Real-stream
testing also found a separate installed-client bug: Rust serialized AC-3 as
`ac3`, whereas the TypeScript descriptor accepts `ac-3`. MPEG-1/2 audio had
the analogous `mpeg1-audio` / `mpeg-1-audio` mismatch. The frontend rejected
these descriptors before starting the MSE engine. Explicit wire names and
a regression test now cover all five Audio Codec variants.

After that fix, the experimental AppImage displayed the private 720p50
sample through Sparrow's actual Rust transport and WebKit/MSE adapter.
This does not establish that the original choppiness is fixed on every
Channel. The owner reports the original problem affected all Channels;
the samples here are a small cross-section, not an exhaustive catalog test.

The owner subsequently chose in-app playback as the Linux default, with an
explicit **Open in mpv** button and a stacked player above the guide. This
accepted product tradeoff is recorded in ADR 0001; these measurements do not
establish equivalent playback quality.

## Requested integrated-player default

The revised Linux build starts in-app without an engine override. The player
spans the guide width above its content, with compact programme metadata below.
The player container enters fullscreen through the button, double-click, or F
while focused; Escape restores the guide. Fullscreen controls overlay the
picture and fade after idle time, remaining available to keyboard focus.

A 180-second packaged run on private sample A produced 181 observations, 177
playing and none failed. Trusted Hyprland input exercised the fullscreen button,
F while the player was focused, and Escape. The probe observed DOM fullscreen
and the compositor reported a 1280×800 logical fullscreen window on the 2× panel.
The picture and idle control hiding were inspected. See
[fullscreen observations](linux-in-app-playback-hyprland-artifacts/stacked-fullscreen/samples.json).
Raw desktop captures remain private. A second 90-second packaged run
clicked the real **Open in mpv** control at 20s and **Play in app** at 50s.
mpv played in its separate window, then disappeared as in-app frame callbacks
resumed. All 91 observations recorded zero failed states, and the app exited
with no remaining mpv sockets. See the [switch samples](linux-in-app-playback-hyprland-artifacts/stacked-switch/samples.json)
and [mpv counters](linux-in-app-playback-hyprland-artifacts/stacked-switch/mpv.json).

The **Open in mpv** and **Play in app** buttons replace the session after final
cleanup acknowledgement. Tests cover waiting for release, blocking a switch on
unconfirmed cleanup, preserving explicit choice on resume, and returning the
next Channel to the default. Fullscreen tests cover the player plus controls
and synchronizing browser exit state. All 307 frontend tests passed, along with
workspace Rust tests, the 99-test installed suite in normal and lab builds,
TypeScript/build, ESLint, and Clippy with warnings denied.

## Host and artifacts

- Arch Linux / Omarchy, Linux `7.2.3-arch1-3`, Hyprland `0.56.2`.
- Intel Meteor Lake-P / Arc Graphics, PCI `8086:7d55`, kernel driver `i915`,
  Mesa `26.2.2`.
- Laptop panel: 2560×1600, 120 Hz, scale 2. No monitor migration or fractional
  scaling matrix was attempted.
- GTK `3.24.52`, WebKitGTK `2.52.6`, GStreamer `1.28.6`, mpv `0.41.0`
  (libmpv pkg-config API version `2.5.0`), FFmpeg `9.0.1`.
- Installed GStreamer base/good/libav plugins. Bad/ugly plugin packages are
  absent. Do not equate a loaded plugin library with an observed decoder.
- All installed comparisons below use the same opt-in release-mode AppImage.
  Its hash is recorded in each run's `artifact.json`. It is not a published
  release, despite retaining the branch's `0.11.4` package version.

The regular packaging helper preparation initially failed: the upstream
continuous AppImage-plugin download no longer matched the repository hash.
The other authorized computer retained the exact matching helper. Copying
it into the local Tauri cache and verifying SHA-256 restored preparation
without changing the pin. A `patchelf` binary was extracted from the Arch
package and installed at `~/.local/bin/patchelf` because this computer lacked
the tool and passwordless sudo. No desktop configuration or system packages
were changed.

The package's actual process mappings included its bundled WebKit,
GStreamer playback and libav libraries, with system EGL libraries. Full
library mappings are retained in the private run directories. Hyprland
reported the installed app as XWayland, matching the packaged launch path.
External mpv used its separate native Wayland window.

## Synthetic desktop matrix

Run with `python3 scripts/debug/linux-playback-lab/run.py --desktop`.
Unlike the original virtual-display experiment, this uses the real desktop
and does not force Mesa software rendering. Seven 28-second variants passed
the process/cleanup checks and displayed the fixture:

| Backend | MSE | Embedded libmpv |
| --- | --- | --- |
| Native Wayland | Disabled renderer and SHM | Disabled renderer and SHM |
| XWayland | Disabled renderer and SHM | SHM |

Both Wayland embedded variants reported
`Mesa Intel(R) Arc(tm) Graphics (MTL)`. Between seconds 3 and 13 each produced
300 new render calls and 300 changed framebuffer samples, with zero decoder
drops. The disabled-policy run retained one initial output drop; SHM had
zero in that interval. These readbacks add overhead and are not benchmarks.
Decode remained `hwdec=no` and audio `ao=null` in this harness.

Hyprland confirmed the actual backend in window metadata. The logs record
fullscreen entry/exit, web-button release/resume/replacement, final stop and
cleanup. A separate window-manager-close test exited zero with cleanup after
five seconds. The harness now delays widget destruction until the libmpv
render context is freed, including that close path.

See [Wayland logs](linux-in-app-playback-hyprland-artifacts/wayland/summary.json)
and [XWayland logs](linux-in-app-playback-hyprland-artifacts/xwayland/summary.json).
Screenshots were reviewed locally and are not committed.

## Installed comparison

The `linux-playback-lab` Cargo feature adds an explicitly selected MSE path
to start and resume, plus a bounded UI probe. Ordinary builds compile out
both facilities. No provider source is passed to JavaScript or to process
arguments. The test runner serves a single private catalog to Rust and uses
a temporary isolated app-data directory for each variant.

Each run samples for 65 seconds after the page loads. Selection occurs through
the real guide button; the existing native loader, audio projection, playback
state machine, and UI remain in use. The probe stops playback and exits. The
runner checks process completion and remaining mpv sockets. The source inputs
are sequential live requests, not byte-identical recordings.

Sample A: progressive H.264 High, 1280×720 at 50 fps, AC-3 and HE-AAC/LATM
audio tracks. MSE selected AC-3. Before the wire-name fix, the app displayed
source unavailable with no video frames. After the fix:

| Packaged sample A | Observation |
| --- | --- |
| XWayland MSE, renderer disabled | First playing sample at 6 s. Between 35–65 s, 1,493 additional video frames, no new reported drops, media time advanced 30.04 s. Video-frame callbacks stayed at zero despite visible video. |
| XWayland MSE, SHM | First playing sample at 5 s. Between 35–65 s, 1,454 callbacks (48.5/s), media time advanced 30.02 s. The decoder counter reset within the interval, so its net frame/drop difference is not a valid cumulative total. |
| External mpv | 50 fps, zero decoder/output drops in the recorded steady samples; see raw numeric samples for A/V timing and cache state. |

MSE reported multiple decoder-counter resets during early playback (ten with
disabled rendering, seven with SHM). They must not be summed or interpreted
as zero drops across the full run. The reported UI playing state and a
screenshot alone do not establish perfectly smooth presentation.

See [sample A evidence](linux-in-app-playback-hyprland-artifacts/packaged-sample-a-fixed/summary.json).
Only bounded sample JSON and aggregate results are committed for real streams.
Raw logs, screenshots, the private catalog and credentials remain local.

Two further private samples tested the corrected SHM candidate against mpv:

| Sample | MSE/SHM observation | External mpv observation |
| --- | --- | --- |
| B, interlaced H.264 1920×1080 / stereo AC-3 | Playing throughout the steady interval; 48.4 callbacks/s in seconds 35–65, with one decoder-counter reset. | 25 fps; zero decoder/output drops. |
| C, interlaced H.264 1920×1080 / 5.1 AC-3 plus MP2 | First playing sample at 4 s; 43.8 callbacks/s in seconds 35–65, with seven decoder-counter resets and only 29.60 s of media-time advancement. | 25 fps; zero decoder/output drops. |

FFprobe reported top-field-first interlacing for both B and C. These are not
equivalent counter definitions: presentation callbacks and mpv's reported
video-filter rate measure different stages of the interlaced playback paths.
In particular, sample C's repeated resets and uneven callback delivery leave
the original smoothness concern open. Successful startup on three samples
does **not** establish that MSE matches mpv's playback quality.

See [sample B](linux-in-app-playback-hyprland-artifacts/packaged-sample-b-fixed/summary.json)
and [sample C](linux-in-app-playback-hyprland-artifacts/packaged-sample-c-fixed/summary.json).
All seven corrected packaged runs exited zero, emitted the completion marker,
and left no mpv sockets. Codec-only FFprobe output is included alongside them.

## Reproduction and remaining gate

Build and run with the [harness README](../../scripts/debug/linux-playback-lab/README.md).
The current computer has a separate private candidate profile at
`~/.local/share/sparrow-playback-lab`, populated from the authorized remote
configuration. The tested AppImage is retained there as `Sparrow-lab.AppImage`,
with a local launcher:

```sh
sparrow-playback-lab       # in-app MSE candidate
sparrow-playback-lab mpv   # external comparison (close the candidate first)
```

For manual testing of a freshly built candidate directly from the repository:

```sh
XDG_DATA_HOME="$HOME/.local/share/sparrow-playback-lab" \
SPARROW_LINUX_PLAYBACK_ENGINE=mse \
  target/release/bundle/appimage/Sparrow_0.11.4_amd64.AppImage
```

Omit the engine variable for the in-app default; use the **Open in mpv**
button or set it to `mpv` for the external comparison.
The automatic probe only runs when `SPARROW_PLAYBACK_LAB_SECONDS` is set.

Remaining: agreed equivalence tolerances, repeated ≥60-second samples and a
ten-minute soak, actual perceived/audio A/V sync, guide/focus interactions,
track changes, rapid switching, suspension, renderer failure, broader codec
coverage and hardware decode. The GTK embedded diagnostic is still not a
Tauri embedding implementation. MSE's decoder was not conclusively identified;
embedded and external mpv explicitly reported software decode.

Validation: 311 Rust workspace tests, normal and opt-in installed tests, workspace
and feature Clippy with warnings denied, frontend lint, 302 frontend tests,
production frontend build and AppImage build. Python syntax and C warning-enabled
compilation passed; the actual desktop and private playback runs provide the
runtime evidence above.
