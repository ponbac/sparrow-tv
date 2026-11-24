use std::sync::Arc;

/// The host could not apply a playback-owned screen-wake flag.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("the host could not apply the playback screen-wake flag")]
pub struct ScreenWakeError;

/// Applies the playback-owned screen-wake flag on the device.
pub trait ScreenWake: Send + Sync + 'static {
    /// Applies the latest playback-owned wake intent. Calls must be idempotent.
    fn set_active(&self, active: bool) -> Result<(), ScreenWakeError>;
}

struct NoopScreenWake;

impl ScreenWake for NoopScreenWake {
    fn set_active(&self, _active: bool) -> Result<(), ScreenWakeError> {
        Ok(())
    }
}

/// Wake adapter for hosts without a playback-owned screen flag.
pub fn noop_screen_wake() -> Arc<dyn ScreenWake> {
    Arc::new(NoopScreenWake)
}
