use std::time::Instant;

use gpui::{Background, BoxShadow, Window, linear_color_stop, linear_gradient, rgb, rgba};
use sparrow_tv_guide::{
    guide_screen::ScreenFocus, now_focus::NowFocus, timeline_focus::TimelineFocus,
};

use super::Layout;

#[derive(Default)]
pub(crate) struct FocusLift {
    last_key: Option<Instant>,
    rapid: bool,
    target: Option<ScreenFocus>,
}

impl FocusLift {
    pub(crate) fn key_down(&mut self, held: bool) {
        let now = Instant::now();
        self.rapid = held
            || self
                .last_key
                .is_some_and(|last| now.duration_since(last).as_millis() < 240);
        self.last_key = Some(now);
        self.target = None;
    }

    pub(crate) fn moved(&mut self, before: ScreenFocus, after: ScreenFocus) {
        if !self.rapid
            && before != after
            && matches!(
                after,
                ScreenFocus::Board(NowFocus::Channel(_))
                    | ScreenFocus::TimelineBoard(TimelineFocus::Programme { .. })
            )
        {
            self.target = Some(after);
        }
    }

    pub(crate) fn frame(
        &mut self,
        focus: ScreenFocus,
        reduced: bool,
        window: &Window,
    ) -> LiftFrame {
        if !reduced
            && self.target == Some(focus)
            && let Some(start) = self.last_key
        {
            let elapsed = start.elapsed().as_secs_f32();
            if elapsed < 0.9 {
                window.request_animation_frame();
                return LiftFrame {
                    amount: 1. - (1. - (elapsed / 0.2).min(1.)).powi(3),
                    sheen: (elapsed >= 0.2).then_some((elapsed - 0.2) / 0.7),
                };
            }
        }
        self.target = None;
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
    pub(super) fn shadow(self, layout: Layout, depth: f32) -> BoxShadow {
        let depth = depth * self.amount;
        BoxShadow::new(layout.px(0.), layout.px(depth), rgba(0x00000099).into())
            .blur_radius(layout.px(depth * 2.))
    }

    // Both ramps share the shape's coordinates. The second restores the base
    // colour beyond the crest, including when painting the elapsed portion.
    pub(super) fn sheen(self, base: u32, aspect_ratio: f32) -> Option<[Background; 2]> {
        let center = self.sheen? * 1.36 - 0.18;
        // GPUI normalizes the gradient direction by the aspect ratio. Keep
        // the visible streak tilted even on very wide Programme cells.
        let angle = 90. + (0.36 * aspect_ratio).atan().to_degrees();
        Some([
            linear_gradient(
                angle,
                linear_color_stop(rgba(0xffffff00), center - 0.18),
                linear_color_stop(rgba(0xffffff85), center),
            ),
            linear_gradient(
                angle,
                linear_color_stop(rgba(base << 8), center),
                linear_color_stop(rgb(base), center + 0.18),
            ),
        ])
    }
}
