use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Weak,
};

use tokio::runtime::Handle;

use crate::{PlaybackManager, playback::MAX_NATIVE_PULL_BYTES};

use super::{AndroidPlaybackError, AndroidPlaybackIdentity};

const MAX_COALESCED_ACTOR_READS: usize = 16;

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
                let mut returned = Vec::with_capacity(maximum);
                for _ in 0..MAX_COALESCED_ACTOR_READS {
                    // Unread bytes stay with the actor's exact StreamInstance.
                    // A retired loader cannot serve or overwrite another stream.
                    let next = playback
                        .read_at_most(
                            identity.session_id().clone(),
                            identity.stream_handle().clone(),
                            maximum - returned.len(),
                        )
                        .await;
                    match next {
                        Ok(bytes) if bytes.is_empty() => break,
                        Ok(bytes) => returned.extend_from_slice(&bytes),
                        Err(_) if !returned.is_empty() => break,
                        Err(_) => return Err(AndroidPlaybackError),
                    }
                    if returned.len() == maximum {
                        break;
                    }
                }
                Ok(returned)
            })
        }))
        .map_err(|_| AndroidPlaybackError)?
    }
}
