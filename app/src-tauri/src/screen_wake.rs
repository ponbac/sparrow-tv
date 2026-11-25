use std::sync::Arc;

use sparrow_installed_playback::ScreenWake;
#[cfg(target_os = "android")]
use sparrow_installed_playback::ScreenWakeError;
#[cfg(not(target_os = "android"))]
use sparrow_installed_playback::noop_screen_wake;

#[cfg(target_os = "android")]
pub(crate) fn platform_screen_wake(_app: tauri::AppHandle) -> Arc<dyn ScreenWake> {
    Arc::new(AndroidScreenWake)
}

#[cfg(not(target_os = "android"))]
pub(crate) fn platform_screen_wake(_app: tauri::AppHandle) -> Arc<dyn ScreenWake> {
    noop_screen_wake()
}

#[cfg(target_os = "android")]
struct AndroidScreenWake;

#[cfg(target_os = "android")]
impl ScreenWake for AndroidScreenWake {
    fn set_active(&self, active: bool) -> Result<(), ScreenWakeError> {
        set_android_keep_screen_on(active)
    }
}

#[cfg(target_os = "android")]
fn set_android_keep_screen_on(active: bool) -> Result<(), ScreenWakeError> {
    use jni::{
        jni_sig, jni_str,
        objects::{Global, JObject, JValue},
    };
    use tauri::tao::platform::android::prelude::main_android_context;

    let context = main_android_context().ok_or(ScreenWakeError)?;
    // SAFETY: Tao owns this process-lifetime JavaVM handle. `from_raw` creates
    // a non-owning Rust view and the closure does not outlive the attachment.
    let java_vm = unsafe { jni::JavaVM::from_raw(context.java_vm.cast()) };
    let applied = java_vm
        .attach_current_thread(|environment| -> jni::errors::Result<bool> {
            let activity_raw = context.context_jobject.cast();
            // SAFETY: Tao retains this valid global Activity reference for the
            // process lifetime. `as_cast_raw` borrows it without taking ownership.
            let activity =
                unsafe { environment.as_cast_raw::<Global<JObject<'static>>>(&activity_raw)? };
            environment
                .call_method(
                    activity.as_ref(),
                    jni_str!("setPlaybackKeepScreenOn"),
                    jni_sig!("(Z)Z"),
                    &[JValue::Bool(active)],
                )?
                .z()
        })
        .map_err(|_| ScreenWakeError)?;
    applied.then_some(()).ok_or(ScreenWakeError)
}
