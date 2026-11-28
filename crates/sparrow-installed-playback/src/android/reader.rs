use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Weak,
};

use tokio::runtime::Handle;

use crate::{PlaybackManager, playback::MAX_NATIVE_PULL_BYTES};

use super::{AndroidPlaybackError, AndroidPlaybackIdentity};

/// Bounded byte pulls for Media3's dedicated loader thread.
pub struct AndroidMedia3Reader {
    runtime: Handle,
    playback: Weak<PlaybackManager>,
}

impl AndroidMedia3Reader {
    pub fn new(runtime: Handle, playback: Weak<PlaybackManager>) -> Self {
        Self { runtime, playback }
    }

    /// Returns at most the requested bytes and 64 KiB. Empty bytes mean EOF.
    /// Must run on the loader thread, outside the async executor and Android UI.
    pub fn read(
        &self,
        identity: AndroidPlaybackIdentity,
        requested: usize,
    ) -> Result<Vec<u8>, AndroidPlaybackError> {
        if requested == 0 {
            return Err(AndroidPlaybackError);
        }
        let maximum = requested.min(MAX_NATIVE_PULL_BYTES);
        let playback = self.playback.upgrade().ok_or(AndroidPlaybackError)?;
        catch_unwind(AssertUnwindSafe(|| {
            self.runtime.block_on(async move {
                let mut buffer = vec![0; maximum];
                let filled = playback
                    .read_native_into(identity.session_id(), identity.stream_handle(), &mut buffer)
                    .await
                    .map_err(|_| AndroidPlaybackError)?;
                buffer.truncate(filled);
                Ok(buffer)
            })
        }))
        .map_err(|_| AndroidPlaybackError)?
    }
}
