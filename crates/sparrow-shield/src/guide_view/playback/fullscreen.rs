//! Fullscreen and band weights also describe the three picture rectangles.
use std::time::Instant;

use gpui::Window;

#[derive(Default, Clone, Copy, PartialEq)]
pub(crate) struct FullscreenFrame {
    pub(crate) fullscreen: f32,
    pub(crate) band: f32,
}

#[derive(Default)]
pub(crate) struct FullscreenMotion {
    target: FullscreenFrame,
    flight: Option<(Instant, FullscreenFrame)>,
}

impl FullscreenMotion {
    pub(crate) fn frame(
        &mut self,
        fullscreen: bool,
        controls: bool,
        instant: bool,
        window: &mut Window,
    ) -> FullscreenFrame {
        let target = FullscreenFrame {
            fullscreen: f32::from(fullscreen),
            band: f32::from(fullscreen && controls),
        };
        let now = Instant::now();
        let current = self.flight.map_or(self.target, |(start, from)| {
            // Matches ShieldPlaybackPlacement's 500 ms cubic ease-out.
            let t = (now.duration_since(start).as_secs_f32() / 0.5).min(1.);
            let remaining = (1. - t).powi(3);
            FullscreenFrame {
                fullscreen: self.target.fullscreen
                    + (from.fullscreen - self.target.fullscreen) * remaining,
                band: self.target.band + (from.band - self.target.band) * remaining,
            }
        });
        if self.target != target {
            self.flight = Some((now, current));
            self.target = target;
        }
        if instant || current == target {
            self.flight = None;
            return target;
        }
        window.request_animation_frame();
        current
    }
}
