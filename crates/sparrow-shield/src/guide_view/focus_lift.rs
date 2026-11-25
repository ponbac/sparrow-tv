use std::time::{Duration, Instant};

use gpui::{Background, BoxShadow, Window, linear_color_stop, linear_gradient, rgb, rgba};
use sparrow_tv_guide::{
    guide_screen::ScreenFocus, now_focus::NowFocus, timeline_focus::TimelineFocus,
};

use super::Layout;

const LIFT: Duration = Duration::from_millis(200);
const SETTLE: Duration = Duration::from_millis(240);
const SWEEP: Duration = Duration::from_millis(700);

#[derive(Default)]
pub(crate) struct FocusLift {
    last_key: Option<Instant>,
    rapid: bool,
    target: Option<(ScreenFocus, Instant)>,
}

impl FocusLift {
    pub(crate) fn key_down(&mut self, held: bool) {
        let now = Instant::now();
        self.rapid = held
            || self
                .last_key
                .is_some_and(|last| now.duration_since(last) < SETTLE);
        self.last_key = Some(now);
        if self.rapid {
            self.target = None;
        }
    }

    pub(crate) fn moved(&mut self, before: ScreenFocus, after: ScreenFocus) {
        if before == after {
            return;
        }
        self.target = if !self.rapid
            && matches!(
                after,
                ScreenFocus::Board(NowFocus::Channel(_))
                    | ScreenFocus::TimelineBoard(TimelineFocus::Programme { .. })
            ) {
            self.last_key.map(|start| (after, start))
        } else {
            None
        };
    }

    pub(crate) fn frame(&self, focus: ScreenFocus, reduced: bool, window: &Window) -> LiftFrame {
        if !reduced
            && let Some((target, start)) = self.target
            && target == focus
        {
            let elapsed = start.elapsed();
            if elapsed < SETTLE + SWEEP {
                window.request_animation_frame();
                return LiftFrame {
                    amount: 1. - (1. - (elapsed.div_duration_f32(LIFT)).min(1.)).powi(3),
                    sheen: elapsed
                        .checked_sub(SETTLE)
                        .map(|t| t.div_duration_f32(SWEEP)),
                };
            }
        }
        LiftFrame {
            amount: 1.,
            sheen: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct LiftFrame {
    pub(super) amount: f32,
    sheen: Option<f32>,
}

impl LiftFrame {
    pub(super) fn shadow(self, layout: Layout, depth: f32, blur: f32, alpha: u32) -> BoxShadow {
        let depth = depth * self.amount;
        BoxShadow::new(layout.px(0.), layout.px(depth), rgba(alpha).into())
            .blur_radius(layout.px(blur * self.amount))
    }

    // Both ramps share the shape's coordinates. The second restores the base
    // colour beyond the crest, including when painting the elapsed portion.
    pub(super) fn sheen(self, base: u32, width: f32, height: f32) -> Option<[Background; 2]> {
        let u = self.sheen?;
        if width <= height || width <= 90. {
            return None;
        }
        let aspect_ratio = width / height;
        let tilt = 0.36_f32;
        // Project the four corners onto the shader's normalized (1, tilt) axis.
        let extent = 0.5 * (1. + tilt / aspect_ratio) / 1_f32.hypot(tilt);
        let half_band = (75. / width).clamp(0.08, 0.28);
        let progress = 1. - (1. - u).powi(3);
        let center = 0.5 + (2. * progress - 1.) * (extent + half_band);
        let alpha = (88. * (std::f32::consts::PI * u).sin().max(0.).sqrt()) as u32;
        // GPUI normalizes the gradient direction by the aspect ratio. Keep
        // the visible streak tilted even on very wide Programme cells.
        let angle = 90. + (tilt * aspect_ratio).atan().to_degrees();
        Some([
            linear_gradient(
                angle,
                linear_color_stop(rgba(0xffffff00), center - half_band),
                linear_color_stop(rgba(0xffffff00 | alpha), center),
            ),
            linear_gradient(
                angle,
                linear_color_stop(rgba(base << 8), center),
                linear_color_stop(rgb(base), center + half_band),
            ),
        ])
    }
}
