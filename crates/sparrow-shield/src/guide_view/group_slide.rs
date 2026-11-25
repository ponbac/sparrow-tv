use std::time::Instant;

use gpui::{Div, Window, prelude::*, rgb};
use sparrow_tv_guide::guide_screen::{GuideMode, LaneFocus};

use super::{CELL_ELAPSED, lane::group_pill_index};

#[derive(Default)]
pub(crate) struct GroupSlide {
    last_press: Option<Instant>,
    transition: Option<(Instant, f32)>,
}

impl GroupSlide {
    pub(crate) fn press(
        &mut self,
        previous: (GuideMode, LaneFocus),
        current: (GuideMode, LaneFocus),
        held: bool,
    ) {
        let now = Instant::now();
        let rapid = self
            .last_press
            .replace(now)
            .is_some_and(|press| now.duration_since(press).as_millis() < 240);
        let direction = if previous.0 != current.0 {
            if current.0 == GuideMode::Timeline {
                1.
            } else {
                -1.
            }
        } else {
            match group_pill_index(current.1).cmp(&group_pill_index(previous.1)) {
                std::cmp::Ordering::Greater => 1.,
                std::cmp::Ordering::Less => -1.,
                std::cmp::Ordering::Equal => 0.,
            }
        };
        self.transition = (direction != 0. && !held && !rapid).then_some((now, direction));
    }

    pub(crate) fn frame(&mut self, window: &Window, reduced: bool) -> SlideFrame {
        if let Some((start, direction)) = self.transition {
            let ms = start.elapsed().as_secs_f32() * 1000.;
            if !reduced && ms < 500. {
                window.request_animation_frame();
                return SlideFrame { ms, direction };
            }
        }
        self.transition = None;
        SlideFrame {
            ms: 500.,
            direction: 0.,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SlideFrame {
    ms: f32,
    direction: f32,
}

impl SlideFrame {
    pub(super) fn item(self, ordinal: usize, focused: bool) -> (f32, f32) {
        let delay = ordinal.min(5) as f32 * 25.;
        let remaining = (1. - ((self.ms - delay) / 375.).clamp(0., 1.)).powi(3);
        (
            110. * self.direction * remaining,
            if focused { 1. } else { 1. - remaining },
        )
    }
}

// The ink starts from its last painted bounds in viewport coordinates. The
// destination comes from the current layout, after the viewport reveals focus.
#[derive(Default)]
pub(super) struct GroupInk {
    last: Option<(usize, [f32; 2])>,
    from: [f32; 2],
}

impl GroupInk {
    pub(super) fn bounds(
        &mut self,
        selected: usize,
        target: [f32; 2],
        slide: SlideFrame,
    ) -> [f32; 2] {
        if self.last.is_none_or(|(index, _)| index != selected) {
            self.from = self.last.map_or(target, |(_, bounds)| bounds);
        }
        let remaining = (1. - slide.ms / 400.).max(0.).powi(3);
        let bounds = std::array::from_fn(|i| target[i] + (self.from[i] - target[i]) * remaining);
        self.last = Some((selected, bounds));
        bounds
    }

    pub(super) fn pill(rect: Div) -> Div {
        rect.rounded_full().bg(rgb(CELL_ELAPSED))
    }
}
