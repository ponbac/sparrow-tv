use std::sync::Arc;

mod agent_control;
mod android_playback;
mod audio_preferences;
mod config_store;
mod instance_lock;
mod ipc;
mod playback;
#[cfg(all(target_os = "linux", feature = "linux-playback-lab"))]
mod playback_lab;
mod runtime;
mod screen_wake;
mod selected_transport_stream;

/// Runs the installed Sparrow shell.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    configure_linux_gstreamer_helpers();
    #[cfg(target_os = "linux")]
    configure_linux_webkit_renderer();
    let builder = tauri::Builder::default();
    #[cfg(all(target_os = "linux", feature = "linux-playback-lab"))]
    let builder = builder.on_page_load(playback_lab::on_page_load);
    let application = match builder
        .manage(runtime::InstalledRuntimeSlot::new())
        .manage(agent_control::ReplyMailbox::default())
        .setup(|app| {
            use tauri::Manager as _;

            #[cfg(target_os = "android")]
            initialize_android_certificate_verifier()
                .map_err(|_| runtime::InstalledStartupError::SourceAdapter)?;
            let app_data = app
                .path()
                .app_data_dir()
                .map_err(|_| runtime::InstalledStartupError::AppData)?;
            let screen_wake = screen_wake::platform_screen_wake(app.handle().clone());
            let runtime = Arc::new(tauri::async_runtime::block_on(
                runtime::InstalledRuntime::open_with_screen_wake(app_data, screen_wake),
            )?);
            #[cfg(target_os = "android")]
            android_playback::bind_runtime(Arc::downgrade(&runtime))
                .map_err(|_| runtime::InstalledStartupError::PlaybackAdapter)?;
            app.state::<runtime::InstalledRuntimeSlot>()
                .fill(runtime)
                .map_err(|_| runtime::InstalledStartupError::Core)?;
            agent_control::start(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::installed_capabilities,
            ipc::catalog_status,
            ipc::catalog_list_groups,
            ipc::catalog_list_channels,
            ipc::catalog_channel,
            ipc::catalog_schedule,
            ipc::catalog_guide_window,
            ipc::catalog_search,
            ipc::catalog_search_channels,
            ipc::catalog_search_programmes,
            ipc::catalog_search_cancel,
            ipc::catalog_refresh,
            ipc::source_configuration_replace,
            ipc::catalog_subscribe,
            ipc::catalog_unsubscribe,
            ipc::playback_start,
            ipc::playback_read,
            ipc::playback_suspend,
            ipc::playback_activity,
            ipc::playback_reopen,
            ipc::playback_restart,
            ipc::playback_stop,
            ipc::playback_android_start,
            ipc::playback_android_status,
            ipc::playback_android_controls,
            ipc::playback_android_viewport,
            ipc::playback_android_stop,
            ipc::playback_mpv_control,
            agent_control::agent_control_reply,
        ])
        .build(tauri::generate_context!())
    {
        Ok(application) => application,
        Err(_) => panic!("the installed Sparrow application could not start"),
    };

    application.run(report_lifecycle);
}

#[cfg(target_os = "linux")]
const WEBKIT_DISABLE_DMABUF_RENDERER: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
#[cfg(target_os = "linux")]
const WEBKIT_DMABUF_RENDERER_FORCE_SHM: &str = "WEBKIT_DMABUF_RENDERER_FORCE_SHM";
#[cfg(target_os = "linux")]
const WEBKIT_GST_DISABLE_GL_SINK: &str = "WEBKIT_GST_DISABLE_GL_SINK";

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinuxWebKitRendererPolicy {
    ForceSharedMemory,
    DisableDmaBuf,
    PreserveExplicit,
}

#[cfg(target_os = "linux")]
fn configure_linux_gstreamer_helpers() {
    use std::path::{Path, PathBuf};

    let plugin_dirs = env_path_list("GST_PLUGIN_PATH_1_0")
        .into_iter()
        .chain(env_path_list("GST_PLUGIN_PATH"))
        .collect::<Vec<_>>();
    let appdir = std::env::var_os("APPDIR").map(PathBuf::from);
    let scanner = resolve_gstreamer_helper(
        std::env::var_os("GST_PLUGIN_SCANNER_1_0")
            .or_else(|| std::env::var_os("GST_PLUGIN_SCANNER"))
            .as_deref()
            .map(Path::new),
        &plugin_dirs,
        appdir.as_deref(),
        "gst-plugin-scanner",
    );
    let ptp = resolve_gstreamer_helper(
        std::env::var_os("GST_PTP_HELPER_1_0")
            .as_deref()
            .map(Path::new),
        &plugin_dirs,
        appdir.as_deref(),
        "gst-ptp-helper",
    );
    // SAFETY: runs before Tauri or WebKit spawn helper processes.
    unsafe {
        if let Some(path) = scanner.as_ref() {
            std::env::set_var("GST_PLUGIN_SCANNER_1_0", path);
            std::env::set_var("GST_PLUGIN_SCANNER", path);
        }
        if let Some(path) = ptp.as_ref() {
            std::env::set_var("GST_PTP_HELPER_1_0", path);
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_nvidia_h264_feature_ranks(
    nvidia_present: bool,
    already_configured: bool,
) -> Option<&'static str> {
    (nvidia_present && !already_configured).then_some(
        "vah264dec:NONE,vaapih264dec:NONE,nvh264dec:NONE,nvh264sldec:NONE,nvv4l2decoder:NONE",
    )
}

#[cfg(target_os = "linux")]
fn env_path_list(key: &str) -> Vec<std::path::PathBuf> {
    std::env::var(key)
        .unwrap_or_default()
        .split(':')
        .filter(|part| !part.is_empty())
        .map(std::path::PathBuf::from)
        .collect()
}

/// Returns a replacement helper path when `current` is missing. `None` keeps env.
#[cfg(target_os = "linux")]
fn resolve_gstreamer_helper(
    current: Option<&std::path::Path>,
    plugin_dirs: &[std::path::PathBuf],
    appdir: Option<&std::path::Path>,
    filename: &str,
) -> Option<std::path::PathBuf> {
    if current.is_some_and(std::path::Path::is_file) {
        return None;
    }
    let mut candidates = Vec::new();
    for dir in plugin_dirs {
        candidates.push(dir.join(filename));
    }
    if let Some(appdir) = appdir {
        candidates.push(appdir.join("usr/lib/gstreamer-1.0").join(filename));
        candidates.push(
            appdir
                .join("usr/lib/gstreamer1.0/gstreamer-1.0")
                .join(filename),
        );
        candidates.push(appdir.join("usr/libexec/gstreamer-1.0").join(filename));
    }
    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(target_os = "linux")]
fn configure_linux_webkit_renderer() {
    let backend = std::env::var("GDK_BACKEND").ok();
    let policy = linux_webkit_renderer_policy(
        backend.as_deref(),
        std::env::var_os(WEBKIT_DISABLE_DMABUF_RENDERER).is_some(),
        std::env::var_os(WEBKIT_DMABUF_RENDERER_FORCE_SHM).is_some(),
    );

    let nvidia_present = linux_nvidia_proprietary_present();
    let disable_gl_sink = linux_should_disable_gst_gl_sink(
        nvidia_present,
        std::env::var_os(WEBKIT_GST_DISABLE_GL_SINK).is_some(),
    );
    let decoder_ranks = linux_nvidia_h264_feature_ranks(
        nvidia_present,
        std::env::var_os("GST_PLUGIN_FEATURE_RANK").is_some(),
    );

    // SAFETY: this runs before Tauri, WebKit, or any application thread is created.
    unsafe {
        match policy {
            LinuxWebKitRendererPolicy::ForceSharedMemory => {
                std::env::set_var(WEBKIT_DMABUF_RENDERER_FORCE_SHM, "1");
            }
            LinuxWebKitRendererPolicy::DisableDmaBuf => {
                std::env::set_var(WEBKIT_DISABLE_DMABUF_RENDERER, "1");
            }
            LinuxWebKitRendererPolicy::PreserveExplicit => {}
        }
        if disable_gl_sink {
            std::env::set_var(WEBKIT_GST_DISABLE_GL_SINK, "1");
        }
        // NVIDIA's VA/NVDEC buffers cannot be uploaded reliably by WebKitGTK.
        // Preserve hardware decoding on other GPUs and explicit operator ranks.
        if let Some(ranks) = decoder_ranks {
            std::env::set_var("GST_PLUGIN_FEATURE_RANK", ranks);
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_webkit_renderer_policy(
    gdk_backend: Option<&str>,
    disable_is_configured: bool,
    force_shm_is_configured: bool,
) -> LinuxWebKitRendererPolicy {
    if disable_is_configured || force_shm_is_configured {
        return LinuxWebKitRendererPolicy::PreserveExplicit;
    }

    let primary_backend = gdk_backend.and_then(|backends| {
        backends
            .split(',')
            .map(str::trim)
            .find(|value| !value.is_empty())
    });
    if primary_backend.is_some_and(|backend| backend.eq_ignore_ascii_case("x11")) {
        LinuxWebKitRendererPolicy::ForceSharedMemory
    } else {
        LinuxWebKitRendererPolicy::DisableDmaBuf
    }
}

#[cfg(target_os = "linux")]
fn linux_nvidia_proprietary_present() -> bool {
    std::path::Path::new("/dev/nvidia0").exists()
        || std::path::Path::new("/proc/driver/nvidia/version").is_file()
}

#[cfg(target_os = "linux")]
fn linux_should_disable_gst_gl_sink(nvidia_present: bool, already_configured: bool) -> bool {
    nvidia_present && !already_configured
}

#[cfg(target_os = "android")]
fn initialize_android_certificate_verifier() -> Result<(), ()> {
    use jni::objects::{Global, JObject};
    use tauri::tao::platform::android::prelude::main_android_context;

    let context = main_android_context().ok_or(())?;
    // SAFETY: Tao created and retains both process-lifetime JNI handles before
    // invoking the Tauri mobile entry point. This function copies the activity
    // reference into rustls-platform-verifier's own global reference exactly once.
    let java_vm = unsafe { jni::JavaVM::from_raw(context.java_vm.cast()) };
    java_vm
        .attach_current_thread(|environment| {
            let activity_raw = context.context_jobject.cast();
            // SAFETY: Tao owns this valid global Activity reference for the
            // process lifetime. The cast borrows it without taking ownership.
            let activity =
                unsafe { environment.as_cast_raw::<Global<JObject<'static>>>(&activity_raw)? };
            let activity = environment.new_local_ref(activity.as_ref())?;
            rustls_platform_verifier::android::init_with_env(environment, activity)
        })
        .map_err(|_| ())
}

fn report_lifecycle(app: &tauri::AppHandle, event: tauri::RunEvent) {
    use sparrow_core::LifecycleSignal;
    use tauri::{Emitter as _, Manager as _};

    if matches!(&event, tauri::RunEvent::Exit) {
        if let Some(runtime) = app
            .try_state::<runtime::InstalledRuntimeSlot>()
            .and_then(|slot| slot.ready())
        {
            let _ = tauri::async_runtime::block_on(runtime.shutdown_playback());
        }
        return;
    }

    let signal = match event {
        tauri::RunEvent::Ready => Some(LifecycleSignal::Started),
        tauri::RunEvent::Resumed => Some(LifecycleSignal::Resumed),
        #[cfg(mobile)]
        tauri::RunEvent::WindowEvent {
            event: tauri::WindowEvent::Suspended,
            ..
        } => Some(LifecycleSignal::Suspended),
        #[cfg(mobile)]
        tauri::RunEvent::WindowEvent {
            event: tauri::WindowEvent::Resumed,
            ..
        } => Some(LifecycleSignal::Resumed),
        _ => None,
    };
    if let (Some(signal), Some(runtime)) = (
        signal,
        app.try_state::<runtime::InstalledRuntimeSlot>()
            .and_then(|slot| slot.ready()),
    ) {
        let app = app.clone();
        let dispatch = runtime.dispatch_lifecycle(signal, move |event| {
            let _ = app.emit("sparrow://playback-lifecycle", event);
        });
        tauri::async_runtime::spawn(async move {
            let _ = dispatch.await;
        });
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn selects_accelerated_shared_memory_for_the_packaged_x11_backend() {
        assert_eq!(
            linux_webkit_renderer_policy(Some("x11"), false, false),
            LinuxWebKitRendererPolicy::ForceSharedMemory,
        );
    }

    #[test]
    fn retains_the_compatibility_renderer_for_native_wayland() {
        assert_eq!(
            linux_webkit_renderer_policy(Some("wayland"), false, false),
            LinuxWebKitRendererPolicy::DisableDmaBuf,
        );
    }

    #[test]
    fn preserves_explicit_renderer_configuration() {
        for (disable_is_configured, force_shm_is_configured) in
            [(true, false), (false, true), (true, true)]
        {
            assert_eq!(
                linux_webkit_renderer_policy(
                    Some("x11"),
                    disable_is_configured,
                    force_shm_is_configured,
                ),
                LinuxWebKitRendererPolicy::PreserveExplicit,
            );
        }
    }

    #[test]
    fn keeps_an_existing_gstreamer_helper_path() {
        let dir = std::env::temp_dir().join("sparrow-gst-helper-keep");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let helper = dir.join("gst-plugin-scanner");
        std::fs::write(&helper, []).expect("helper");
        assert_eq!(
            resolve_gstreamer_helper(Some(&helper), &[], None, "gst-plugin-scanner"),
            None,
        );
    }

    #[test]
    fn repairs_a_missing_gstreamer_helper_from_the_plugin_dir() {
        let dir = std::env::temp_dir().join("sparrow-gst-helper-repair");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let helper = dir.join("gst-plugin-scanner");
        std::fs::write(&helper, []).expect("helper");
        let missing = dir.join("missing-scanner");
        assert_eq!(
            resolve_gstreamer_helper(
                Some(&missing),
                std::slice::from_ref(&dir),
                None,
                "gst-plugin-scanner",
            ),
            Some(helper),
        );
    }

    #[test]
    fn nvidia_h264_ranks_preserve_other_gpus_and_explicit_configuration() {
        let ranks = linux_nvidia_h264_feature_ranks(true, false).expect("NVIDIA workaround");
        assert!(ranks.contains("vah264dec:NONE"));
        assert!(ranks.contains("vaapih264dec:NONE"));
        assert!(ranks.contains("nvh264dec:NONE"));
        assert_eq!(linux_nvidia_h264_feature_ranks(false, false), None);
        assert_eq!(linux_nvidia_h264_feature_ranks(false, true), None);
        assert_eq!(linux_nvidia_h264_feature_ranks(true, true), None);
    }

    #[test]
    fn disables_webkit_gl_video_sink_only_for_unconfigured_nvidia() {
        assert!(linux_should_disable_gst_gl_sink(true, false));
        assert!(!linux_should_disable_gst_gl_sink(false, false));
        assert!(!linux_should_disable_gst_gl_sink(true, true));
    }
}
