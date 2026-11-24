//! The Shield's Now Guide, laid out in 1920-wide reference units.

mod band;
mod board;
mod lane;

pub(crate) use lane::LaneViewport;

use chrono::{DateTime, FixedOffset, Local, Utc};
use gpui::{Div, FontWeight, IntoElement, Pixels, Size, Styled, Window, div, prelude::*, px, svg};
use sparrow_core::{GuideProgramme, GuideWindowChannel};
use sparrow_tv_guide::{
    VariantPreferences,
    now_screen::NowScreen,
    time::{next_after, programme_at},
};

const CANVAS_WIDTH: f32 = 1920.;
const GUTTER: f32 = 40.;
const BAND_BOTTOM: f32 = 444.;
const LANE_HEIGHT: f32 = 78.;
const BOARD_TOP: f32 = BAND_BOTTOM + LANE_HEIGHT;
const CARD_GAP: f32 = 11.;

const INK: u32 = 0x0a111e;
const PANEL: u32 = 0x0f1828;
const CELL: u32 = 0x131d30;
const CELL_ELAPSED: u32 = 0x1a2842;
const PLATE: u32 = 0x22324f;
const FOCUSED_PLATE: u32 = 0x39637a;
const TEXT: u32 = 0xeef3f8;
const DIM: u32 = 0xeef3f8b8;
const MUTE: u32 = 0xeef3f894;
const FAINT: u32 = 0xeef3f81f;
const ICE: u32 = 0x9fe0ff;
const ICE_ELAPSED: u32 = 0x78c8ee;
const ON_ICE: u32 = 0x06121c;
const ON_ICE_DIM: u32 = 0x06121cb8;

const SAIRA: &str = "Sparrow Guide 92";
const SAIRA_PREVIEW: &str = "Sparrow Guide 72";
const SAIRA_TITLE: &str = "Sparrow Guide 80";
const SAIRA_NOTICE: &str = "Sparrow Guide 84";
const SAIRA_CLOCK: &str = "Sparrow Guide 100";
const SAIRA_NUMBER: &str = "Sparrow Guide 110";

#[derive(Clone, Copy)]
struct Layout {
    scale: f32,
    height: f32,
}

impl Layout {
    fn new(viewport: Size<Pixels>) -> Self {
        let scale = (f32::from(viewport.width) / CANVAS_WIDTH).max(f32::EPSILON);
        Self {
            scale,
            height: f32::from(viewport.height) / scale,
        }
    }

    fn px(self, length: f32) -> Pixels {
        px(length * self.scale)
    }

    fn rect(self, x: f32, y: f32, width: f32, height: f32) -> Div {
        div()
            .absolute()
            .left(self.px(x))
            .top(self.px(y))
            .w(self.px(width.max(0.)))
            .h(self.px(height.max(0.)))
    }

    fn text(self, font: &'static str, size: f32, line: f32, weight: f32) -> Div {
        div()
            .font_family(font)
            .font_weight(FontWeight(weight))
            .text_size(self.px(size))
            .line_height(self.px(line))
    }

    fn icon(self, data: &'static [u8], size: f32, color: gpui::Rgba) -> impl IntoElement {
        svg()
            .data(data)
            .size(self.px(size))
            .text_color(color)
            .flex_none()
    }
}

pub(super) fn board_height(viewport: Size<Pixels>) -> f32 {
    (Layout::new(viewport).height - BOARD_TOP).max(0.)
}

pub(super) fn render(
    screen: &NowScreen,
    now: DateTime<FixedOffset>,
    description: Option<&gpui::SharedString>,
    lane_viewport: &mut LaneViewport,
    window: &mut Window,
) -> Div {
    let layout = Layout::new(window.viewport_size());
    div()
        .size_full()
        .relative()
        .overflow_hidden()
        .bg(gpui::rgb(INK))
        .text_color(gpui::rgb(TEXT))
        .font_family(SAIRA)
        .child(band::render(layout, screen, now, description))
        .child(lane::render(layout, screen, lane_viewport, window))
        .child(board::render(layout, screen, now))
}

pub(super) fn preview_programme(
    screen: &NowScreen,
    now: DateTime<FixedOffset>,
) -> Option<(&GuideWindowChannel, &GuideProgramme)> {
    let (_, source) = screen
        .preview_family()?
        .preferred_with_schedule(&VariantPreferences::default());
    let programmes = source.programmes();
    let instant = now.to_utc();
    let programme =
        programme_at(programmes, instant).or_else(|| next_after(programmes, instant))?;
    Some((source, programme))
}

fn programme_title(programme: &GuideProgramme) -> gpui::SharedString {
    if programme.title_truncated() {
        format!("{}…", programme.title()).into()
    } else {
        programme.title().into()
    }
}

fn clock_time(instant: DateTime<Utc>) -> String {
    instant.with_timezone(&Local).format("%H:%M").to_string()
}
