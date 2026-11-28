use std::{
    ffi::{CStr, c_char, c_double, c_int, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr::{NonNull, copy_nonoverlapping},
    sync::{
        Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::Thread,
    time::{Duration, Instant},
};

use sparrow_installed_playback::{
    PlaybackManager,
    android::{AndroidPlaybackControls, AndroidPlaybackError, AndroidPlaybackIdentity},
};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use crate::{Backend, frame::FrameSize};

#[repr(C)]
struct NativeMpv {
    _opaque: [u8; 0],
}

#[repr(C)]
struct NativeRenderer {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn sparrow_mpv_create(
        stream: *mut c_void,
        read: extern "C" fn(*mut c_void, *mut c_char, u64) -> i64,
        cancel: extern "C" fn(*mut c_void),
        wake_context: *mut c_void,
        wake: extern "C" fn(*mut c_void),
        backend: c_int,
    ) -> *mut NativeMpv;
    fn sparrow_mpv_destroy(mpv: *mut NativeMpv);
    fn sparrow_mpv_load(mpv: *mut NativeMpv) -> c_int;
    fn sparrow_mpv_controls(
        mpv: *mut NativeMpv,
        volume: c_double,
        mute: c_int,
        pause: c_int,
    ) -> c_int;
    fn sparrow_mpv_event(mpv: *mut NativeMpv) -> c_int;
    fn sparrow_mpv_create_render(
        mpv: *mut NativeMpv,
        wake_context: *mut c_void,
        wake: extern "C" fn(*mut c_void),
        backend: c_int,
    ) -> *mut NativeRenderer;
    fn sparrow_mpv_free_render(renderer: *mut NativeRenderer);
    fn sparrow_mpv_update(renderer: *mut NativeRenderer, delay_ns: *mut i64) -> c_int;
    fn sparrow_mpv_render(
        renderer: *mut NativeRenderer,
        width: c_int,
        height: c_int,
        stride: usize,
        pixels: *mut c_void,
    ) -> c_int;
    fn sparrow_mpv_integer(mpv: *mut NativeMpv, name: *const c_char) -> i64;
    fn sparrow_mpv_number(mpv: *mut NativeMpv, name: *const c_char) -> c_double;
    fn sparrow_mpv_flag(mpv: *mut NativeMpv, name: *const c_char) -> c_int;
    fn sparrow_mpv_string_is(
        mpv: *mut NativeMpv,
        name: *const c_char,
        expected: *const c_char,
    ) -> c_int;
    fn sparrow_mpv_has_string(mpv: *mut NativeMpv, name: *const c_char) -> c_int;
    fn sparrow_mpv_private_path(mpv: *mut NativeMpv) -> c_int;
}

struct Stream {
    playback: Weak<PlaybackManager>,
    runtime: Handle,
    identity: AndroidPlaybackIdentity,
    cancelled: CancellationToken,
    failed: AtomicBool,
    worker: Thread,
}

extern "C" fn read(opaque: *mut c_void, output: *mut c_char, requested: u64) -> i64 {
    if requested == 0 {
        return 0;
    }
    // libmpv keeps this pointer live through termination, and exclusively
    // owns the writable output for the duration of its read callback.
    let stream = unsafe { &*opaque.cast::<Stream>() };
    catch_unwind(AssertUnwindSafe(|| {
        let mut buffer = [0; 64 * 1024];
        let maximum = requested.min(buffer.len() as u64) as usize;
        let Some(playback) = stream.playback.upgrade() else {
            return -1;
        };
        let result = stream.runtime.block_on(async {
            tokio::select! {
                biased;
                _ = stream.cancelled.cancelled() => None,
                result = playback.read_native_into(
                    stream.identity.session_id(),
                    stream.identity.stream_handle(),
                    &mut buffer[..maximum],
                ) => Some(result),
            }
        });
        match result {
            Some(Ok(filled)) => {
                // The mpv buffer may be uninitialized. Only publish the bytes read.
                unsafe { copy_nonoverlapping(buffer.as_ptr(), output.cast::<u8>(), filled) };
                filled as i64
            }
            Some(Err(_)) => {
                stream.failed.store(true, Ordering::Release);
                -1
            }
            None => -1,
        }
    }))
    .unwrap_or(-1)
}

extern "C" fn cancel(opaque: *mut c_void) {
    // libmpv may cancel concurrently with a read. Only the token is mutated.
    unsafe { &*opaque.cast::<Stream>() }.cancelled.cancel();
}

extern "C" fn wake(opaque: *mut c_void) {
    // Both callbacks keep their registration alive until destroy returns.
    unsafe { &*opaque.cast::<Thread>() }.unpark();
}

pub(crate) enum Event {
    Pending,
    Other,
    Ended,
    Failed,
}

/// The client API is confined to the presentation command worker.
pub(crate) struct Engine {
    handle: NonNull<NativeMpv>,
    stream: Box<Stream>,
}

impl Engine {
    pub(crate) fn new(
        playback: Weak<PlaybackManager>,
        runtime: Handle,
        identity: AndroidPlaybackIdentity,
        backend: Backend,
    ) -> Result<Self, AndroidPlaybackError> {
        let stream = Box::new(Stream {
            playback,
            runtime,
            identity,
            cancelled: CancellationToken::new(),
            failed: AtomicBool::new(false),
            worker: std::thread::current(),
        });
        let context = std::ptr::from_ref(&*stream).cast_mut().cast();
        let wake_context = std::ptr::from_ref(&stream.worker).cast_mut().cast();
        // The callback context is pinned by its Box until after destroy.
        let handle = unsafe {
            sparrow_mpv_create(context, read, cancel, wake_context, wake, backend as c_int)
        };
        Ok(Self {
            handle: NonNull::new(handle).ok_or(AndroidPlaybackError)?,
            stream,
        })
    }

    pub(crate) fn load(&self) -> Result<(), AndroidPlaybackError> {
        check(unsafe { sparrow_mpv_load(self.handle.as_ptr()) })
    }

    pub(crate) fn render_handle(&self) -> RenderHandle {
        RenderHandle(self.handle)
    }

    pub(crate) fn cancel_reads(&self) {
        self.stream.cancelled.cancel();
    }

    pub(crate) fn controls(
        &self,
        controls: AndroidPlaybackControls,
    ) -> Result<(), AndroidPlaybackError> {
        check(unsafe {
            sparrow_mpv_controls(
                self.handle.as_ptr(),
                f64::from(controls.volume()) * 100.0,
                controls.muted().into(),
                controls.paused().into(),
            )
        })
    }

    pub(crate) fn event(&self) -> Event {
        match unsafe { sparrow_mpv_event(self.handle.as_ptr()) } {
            0 => Event::Pending,
            2 if !self.stream.failed.load(Ordering::Acquire) => Event::Ended,
            2 | 3 => Event::Failed,
            _ => Event::Other,
        }
    }

    pub(crate) fn integer(&self, name: &CStr) -> Option<u64> {
        u64::try_from(unsafe { sparrow_mpv_integer(self.handle.as_ptr(), name.as_ptr()) }).ok()
    }

    pub(crate) fn number(&self, name: &CStr) -> f64 {
        unsafe { sparrow_mpv_number(self.handle.as_ptr(), name.as_ptr()) }
    }

    pub(crate) fn flag(&self, name: &CStr) -> bool {
        unsafe { sparrow_mpv_flag(self.handle.as_ptr(), name.as_ptr()) != 0 }
    }

    pub(crate) fn has_string(&self, name: &CStr) -> bool {
        unsafe { sparrow_mpv_has_string(self.handle.as_ptr(), name.as_ptr()) != 0 }
    }

    pub(crate) fn string_is(&self, name: &CStr, value: &CStr) -> bool {
        unsafe { sparrow_mpv_string_is(self.handle.as_ptr(), name.as_ptr(), value.as_ptr()) != 0 }
    }

    pub(crate) fn private_path(&self) -> bool {
        unsafe { sparrow_mpv_private_path(self.handle.as_ptr()) != 0 }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Interrupt a provider read before libmpv waits for its demux thread.
        self.stream.cancelled.cancel();
        unsafe { sparrow_mpv_destroy(self.handle.as_ptr()) };
    }
}

/// Moved once to the render worker. The Engine is kept alive until that worker
/// has joined. libmpv permits calling the render API independently of the
/// thread using the client API. No client operations use this handle.
pub(crate) struct RenderHandle(NonNull<NativeMpv>);

// SAFETY: RenderHandle only creates one renderer for a live core. The client
// joins that renderer before dropping the core or its callback state.
unsafe impl Send for RenderHandle {}

impl RenderHandle {
    pub(crate) fn create(self, backend: Backend) -> Result<Renderer, AndroidPlaybackError> {
        let worker = Box::new(std::thread::current());
        let wake_context = std::ptr::from_ref(&*worker).cast_mut().cast();
        let handle = unsafe {
            sparrow_mpv_create_render(self.0.as_ptr(), wake_context, wake, backend as c_int)
        };
        Ok(Renderer {
            handle: NonNull::new(handle).ok_or(AndroidPlaybackError)?,
            _worker: worker,
            scratch: Vec::new(),
        })
    }
}

#[derive(PartialEq, Eq)]
pub(crate) enum Update {
    None,
    Frame(Duration),
    Redraw,
}

pub(crate) struct Renderer {
    handle: NonNull<NativeRenderer>,
    _worker: Box<Thread>,
    scratch: Vec<u8>,
}

impl Renderer {
    pub(crate) fn updated(&self) -> Update {
        let mut delay_ns = 0;
        match unsafe { sparrow_mpv_update(self.handle.as_ptr(), &mut delay_ns) } {
            0 => Update::None,
            2 => Update::Frame(Duration::from_nanos(delay_ns.max(0) as u64)),
            _ => Update::Redraw,
        }
    }

    pub(crate) fn draw(
        &mut self,
        size: FrameSize,
        bgra: &mut Vec<u8>,
    ) -> Result<Duration, AndroidPlaybackError> {
        let row_bytes = size.width as usize * 4;
        let stride = row_bytes.next_multiple_of(64);
        let height = size.height as usize;
        self.scratch.resize(stride * height + 63, 0);
        let offset = self.scratch.as_ptr().align_offset(64);
        let target = &mut self.scratch[offset..offset + stride * height];
        let started = Instant::now();
        check(unsafe {
            sparrow_mpv_render(
                self.handle.as_ptr(),
                size.width as c_int,
                size.height as c_int,
                stride,
                target.as_mut_ptr().cast(),
            )
        })?;
        let native_render_time = started.elapsed();
        bgra.resize(row_bytes * height, 0);
        // The SW API's fourth byte is unspecified. GPUI's BGRA expects opacity.
        for (source, output) in target
            .chunks_exact(stride)
            .zip(bgra.chunks_exact_mut(row_bytes))
        {
            let (source, _) = source[..row_bytes].as_chunks::<4>();
            let (output, _) = output.as_chunks_mut::<4>();
            for (&[blue, green, red, _], pixel) in source.iter().zip(output) {
                *pixel = [blue, green, red, 255];
            }
        }
        Ok(native_render_time)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe { sparrow_mpv_free_render(self.handle.as_ptr()) };
    }
}

fn check(code: c_int) -> Result<(), AndroidPlaybackError> {
    if code >= 0 {
        Ok(())
    } else {
        Err(AndroidPlaybackError)
    }
}
