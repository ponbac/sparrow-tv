//! Shared Android presentation protocol, ordering, and Media3 byte reads.

mod gate;
mod protocol;
mod reader;

pub use gate::AndroidPresentationGate;
pub use protocol::{
    AndroidPlaybackAudio, AndroidPlaybackControls, AndroidPlaybackError, AndroidPlaybackIdentity,
    AndroidPlaybackPhase, AndroidPlaybackStatus, AndroidPlaybackViewport,
};
pub use reader::AndroidMedia3Reader;
