use std::time::Instant;

use gpui::{Div, Window, prelude::*, rgb};
use sparrow_tv_guide::guide_screen::{GuideMode, LaneFocus};

use super::{WASH, lane::group_pill_index};

const RAPID_CHANGE_MS: u128 = 240;
const INK_MS: f32 = 350.;
const TRAVEL_MS: f32 = 360.;
const FADE_MS: f32 = 260.;
const OPACITY_FLOOR: f32 = 0.25;
const ROW_DELAY_MS: f32 = 20.;
const MAX_STAGGER_ROW: usize = 5;
const TOTAL_MS: f32 = TRAVEL_MS + MAX_STAGGER_ROW as f32 * ROW_DELAY_MS;

#[derive(Default)]
pub(crate) struct GroupSlide {
    last_change: Option<Instant>,
    transition: Option<Transition>,
}

struct Transition {
    direction: f32,
    rows_started: Option<Instant>,
}

impl GroupSlide {
    pub(crate) fn press(
        &mut self,
        previous: (GuideMode, LaneFocus),
        current: (GuideMode, LaneFocus),
        group_changed: bool,
        held: bool,
    ) {
        self.transition = None;
        if previous.0 == current.0 && !group_changed {
            return;
        }
        let now = Instant::now();
        let rapid = self
            .last_change
            .replace(now)
            .is_some_and(|last| now.duration_since(last).as_millis() < RAPID_CHANGE_MS);
        if !held && !rapid {
            let forward = if previous.0 == current.0 {
                group_pill_index(current.1) >= group_pill_index(previous.1)
            } else {
                current.0 == GuideMode::Timeline
            };
            self.transition = Some(Transition {
                direction: if forward { 1. } else { -1. },
                rows_started: None,
            });
        }
    }

    pub(crate) fn frame(&mut self, window: &Window, reduced: bool, has_rows: bool) -> SlideFrame {
        if reduced {
            self.transition = None;
        }
        let (Some(transition), Some(change)) = (&mut self.transition, self.last_change) else {
            return SlideFrame::SETTLED;
        };
        let ink_ms = change.elapsed().as_secs_f32() * 1000.;
        if has_rows {
            transition.rows_started.get_or_insert_with(Instant::now);
        }
        let ms = transition
            .rows_started
            .map_or(0., |start| start.elapsed().as_secs_f32() * 1000.);
        if ms >= TOTAL_MS {
            self.transition = None;
            return SlideFrame::SETTLED;
        }
        if ink_ms < INK_MS || transition.rows_started.is_some() {
            window.request_animation_frame();
        }
        SlideFrame {
            ms,
            ink_ms,
            direction: transition.direction,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct SlideFrame {
    ms: f32,
    ink_ms: f32,
    direction: f32,
}

impl SlideFrame {
    pub(crate) const SETTLED: Self = Self {
        ms: TOTAL_MS,
        ink_ms: INK_MS,
        direction: 0.,
    };

    pub(super) fn item(self, row: usize, travel: f32, focused: bool) -> (f32, f32) {
        if focused {
            return (0., 1.);
        }
        let elapsed = self.ms - row.min(MAX_STAGGER_ROW) as f32 * ROW_DELAY_MS;
        let remaining = |duration: f32| (1. - (elapsed / duration).clamp(0., 1.)).powi(5);
        (
            travel * self.direction * remaining(TRAVEL_MS),
            1. - (1. - OPACITY_FLOOR) * remaining(FADE_MS),
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
        let remaining = (1. - slide.ink_ms / INK_MS).max(0.).powi(5);
        let bounds = std::array::from_fn(|i| target[i] + (self.from[i] - target[i]) * remaining);
        self.last = Some((selected, bounds));
        bounds
    }

    pub(super) fn pill(rect: Div) -> Div {
        rect.rounded_full().bg(rgb(WASH))
    }
}
