//! Installed Playback Session transport shared by the device shells.
//!
//! `PlaybackManager` serializes transport ownership, selects the requested
//! Audio Track, and returns a correlated stream handle. Device shells own
//! presentation, visibility signals and the platform screen-wake adapter.

pub mod android;
mod audio_preferences;
mod lifecycle;
mod playback;
mod screen_wake;
mod selected_transport_stream;

pub use lifecycle::LifecycleOrder;
pub use playback::{
    NativeStreamHandle, PlaybackManager, PlaybackManagerError, PlaybackRestartIntent,
    PlaybackSessionId, StartedPlayback,
};
pub use screen_wake::noop_screen_wake;
pub use screen_wake::{ScreenWake, ScreenWakeError};
pub use selected_transport_stream::{
    AudioSelection, AudioSelectionReason, AudioTrack, AudioTrackId, MissingAudioSelection,
    PreferenceStatus, TransportStreamError,
};

pub mod tv;
