use std::{
    cell::Cell,
    time::{Duration, Instant},
};

use chrono::{DateTime, FixedOffset, Utc};
use gpui::{Div, Task, Window, div, prelude::*};
use sparrow_core::{GuideProgramme, GuideWindowChannel};
use sparrow_tv_guide::{GuideFamily, time::programme_at};

use super::{Layout, SAIRA_CLOCK, clock_time};

pub(crate) struct MinuteTick {
    previous: DateTime<FixedOffset>,
    started: Instant,
    rolled: bool,
    decorate: bool,
    prior_rows: Vec<GuideFamily>,
    has_badge: Cell<bool>,
    pub(crate) badge_expiry: Option<Task<()>>,
}

impl MinuteTick {
    const METER: f32 = 0.6;
    const SWEEP: f32 = 0.8;
    const STAGGER: f32 = 0.03;
    const MAX_STAGGER: f32 = 0.24;
    const FADE_AT: Duration = Duration::from_millis(4200);
    const FADE: Duration = Duration::from_millis(260);

    pub(crate) fn new(
        previous: DateTime<FixedOffset>,
        rolled: bool,
        decorate: bool,
        prior_rows: Vec<GuideFamily>,
    ) -> Self {
        Self {
            previous,
            started: Instant::now(),
            rolled,
            decorate,
            prior_rows,
            has_badge: Cell::new(false),
            badge_expiry: None,
        }
    }

    pub(crate) fn needs_badge_timer(&self) -> bool {
        self.has_badge.get() && self.badge_expiry.is_none()
    }

    pub(crate) fn badge_fade_at(&self) -> Instant {
        self.started + Self::FADE_AT
    }

    pub(crate) fn expires_at(&self) -> Instant {
        self.badge_fade_at() + Self::FADE
    }
}

pub(super) struct Rollover<'a> {
    pub previous: DateTime<Utc>,
    pub prior_programmes: &'a [GuideProgramme],
    pub outgoing: f32,
    pub incoming: f32,
    pub sweep: f32,
    pub badge: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct MinuteFrame<'a> {
    pub(crate) now: DateTime<FixedOffset>,
    tick: Option<&'a MinuteTick>,
    seconds: f32,
    motion: bool,
}

fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

impl<'a> MinuteFrame<'a> {
    pub(crate) fn new(
        now: DateTime<FixedOffset>,
        tick: Option<&'a MinuteTick>,
        motion: bool,
        window: &Window,
    ) -> Self {
        let seconds = tick.map_or(0., |tick| tick.started.elapsed().as_secs_f32());
        if motion && let Some(tick) = tick {
            let badge = tick.has_badge.get();
            if seconds < MinuteTick::METER
                || (badge
                    && (seconds < MinuteTick::SWEEP + MinuteTick::MAX_STAGGER
                        || (MinuteTick::FADE_AT.as_secs_f32()
                            ..(MinuteTick::FADE_AT + MinuteTick::FADE).as_secs_f32())
                            .contains(&seconds)))
            {
                window.request_animation_frame();
            }
        }
        Self {
            now,
            tick,
            seconds,
            motion,
        }
    }

    fn phase(self, start: f32, duration: f32) -> f32 {
        if self.motion && self.tick.is_some() {
            ((self.seconds - start) / duration).clamp(0., 1.)
        } else {
            1.
        }
    }

    pub(crate) fn meter_time(self) -> DateTime<Utc> {
        let now = self.now.to_utc();
        match self.tick {
            Some(tick) if !tick.rolled && self.motion && self.seconds < MinuteTick::METER => {
                let delta = now - tick.previous.to_utc();
                now - chrono::TimeDelta::milliseconds(
                    (delta.num_milliseconds() as f32
                        * (1. - ease_out(self.phase(0., MinuteTick::METER))))
                        as i64,
                )
            }
            _ => now,
        }
    }

    pub(super) fn rollover(
        self,
        source: &'a GuideWindowChannel,
        live: Option<&GuideProgramme>,
        position: usize,
    ) -> Option<Rollover<'a>> {
        let tick = self.tick?;
        let live = live?;
        if !tick.decorate || self.seconds >= (MinuteTick::FADE_AT + MinuteTick::FADE).as_secs_f32()
        {
            return None;
        }
        let previous = tick.previous.to_utc();
        let prior_programmes = tick
            .prior_rows
            .iter()
            .flat_map(GuideFamily::variants)
            .find(|row| row.channel().id() == source.channel().id())
            .unwrap_or(source)
            .programmes();
        if programme_at(prior_programmes, previous) == Some(live) {
            return None;
        }
        tick.has_badge.set(true);
        let sweep = self.phase(
            (position as f32 * MinuteTick::STAGGER).min(MinuteTick::MAX_STAGGER),
            MinuteTick::SWEEP,
        );
        let fade = ((self.seconds - MinuteTick::FADE_AT.as_secs_f32())
            / MinuteTick::FADE.as_secs_f32())
        .clamp(0., 1.);
        Some(Rollover {
            previous,
            prior_programmes,
            outgoing: self.phase(0., 0.22),
            incoming: ease_out(self.phase(0.1, 0.38)),
            sweep: sweep * sweep * (3. - 2. * sweep),
            badge: self.phase(0.16, 0.24) * if self.motion { 1. - fade } else { 1. },
        })
    }

    pub(super) fn clock(self, layout: Layout, size: f32) -> Div {
        let now = clock_time(self.now.to_utc());
        let progress = ease_out(self.phase(0., 0.45));
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
                        .top(layout.px(size * 0.85 * (1. - progress)))
                        .opacity(((progress - 0.35) / 0.65).clamp(0., 1.))
                        .child(label),
                )
                .child(
                    div()
                        .absolute()
                        .top(layout.px(-size * 0.85 * progress))
                        .opacity((1. - progress / 0.65).max(0.))
                        .child(char::from(previous).to_string()),
                )
            };
            clock = clock.child(slot);
        }
        clock
    }
}
