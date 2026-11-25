//! Android entry points for the Sparrow Shield development host.
#![cfg(target_os = "android")]

mod activity;
mod android_player;
mod catalog;
mod channel_icons;
mod guide_fonts;
mod guide_surface;
mod guide_view;
mod playback;
mod search_flow;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, Once, OnceLock},
    time::Instant,
};

use gpui::{AnyWindowHandle, Global, QuitMode};
use gpui_mobile::android::{host, jni as mobile_jni};
use jni::{
    EnvUnowned,
    errors::{Error, LogErrorAndDefault},
    objects::{JObject, JString},
    sys::{jboolean, jstring},
};
use ndk::native_window::NativeWindow;
use tokio::sync::mpsc;

static INITIALIZE_LOGS: Once = Once::new();
static START_RENDERER: OnceLock<mpsc::UnboundedSender<host::HostId>> = OnceLock::new();
static ACTIVITY_EVENTS: OnceLock<mpsc::UnboundedSender<(host::HostId, activity::Event)>> =
    OnceLock::new();
static DIAGNOSTICS: Mutex<RendererSnapshot> = Mutex::new(RendererSnapshot {
    renderer: None,
    graphics_failed: false,
    gpui_windows: 0,
    windows_opened: 0,
    software: None,
    initial_window_ms: None,
    started: None,
});

#[derive(Clone, Copy)]
struct RendererSnapshot {
    renderer: Option<host::GpuOutcome>,
    graphics_failed: bool,
    gpui_windows: usize,
    windows_opened: u64,
    software: Option<bool>,
    initial_window_ms: Option<u128>,
    started: Option<Instant>,
}

#[derive(Default)]
struct ActivityWindows(HashMap<host::HostId, AnyWindowHandle>);

impl Global for ActivityWindows {}

fn init_logging() {
    INITIALIZE_LOGS.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Warn)
                // Dependency diagnostics can include private Source metadata.
                .with_filter(
                    android_logger::FilterBuilder::new()
                        .filter_level(log::LevelFilter::Off)
                        .filter_module("sparrow_shield", log::LevelFilter::Warn)
                        .build(),
                )
                .with_tag("SparrowShield"),
        );
        // An unwind would kill the detached GPUI thread and leave its Activity alive.
        // Panic payloads can include transport context. Only log the Rust location.
        std::panic::set_hook(Box::new(|panic| {
            if sparrow_channel_icons::is_contained_icon_decode_panic() {
                return;
            }
            if let Some(location) = panic.location() {
                let file = Path::new(location.file())
                    .file_name()
                    .and_then(|file| file.to_str())
                    .unwrap_or("Rust source");
                log::error!("fatal Rust error at {file}:{}", location.line());
            } else {
                log::error!("fatal Rust error");
            }
            std::process::abort();
        }));
    });
}

fn start_renderer() {
    START_RENDERER.get_or_init(|| {
        let (closed, mut destroyed) = mpsc::unbounded_channel();
        let (activity_events, mut activity_updates) = mpsc::unbounded_channel();
        let _ = ACTIVITY_EVENTS.set(activity_events);
        DIAGNOSTICS.lock().unwrap().started = Some(Instant::now());
        host::on_gpu_outcome(|outcome| {
            let mut diagnostics = DIAGNOSTICS.lock().unwrap();
            match outcome {
                host::GpuOutcome::Failed => diagnostics.graphics_failed = true,
                backend => diagnostics.renderer = Some(backend),
            }
        });
        host::on_open_window(|id, cx| {
            if let Some(window) = guide_surface::open(id, cx) {
                cx.global_mut::<ActivityWindows>().0.insert(id, window);
                activity::enable_key_routing(id);
                let mut diagnostics = DIAGNOSTICS.lock().unwrap();
                diagnostics.graphics_failed = false;
                diagnostics.gpui_windows = cx.windows().len();
                diagnostics.windows_opened += 1;
                if let Some(started) = diagnostics.started.take() {
                    diagnostics.initial_window_ms = Some(started.elapsed().as_millis());
                }
            }
        });
        host::start(move |cx| {
            // Android can destroy the last Activity and relaunch in this process.
            cx.set_quit_mode(QuitMode::Explicit);
            guide_fonts::register(cx);
            cx.set_global(ActivityWindows::default());
            cx.spawn(async move |cx| {
                while let Some(id) = destroyed.recv().await {
                    cx.update(|cx| {
                        // The Android backend removes its window on HostDestroyed.
                        // Separately close GPUI's window so it releases the root view.
                        if let Some(window) = cx.global_mut::<ActivityWindows>().0.remove(&id) {
                            let _ = window.update(cx, |_, window, _| window.remove_window());
                            DIAGNOSTICS.lock().unwrap().gpui_windows = cx.windows().len();
                        }
                    });
                }
            })
            .detach();
            cx.spawn(async move |cx| {
                while let Some((id, event)) = activity_updates.recv().await {
                    cx.update(|cx| {
                        let window = cx
                            .global::<ActivityWindows>()
                            .0
                            .get(&id)
                            .and_then(AnyWindowHandle::downcast::<guide_surface::GuideSurface>);
                        if let Some(window) = window {
                            let _ = window.update(cx, |view, window, cx| {
                                view.activity_event(event, window, cx);
                            });
                        }
                    });
                }
            })
            .detach();
        });
        closed
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeStart<'local>(
    mut env: EnvUnowned<'local>,
    activity: JObject<'local>,
    application_context: JObject<'local>,
    data_dir: JString<'local>,
) {
    init_logging();
    let data_root = env
        .with_env_no_catch(|env| -> Result<_, Error> {
            let result = (|| {
                mobile_jni::set_host_activity(env, &activity).ok()?;
                rustls_platform_verifier::android::init_with_env(env, application_context).ok()?;
                let data_root = PathBuf::from(data_dir.try_to_string(env).ok()?);
                data_root.is_absolute().then_some(data_root)
            })();
            if result.is_none() && env.exception_check() {
                env.exception_clear();
            }
            Ok(result)
        })
        .resolve::<LogErrorAndDefault>();
    if let Some(data_root) = data_root {
        catalog::start(data_root);
    } else {
        catalog::set_start_failed();
    }
    start_renderer();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeCatalogActive(
    _: EnvUnowned,
    _: JObject,
    id: i64,
    active: jboolean,
) {
    catalog::set_active(id as u64, active);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeRegisterHost<'local>(
    mut env: EnvUnowned<'local>,
    owner: JObject<'local>,
    id: i64,
) {
    env.with_env_no_catch(|env| activity::register(env, id as u64, &owner))
        .resolve::<LogErrorAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeSurface<'local>(
    env: EnvUnowned<'local>,
    _: JObject<'local>,
    id: i64,
    surface: JObject<'local>,
    density: f32,
) {
    // SAFETY: Java owns the live Surface reference during this call. NativeWindow
    // acquires its own Android reference before passing it to the renderer.
    if let Some(window) =
        unsafe { NativeWindow::from_surface(env.as_raw() as _, surface.as_raw() as _) }
    {
        host::surface_created(id as u64, window, density);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeReleaseSurface(
    _: EnvUnowned,
    _: JObject,
    id: i64,
) {
    host::surface_destroyed(id as u64);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeResume(
    _: EnvUnowned,
    _: JObject,
    id: i64,
) {
    host::resumed(id as u64);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativePause(
    _: EnvUnowned,
    _: JObject,
    id: i64,
) {
    host::paused(id as u64);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeDestroy(
    _: EnvUnowned,
    _: JObject,
    id: i64,
    finishing: jboolean,
) {
    let id = id as host::HostId;
    catalog::host_destroyed(id, finishing);
    activity::unregister(id);
    host::host_destroyed(id);
    if let Some(closed) = START_RENDERER.get() {
        let _ = closed.send(id);
    }
}

fn enqueue_activity(id: i64, event: activity::Event) {
    if let Some(events) = ACTIVITY_EVENTS.get() {
        let _ = events.send((id as u64, event));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeBack(
    _: EnvUnowned,
    _: JObject,
    id: i64,
) {
    enqueue_activity(id, activity::Event::Back);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeHostKey(
    _: EnvUnowned,
    _: JObject,
    id: i64,
    code: i32,
    action: i32,
    modifiers: i32,
    repeat: i32,
) {
    const ALT_CTRL_META: i32 = 0x02 | 0x1000 | 0x10000;
    let guide_shortcut = matches!(code, 19..=22) && modifiers & ALT_CTRL_META != 0;
    if action == 0 && !guide_shortcut {
        enqueue_activity(id, activity::Event::Key { code, repeat });
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeEditorEvent<'local>(
    mut env: EnvUnowned<'local>,
    _: JObject<'local>,
    id: i64,
    generation: i64,
    event: i32,
    text: JString<'local>,
) {
    env.with_env_no_catch(|env| -> Result<(), Error> {
        let edit = match event {
            0 => activity::Edit::Changed(text.try_to_string(env)?),
            1 => activity::Edit::Committed(text.try_to_string(env)?),
            2 => activity::Edit::Cancelled,
            3 => activity::Edit::Rejected,
            _ => return Ok(()),
        };
        enqueue_activity(
            id,
            activity::Event::Edited {
                generation: generation as u64,
                edit,
            },
        );
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeSourceDraftStatus(
    _: EnvUnowned,
    _: JObject,
    id: i64,
    session: i64,
    attempt: i64,
    status: i32,
) {
    enqueue_activity(
        id,
        activity::Event::Source {
            session: session as u64,
            update: activity::SourceEvent::DraftStatus {
                attempt: attempt as u64,
                status,
            },
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeSourceClipboardResult(
    _: EnvUnowned,
    _: JObject,
    id: i64,
    session: i64,
    attempt: i64,
    copied: jboolean,
) {
    enqueue_activity(
        id,
        activity::Event::Source {
            session: session as u64,
            update: activity::SourceEvent::Copied {
                attempt: attempt as u64,
                copied,
            },
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativePlaybackClipboardResult(
    _: EnvUnowned,
    _: JObject,
    id: i64,
    attempt: i64,
    copied: jboolean,
) {
    enqueue_activity(
        id,
        activity::Event::PlaybackDiagnosticsCopied {
            attempt: attempt as u64,
            copied,
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeSourceFieldEvent(
    _: EnvUnowned,
    _: JObject,
    id: i64,
    session: i64,
    generation: i64,
    field: i32,
    event: i32,
    has_text: jboolean,
) {
    enqueue_activity(
        id,
        activity::Event::Source {
            session: session as u64,
            update: activity::SourceEvent::Field {
                generation: generation as u64,
                field,
                event,
                has_text,
            },
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeSaveSourceDraft<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _: JObject<'local>,
    id: i64,
    session: i64,
    attempt: i64,
    m3u: JString<'local>,
    epg: JString<'local>,
) {
    env.with_env_no_catch(|env| -> Result<(), Error> {
        let m3u = m3u.try_to_string(env)?;
        let epg = if epg.is_null() {
            None
        } else {
            Some(epg.try_to_string(env)?)
        };
        enqueue_activity(
            id,
            activity::Event::Source {
                session: session as u64,
                update: activity::SourceEvent::Submit {
                    attempt: attempt as u64,
                    m3u,
                    epg,
                },
            },
        );
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeKey(
    _: EnvUnowned,
    _: JObject,
    code: i32,
    action: i32,
    modifiers: i32,
) {
    host::key(code, action, modifiers);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldActivity_nativeSnapshot(
    mut env: EnvUnowned,
    _: JObject,
) -> jstring {
    let renderer = *DIAGNOSTICS.lock().unwrap();
    let (catalog, subscriptions) = catalog::diagnostics();
    let guide = *guide_surface::GUIDE_DIAGNOSTICS.lock().unwrap();
    let workflows = *guide_surface::WORKFLOW_DIAGNOSTICS.lock().unwrap();
    let (playback, playback_channel, fullscreen, decoded_frames) = playback::diagnostics();
    let snapshot = format!(
        "sparrow-shield-v3 phase={:?} generation_changes={} active={} refreshing={} https={:?} seed={:?} renderer={:?} graphics_failed={} software={:?} initial_window_ms={:?} gpui_windows={} windows_opened={} catalog_subscriptions={} mode={:?} focus={} channel={:?} group_index={:?} chip_index={:?} programme_index={:?} pan_minutes={:?} first_row={} scroll_offset_units={} loaded_families={} page_status={:?} search={:?} channel_results={} programme_results={} hidden_groups={} groups_open={} sources_open={} editor_open={} playback={} playback_channel={:?} fullscreen={} decoded_frames={}",
        catalog.phase,
        catalog.generation_changes,
        catalog.active,
        catalog.refreshing,
        catalog.https,
        catalog.seed,
        renderer.renderer,
        renderer.graphics_failed,
        renderer.software,
        renderer.initial_window_ms,
        renderer.gpui_windows,
        renderer.windows_opened,
        subscriptions,
        guide.mode,
        guide.focus,
        guide.channel,
        guide.group_index,
        guide.chip_index,
        guide.programme_index,
        guide.pan_minutes,
        guide.first_row,
        guide.scroll_offset_units,
        guide.loaded_families,
        guide.page_status,
        workflows.search,
        workflows.channel_results,
        workflows.programme_results,
        workflows.hidden_groups,
        workflows.groups_open,
        workflows.sources_open,
        workflows.editor_open,
        playback,
        playback_channel,
        fullscreen,
        decoded_frames,
    );
    env.with_env_no_catch(|env| Ok::<_, Error>(JString::from_str(env, snapshot)?.into_raw()))
        .resolve::<LogErrorAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_FixtureReceiver_nativeCommand(
    _: EnvUnowned,
    _: JObject,
    command: i32,
) -> jboolean {
    #[cfg(feature = "fixture")]
    return catalog::fixture_command(command);
    #[cfg(not(feature = "fixture"))]
    {
        let _ = command;
        false
    }
}

// The upstream android-activity crate references this symbol. Sparrow launches
// through its plain Activity and provides a separate JNI entry point above.
#[unsafe(no_mangle)]
fn android_main(_: android_activity::AndroidApp) {
    std::process::abort();
}
