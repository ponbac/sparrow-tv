use std::sync::Arc;

mod audio_preferences;
mod bounded_blocking;
mod config_store;
mod instance_lock;
mod ipc;
mod playback;
mod runtime;
mod screen_wake;
mod selected_transport_stream;

/// Runs the installed Sparrow shell.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    configure_platform_before_webview();
    let application = match tauri::Builder::default()
        .manage(runtime::InstalledRuntimeSlot::new())
        .setup(|app| {
            use tauri::Manager as _;

            #[cfg(target_os = "android")]
            initialize_android_certificate_verifier()
                .map_err(|_| runtime::InstalledStartupError::SourceAdapter)?;
            #[cfg(target_os = "linux")]
            enable_linux_media_source(app)?;
            let app_data = app
                .path()
                .app_data_dir()
                .map_err(|_| runtime::InstalledStartupError::AppData)?;
            let screen_wake = screen_wake::platform_screen_wake(app.handle().clone());
            let runtime = Arc::new(tauri::async_runtime::block_on(
                runtime::InstalledRuntime::open_with_screen_wake(app_data, screen_wake),
            )?);
            app.state::<runtime::InstalledRuntimeSlot>()
                .fill(runtime)
                .map_err(|_| runtime::InstalledStartupError::Core)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::installed_capabilities,
            ipc::catalog_status,
            ipc::catalog_list_groups,
            ipc::catalog_list_channels,
            ipc::catalog_channel,
            ipc::catalog_schedule,
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
            ipc::playback_mpv_start,
            ipc::playback_mpv_stop,
        ])
        .build(tauri::generate_context!())
    {
        Ok(application) => application,
        Err(_) => panic!("the installed Sparrow application could not start"),
    };

    application.run(report_lifecycle);
}

fn configure_platform_before_webview() {
    #[cfg(target_os = "linux")]
    // SAFETY: this runs before Tauri, WebKit, or any application thread is created.
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}

#[cfg(target_os = "linux")]
fn enable_linux_media_source(app: &tauri::App) -> Result<(), runtime::InstalledStartupError> {
    use tauri::Manager as _;

    let webview = app
        .get_webview_window("main")
        .ok_or(runtime::InstalledStartupError::PlaybackAdapter)?;
    webview
        .with_webview(|platform_webview| {
            use webkit2gtk::{SettingsExt as _, WebViewExt as _};

            if let Some(settings) = platform_webview.inner().settings() {
                settings.set_enable_mediasource(true);
            }
        })
        .map_err(|_| runtime::InstalledStartupError::PlaybackAdapter)
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
        tauri::async_runtime::spawn(async move {
            if let Ok(Some(event)) = runtime.report_lifecycle(signal).await {
                let _ = app.emit("sparrow://playback-lifecycle", event);
            }
        });
    }
}
