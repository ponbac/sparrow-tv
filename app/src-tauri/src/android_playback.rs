use sparrow_installed_playback::{
    PlaybackSessionId,
    android::{
        AndroidPlaybackControls, AndroidPlaybackError, AndroidPlaybackIdentity,
        AndroidPlaybackStatus, AndroidPlaybackViewport,
    },
};

/// Tauri's Java adapter can wait for the Android main thread. Keep that wait
/// on a blocking worker while the shared gate awaits the result.
pub(crate) async fn blocking<Output: Send + 'static>(
    operation: impl FnOnce() -> Result<Output, AndroidPlaybackError> + Send + 'static,
) -> Result<Output, AndroidPlaybackError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| AndroidPlaybackError)?
}

#[cfg(target_os = "android")]
mod platform {
    use std::sync::{Arc, OnceLock, Weak};

    use jni::{
        Env, EnvUnowned, JValue, jni_sig, jni_str,
        objects::{Global, JByteArray, JObject, JString},
        sys::{jint, jsize},
    };
    use sparrow_installed_playback::{NativeStreamHandle, PlaybackSessionId};
    use tauri::tao::platform::android::prelude::main_android_context;

    use sparrow_installed_playback::android::AndroidMedia3Reader;

    use super::{
        AndroidPlaybackControls, AndroidPlaybackError, AndroidPlaybackIdentity,
        AndroidPlaybackStatus, AndroidPlaybackViewport,
    };
    use crate::runtime::InstalledRuntime;

    const NATIVE_READ_FAILED: jint = -2;
    static READER: OnceLock<AndroidMedia3Reader> = OnceLock::new();

    pub(crate) fn bind_runtime(
        runtime: Weak<InstalledRuntime>,
    ) -> Result<(), AndroidPlaybackError> {
        let runtime = runtime.upgrade().ok_or(AndroidPlaybackError)?;
        READER
            .set(AndroidMedia3Reader::new(
                tauri::async_runtime::handle().inner().clone(),
                Arc::downgrade(runtime.playback_manager()),
            ))
            .map_err(|_| AndroidPlaybackError)
    }

    pub(super) fn start(
        identity: &AndroidPlaybackIdentity,
        viewport: AndroidPlaybackViewport,
        controls: AndroidPlaybackControls,
    ) -> Result<(), AndroidPlaybackError> {
        with_activity(|environment, activity| {
            let session_id = environment.new_string(identity.session_id().as_str())?;
            let stream_handle = environment.new_string(identity.stream_handle().as_str())?;
            environment
                .call_method(
                    activity,
                    jni_str!("startNativePlayback"),
                    jni_sig!("(Ljava/lang/String;Ljava/lang/String;IIIIFZZ)Z"),
                    &[
                        JValue::Object(&session_id),
                        JValue::Object(&stream_handle),
                        JValue::Int(viewport.left() as jint),
                        JValue::Int(viewport.top() as jint),
                        JValue::Int(viewport.width() as jint),
                        JValue::Int(viewport.height() as jint),
                        JValue::Float(controls.volume()),
                        JValue::Bool(controls.muted()),
                        JValue::Bool(viewport.fullscreen()),
                    ],
                )?
                .z()
        })?
        .then_some(())
        .ok_or(AndroidPlaybackError)
    }

    pub(super) fn status(
        identity: &AndroidPlaybackIdentity,
    ) -> Result<AndroidPlaybackStatus, AndroidPlaybackError> {
        let wire = with_activity(|environment, activity| {
            let session_id = environment.new_string(identity.session_id().as_str())?;
            let stream_handle = environment.new_string(identity.stream_handle().as_str())?;
            let value = environment
                .call_method(
                    activity,
                    jni_str!("nativePlaybackStatus"),
                    jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"),
                    &[JValue::Object(&session_id), JValue::Object(&stream_handle)],
                )?
                .l()?;
            environment
                .cast_local::<JString>(value)?
                .try_to_string(environment)
        })?;
        AndroidPlaybackStatus::parse_json(&wire)
    }

    pub(super) fn set_controls(
        identity: &AndroidPlaybackIdentity,
        controls: AndroidPlaybackControls,
    ) -> Result<(), AndroidPlaybackError> {
        with_activity(|environment, activity| {
            let session_id = environment.new_string(identity.session_id().as_str())?;
            let stream_handle = environment.new_string(identity.stream_handle().as_str())?;
            environment
                .call_method(
                    activity,
                    jni_str!("setNativePlaybackControls"),
                    jni_sig!("(Ljava/lang/String;Ljava/lang/String;FZZ)Z"),
                    &[
                        JValue::Object(&session_id),
                        JValue::Object(&stream_handle),
                        JValue::Float(controls.volume()),
                        JValue::Bool(controls.muted()),
                        JValue::Bool(controls.paused()),
                    ],
                )?
                .z()
        })?
        .then_some(())
        .ok_or(AndroidPlaybackError)
    }

    pub(super) fn set_viewport(
        identity: &AndroidPlaybackIdentity,
        viewport: AndroidPlaybackViewport,
    ) -> Result<(), AndroidPlaybackError> {
        with_activity(|environment, activity| {
            let session_id = environment.new_string(identity.session_id().as_str())?;
            let stream_handle = environment.new_string(identity.stream_handle().as_str())?;
            environment
                .call_method(
                    activity,
                    jni_str!("setNativePlaybackViewport"),
                    jni_sig!("(Ljava/lang/String;Ljava/lang/String;IIIIZ)Z"),
                    &[
                        JValue::Object(&session_id),
                        JValue::Object(&stream_handle),
                        JValue::Int(viewport.left() as jint),
                        JValue::Int(viewport.top() as jint),
                        JValue::Int(viewport.width() as jint),
                        JValue::Int(viewport.height() as jint),
                        JValue::Bool(viewport.fullscreen()),
                    ],
                )?
                .z()
        })?
        .then_some(())
        .ok_or(AndroidPlaybackError)
    }

    pub(super) fn stop(identity: &AndroidPlaybackIdentity) -> Result<(), AndroidPlaybackError> {
        let stopped = with_activity(|environment, activity| {
            let session_id = environment.new_string(identity.session_id().as_str())?;
            let stream_handle = environment.new_string(identity.stream_handle().as_str())?;
            environment
                .call_method(
                    activity,
                    jni_str!("stopNativePlayback"),
                    jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Z"),
                    &[JValue::Object(&session_id), JValue::Object(&stream_handle)],
                )?
                .z()
        })?;
        if !stopped {
            return Err(AndroidPlaybackError);
        }
        Ok(())
    }

    pub(super) fn stop_session(session_id: &PlaybackSessionId) -> Result<(), AndroidPlaybackError> {
        let stopped = with_activity(|environment, activity| {
            let session_id = environment.new_string(session_id.as_str())?;
            environment
                .call_method(
                    activity,
                    jni_str!("stopNativePlaybackSession"),
                    jni_sig!("(Ljava/lang/String;)Z"),
                    &[JValue::Object(&session_id)],
                )?
                .z()
        })?;
        if !stopped {
            return Err(AndroidPlaybackError);
        }
        Ok(())
    }

    pub(super) fn suspend_session(
        session_id: &PlaybackSessionId,
    ) -> Result<(), AndroidPlaybackError> {
        let suspended = with_activity(|environment, activity| {
            let session_id = environment.new_string(session_id.as_str())?;
            environment
                .call_method(
                    activity,
                    jni_str!("suspendNativePlaybackSession"),
                    jni_sig!("(Ljava/lang/String;)Z"),
                    &[JValue::Object(&session_id)],
                )?
                .z()
        })?;
        if !suspended {
            return Err(AndroidPlaybackError);
        }
        Ok(())
    }

    pub(super) fn suspend_all() -> Result<(), AndroidPlaybackError> {
        let suspended = with_activity(|environment, activity| {
            environment
                .call_method(
                    activity,
                    jni_str!("suspendAllNativePlayback"),
                    jni_sig!("()Z"),
                    &[],
                )?
                .z()
        })?;
        if !suspended {
            return Err(AndroidPlaybackError);
        }
        Ok(())
    }

    pub(super) fn stop_all() -> Result<(), AndroidPlaybackError> {
        let stopped = with_activity(|environment, activity| {
            environment
                .call_method(
                    activity,
                    jni_str!("stopAllNativePlayback"),
                    jni_sig!("()Z"),
                    &[],
                )?
                .z()
        })?;
        if !stopped {
            return Err(AndroidPlaybackError);
        }
        Ok(())
    }

    fn with_activity<T>(
        operation: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> jni::errors::Result<T>,
    ) -> Result<T, AndroidPlaybackError> {
        let context = main_android_context().ok_or(AndroidPlaybackError)?;
        // SAFETY: Tao owns this process-lifetime JavaVM handle. `from_raw`
        // creates a non-owning Rust view for the duration of this call.
        let java_vm = unsafe { jni::JavaVM::from_raw(context.java_vm.cast()) };
        java_vm
            .attach_current_thread(|environment| {
                let activity_raw = context.context_jobject.cast();
                // SAFETY: Tao retains this global Activity reference for the
                // process lifetime. The cast only borrows it.
                let activity =
                    unsafe { environment.as_cast_raw::<Global<JObject<'static>>>(&activity_raw)? };
                operation(environment, activity.as_ref())
            })
            .map_err(|_| AndroidPlaybackError)
    }

    fn write_native_bytes(
        environment: &Env<'_>,
        session_id: &JString<'_>,
        stream_handle: &JString<'_>,
        output: &JByteArray<'_>,
        offset: jint,
        length: jint,
    ) -> jni::errors::Result<jint> {
        if offset < 0 || length <= 0 {
            return Ok(NATIVE_READ_FAILED);
        }
        let array_length = output.len(environment)?;
        let offset = offset as usize;
        let requested = length as usize;
        if offset > array_length || requested > array_length.saturating_sub(offset) {
            return Ok(NATIVE_READ_FAILED);
        }
        let session_id = match PlaybackSessionId::parse(session_id.try_to_string(environment)?) {
            Ok(session_id) => session_id,
            Err(_) => return Ok(NATIVE_READ_FAILED),
        };
        let stream_handle =
            match NativeStreamHandle::parse(stream_handle.try_to_string(environment)?) {
                Ok(stream_handle) => stream_handle,
                Err(_) => return Ok(NATIVE_READ_FAILED),
            };
        let Some(reader) = READER.get() else {
            return Ok(NATIVE_READ_FAILED);
        };
        let bytes = match reader.read(
            AndroidPlaybackIdentity::new(session_id, stream_handle),
            requested,
        ) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(NATIVE_READ_FAILED),
        };
        // SAFETY: `u8` and `i8` have identical size and alignment (1 byte), and
        // every bit pattern is valid for `i8`.
        let signed =
            unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<i8>(), bytes.len()) };
        output.set_region(environment, offset as jsize, signed)?;
        Ok(bytes.len() as jint)
    }

    /// Supplies Media3 only the bytes owned by the matching Rust transport.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_xyz_ponbac_sparrow_NativePlaybackDataSource_readNativePlayback<
        'caller,
    >(
        mut unowned_environment: EnvUnowned<'caller>,
        _instance: JObject<'caller>,
        session_id: JString<'caller>,
        stream_handle: JString<'caller>,
        output: JByteArray<'caller>,
        offset: jint,
        length: jint,
    ) -> jint {
        unowned_environment
            .with_env(|environment| {
                write_native_bytes(
                    environment,
                    &session_id,
                    &stream_handle,
                    &output,
                    offset,
                    length,
                )
            })
            .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }
}

#[cfg(target_os = "android")]
pub(crate) use platform::bind_runtime;

#[cfg(target_os = "android")]
pub(crate) fn start(
    identity: &AndroidPlaybackIdentity,
    viewport: AndroidPlaybackViewport,
    controls: AndroidPlaybackControls,
) -> Result<(), AndroidPlaybackError> {
    platform::start(identity, viewport, controls)
}

#[cfg(not(target_os = "android"))]
pub(crate) fn start(
    _identity: &AndroidPlaybackIdentity,
    _viewport: AndroidPlaybackViewport,
    _controls: AndroidPlaybackControls,
) -> Result<(), AndroidPlaybackError> {
    Err(AndroidPlaybackError)
}

#[cfg(target_os = "android")]
pub(crate) fn status(
    identity: &AndroidPlaybackIdentity,
) -> Result<AndroidPlaybackStatus, AndroidPlaybackError> {
    platform::status(identity)
}

#[cfg(not(target_os = "android"))]
pub(crate) fn status(
    _identity: &AndroidPlaybackIdentity,
) -> Result<AndroidPlaybackStatus, AndroidPlaybackError> {
    Err(AndroidPlaybackError)
}

#[cfg(target_os = "android")]
pub(crate) fn set_controls(
    identity: &AndroidPlaybackIdentity,
    controls: AndroidPlaybackControls,
) -> Result<(), AndroidPlaybackError> {
    platform::set_controls(identity, controls)
}

#[cfg(not(target_os = "android"))]
pub(crate) fn set_controls(
    _identity: &AndroidPlaybackIdentity,
    _controls: AndroidPlaybackControls,
) -> Result<(), AndroidPlaybackError> {
    Err(AndroidPlaybackError)
}

#[cfg(target_os = "android")]
pub(crate) fn set_viewport(
    identity: &AndroidPlaybackIdentity,
    viewport: AndroidPlaybackViewport,
) -> Result<(), AndroidPlaybackError> {
    platform::set_viewport(identity, viewport)
}

#[cfg(not(target_os = "android"))]
pub(crate) fn set_viewport(
    _identity: &AndroidPlaybackIdentity,
    _viewport: AndroidPlaybackViewport,
) -> Result<(), AndroidPlaybackError> {
    Err(AndroidPlaybackError)
}

#[cfg(target_os = "android")]
pub(crate) fn stop(identity: &AndroidPlaybackIdentity) -> Result<(), AndroidPlaybackError> {
    platform::stop(identity)
}

#[cfg(not(target_os = "android"))]
pub(crate) fn stop(_identity: &AndroidPlaybackIdentity) -> Result<(), AndroidPlaybackError> {
    Err(AndroidPlaybackError)
}

#[cfg(target_os = "android")]
pub(crate) fn stop_session(session_id: &PlaybackSessionId) -> Result<(), AndroidPlaybackError> {
    platform::stop_session(session_id)
}

#[cfg(not(target_os = "android"))]
pub(crate) fn stop_session(_session_id: &PlaybackSessionId) -> Result<(), AndroidPlaybackError> {
    Ok(())
}

#[cfg(target_os = "android")]
pub(crate) fn suspend_session(session_id: &PlaybackSessionId) -> Result<(), AndroidPlaybackError> {
    platform::suspend_session(session_id)
}

#[cfg(not(target_os = "android"))]
pub(crate) fn suspend_session(_session_id: &PlaybackSessionId) -> Result<(), AndroidPlaybackError> {
    Ok(())
}

#[cfg(target_os = "android")]
pub(crate) fn suspend_all() -> Result<(), AndroidPlaybackError> {
    platform::suspend_all()
}

#[cfg(not(target_os = "android"))]
pub(crate) fn suspend_all() -> Result<(), AndroidPlaybackError> {
    Ok(())
}

#[cfg(target_os = "android")]
pub(crate) fn stop_all() -> Result<(), AndroidPlaybackError> {
    platform::stop_all()
}

#[cfg(not(target_os = "android"))]
pub(crate) fn stop_all() -> Result<(), AndroidPlaybackError> {
    Ok(())
}
