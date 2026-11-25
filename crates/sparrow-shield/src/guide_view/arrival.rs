//! The first usable Guide claims the entrance for the lifetime of the process.
use super::{CANVAS_WIDTH, DIM, ICE, INK, Layout, SAIRA, SAIRA_TITLE, TEXT, playback::signal_mark};
use gpui::{Animation, AnimationExt, Div, Pixels, Size, Window, div, prelude::*, rgb, rgba};
use sparrow_tv_guide::{
    PageStatus,
    guide_screen::{GuideScreen, ScreenNotice},
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

static ARRIVED: AtomicBool = AtomicBool::new(false);
const END_MS: f32 = 800.;
const TRAVEL_MS: f32 = 480.;

#[derive(Default)]
pub(crate) struct Arrival {
    pub(crate) presented: bool,
    status: Option<String>,
    clock: WaveClock,
    pulse_since: Option<Instant>,
}

#[derive(Default)]
enum WaveClock {
    #[default]
    Idle,
    FirstFrame,
    Running(Instant),
}

impl Arrival {
    pub(crate) fn leave(&mut self) {
        self.status = None;
        self.pulse_since = None;
        self.clock = WaveClock::Idle;
    }

    pub(crate) fn cancel(&mut self) -> bool {
        let running = !matches!(self.clock, WaveClock::Idle);
        if running {
            self.leave();
        }
        running
    }

    pub(crate) fn loading(
        &mut self,
        screen: &GuideScreen,
        viewport: Size<Pixels>,
        reduced: bool,
    ) -> Option<Div> {
        if ARRIVED.load(Ordering::Relaxed)
            || (self.presented && screen.notice() != Some(ScreenNotice::Loading))
        {
            return None;
        }
        let status = loading_status(screen);
        if self.presented && !reduced {
            self.pulse_since.get_or_insert_with(Instant::now);
        } else {
            self.pulse_since = None;
        }
        let view = lockup(Layout::new(viewport), &status, self.pulse_since);
        self.status = Some(status);
        Some(div().relative().size_full().bg(rgb(INK)).child(view))
    }

    pub(crate) fn frame(&mut self, window: &Window, reduced: bool) -> Wave {
        // Showing any Guide chrome, including a notice, consumes this process's entrance.
        if !ARRIVED.swap(true, Ordering::Relaxed) && !reduced {
            // The first layout shapes the board's text and can take several frames on a cold GPU.
            self.clock = WaveClock::FirstFrame;
            window.request_animation_frame();
            return Wave { ms: 0. };
        }
        if matches!(self.clock, WaveClock::FirstFrame) {
            self.clock = WaveClock::Running(Instant::now());
        }
        if let WaveClock::Running(start) = self.clock {
            let ms = start.elapsed().as_secs_f32() * 1000.;
            if !reduced && ms < END_MS {
                window.request_animation_frame();
                return Wave { ms };
            }
        }
        self.leave();
        Wave { ms: END_MS }
    }

    pub(crate) fn landing(&self, viewport: Size<Pixels>, wave: Wave) -> Option<Div> {
        let opacity = (1. - wave.ms / 200.).max(0.).powi(2);
        self.status
            .as_deref()
            .filter(|_| opacity > 0.)
            .map(|status| lockup(Layout::new(viewport), status, self.pulse_since).opacity(opacity))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Wave {
    ms: f32,
}

impl Wave {
    pub(super) fn card(
        self,
        layout: Layout,
        rect: Div,
        diagonal: usize,
        last: usize,
        focused: bool,
    ) -> Div {
        let delay =
            110. + (END_MS - TRAVEL_MS - 110.) * diagonal.min(last) as f32 / last.max(1) as f32;
        let rect = self.enter(layout, rect, delay, 0., 32.);
        if focused {
            rect.opacity(if self.ms >= delay { 1. } else { 0. })
        } else {
            rect
        }
    }

    pub(super) fn enter(self, layout: Layout, rect: Div, delay: f32, x: f32, y: f32) -> Div {
        let t = ((self.ms - delay) / TRAVEL_MS).clamp(0., 1.);
        if self.ms >= END_MS || t >= 1. {
            return rect;
        }
        let remaining = (1. - t).powi(5);
        rect.ml(layout.px(x * remaining))
            .mt(layout.px(y * remaining))
            .opacity(1. - (1. - (t / 0.6).min(1.)).powi(3))
    }
}

fn loading_status(screen: &GuideScreen) -> String {
    if screen.group_roster_status() == Some(PageStatus::Complete) {
        let channels: usize = screen
            .group_roster()
            .iter()
            .map(|group| group.channel_count() as usize)
            .sum();
        format!("{channels} channels · Opening the guide")
    } else if screen.generation().is_some() {
        "Opening the guide".into()
    } else {
        "Opening the channel catalog".into()
    }
}

fn lockup(layout: Layout, status: &str, pulse_since: Option<Instant>) -> Div {
    let mark = Layout {
        scale: layout.scale * 6.,
        ..layout
    };
    let middle = layout.height / 2. - 165.;
    let frame = signal_mark::steady(mark, div().relative().size_full(), ICE);
    let frame = if let Some(start) = pulse_since {
        frame
            .with_animation(
                "boot-search",
                Animation::new(Duration::from_millis(1100))
                    .repeat()
                    .with_max_fps(24.),
                move |frame, _| {
                    let phase = start.elapsed().as_secs_f32() * std::f32::consts::TAU / 1.1;
                    frame.opacity(0.85 + 0.15 * phase.cos())
                },
            )
            .into_any_element()
    } else {
        frame.into_any_element()
    };
    layout
        .rect(0., 0., CANVAS_WIDTH, layout.height)
        .text_color(rgb(TEXT))
        .child(layout.rect(690., middle - 72., 252., 144.).child(frame))
        .child(
            layout
                .rect(958., middle - 48., 400., 96.)
                .child(layout.text(SAIRA_TITLE, 72., 96., 700.).child("Sparrow")),
        )
        .child(
            layout
                .rect(0., middle + 56., CANVAS_WIDTH, 36.)
                .text_center()
                .child(
                    layout
                        .text(SAIRA, 26., 36., 400.)
                        .text_color(rgba(DIM))
                        .child(status.to_owned()),
                ),
        )
}
