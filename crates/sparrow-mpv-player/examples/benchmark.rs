#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
fn main() {
    linux::run();
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        io,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
        },
        time::{Duration, Instant},
    };

    use sparrow_core::{ChannelQuery, PageLimit, PageRequest};
    use sparrow_installed_catalog::{InstalledCatalog, SourceConfigurationUpdate};
    use sparrow_installed_playback::{
        PlaybackManager,
        android::AndroidPlaybackViewport,
        noop_screen_wake,
        tv::{
            TvPlaybackCommand as Command, TvPlaybackPhase as Phase, TvPlaybackRunner,
            TvPresentation, TvTuneTarget,
        },
    };
    use sparrow_mpv_player::{Backend, MpvPresentation, VideoFrame};
    use sparrow_source_http::HttpPlaybackAccess;
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        net::{TcpListener, TcpStream},
        sync::mpsc,
        time::{interval, sleep, timeout},
    };

    struct Fixture {
        ts: Vec<u8>,
        bytes_per_tick: usize,
        active: AtomicUsize,
        peak: AtomicUsize,
        stalled: AtomicBool,
        interrupt: AtomicBool,
    }

    struct Active<'a>(&'a Fixture);

    impl Drop for Active<'_> {
        fn drop(&mut self) {
            self.0.active.fetch_sub(1, Relaxed);
        }
    }

    impl Fixture {
        async fn serve(&self, mut socket: TcpStream, port: u16) -> io::Result<()> {
            let mut request = String::new();
            let mut incoming = BufReader::new(&mut socket);
            incoming.read_line(&mut request).await?;
            loop {
                let mut header = String::new();
                incoming.read_line(&mut header).await?;
                if header == "\r\n" || header.is_empty() {
                    break;
                }
            }
            if request.starts_with("GET /catalog.m3u ") {
                let playlist = format!(
                    "#EXTM3U\n#EXTINF:-1,Embedded benchmark\nhttp://127.0.0.1:{port}/private-fixture.ts\n"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{playlist}",
                    playlist.len()
                );
                return socket.write_all(response.as_bytes()).await;
            }
            if !request.starts_with("GET /private-fixture.ts ") {
                return socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            }
            let active = self.active.fetch_add(1, Relaxed) + 1;
            self.peak.fetch_max(active, Relaxed);
            let _active = Active(self);
            socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                self.ts.len(),
            ).as_bytes()).await?;
            let (mut reader, mut writer) = socket.split();
            let mut tick = interval(Duration::from_millis(10));
            let mut disconnect = [0; 1];
            for chunk in self.ts.chunks(self.bytes_per_tick) {
                loop {
                    tokio::select! {
                        _ = reader.read(&mut disconnect) => return Ok(()),
                        _ = tick.tick() => {
                            if self.interrupt.swap(false, Relaxed) {
                                return Ok(());
                            }
                            if !self.stalled.load(Relaxed) {
                                break;
                            }
                        }
                    }
                }
                writer.write_all(chunk).await?;
            }
            Ok(())
        }
    }

    fn viewport(width: u32, height: u32, fullscreen: bool) -> AndroidPlaybackViewport {
        AndroidPlaybackViewport::parse(0, 0, width, height, fullscreen).unwrap()
    }

    async fn wait_until<T>(mut check: impl FnMut() -> Option<T>) -> T {
        timeout(Duration::from_secs(10), async {
            loop {
                if let Some(result) = check() {
                    return result;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("benchmark condition timed out")
    }

    async fn send(commands: &mpsc::Sender<Command>, command: Command) {
        commands.send(command).await.expect("runner is live");
    }

    fn count(path: &str) -> usize {
        std::fs::read_dir(path).unwrap().count()
    }

    fn resources() -> (usize, usize) {
        (count("/proc/self/fd"), count("/proc/self/task"))
    }

    fn cpu_seconds() -> f64 {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) },
            0
        );
        time.tv_sec as f64 + time.tv_nsec as f64 / 1_000_000_000.
    }

    async fn picture(player: &MpvPresentation, width: u32, height: u32) -> Arc<VideoFrame> {
        wait_until(|| {
            player
                .latest_frame()
                .filter(|frame| frame.width == width && frame.height == height)
        })
        .await
    }

    #[tokio::main]
    pub(super) async fn run() {
        let args: Vec<_> = std::env::args().skip(1).collect();
        let [file, clip_seconds, mode, backend, sample_seconds] = args.as_slice() else {
            panic!("usage: benchmark FILE.ts CLIP_SECONDS docked|fullscreen sw|egl SAMPLE_SECONDS");
        };
        let clip_seconds: usize = clip_seconds.parse().unwrap();
        let sample_seconds: u64 = sample_seconds.parse().unwrap();
        assert!(
            sample_seconds > 0 && clip_seconds as u64 >= sample_seconds + 8,
            "allow at least eight extra seconds for warmup and checks"
        );
        let backend = match backend.as_str() {
            "sw" => Backend::Software,
            #[cfg(feature = "egl")]
            "egl" => Backend::Egl,
            _ => panic!("backend unavailable"),
        };
        let ts = std::fs::read(file).unwrap();
        let fixture = Arc::new(Fixture {
            bytes_per_tick: ts.len() / (clip_seconds * 100),
            ts,
            active: 0.into(),
            peak: 0.into(),
            stalled: false.into(),
            interrupt: false.into(),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let origin = fixture.clone();
        let server = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let origin = origin.clone();
                tokio::spawn(async move {
                    let _ = origin.serve(socket, port).await;
                });
            }
        });

        let temporary = tempfile::tempdir().unwrap();
        let catalog = InstalledCatalog::prepare(temporary.path())
            .unwrap()
            .finish_open()
            .await
            .unwrap();
        catalog
            .replace_configuration(
                SourceConfigurationUpdate::parse(
                    format!("http://127.0.0.1:{port}/catalog.m3u"),
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let core = catalog.core().clone();
        let page = wait_until(|| {
            core.list_channels(ChannelQuery::all(PageRequest::first(
                PageLimit::new(1).unwrap(),
            )))
            .ok()
            .filter(|page| !page.items().is_empty())
        })
        .await;
        let target = TvTuneTarget::new(page.items()[0].id().clone(), 1);
        let playback = Arc::new(PlaybackManager::new(
            core.clone(),
            HttpPlaybackAccess::new().unwrap(),
            catalog.private_root().to_path_buf(),
            noop_screen_wake(),
        ));
        let player = Arc::new(MpvPresentation::new(&playback, backend).unwrap());
        let runner = TvPlaybackRunner::spawn(core, playback, player.clone()).unwrap();
        player.connect(&runner);
        let commands = runner.commands();
        let state = runner.subscribe();
        runner.set_viewports(
            1,
            Some(viewport(640, 360, false)),
            Some(viewport(1920, 1080, true)),
        );
        runner.host_visibility(1, true).await.unwrap();
        send(&commands, Command::SetVolume(0.01)).await;
        send(&commands, Command::ToggleMute).await;

        // Warm up library and audio backend process-wide resources before measuring teardown.
        send(
            &commands,
            Command::Tune {
                target: target.clone(),
                repeat: false,
            },
        )
        .await;
        picture(&player, 640, 360).await;
        send(&commands, Command::Stop).await;
        wait_until(|| {
            (state.borrow().phase == Phase::Idle && fixture.active.load(Relaxed) == 0).then_some(())
        })
        .await;
        let baseline = resources();

        send(
            &commands,
            Command::Tune {
                target,
                repeat: false,
            },
        )
        .await;
        let (width, height) = match mode.as_str() {
            "docked" => (640, 360),
            "fullscreen" => {
                send(&commands, Command::Fullscreen).await;
                (1920, 1080)
            }
            _ => panic!("unknown viewport"),
        };
        let initial = picture(&player, width, height).await;
        let still = player.capture_frame(initial.key.clone()).await.unwrap();
        assert_eq!(
            (still.width, still.height, still.bgra.len()),
            (640, 360, 640 * 360 * 4)
        );
        wait_until(|| (state.borrow().phase == Phase::Playing).then_some(())).await;
        sleep(Duration::from_secs(2)).await;
        let pre = player.diagnostics().await.unwrap();
        assert!(
            pre.benchmark.audio_output_active
                && pre.opaque_playlist
                && pre.benchmark.muted
                && !pre.paused,
            "{pre:?}"
        );
        assert!((pre.benchmark.volume - 1.).abs() < 0.01, "{pre:?}");
        assert!(
            pre.benchmark.track_count == Some(2) && pre.benchmark.audio_stream_id.is_some(),
            "{pre:?}"
        );
        let start = Instant::now();
        let cpu = cpu_seconds();
        let mut serial = player.latest_frame().unwrap().serial;
        let mut samples = Vec::new();
        let mut native_samples = Vec::new();
        let mut changed_ambient = false;
        let mut sample_tick = interval(Duration::from_millis(1));
        while start.elapsed() < Duration::from_secs(sample_seconds) {
            sample_tick.tick().await;
            if let Some(frame) = player.latest_frame()
                && frame.serial != serial
            {
                serial = frame.serial;
                samples.push(frame.render_time.as_secs_f64() * 1_000.);
                native_samples.push(frame.native_render_time.as_secs_f64() * 1_000.);
                changed_ambient |= frame.ambient != initial.ambient;
            }
        }
        let elapsed = start.elapsed().as_secs_f64();
        let cpu = (cpu_seconds() - cpu) * 100. / elapsed;
        let post = player.diagnostics().await.unwrap();
        samples.sort_by(f64::total_cmp);
        native_samples.sort_by(f64::total_cmp);
        let p = |values: &[f64], fraction: f64| {
            values[((values.len() as f64 * fraction).ceil() as usize) - 1]
        };
        let delta = |a: Option<u64>, b: Option<u64>| a.zip(b).map(|(a, b)| b.saturating_sub(a));
        println!(
            "{backend:?} {width}x{height}, {elapsed:.3}s, {:.2} FPS, CPU {cpu:.1}% of one core, {} timings, p50/p95 native {:.3}/{:.3}ms, packed {:.3}/{:.3}ms, delayed {:?}, decoder drops {:?}, VO drops {:?}, hardware decode {}",
            (post.presented_frames - pre.presented_frames) as f64 / elapsed,
            samples.len(),
            p(&native_samples, 0.50),
            p(&native_samples, 0.95),
            p(&samples, 0.50),
            p(&samples, 0.95),
            delta(
                pre.benchmark.vo_delayed_frames,
                post.benchmark.vo_delayed_frames
            ),
            delta(pre.decoder_dropped_frames, post.decoder_dropped_frames),
            delta(pre.vo_dropped_frames, post.vo_dropped_frames),
            post.benchmark.hardware_decoding,
        );
        assert!(changed_ambient && post.opaque_playlist);
        assert!(
            initial
                .bgra
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 255)
        );
        drop(initial);

        let same = player.latest_frame().unwrap().key.clone();
        let (switch, new_width, new_height) = if width == 640 {
            (Command::Fullscreen, 1920, 1080)
        } else {
            (Command::ExitFullscreen, 640, 360)
        };
        send(&commands, switch).await;
        assert_eq!(picture(&player, new_width, new_height).await.key, same);
        send(&commands, Command::SetVolume(0.0)).await;
        send(&commands, Command::ToggleMute).await;
        wait_until(|| {
            (!state.borrow().controls.muted && state.borrow().controls.volume == 0.).then_some(())
        })
        .await;
        let controls = player.diagnostics().await.unwrap();
        assert!(
            !controls.benchmark.muted && controls.benchmark.volume == 0.,
            "{controls:?}"
        );

        send(&commands, Command::Pause).await;
        wait_until(|| {
            (matches!(state.borrow().phase, Phase::Paused(_)) && fixture.active.load(Relaxed) == 0)
                .then_some(())
        })
        .await;
        let paused = player.latest_frame().expect("paused picture");
        sleep(Duration::from_millis(100)).await;
        assert_eq!(player.latest_frame().unwrap().serial, paused.serial);
        assert!(player.capture_frame(paused.key.clone()).await.is_ok());
        send(&commands, Command::Resume).await;
        wait_until(|| {
            player
                .latest_frame()
                .filter(|frame| frame.key != paused.key)
        })
        .await;
        wait_until(|| (state.borrow().phase == Phase::Playing).then_some(())).await;

        let snapshot = state.borrow().clone();
        let track = snapshot
            .audio
            .unwrap()
            .tracks
            .into_iter()
            .find(|track| !track.selected())
            .expect("fixture must have two Audio Tracks")
            .id()
            .clone();
        let token = snapshot.transport_token.unwrap();
        send(
            &commands,
            Command::SelectAudio {
                token,
                track: track.clone(),
            },
        )
        .await;
        wait_until(|| {
            let snapshot = state.borrow();
            (snapshot.phase == Phase::Playing
                && snapshot.transport_token != Some(token)
                && snapshot.audio.as_ref().is_some_and(|audio| {
                    audio
                        .tracks
                        .iter()
                        .any(|item| item.id() == &track && item.selected())
                }))
            .then_some(())
        })
        .await;
        let diagnostics = player.diagnostics().await.unwrap();
        assert!(diagnostics.benchmark.audio_output_active && diagnostics.opaque_playlist);
        assert!(
            diagnostics.benchmark.track_count == Some(2)
                && diagnostics.benchmark.audio_stream_id.is_some()
                && diagnostics.benchmark.audio_stream_id != pre.benchmark.audio_stream_id,
            "{diagnostics:?}"
        );

        // Break an in-flight response without finishing its Content-Length.
        // Acknowledging the captured still as the GUI would lets recovery rebind.
        let until_failure = player.latest_frame().unwrap().key.clone();
        let mut painted_still = false;
        fixture.interrupt.store(true, Relaxed);
        wait_until(|| {
            let snapshot = state.borrow();
            if let Some(handoff) = &snapshot.still_handoff {
                assert!(snapshot.recovery_frame.is_some());
                painted_still = true;
                handoff.painted();
            }
            (snapshot.phase == Phase::Playing && snapshot.recovery_attempts > 0)
                .then(|| player.latest_frame())
                .flatten()
                .filter(|frame| frame.key != until_failure)
        })
        .await;
        assert!(painted_still);

        fixture.stalled.store(true, Relaxed);
        sleep(Duration::from_millis(600)).await;
        let stopping = Instant::now();
        send(&commands, Command::Stop).await;
        wait_until(|| {
            (state.borrow().phase == Phase::Idle
                && fixture.active.load(Relaxed) == 0
                && player.latest_frame().is_none())
            .then_some(())
        })
        .await;
        wait_until(|| (resources() == baseline).then_some(())).await;
        assert_eq!(
            fixture.peak.load(Relaxed),
            1,
            "provider connections overlapped"
        );
        println!(
            "verified viewport switch, still hold, resume, controls, Audio Track, private URI, real audio and recovery; stalled stop {:.1}ms; idle FDs/threads {baseline:?} restored",
            stopping.elapsed().as_secs_f64() * 1000.
        );
        drop(commands);
        drop(runner);
        wait_until(|| (Arc::strong_count(&player) == 1).then_some(())).await;
        drop(player);
        wait_until(|| (count("/proc/self/task") < baseline.1).then_some(())).await;
        server.abort();
    }
}
