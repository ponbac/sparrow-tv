use std::sync::Arc;

mod bounded_blocking;
mod config_store;
mod instance_lock;
mod ipc;
mod playback;
mod runtime;

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
            let runtime = Arc::new(tauri::async_runtime::block_on(
                runtime::InstalledRuntime::open(app_data),
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
            ipc::playback_stop,
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
    use jni::objects::JObject;
    use tauri::tao::platform::android::prelude::main_android_context;

    let context = main_android_context().ok_or(())?;
    // SAFETY: Tao created and retains both process-lifetime JNI handles before
    // invoking the Tauri mobile entry point. This function copies the activity
    // reference into rustls-platform-verifier's own global reference exactly once.
    let java_vm = unsafe { jni::JavaVM::from_raw(context.java_vm.cast()) };
    java_vm
        .attach_current_thread(|environment| {
            // SAFETY: `context_jobject` is Tao's live global Activity reference.
            // `JObject` is a transparent, non-dropping view used only during this
            // attached local frame; the verifier immediately creates a global copy.
            let activity =
                unsafe { JObject::from_raw(environment, context.context_jobject.cast()) };
            rustls_platform_verifier::android::init_with_env(environment, activity)
        })
        .map_err(|_| ())
}

fn report_lifecycle(app: &tauri::AppHandle, event: tauri::RunEvent) {
    use sparrow_core::LifecycleSignal;
    use tauri::Manager as _;

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
        runtime.core().report_lifecycle(signal);
    }
}
