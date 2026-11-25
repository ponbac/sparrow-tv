use std::time::Instant;

use chrono::{DateTime, FixedOffset, Utc};
use gpui::{Div, Window, div, prelude::*};
use sparrow_core::ChannelId;
use sparrow_tv_guide::{guide_screen::GuideScreen, time::programme_at};

use super::{Layout, SAIRA_CLOCK, clock_time};

pub(crate) struct MinuteTick {
    previous: DateTime<FixedOffset>,
    started: Instant,
    pub(crate) rollovers: Vec<ChannelId>,
    pub(crate) interrupted: bool,
}

impl MinuteTick {
    pub(crate) fn new(
        previous: DateTime<FixedOffset>,
        now: DateTime<FixedOffset>,
        board: Option<&GuideScreen>,
    ) -> Self {
        let mut rollovers = Vec::new();
        if let Some(screen) = board {
            for index in screen.visible_families(0) {
                let (_, source) =
                    screen.families()[index].preferred_with_schedule(screen.variant_preferences());
                let current = programme_at(source.programmes(), now.to_utc());
                if current.is_some()
                    && current != programme_at(source.programmes(), previous.to_utc())
                {
                    rollovers.push(source.channel().id().clone());
                }
            }
        }
        Self {
            previous,
            started: Instant::now(),
            rollovers,
            interrupted: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct MinuteFrame<'a> {
    pub(crate) now: DateTime<FixedOffset>,
    tick: Option<&'a MinuteTick>,
    seconds: f32,
    motion: bool,
}

impl<'a> MinuteFrame<'a> {
    pub(crate) fn new(
        now: DateTime<FixedOffset>,
        tick: Option<&'a MinuteTick>,
        motion: bool,
        window: &Window,
    ) -> Self {
        let seconds = tick.map_or(5., |t| t.started.elapsed().as_secs_f32());
        let motion = motion && tick.is_some_and(|t| !t.interrupted);
        if motion
            && (seconds < 0.6
                || ((4.0..4.2).contains(&seconds) && tick.is_some_and(|t| !t.rollovers.is_empty())))
        {
            window.request_animation_frame();
        }
        Self {
            now,
            tick,
            seconds,
            motion,
        }
    }

    fn progress(self, duration: f32) -> f32 {
        if self.motion {
            1. - (1. - self.seconds / duration).max(0.).powi(3)
        } else {
            1.
        }
    }

    pub(super) fn meter_time(self) -> DateTime<Utc> {
        let now = self.now.to_utc();
        match self.tick {
            Some(tick) if self.motion && self.seconds < 0.6 => {
                let delta = now - tick.previous.to_utc();
                now - chrono::TimeDelta::milliseconds(
                    (delta.num_milliseconds() as f32 * (1. - self.progress(0.6))) as i64,
                )
            }
            _ => now,
        }
    }

    pub(super) fn rollover(self, id: &ChannelId) -> Option<(DateTime<Utc>, f32, f32)> {
        let tick = self.tick?;
        if !tick.rollovers.contains(id) || self.seconds >= 4.2 {
            return None;
        }
        let opacity = if self.motion {
            ((4.2 - self.seconds) / 0.2).min(1.)
        } else {
            1.
        };
        Some((tick.previous.to_utc(), self.progress(0.48), opacity))
    }

    pub(super) fn clock(self, layout: Layout, size: f32) -> Div {
        let now = clock_time(self.now.to_utc());
        let progress = self.progress(0.45);
        let old = self
            .tick
            .filter(|_| progress < 1.)
            .map(|t| clock_time(t.previous.to_utc()));
        let mut clock = layout
            .text(SAIRA_CLOCK, size, size, 600.)
            .flex()
            .flex_none()
            .h(layout.px(size));
        for (index, digit) in now.bytes().enumerate() {
            let previous = old.as_ref().map(|s| s.as_bytes()[index]).unwrap_or(digit);
            let slot = div().relative().overflow_hidden().h(layout.px(size));
            let label = char::from(digit).to_string();
            let slot = if previous == digit {
                slot.child(label)
            } else {
                slot.child(
                    div()
                        .relative()
                        .top(layout.px(size * (1. - progress)))
                        .child(label),
                )
                .child(
                    div()
                        .absolute()
                        .top(layout.px(-size * progress))
                        .child(char::from(previous).to_string()),
                )
            };
            clock = clock.child(slot);
        }
        clock
    }
}
