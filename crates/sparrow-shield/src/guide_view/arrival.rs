//! The first usable Guide claims the entrance for the lifetime of the process.
use super::{CANVAS_WIDTH, DIM, ICE, INK, Layout, SAIRA, SAIRA_TITLE, TEXT, playback::signal_mark};
use gpui::{Div, Pixels, Size, Window, div, prelude::*, rgb, rgba};
use sparrow_tv_guide::{
    PageStatus,
    guide_screen::{GuideScreen, ScreenNotice},
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

static ARRIVED: AtomicBool = AtomicBool::new(false);
const END_MS: f32 = 800.;
const TRAVEL_MS: f32 = 480.;

#[derive(Default)]
pub(crate) enum Arrival {
    #[default]
    Pending,
    Loading,
    Running {
        start: Instant,
        had_mark: bool,
    },
    Done,
}

impl Arrival {
    pub(crate) fn loading(&mut self, screen: &GuideScreen) -> bool {
        if matches!(self, Self::Pending | Self::Loading)
            && !ARRIVED.load(Ordering::Relaxed)
            && screen.notice() == Some(ScreenNotice::Loading)
        {
            *self = Self::Loading;
            return true;
        }
        false
    }

    pub(crate) fn cancel(&mut self) -> bool {
        if matches!(self, Self::Running { .. } | Self::Loading) {
            *self = Self::Done;
            ARRIVED.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    pub(crate) fn frame(&mut self, screen: &GuideScreen, window: &Window, reduced: bool) -> Wave {
        if matches!(self, Self::Pending | Self::Loading) && !screen.families().is_empty() {
            *self = if ARRIVED.swap(true, Ordering::Relaxed) || reduced {
                Self::Done
            } else {
                Self::Running {
                    start: Instant::now(),
                    had_mark: matches!(self, Self::Loading),
                }
            };
        }
        if let Self::Running { start, had_mark } = self {
            let ms = start.elapsed().as_secs_f32() * 1000.;
            if !reduced && ms < END_MS {
                window.request_animation_frame();
                return Wave {
                    ms,
                    had_mark: *had_mark,
                };
            }
            *self = Self::Done;
        }
        Wave {
            ms: END_MS,
            had_mark: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Wave {
    ms: f32,
    had_mark: bool,
}

impl Wave {
    pub(super) fn enter(self, layout: Layout, rect: Div, delay: f32, x: f32, y: f32) -> Div {
        let t = ((self.ms - delay.min(END_MS - TRAVEL_MS)) / TRAVEL_MS).clamp(0., 1.);
        if t >= 1. {
            return rect;
        }
        let remaining = (1. - t).powi(3);
        rect.ml(layout.px(x * remaining))
            .mt(layout.px(y * remaining))
            .opacity(1. - remaining)
    }

    pub(crate) fn landing(self, viewport: Size<Pixels>) -> Option<Div> {
        (self.had_mark && self.ms < 270.).then(|| {
            let layout = Layout::new(viewport);
            let fold = (1. - self.ms / 180.).max(0.);
            mark_bounds(layout)
                .opacity(((270. - self.ms) / 120.).min(1.))
                .child(signal_mark::folding(mark_layout(layout), fold))
        })
    }
}

fn mark_layout(layout: Layout) -> Layout {
    Layout {
        scale: layout.scale * 3.,
        ..layout
    }
}
fn mark_bounds(layout: Layout) -> Div {
    layout.rect(
        (CANVAS_WIDTH - 126.) / 2.,
        layout.height / 2. - 110.,
        126.,
        72.,
    )
}

pub(crate) fn loading(viewport: Size<Pixels>, screen: &GuideScreen) -> Div {
    let layout = Layout::new(viewport);
    let mark = mark_layout(layout);
    let status = if screen.group_roster_status() == Some(PageStatus::Complete) {
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
    };
    div()
        .size_full()
        .relative()
        .bg(rgb(INK))
        .text_color(rgb(TEXT))
        .child(mark_bounds(layout).child(signal_mark::searching(
            mark,
            div().relative().size_full(),
            ICE,
            1100,
            "boot-search",
        )))
        .child(
            layout
                .rect(0., layout.height / 2. - 30., CANVAS_WIDTH, 140.)
                .flex()
                .flex_col()
                .items_center()
                .child(layout.text(SAIRA_TITLE, 54., 70., 700.).child("Sparrow"))
                .child(
                    layout
                        .text(SAIRA, 22., 34., 400.)
                        .text_color(rgba(DIM))
                        .child(status),
                ),
        )
}
