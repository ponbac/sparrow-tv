//! A single sample drives both the native viewport and the Guide chrome.
use std::time::Instant;

use gpui::Window;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct FullscreenFrame {
    pub(crate) fullscreen: f32,
    pub(crate) band: f32,
    pub(crate) caption: f32,
    pub(crate) stripe: f32,
}

impl Default for FullscreenFrame {
    fn default() -> Self {
        Self {
            fullscreen: 0.,
            band: 0.,
            caption: 1.,
            stripe: 1.,
        }
    }
}

struct Transition {
    start: Instant,
    from: FullscreenFrame,
    seconds: f32,
    power: i32,
}

#[derive(Default)]
pub(crate) struct FullscreenMotion {
    target: FullscreenFrame,
    flight: Option<Transition>,
    cut_from: Option<FullscreenFrame>,
}

impl FullscreenMotion {
    pub(crate) fn retarget(&mut self, fullscreen: bool, controls: bool, scale: f32) {
        let dock = f32::from(!fullscreen);
        let target = FullscreenFrame {
            fullscreen: f32::from(fullscreen),
            band: f32::from(fullscreen && controls),
            caption: dock,
            stripe: dock,
        };
        if scale <= 0. {
            if target.fullscreen != self.target.fullscreen {
                self.cut_from = Some(self.sample(Instant::now()));
            }
            self.flight = None;
        } else if self.target != target {
            self.cut_from = None;
            let (seconds, power) = if target.fullscreen != self.target.fullscreen {
                if fullscreen { (0.48, 3) } else { (0.39, 3) }
            } else if controls {
                (0.28, 4)
            } else {
                (0.48, 3)
            };
            let start = Instant::now();
            self.flight = Some(Transition {
                start,
                from: self.sample(start),
                seconds: seconds * scale,
                power,
            });
        }
        self.target = target;
    }

    fn sample(&self, now: Instant) -> FullscreenFrame {
        let Some(flight) = &self.flight else {
            return self.target;
        };
        let t = (now.duration_since(flight.start).as_secs_f32() / flight.seconds).min(1.);
        let ease = 1. - (1. - t).powi(flight.power);
        let mix = |from, to, weight: f32| from * (1. - weight) + to * weight;
        let chrome = |from, early| {
            let weight = if self.target.fullscreen > 0. {
                t / early
            } else {
                (t - 0.72) / 0.28
            };
            mix(from, self.target.caption, weight.clamp(0., 1.))
        };
        FullscreenFrame {
            fullscreen: mix(flight.from.fullscreen, self.target.fullscreen, ease),
            band: mix(flight.from.band, self.target.band, ease),
            caption: chrome(flight.from.caption, 0.28),
            stripe: chrome(flight.from.stripe, 0.10),
        }
    }

    pub(crate) fn frame(
        &mut self,
        instant: bool,
        window: &mut Window,
    ) -> (FullscreenFrame, FullscreenFrame) {
        if instant {
            self.flight = None;
        }
        // Across a cut, clear the Guide before expanding the native surface,
        // and shrink the surface before restoring the opaque Guide.
        if let Some(from) = self.cut_from.take() {
            window.request_animation_frame();
            return if self.target.fullscreen > 0. {
                (self.target, from)
            } else {
                (from, self.target)
            };
        }
        let frame = self.sample(Instant::now());
        if frame == self.target {
            self.flight = None;
        } else {
            window.request_animation_frame();
        }
        (frame, frame)
    }
}
