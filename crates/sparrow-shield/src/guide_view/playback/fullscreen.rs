//! A single sample drives both the native viewport and the Guide chrome.
use std::time::{Duration, Instant};

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

/// Dock-to-fullscreen and back. Kept short: the native picture pauses on a
/// still while it flies.
const ENTER_SECONDS: f32 = 0.36;
const EXIT_SECONDS: f32 = 0.30;

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
    /// When the latest dock-to-fullscreen leg ends. A band change during the
    /// leg keeps this deadline.
    leg_ends: Option<Instant>,
}

impl FullscreenMotion {
    pub(crate) fn retarget(&mut self, fullscreen: bool, controls: bool, cut: bool) {
        let dock = f32::from(!fullscreen);
        let target = FullscreenFrame {
            fullscreen: f32::from(fullscreen),
            band: f32::from(fullscreen && controls),
            caption: dock,
            stripe: dock,
        };
        if cut {
            if target.fullscreen != self.target.fullscreen {
                self.cut_from = Some(self.sample(Instant::now()));
            }
            self.flight = None;
            self.leg_ends = None;
        } else if self.target != target {
            self.cut_from = None;
            let (seconds, power) = if target.fullscreen != self.target.fullscreen {
                if fullscreen {
                    (ENTER_SECONDS, 3)
                } else {
                    (EXIT_SECONDS, 3)
                }
            } else if controls {
                (0.28, 4)
            } else {
                (0.48, 3)
            };
            let start = Instant::now();
            if target.fullscreen != self.target.fullscreen {
                self.leg_ends = Some(start + Duration::from_secs_f32(seconds));
            }
            self.flight = Some(Transition {
                start,
                from: self.sample(start),
                seconds,
                power,
            });
        }
        self.target = target;
    }

    /// Whether the picture is on its way between the dock and fullscreen.
    /// Moving the native surface every frame stalls the Shield's compositor, so
    /// GPUI flies a still instead. Band motion outside that leg, including the
    /// rest of a band change begun during it, still moves the native surface.
    pub(crate) fn flying(&self) -> bool {
        self.flight.is_some() && self.leg_ends.is_some_and(|ends| Instant::now() < ends)
    }

    fn sample(&self, now: Instant) -> FullscreenFrame {
        let Some(flight) = &self.flight else {
            return self.target;
        };
        let t = now.duration_since(flight.start).as_secs_f32() / flight.seconds;
        // The late caption ramp can round just below one. Returning the exact
        // endpoint lets frame() retire the transition and stop requesting draws.
        if t >= 1. {
            return self.target;
        }
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
