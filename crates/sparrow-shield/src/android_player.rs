//! Asynchronous bridge from the installed playback runner to the Activity.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use jni::{
    Env, EnvUnowned,
    errors::{LogErrorAndDefault, Result as JniResult},
    jni_sig, jni_str,
    objects::{JByteArray, JObject, JString, JValue},
};
use sparrow_installed_playback::{
    NativeStreamHandle, PlaybackSessionId, ScreenWake, ScreenWakeError,
    android::{
        AndroidMedia3Reader, AndroidPlaybackControls, AndroidPlaybackError,
        AndroidPlaybackIdentity, AndroidPlaybackStatus, AndroidPlaybackViewport,
    },
    tv::{
        TvHostId, TvPlaybackCommand, TvPresentation, TvPresentationEvent, TvPresentationFuture,
        TvPresentationKey, TvPresentationStop, TvRecoveryFrame,
    },
};
use tokio::sync::oneshot;

use crate::activity;

const ACK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct AndroidPlayer {
    inner: Arc<Bridge>,
}

#[derive(Default)]
struct Bridge {
    state: Mutex<State>,
    next_generation: AtomicU64,
    next_request: AtomicU64,
}

#[derive(Default)]
struct State {
    bindings: HashMap<(TvHostId, u64), Binding>,
    pending: HashMap<u64, Pending>,
    started_hosts: HashSet<TvHostId>,
    wake: bool,
}

#[derive(Clone)]
struct Binding {
    generation: u64,
    identity: AndroidPlaybackIdentity,
    live: bool,
    // Activity teardown also retires clones held outside the bridge lock.
    activity_released: Arc<AtomicBool>,
}

struct Pending {
    host: TvHostId,
    generation: u64,
    kind: CallKind,
    activity_released: Arc<AtomicBool>,
    send: oneshot::Sender<Ack>,
}

struct Ack {
    frame: Option<Arc<TvRecoveryFrame>>,
    applied: bool,
    status: Option<AndroidPlaybackStatus>,
}

impl Ack {
    fn activity_released(kind: CallKind) -> Self {
        Self {
            frame: None,
            applied: kind == CallKind::Stop,
            status: None,
        }
    }

    fn into_result(self) -> Result<Self, AndroidPlaybackError> {
        self.applied.then_some(self).ok_or(AndroidPlaybackError)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
enum CallKind {
    Start = 1,
    Viewport = 2,
    Controls = 3,
    Sample = 4,
    Stop = 5,
    Wake = 6,
    Capture = 7,
}

enum Call {
    Start(AndroidPlaybackViewport, AndroidPlaybackControls),
    Viewport(Option<AndroidPlaybackViewport>),
    Controls(AndroidPlaybackControls),
    Sample,
    Stop(bool),
    Capture,
}

impl Call {
    fn kind(&self) -> CallKind {
        match self {
            Self::Start(..) => CallKind::Start,
            Self::Viewport(_) => CallKind::Viewport,
            Self::Controls(_) => CallKind::Controls,
            Self::Sample => CallKind::Sample,
            Self::Capture => CallKind::Capture,
            Self::Stop(_) => CallKind::Stop,
        }
    }

    fn post(&self, host: TvHostId, binding: &Binding, request: u64) -> bool {
        let mut queued = false;
        let called = activity::call(host, |env, activity| {
            let player = playback_host(env, activity)?;
            let session = JString::from_str(env, binding.identity.session_id().as_str())?;
            let stream = JString::from_str(env, binding.identity.stream_handle().as_str())?;
            let mut args = vec![
                JValue::Long(host as i64),
                JValue::Long(binding.generation as i64),
                JValue::Long(request as i64),
                JValue::Object(session.as_ref()),
                JValue::Object(stream.as_ref()),
            ];
            let (name, signature) = match *self {
                Self::Start(viewport, controls) => {
                    args.extend(bounds(viewport));
                    args.extend(control_values(controls));
                    args.push(JValue::Bool(viewport.fullscreen()));
                    (
                        jni_str!("startPlayback"),
                        jni_sig!("(JJJLjava/lang/String;Ljava/lang/String;IIIIFZZZ)Z"),
                    )
                }
                Self::Viewport(viewport) => {
                    args.extend(viewport.map_or([0, 0, 0, 0].map(JValue::Int), bounds));
                    args.push(JValue::Bool(
                        viewport.is_some_and(AndroidPlaybackViewport::fullscreen),
                    ));
                    (
                        jni_str!("setPlaybackViewport"),
                        jni_sig!("(JJJLjava/lang/String;Ljava/lang/String;IIIIZ)Z"),
                    )
                }
                Self::Controls(controls) => {
                    args.extend(control_values(controls));
                    (
                        jni_str!("setPlaybackControls"),
                        jni_sig!("(JJJLjava/lang/String;Ljava/lang/String;FZZ)Z"),
                    )
                }
                Self::Capture => (
                    jni_str!("capturePlaybackFrame"),
                    jni_sig!("(JJJLjava/lang/String;Ljava/lang/String;)Z"),
                ),
                Self::Sample => (
                    jni_str!("samplePlayback"),
                    jni_sig!("(JJJLjava/lang/String;Ljava/lang/String;)Z"),
                ),
                Self::Stop(retain) => {
                    args.push(JValue::Bool(retain));
                    (
                        jni_str!("stopPlayback"),
                        jni_sig!("(JJJLjava/lang/String;Ljava/lang/String;Z)Z"),
                    )
                }
            };
            queued = env.call_method(&player, name, signature, &args)?.z()?;
            Ok(())
        });
        called && queued
    }
}

pub(super) fn set_ambient_sampling(host: u64, epoch: u64) {
    activity::call(host, |env, activity| {
        let player = playback_host(env, activity)?;
        env.call_method(
            &player,
            jni_str!("setAmbientSampling"),
            jni_sig!("(J)V"),
            &[JValue::Long(epoch as i64)],
        )?;
        Ok(())
    });
}

/// Asks for a still of the visible picture. It returns as `Event::FlightStill`
/// with the request, or `None` when there was no picture to copy.
pub(super) fn capture_flight_still(host: u64, request: u64, [width, height]: [u32; 2]) {
    let called = activity::call(host, |env, activity| {
        let player = playback_host(env, activity)?;
        env.call_method(
            &player,
            jni_str!("captureFlightStill"),
            jni_sig!("(JII)V"),
            &[
                JValue::Long(request as i64),
                JValue::Int(width as i32),
                JValue::Int(height as i32),
            ],
        )?;
        Ok(())
    });
    if !called {
        crate::enqueue_activity(host as i64, activity::Event::FlightStill(request, None));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldPlayback_nativeFlightStill<'local>(
    mut env: EnvUnowned<'local>,
    _: JObject<'local>,
    host: i64,
    request: i64,
    width: i32,
    height: i32,
    pixels: JByteArray<'local>,
) {
    let frame = if width > 0 && height > 0 && !pixels.is_null() {
        env.with_env_no_catch(|env| env.convert_byte_array(&pixels).map(Some))
            .resolve::<LogErrorAndDefault>()
            // GPUI images are BGRA, as Android copies them.
            .and_then(|bgra| image::RgbaImage::from_raw(width as u32, height as u32, bgra))
    } else {
        None
    };
    crate::enqueue_activity(host, activity::Event::FlightStill(request as u64, frame));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldPlayback_nativeAmbient(
    _: EnvUnowned,
    _: JObject,
    host: i64,
    epoch: i64,
    left: u32,
    right: u32,
    bottom: u32,
) {
    crate::enqueue_activity(
        host,
        activity::Event::Ambient {
            epoch: epoch as u64,
            colors: [left, right, bottom],
        },
    );
}

fn playback_host<'local>(
    env: &mut Env<'local>,
    activity: &JObject<'_>,
) -> JniResult<JObject<'local>> {
    env.call_method(
        activity,
        jni_str!("getPlaybackHost"),
        jni_sig!("()Lxyz/ponbac/sparrow/shield/ShieldPlayback;"),
        &[],
    )?
    .l()
}

fn bounds(viewport: AndroidPlaybackViewport) -> [JValue<'static>; 4] {
    [
        JValue::Int(viewport.left() as i32),
        JValue::Int(viewport.top() as i32),
        JValue::Int(viewport.width() as i32),
        JValue::Int(viewport.height() as i32),
    ]
}

fn control_values(controls: AndroidPlaybackControls) -> [JValue<'static>; 3] {
    [
        JValue::Float(controls.volume()),
        JValue::Bool(controls.muted()),
        JValue::Bool(controls.paused()),
    ]
}

struct PendingGuard<'a> {
    bridge: &'a Bridge,
    request: u64,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.bridge
            .state
            .lock()
            .unwrap()
            .pending
            .remove(&self.request);
    }
}

impl Bridge {
    async fn request(
        &self,
        host: TvHostId,
        binding: &Binding,
        call: Call,
    ) -> Result<Ack, AndroidPlaybackError> {
        self.request_with_post(host, binding, call.kind(), |request| {
            call.post(host, binding, request)
        })
        .await
    }

    async fn request_with_post(
        &self,
        host: TvHostId,
        binding: &Binding,
        kind: CallKind,
        post: impl FnOnce(u64) -> bool,
    ) -> Result<Ack, AndroidPlaybackError> {
        let request = self.next_request.fetch_add(1, Ordering::Relaxed) + 1;
        let (send, receive) = oneshot::channel();
        {
            let mut state = self.state.lock().unwrap();
            if binding.activity_released.load(Ordering::Acquire) {
                return Ack::activity_released(kind).into_result();
            }
            state.pending.insert(
                request,
                Pending {
                    host,
                    generation: binding.generation,
                    kind,
                    activity_released: Arc::clone(&binding.activity_released),
                    send,
                },
            );
        }
        let _pending = PendingGuard {
            bridge: self,
            request,
        };
        // Kotlin rejects new posts as soon as its synchronous release starts.
        // For Stop, wait for nativeDestroy to confirm the release.
        if !post(request) && kind != CallKind::Stop {
            return Err(AndroidPlaybackError);
        }
        match tokio::time::timeout(ACK_TIMEOUT, receive).await {
            Ok(Ok(ack)) => ack.into_result(),
            _ if binding.activity_released.load(Ordering::Acquire) => {
                Ack::activity_released(kind).into_result()
            }
            _ => Err(AndroidPlaybackError),
        }
    }

    fn binding(&self, key: &TvPresentationKey) -> Option<Binding> {
        self.state
            .lock()
            .unwrap()
            .bindings
            .iter()
            .filter(|((host, _), binding)| *host == key.host && binding.identity == key.identity)
            .map(|(_, binding)| binding)
            .max_by_key(|binding| binding.generation)
            .cloned()
    }

    fn forget_binding(&self, host: TvHostId, generation: u64) {
        self.state
            .lock()
            .unwrap()
            .bindings
            .remove(&(host, generation));
    }

    fn retire_readers(&self, host: TvHostId, generation: u64) {
        let mut state = self.state.lock().unwrap();
        if let Some(binding) = state.bindings.get_mut(&(host, generation)) {
            binding.live = false;
        }
    }

    async fn apply(&self, key: TvPresentationKey, call: Call) -> Result<Ack, AndroidPlaybackError> {
        let binding = self.binding(&key).ok_or(AndroidPlaybackError)?;
        self.request(key.host, &binding, call).await
    }

    fn post_wake(&self, hosts: impl IntoIterator<Item = TvHostId>, active: bool, generation: u64) {
        for host in hosts {
            let request = self.next_request.fetch_add(1, Ordering::Relaxed) + 1;
            activity::call(host, |env, activity| {
                let player = playback_host(env, activity)?;
                env.call_method(
                    &player,
                    jni_str!("setPlaybackScreenWake"),
                    jni_sig!("(JJJZ)Z"),
                    &[
                        JValue::Long(host as i64),
                        JValue::Long(generation as i64),
                        JValue::Long(request as i64),
                        JValue::Bool(active),
                    ],
                )?;
                Ok(())
            });
        }
    }
}

impl AndroidPlayer {
    pub(super) fn host_active(&self, host: TvHostId, started: bool) {
        let (desired, generation) = {
            let mut state = self.inner.state.lock().unwrap();
            if started {
                state.started_hosts.insert(host);
            } else {
                state.started_hosts.remove(&host);
            }
            (
                state.wake && started,
                self.inner.next_generation.fetch_add(1, Ordering::Relaxed) + 1,
            )
        };
        self.inner.post_wake([host], desired, generation);
    }

    pub(super) fn host_destroyed(&self, host: TvHostId) {
        let mut state = self.inner.state.lock().unwrap();
        state.started_hosts.remove(&host);
        for (_, binding) in state
            .bindings
            .extract_if(|(binding_host, _), _| *binding_host == host)
        {
            binding.activity_released.store(true, Ordering::Release);
        }
        for (_, pending) in state.pending.extract_if(|_, pending| pending.host == host) {
            pending.activity_released.store(true, Ordering::Release);
            let _ = pending.send.send(Ack::activity_released(pending.kind));
        }
    }

    pub(super) fn identity(
        &self,
        host: TvHostId,
        generation: u64,
    ) -> Option<AndroidPlaybackIdentity> {
        let state = self.inner.state.lock().unwrap();
        let binding = state
            .bindings
            .get(&(host, generation))
            .filter(|binding| binding.live)?;
        Some(binding.identity.clone())
    }

    fn acknowledge(&self, host: TvHostId, generation: u64, request: u64, kind: i32, ack: Ack) {
        if kind == CallKind::Wake as i32 {
            return;
        }
        let mut state = self.inner.state.lock().unwrap();
        if state.pending.get(&request).is_some_and(|pending| {
            pending.host == host && pending.generation == generation && pending.kind as i32 == kind
        }) && let Some(pending) = state.pending.remove(&request)
        {
            let _ = pending.send.send(ack);
        }
    }
}

impl ScreenWake for AndroidPlayer {
    fn set_active(&self, active: bool) -> Result<(), ScreenWakeError> {
        let (hosts, generation) = {
            let mut state = self.inner.state.lock().unwrap();
            state.wake = active;
            (
                state.started_hosts.iter().copied().collect::<Vec<_>>(),
                self.inner.next_generation.fetch_add(1, Ordering::Relaxed) + 1,
            )
        };
        self.inner.post_wake(hosts, active, generation);
        Ok(())
    }
}

impl TvPresentation for AndroidPlayer {
    fn start(
        &self,
        key: TvPresentationKey,
        viewport: AndroidPlaybackViewport,
        controls: AndroidPlaybackControls,
    ) -> TvPresentationFuture<()> {
        let bridge = Arc::clone(&self.inner);
        Box::pin(async move {
            let binding = Binding {
                generation: bridge.next_generation.fetch_add(1, Ordering::Relaxed) + 1,
                identity: key.identity,
                live: true,
                activity_released: Arc::new(AtomicBool::new(false)),
            };
            {
                let mut state = bridge.state.lock().unwrap();
                if !state.started_hosts.contains(&key.host) {
                    return Err(AndroidPlaybackError);
                }
                state
                    .bindings
                    .insert((key.host, binding.generation), binding.clone());
            }
            let result = bridge
                .request(key.host, &binding, Call::Start(viewport, controls))
                .await;
            if result.is_ok() {
                // Android replaced the retained presentation when it acknowledged the start.
                bridge
                    .state
                    .lock()
                    .unwrap()
                    .bindings
                    .retain(|(host, generation), _| {
                        *host != key.host || *generation == binding.generation
                    });
            } else {
                // Stop also retires a generation whose queued start never ran.
                bridge.retire_readers(key.host, binding.generation);
                if bridge
                    .request(key.host, &binding, Call::Stop(false))
                    .await
                    .is_ok()
                {
                    bridge.forget_binding(key.host, binding.generation);
                }
            }
            result.map(drop)
        })
    }

    fn stop(&self, key: TvPresentationKey, mode: TvPresentationStop) -> TvPresentationFuture<()> {
        let bridge = Arc::clone(&self.inner);
        Box::pin(async move {
            let Some(binding) = bridge.binding(&key) else {
                return Ok(());
            };
            let retain = matches!(mode, TvPresentationStop::RetainPicture);
            bridge.retire_readers(key.host, binding.generation);
            let result = bridge.request(key.host, &binding, Call::Stop(retain)).await;
            if result.is_ok() && !retain {
                bridge.forget_binding(key.host, binding.generation);
            }
            result.map(drop)
        })
    }

    fn capture_frame(&self, key: TvPresentationKey) -> TvPresentationFuture<Arc<TvRecoveryFrame>> {
        let bridge = Arc::clone(&self.inner);
        Box::pin(async move {
            bridge
                .apply(key, Call::Capture)
                .await?
                .frame
                .ok_or(AndroidPlaybackError)
        })
    }

    fn status(&self, key: TvPresentationKey) -> TvPresentationFuture<AndroidPlaybackStatus> {
        let bridge = Arc::clone(&self.inner);
        Box::pin(async move {
            bridge
                .apply(key, Call::Sample)
                .await?
                .status
                .ok_or(AndroidPlaybackError)
        })
    }

    fn set_controls(
        &self,
        key: TvPresentationKey,
        controls: AndroidPlaybackControls,
    ) -> TvPresentationFuture<()> {
        let bridge = Arc::clone(&self.inner);
        Box::pin(async move { bridge.apply(key, Call::Controls(controls)).await.map(drop) })
    }

    fn set_viewport(
        &self,
        key: TvPresentationKey,
        viewport: Option<AndroidPlaybackViewport>,
    ) -> TvPresentationFuture<()> {
        let bridge = Arc::clone(&self.inner);
        Box::pin(async move { bridge.apply(key, Call::Viewport(viewport)).await.map(drop) })
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldPlayback_nativePlaybackAck<'local>(
    mut env: EnvUnowned<'local>,
    _: JObject<'local>,
    host: i64,
    generation: i64,
    request: i64,
    kind: i32,
    applied: bool,
    status: JString<'local>,
) {
    let status = if kind == CallKind::Sample as i32 && applied && !status.is_null() {
        let status = env
            .with_env_no_catch(|env| status.try_to_string(env))
            .resolve::<LogErrorAndDefault>();
        AndroidPlaybackStatus::parse_json(&status).ok()
    } else {
        None
    };
    if let Some(playback) = crate::playback::get() {
        playback.player.acknowledge(
            host as u64,
            generation as u64,
            request as u64,
            kind,
            Ack {
                applied,
                status,
                frame: None,
            },
        );
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldPlayback_nativePlaybackFrame<'local>(
    mut env: EnvUnowned<'local>,
    _: JObject<'local>,
    host: i64,
    generation: i64,
    request: i64,
    width: i32,
    height: i32,
    pixels: JByteArray<'local>,
) {
    let Some(playback) = crate::playback::get() else {
        return;
    };
    let frame = if (1..=640).contains(&width) && (1..=360).contains(&height) && !pixels.is_null() {
        env.with_env_no_catch(|env| env.convert_byte_array(&pixels).map(Some))
            .resolve::<LogErrorAndDefault>()
            .filter(|bytes| bytes.len() == (width * height * 4) as usize)
            .map(|bgra| {
                Arc::new(TvRecoveryFrame {
                    width: width as u32,
                    height: height as u32,
                    bgra,
                })
            })
    } else {
        None
    };
    playback.player.acknowledge(
        host as u64,
        generation as u64,
        request as u64,
        CallKind::Capture as i32,
        Ack {
            applied: frame.is_some(),
            frame,
            status: None,
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldPlayback_nativePlaybackSignal(
    _: EnvUnowned,
    _: JObject,
    host: i64,
    generation: i64,
    signal: i32,
) {
    let Some(playback) = crate::playback::get() else {
        return;
    };
    let host = host as u64;
    let Some(identity) = playback.player.identity(host, generation as u64) else {
        return;
    };
    let event = match signal {
        1 => TvPresentationEvent::FirstFrame,
        // Media3 EOF for a live Channel is an unexpected interruption.
        2 | 3 => TvPresentationEvent::StreamInterrupted,
        4 => TvPresentationEvent::PlayerUnavailable,
        5 => TvPresentationEvent::AudioFocusLost,
        _ => return,
    };
    playback.send_critical(TvPlaybackCommand::PresentationEvent {
        key: TvPresentationKey { host, identity },
        event,
    });
}

struct MediaRead<'local> {
    host: i64,
    generation: i64,
    session: JString<'local>,
    stream: JString<'local>,
    output: JByteArray<'local>,
    offset: i32,
    length: i32,
}

fn read_bytes(
    reader: &AndroidMedia3Reader,
    player: &AndroidPlayer,
    env: &Env<'_>,
    request: MediaRead<'_>,
) -> JniResult<i32> {
    let MediaRead {
        host,
        generation,
        session,
        stream,
        output,
        offset,
        length,
    } = request;
    if offset < 0 || length <= 0 {
        return Ok(-1);
    }
    let array_length = output.len(env)?;
    let requested = length as usize;
    if requested > array_length.saturating_sub(offset as usize) {
        return Ok(-1);
    }
    let Ok(session) = PlaybackSessionId::parse(session.try_to_string(env)?) else {
        return Ok(-1);
    };
    let Ok(stream) = NativeStreamHandle::parse(stream.try_to_string(env)?) else {
        return Ok(-1);
    };
    let identity = AndroidPlaybackIdentity::new(session, stream);
    if player.identity(host as u64, generation as u64).as_ref() != Some(&identity) {
        return Ok(-1);
    }
    let Ok(bytes) = reader.read(identity.clone(), requested) else {
        return Ok(-1);
    };
    if player.identity(host as u64, generation as u64).as_ref() != Some(&identity) {
        return Ok(-1);
    }
    // SAFETY: u8 and i8 have the same layout, and every bit pattern is valid in both.
    let signed = unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<i8>(), bytes.len()) };
    output.set_region(env, offset, signed)?;
    Ok(bytes.len() as i32)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_xyz_ponbac_sparrow_shield_ShieldPlayback_readNativePlayback<'local>(
    mut env: EnvUnowned<'local>,
    _: JObject<'local>,
    host: i64,
    generation: i64,
    session: JString<'local>,
    stream: JString<'local>,
    output: JByteArray<'local>,
    offset: i32,
    length: i32,
) -> i32 {
    let Some(playback) = crate::playback::get() else {
        return -1;
    };
    env.with_env_no_catch(|env| {
        let request = MediaRead {
            host,
            generation,
            session,
            stream,
            output,
            offset,
            length,
        };
        read_bytes(&playback.reader, &playback.player, env, request).map(Some)
    })
    .resolve::<LogErrorAndDefault>()
    .unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use std::{
        future::poll_fn,
        pin::{Pin, pin},
        task::Poll,
    };

    use super::*;

    fn bind(player: &AndroidPlayer, host: TvHostId, generation: u64, live: bool) -> Binding {
        let session =
            PlaybackSessionId::parse("play1_00000000000000000000000000000001_1".into()).unwrap();
        let stream = NativeStreamHandle::parse(format!("stream1_{generation:016x}")).unwrap();
        let binding = Binding {
            generation,
            identity: AndroidPlaybackIdentity::new(session, stream),
            live,
            activity_released: Arc::default(),
        };
        player
            .inner
            .state
            .lock()
            .unwrap()
            .bindings
            .insert((host, generation), binding.clone());
        binding
    }

    async fn is_waiting<T>(mut request: Pin<&mut impl Future<Output = T>>) -> bool {
        poll_fn(|cx| Poll::Ready(request.as_mut().poll(cx).is_pending())).await
    }

    #[tokio::test]
    async fn activity_release_confirms_waiting_stop_even_if_java_refused_the_enqueue() {
        let player = AndroidPlayer::default();
        let live = bind(&player, 1, 1, true);
        let retained = bind(&player, 1, 2, false);
        let other = bind(&player, 2, 3, true);
        let mut stop = pin!(
            player
                .inner
                .request_with_post(1, &retained, CallKind::Stop, |_| false)
        );
        let mut sample = pin!(
            player
                .inner
                .request_with_post(1, &live, CallKind::Sample, |_| true)
        );
        let mut other_stop = pin!(player.inner.request_with_post(
            2,
            &other,
            CallKind::Stop,
            |_| true
        ));

        assert!(is_waiting(stop.as_mut()).await);
        assert!(is_waiting(sample.as_mut()).await);
        assert!(is_waiting(other_stop.as_mut()).await);
        player.host_destroyed(1);

        assert!(stop.await.is_ok());
        assert!(sample.await.is_err());
        assert!(is_waiting(other_stop.as_mut()).await);
        player.host_destroyed(2);
        assert!(other_stop.await.is_ok());
    }

    #[tokio::test]
    async fn activity_release_reaches_a_binding_looked_up_before_stop_registration() {
        let player = AndroidPlayer::default();
        let stored = bind(&player, 1, 1, true);
        let key = TvPresentationKey {
            host: 1,
            identity: stored.identity,
        };
        let binding = player.inner.binding(&key).unwrap();
        player.host_destroyed(1);

        assert!(player.inner.binding(&key).is_none());
        let stop = player
            .inner
            .request_with_post(1, &binding, CallKind::Stop, |_| {
                panic!("stop was dispatched after Activity release")
            });
        assert!(stop.await.is_ok());
        let sample = player
            .inner
            .request_with_post(1, &binding, CallKind::Sample, |_| {
                panic!("sample was dispatched after Activity release")
            });
        assert!(sample.await.is_err());
    }
}
