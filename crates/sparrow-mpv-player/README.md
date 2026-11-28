# Embedded Linux playback

Linux builds of `sparrow-desktop` enable `embedded` and link the system libmpv through pkg-config. A full Linux workspace build therefore needs libmpv development headers. The minimum pkg-config version is **1.109.0**, the software rendering API introduced in mpv 0.33. Ubuntu 22.04 supplies it in `libmpv-dev`. Building an individual package that does not enable `embedded` does not require libmpv. The optional `egl` feature also builds the offscreen OpenGL renderer and requires system EGL and GL development headers.

`TvPlaybackRunner` owns Playback Sessions, recovery and Audio Track selection. `PlaybackManager` selects and filters the MPEG-TS Audio Track before returning bytes. libmpv opens a single `sparrow://playback` callback tied to the exact stream generation. The callback receives bytes and privacy-safe errors. The HTTP connection and Playback Source stay in Rust. The libmpv instance disables config loading, external file autoloading and logs, and forces MPEG-TS demuxing.

The client and render APIs run on separate threads. Client controls and shutdown can proceed during rendering. Stopping interrupts any byte pull, joins the renderer and destroys the core. The latest frame uses packed opaque BGRA, carries an exact presentation key and serial, and includes the same three Ambient Playback Lighting zones as Shield. Samples update at most eight times per second. Recovery returns a still fitted to 640×360.

## GPUI handoff

- In `sparrow-desktop`, build the Playback Manager and `MpvPresentation` on the playback runtime, pass the presentation to `TvPlaybackRunner::spawn`, and call `connect`. Reuse the Guide's playback command, snapshot, host visibility and physical viewport interfaces.
- The desktop subscribes to frame publication, deduplicates by frame serial, and constructs each `RenderImage` on GPUI's background executor. The Guide paints the image below the controls, removes superseded images with `Window::drop_image`, and takes ambient colors from the same video frame.
- Use the Guide's still flight at dock/fullscreen transition. Hand off physical viewport sizes at the endpoints to avoid resizing the software buffer or EGL framebuffer on each animation tick. Acknowledge `still_handoff` when the recovery still paints. A user pause releases the Playback Source and holds the latest frame, then resume starts a new stream generation at the live edge.

## Playback test

`cargo test -p sparrow-mpv-player --features embedded` uses a loopback Playback Source and runs without a window or audio device. Its video-only fixture was generated with `ffmpeg -f lavfi -i 'color=c=blue:s=64x48:r=12:d=4' -an -c:v libx264 -g 12 -mpegts_flags +pat_pmt_at_frames -f mpegts tests/fixtures/blue.mpegts` from this crate's directory.

## Run the benchmark

Generate a 40-second moving MPEG-TS clip with two Audio Tracks.

```sh
ffmpeg -y -f lavfi -i 'testsrc2=size=1920x1080:rate=60' \
  -f lavfi -i 'sine=frequency=440:sample_rate=48000' \
  -f lavfi -i 'sine=frequency=660:sample_rate=48000' \
  -map 0:v -map 1:a -map 2:a -metadata:s:a:0 language=eng \
  -metadata:s:a:1 language=swe -t 40 -c:v libx264 -preset veryfast \
  -crf 21 -threads 8 -g 60 -pix_fmt yuv420p -c:a aac \
  -f mpegts /tmp/sparrow-mpv-1080p60.ts
cargo build --release -p sparrow-mpv-player --features benchmark,egl --example benchmark
target/release/examples/benchmark /tmp/sparrow-mpv-1080p60.ts 40 docked sw 18
target/release/examples/benchmark /tmp/sparrow-mpv-1080p60.ts 40 fullscreen sw 18
target/release/examples/benchmark /tmp/sparrow-mpv-1080p60.ts 40 docked egl 18
target/release/examples/benchmark /tmp/sparrow-mpv-1080p60.ts 40 fullscreen egl 18
```

The fixture serves paced bytes over loopback, with a temporary Source Configuration. CPU is process time divided by wall time. Native timings cover libmpv rendering and EGL readback where applicable. Packed timings also include the copy into opaque BGRA. FPS counts new pictures published by the renderer. GPUI upload/compositing and production Playback Sources require a separate installed-build measurement.

## Results

Ryzen 7 9800X3D, GeForce RTX 4070 SUPER and Radeon 610M, libmpv 2.5.0, PipeWire. Each row samples 18 seconds after a two-second warmup.

| Renderer | Target | FPS | CPU (% of one core) | Native p50 / p95 (ms) | Packed p50 / p95 (ms) | VO delayed / decoder drops / VO drops |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Software | 640×360 | 60.00 | 24.2 | 0.292 / 0.407 | 0.379 / 0.482 | 0 / 0 / 0 |
| Software | 1920×1080 | 60.00 | 39.7 | 0.547 / 0.605 | 1.184 / 1.381 | 0 / 0 / 0 |
| EGL + hardware decode | 640×360 | 59.94 | 7.2 | 0.677 / 1.785 | 0.755 / 2.208 | 0 / 0 / 1 |
| EGL + hardware decode | 1920×1080 | 60.00 | 11.4 | 0.969 / 1.773 | 1.644 / 2.422 | 0 / 0 / 0 |

The fixture verified real audio device activation, volume and mute, resolution changes within one binding, pause and retained picture, resume, selection of a different MPEG-TS Audio Track, recovery after a truncated response, and an opaque mpv path and playlist. Stopping a stalled response took 84.1–176.1 ms. The fixture observed at most one active Playback Source connection. Idle resource counts returned to the warm baseline of 12 FDs and 19 threads for software, or 42 FDs and 20 threads with the EGL driver loaded.

**Go for GPUI integration.** Software is the baseline for the first integration. EGL measured 71% lower process CPU at fullscreen with the cost of synchronous GPU readback. Measure texture upload and composition in the installed Guide with representative Playback Sources before switching the Linux Primary Playback Engine.
