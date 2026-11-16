//! The Android Activity's own word on whether the picture can be seen.
//!
//! On Android a Playback Session lives while the Activity is started, not
//! while it is resumed: a picture-in-picture window, split screen or a dialog
//! over the app all pause the Activity and leave the picture on screen. tao's
//! Suspended and Resumed follow the process-wide pause and resume instead, so
//! `MainActivity` reports `onStart` and `onStop` here and they replace them.

#[cfg(target_os = "android")]
mod platform {
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        sync::OnceLock,
    };

    use jni::{EnvUnowned, objects::JObject, sys::jboolean};
    use sparrow_core::LifecycleSignal;

    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

    /// Lets later Activity reports reach the installed runtime.
    pub(crate) fn bind(app: tauri::AppHandle) {
        let _ = APP.set(app);
    }

    /// Called on the main thread from `MainActivity.onStart` and `onStop`.
    /// The runtime orders lifecycle signals by the time of this call.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_xyz_ponbac_sparrow_MainActivity_reportPlaybackVisibility<
        'caller,
    >(
        _environment: EnvUnowned<'caller>,
        _activity: JObject<'caller>,
        visible: jboolean,
    ) {
        let Some(app) = APP.get() else {
            return;
        };
        let signal = if visible {
            LifecycleSignal::Resumed
        } else {
            LifecycleSignal::Suspended
        };
        // An unwind must not cross into the JVM.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            crate::dispatch_lifecycle_signal(app, signal);
        }));
    }
}

#[cfg(target_os = "android")]
pub(crate) use platform::bind;
