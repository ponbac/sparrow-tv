use serde::Deserialize;

use crate::playback::{NativeStreamHandle, PlaybackSessionId};

const MAX_VIEWPORT_COORDINATE: u32 = 32_768;
const MAX_VIEWPORT_EXTENT: u32 = 32_768;
#[cfg(any(test, target_os = "android"))]
const MAX_SAFE_COUNTER: u64 = 9_007_199_254_740_991;

/// Physical-pixel rectangle occupied by the Android native video surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AndroidPlaybackViewport {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    fullscreen: bool,
}

impl AndroidPlaybackViewport {
    pub(crate) fn parse(
        left: u32,
        top: u32,
        width: u32,
        height: u32,
        fullscreen: bool,
    ) -> Result<Self, AndroidPlaybackError> {
        if left > MAX_VIEWPORT_COORDINATE
            || top > MAX_VIEWPORT_COORDINATE
            || width == 0
            || width > MAX_VIEWPORT_EXTENT
            || height == 0
            || height > MAX_VIEWPORT_EXTENT
        {
            return Err(AndroidPlaybackError);
        }
        Ok(Self {
            left,
            top,
            width,
            height,
            fullscreen,
        })
    }

    #[cfg(target_os = "android")]
    const fn left(self) -> u32 {
        self.left
    }

    #[cfg(target_os = "android")]
    const fn top(self) -> u32 {
        self.top
    }

    #[cfg(target_os = "android")]
    const fn width(self) -> u32 {
        self.width
    }

    #[cfg(target_os = "android")]
    const fn height(self) -> u32 {
        self.height
    }

    #[cfg(target_os = "android")]
    const fn fullscreen(self) -> bool {
        self.fullscreen
    }
}

/// Bounded presentation controls applied together on Android's UI thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AndroidPlaybackControls {
    volume: f32,
    muted: bool,
    paused: bool,
}

impl AndroidPlaybackControls {
    pub(crate) fn parse(
        volume: f64,
        muted: bool,
        paused: bool,
    ) -> Result<Self, AndroidPlaybackError> {
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err(AndroidPlaybackError);
        }
        Ok(Self {
            volume: volume as f32,
            muted,
            paused,
        })
    }

    #[cfg(target_os = "android")]
    const fn volume(self) -> f32 {
        self.volume
    }

    #[cfg(target_os = "android")]
    const fn muted(self) -> bool {
        self.muted
    }

    #[cfg(target_os = "android")]
    const fn paused(self) -> bool {
        self.paused
    }
}

/// Opaque identity shared by the Rust stream actor and Android DataSource.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AndroidPlaybackIdentity {
    session_id: PlaybackSessionId,
    stream_handle: NativeStreamHandle,
}

impl AndroidPlaybackIdentity {
    pub(crate) const fn new(
        session_id: PlaybackSessionId,
        stream_handle: NativeStreamHandle,
    ) -> Self {
        Self {
            session_id,
            stream_handle,
        }
    }

    pub(crate) fn session_id(&self) -> &PlaybackSessionId {
        &self.session_id
    }

    pub(crate) fn stream_handle(&self) -> &NativeStreamHandle {
        &self.stream_handle
    }
}

/// Safe aggregate Android player phases returned to the WebView controller.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AndroidPlaybackPhase {
    Starting,
    Playing,
    Paused,
    Failed,
    Stopped,
}

/// Aggregate-only Android player status. It cannot contain provider data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AndroidPlaybackStatus {
    phase: AndroidPlaybackPhase,
    decoded_frames: u64,
    dropped_frames: u64,
    buffered_duration_ms: u64,
    silent: bool,
}

impl AndroidPlaybackStatus {
    pub(crate) const fn phase(self) -> AndroidPlaybackPhase {
        self.phase
    }

    pub(crate) const fn dropped_frames(self) -> u64 {
        self.dropped_frames
    }

    pub(crate) const fn decoded_frames(self) -> u64 {
        self.decoded_frames
    }

    pub(crate) const fn buffered_duration_ms(self) -> u64 {
        self.buffered_duration_ms
    }

    pub(crate) const fn silent(self) -> bool {
        self.silent
    }
}

#[cfg(any(test, target_os = "android"))]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AndroidPlaybackStatusWire {
    state: AndroidPlaybackPhase,
    decoded_frames: u64,
    dropped_frames: u64,
    buffered_duration_ms: u64,
    silent: bool,
}

/// Privacy-safe Android adapter failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Android native playback is unavailable")]
pub(crate) struct AndroidPlaybackError;

#[cfg(any(test, target_os = "android"))]
fn parse_status_json(value: &str) -> Result<AndroidPlaybackStatus, AndroidPlaybackError> {
    let status: AndroidPlaybackStatusWire =
        serde_json::from_str(value).map_err(|_| AndroidPlaybackError)?;
    if status.decoded_frames > MAX_SAFE_COUNTER
        || status.dropped_frames > MAX_SAFE_COUNTER
        || status.buffered_duration_ms > MAX_SAFE_COUNTER
    {
        return Err(AndroidPlaybackError);
    }
    Ok(AndroidPlaybackStatus {
        phase: status.state,
        decoded_frames: status.decoded_frames,
        dropped_frames: status.dropped_frames,
        buffered_duration_ms: status.buffered_duration_ms,
        silent: status.silent,
    })
}

#[cfg(any(test, target_os = "android"))]
#[derive(Clone, Copy)]
enum RemainderStop<'a> {
    Exact(&'a AndroidPlaybackIdentity),
    Session(&'a PlaybackSessionId),
    All,
}

#[cfg(any(test, target_os = "android"))]
impl RemainderStop<'_> {
    fn matches(self, identity: &AndroidPlaybackIdentity) -> bool {
        match self {
            Self::Exact(expected) => expected == identity,
            Self::Session(session_id) => &identity.session_id == session_id,
            Self::All => true,
        }
    }
}

#[cfg(any(test, target_os = "android"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemainderReadAction {
    Serve,
    PreserveForOwner,
}

#[cfg(any(test, target_os = "android"))]
fn remainder_read_action(
    buffered: &AndroidPlaybackIdentity,
    requested: &AndroidPlaybackIdentity,
) -> RemainderReadAction {
    if buffered == requested {
        RemainderReadAction::Serve
    } else {
        RemainderReadAction::PreserveForOwner
    }
}

#[cfg(target_os = "android")]
mod platform {
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        sync::{Mutex, OnceLock, Weak},
    };

    use jni::{
        Env, EnvUnowned, JValue, jni_sig, jni_str,
        objects::{Global, JByteArray, JObject, JString},
        sys::{jint, jsize},
    };
    use tauri::tao::platform::android::prelude::main_android_context;

    use super::{
        AndroidPlaybackControls, AndroidPlaybackError, AndroidPlaybackIdentity,
        AndroidPlaybackStatus, AndroidPlaybackViewport, RemainderReadAction, RemainderStop,
        parse_status_json, remainder_read_action,
    };
    use crate::{
        playback::{NativeStreamHandle, PlaybackSessionId},
        runtime::InstalledRuntime,
    };

    const MAX_NATIVE_READ_BYTES: usize = 64 * 1024;
    const MAX_COALESCED_ACTOR_READS: usize = 16;
    const NATIVE_READ_FAILED: jint = -2;

    static BRIDGE: OnceLock<AndroidPlaybackBridge> = OnceLock::new();

    struct AndroidPlaybackBridge {
        runtime: Weak<InstalledRuntime>,
        remainder: Mutex<Option<BufferedRead>>,
    }

    struct BufferedRead {
        identity: AndroidPlaybackIdentity,
        bytes: Vec<u8>,
        offset: usize,
    }

    pub(crate) fn bind_runtime(
        runtime: Weak<InstalledRuntime>,
    ) -> Result<(), AndroidPlaybackError> {
        BRIDGE
            .set(AndroidPlaybackBridge {
                runtime,
                remainder: Mutex::new(None),
            })
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
        parse_status_json(&wire)
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
        forget_remainder(RemainderStop::Exact(identity));
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
        forget_remainder(RemainderStop::Session(session_id));
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
        forget_remainder(RemainderStop::Session(session_id));
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
        forget_remainder(RemainderStop::All);
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
        forget_remainder(RemainderStop::All);
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

    fn forget_remainder(scope: RemainderStop<'_>) {
        let Some(bridge) = BRIDGE.get() else {
            return;
        };
        let Ok(mut remainder) = bridge.remainder.lock() else {
            return;
        };
        if remainder
            .as_ref()
            .is_some_and(|buffered| scope.matches(&buffered.identity))
        {
            *remainder = None;
        }
    }

    fn read_native(
        identity: AndroidPlaybackIdentity,
        maximum: usize,
    ) -> Result<Vec<u8>, AndroidPlaybackError> {
        let bridge = BRIDGE.get().ok_or(AndroidPlaybackError)?;
        if let Some(bytes) = take_remainder(bridge, &identity, maximum)? {
            return Ok(bytes);
        }
        let runtime = bridge.runtime.upgrade().ok_or(AndroidPlaybackError)?;
        let session_id = identity.session_id().clone();
        let stream_handle = identity.stream_handle().clone();
        let (returned, overflow) = catch_unwind(AssertUnwindSafe(|| {
            tauri::async_runtime::block_on(async move {
                let mut returned = Vec::with_capacity(maximum);
                let mut overflow = None;
                for _ in 0..MAX_COALESCED_ACTOR_READS {
                    let next = runtime
                        .read_playback(session_id.clone(), stream_handle.clone())
                        .await;
                    let mut bytes = match next {
                        Ok(bytes) => bytes,
                        Err(_) if !returned.is_empty() => break,
                        Err(_) => return Err(AndroidPlaybackError),
                    };
                    if bytes.is_empty() {
                        break;
                    }
                    let remaining = maximum - returned.len();
                    if bytes.len() > remaining {
                        overflow = Some(bytes.split_off(remaining));
                    }
                    returned.extend_from_slice(&bytes);
                    if returned.len() == maximum || overflow.is_some() {
                        break;
                    }
                }
                Ok::<_, AndroidPlaybackError>((returned, overflow))
            })
        }))
        .map_err(|_| AndroidPlaybackError)??;
        if let Some(bytes) = overflow {
            let mut remainder = bridge.remainder.lock().map_err(|_| AndroidPlaybackError)?;
            *remainder = Some(BufferedRead {
                identity,
                bytes,
                offset: 0,
            });
        }
        Ok(returned)
    }

    fn take_remainder(
        bridge: &AndroidPlaybackBridge,
        identity: &AndroidPlaybackIdentity,
        maximum: usize,
    ) -> Result<Option<Vec<u8>>, AndroidPlaybackError> {
        let mut slot = bridge.remainder.lock().map_err(|_| AndroidPlaybackError)?;
        let Some(buffered) = slot.as_mut() else {
            return Ok(None);
        };
        if remainder_read_action(&buffered.identity, identity)
            == RemainderReadAction::PreserveForOwner
        {
            return Ok(None);
        }
        let end = buffered
            .offset
            .saturating_add(maximum)
            .min(buffered.bytes.len());
        let bytes = buffered.bytes[buffered.offset..end].to_vec();
        buffered.offset = end;
        if buffered.offset == buffered.bytes.len() {
            *slot = None;
        }
        Ok(Some(bytes))
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
        let maximum = requested.min(MAX_NATIVE_READ_BYTES);
        let bytes = match read_native(
            AndroidPlaybackIdentity::new(session_id, stream_handle),
            maximum,
        ) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(NATIVE_READ_FAILED),
        };
        let signed = bytes.iter().map(|byte| *byte as i8).collect::<Vec<_>>();
        output.set_region(environment, offset as jsize, &signed)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_and_controls_reject_unbounded_or_non_finite_values() {
        assert!(AndroidPlaybackViewport::parse(0, 0, 1920, 1080, false).is_ok());
        assert!(AndroidPlaybackViewport::parse(0, 0, 0, 1080, false).is_err());
        assert!(AndroidPlaybackViewport::parse(32_769, 0, 1, 1, true).is_err());
        assert!(AndroidPlaybackViewport::parse(0, 0, 32_769, 1, true).is_err());

        assert!(AndroidPlaybackControls::parse(0.75, false, false).is_ok());
        assert!(AndroidPlaybackControls::parse(f64::NAN, false, false).is_err());
        assert!(AndroidPlaybackControls::parse(-0.1, false, false).is_err());
        assert!(AndroidPlaybackControls::parse(1.1, false, false).is_err());
    }

    #[test]
    fn aggregate_status_is_closed_bounded_and_rejects_extra_context() {
        assert_eq!(
            parse_status_json(
                r#"{"state":"playing","decodedFrames":1200,"droppedFrames":3,"bufferedDurationMs":1250,"silent":true}"#,
            ),
            Ok(AndroidPlaybackStatus {
                phase: AndroidPlaybackPhase::Playing,
                decoded_frames: 1_200,
                dropped_frames: 3,
                buffered_duration_ms: 1_250,
                silent: true,
            })
        );
        assert!(
            parse_status_json(
                r#"{"state":"playing","decodedFrames":0,"droppedFrames":0,"bufferedDurationMs":0,"silent":true,"url":"https://provider.invalid/private"}"#,
            )
            .is_err()
        );
        assert!(
            parse_status_json(
                r#"{"state":"unknown","decodedFrames":0,"droppedFrames":0,"bufferedDurationMs":0,"silent":true}"#,
            )
            .is_err()
        );
        assert!(
            parse_status_json(&format!(
                r#"{{"state":"playing","decodedFrames":0,"droppedFrames":{},"bufferedDurationMs":0,"silent":true}}"#,
                MAX_SAFE_COUNTER + 1
            ))
            .is_err()
        );
    }

    #[test]
    fn remainder_stop_scope_is_exact_before_it_is_session_wide() {
        let session_id =
            PlaybackSessionId::parse("play1_0123456789abcdef0123456789abcdef_1".to_owned())
                .expect("fixture session parses");
        let first = AndroidPlaybackIdentity::new(
            session_id.clone(),
            NativeStreamHandle::parse("stream1_0000000000000001".to_owned())
                .expect("fixture handle parses"),
        );
        let second = AndroidPlaybackIdentity::new(
            session_id.clone(),
            NativeStreamHandle::parse("stream1_0000000000000002".to_owned())
                .expect("fixture handle parses"),
        );

        assert!(RemainderStop::Exact(&first).matches(&first));
        assert!(!RemainderStop::Exact(&first).matches(&second));
        assert!(RemainderStop::Session(&session_id).matches(&first));
        assert!(RemainderStop::Session(&session_id).matches(&second));
        assert!(RemainderStop::All.matches(&first));
        assert_eq!(
            remainder_read_action(&first, &second),
            RemainderReadAction::PreserveForOwner,
            "a late old read cannot discard a new generation's buffered bytes"
        );
        assert_eq!(
            remainder_read_action(&first, &first),
            RemainderReadAction::Serve
        );
    }
}
