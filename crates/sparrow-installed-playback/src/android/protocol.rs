use serde::Deserialize;

use crate::{NativeStreamHandle, PlaybackSessionId};

const MAX_VIEWPORT_COORDINATE: u32 = 32_768;
const MAX_VIEWPORT_EXTENT: u32 = 32_768;
const MAX_SAFE_COUNTER: u64 = 9_007_199_254_740_991;

/// Physical-pixel rectangle occupied by the Android native video surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AndroidPlaybackViewport {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    fullscreen: bool,
}

impl AndroidPlaybackViewport {
    pub fn parse(
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

    pub const fn left(self) -> u32 {
        self.left
    }

    pub const fn top(self) -> u32 {
        self.top
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }

    pub const fn fullscreen(self) -> bool {
        self.fullscreen
    }
}

/// Bounded presentation controls applied together on Android's UI thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AndroidPlaybackControls {
    volume: f32,
    muted: bool,
    paused: bool,
}

impl AndroidPlaybackControls {
    pub fn parse(volume: f64, muted: bool, paused: bool) -> Result<Self, AndroidPlaybackError> {
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err(AndroidPlaybackError);
        }
        Ok(Self {
            volume: volume as f32,
            muted,
            paused,
        })
    }

    pub const fn volume(self) -> f32 {
        self.volume
    }

    pub const fn muted(self) -> bool {
        self.muted
    }

    pub const fn paused(self) -> bool {
        self.paused
    }
}

/// Opaque identity shared by the Rust stream actor and Android DataSource.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AndroidPlaybackIdentity {
    session_id: PlaybackSessionId,
    stream_handle: NativeStreamHandle,
}

impl AndroidPlaybackIdentity {
    pub const fn new(session_id: PlaybackSessionId, stream_handle: NativeStreamHandle) -> Self {
        Self {
            session_id,
            stream_handle,
        }
    }

    pub fn session_id(&self) -> &PlaybackSessionId {
        &self.session_id
    }

    pub fn stream_handle(&self) -> &NativeStreamHandle {
        &self.stream_handle
    }
}

/// Safe aggregate Android player phases returned by Media3.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum AndroidPlaybackPhase {
    Starting,
    Playing,
    Paused,
    Failed,
    Stopped,
}

/// What Media3 is doing with the Audio Track of the transport it was given.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum AndroidPlaybackAudio {
    /// The player has not yet settled what it can play.
    Pending,
    /// The device is decoding the track: with a decoder of its own, or in
    /// the audio output it hands the track to undecoded.
    DeviceDecoder,
    /// The decoder bundled with the app is decoding the track.
    BundledDecoder,
    /// The transport carries an audio track that nothing here can decode.
    Undecodable,
    /// The player found no audio track in the transport.
    Absent,
}

/// Aggregate-only Android player status. It cannot contain provider data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AndroidPlaybackStatus {
    phase: AndroidPlaybackPhase,
    decoded_frames: u64,
    dropped_frames: u64,
    buffered_duration_ms: u64,
    silent: bool,
    audio: AndroidPlaybackAudio,
}

impl AndroidPlaybackStatus {
    pub const fn phase(self) -> AndroidPlaybackPhase {
        self.phase
    }

    pub const fn dropped_frames(self) -> u64 {
        self.dropped_frames
    }

    pub const fn decoded_frames(self) -> u64 {
        self.decoded_frames
    }

    pub const fn buffered_duration_ms(self) -> u64 {
        self.buffered_duration_ms
    }

    pub const fn silent(self) -> bool {
        self.silent
    }

    pub const fn audio(self) -> AndroidPlaybackAudio {
        self.audio
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AndroidPlaybackStatusWire {
    state: AndroidPlaybackPhase,
    decoded_frames: u64,
    dropped_frames: u64,
    buffered_duration_ms: u64,
    silent: bool,
    audio: AndroidPlaybackAudio,
}

/// Privacy-safe Android adapter failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Android native playback is unavailable")]
pub struct AndroidPlaybackError;

impl AndroidPlaybackStatus {
    /// Decodes the allowlisted Media3 status without reflecting malformed input.
    pub fn parse_json(value: &str) -> Result<Self, AndroidPlaybackError> {
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
            audio: status.audio,
        })
    }
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
            AndroidPlaybackStatus::parse_json(
                r#"{"state":"playing","decodedFrames":1200,"droppedFrames":3,"bufferedDurationMs":1250,"silent":true,"audio":"bundled-decoder"}"#,
            ),
            Ok(AndroidPlaybackStatus {
                phase: AndroidPlaybackPhase::Playing,
                decoded_frames: 1_200,
                dropped_frames: 3,
                buffered_duration_ms: 1_250,
                silent: true,
                audio: AndroidPlaybackAudio::BundledDecoder,
            })
        );
        for (wire, audio) in [
            ("pending", AndroidPlaybackAudio::Pending),
            ("device-decoder", AndroidPlaybackAudio::DeviceDecoder),
            ("undecodable", AndroidPlaybackAudio::Undecodable),
            ("absent", AndroidPlaybackAudio::Absent),
        ] {
            assert_eq!(
                AndroidPlaybackStatus::parse_json(&format!(
                    r#"{{"state":"playing","decodedFrames":0,"droppedFrames":0,"bufferedDurationMs":0,"silent":false,"audio":"{wire}"}}"#,
                ))
                .map(AndroidPlaybackStatus::audio),
                Ok(audio)
            );
        }
        assert!(
            AndroidPlaybackStatus::parse_json(
                r#"{"state":"playing","decodedFrames":0,"droppedFrames":0,"bufferedDurationMs":0,"silent":true,"audio":"c2.vendor.decoder"}"#,
            )
            .is_err()
        );
        assert!(
            AndroidPlaybackStatus::parse_json(
                r#"{"state":"playing","decodedFrames":0,"droppedFrames":0,"bufferedDurationMs":0,"silent":true,"audio":"pending","url":"https://provider.invalid/private"}"#,
            )
            .is_err()
        );
        assert!(
            AndroidPlaybackStatus::parse_json(
                r#"{"state":"unknown","decodedFrames":0,"droppedFrames":0,"bufferedDurationMs":0,"silent":true,"audio":"pending"}"#,
            )
            .is_err()
        );
        assert!(
            AndroidPlaybackStatus::parse_json(&format!(
                r#"{{"state":"playing","decodedFrames":0,"droppedFrames":{},"bufferedDurationMs":0,"silent":true,"audio":"pending"}}"#,
                MAX_SAFE_COUNTER + 1
            ))
            .is_err()
        );
    }
}
