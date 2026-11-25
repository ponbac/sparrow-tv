//! The Shield Guide, laid out in 1920-wide reference units.

mod band;
mod board;
mod channel_icon;
mod groups;
mod lane;
pub(super) mod playback;
mod search_panel;
mod setup;
mod sources;
mod timeline;

use std::sync::Arc;

pub(super) use channel_icon::family_icon;
pub(super) use groups::{GroupViewport, group_filter_rect, group_sheet};
pub(crate) use lane::LaneViewport;
pub(super) use search_panel::{SearchPanel, SearchViewport, search_editor_rect, search_panel};
pub(super) use setup::{SetupPanel, render as setup_screen, setup_editor_rect};
pub(super) use sources::{SourceFocus, SourcePanel, source_editor_rect, sources_sheet};
pub(crate) use timeline::RailViewport;

use crate::channel_icons::ChannelIcons;
use chrono::{DateTime, FixedOffset, Local, Utc};
use gpui::{Div, FontWeight, IntoElement, Pixels, Size, Styled, Window, div, prelude::*, px, svg};
use sparrow_core::{GuideProgramme, GuideWindowChannel};
use sparrow_tv_guide::{
    GuideFamily, VariantPreferences,
    guide_screen::{GuideMode, GuideScreen, NOW_BOARD_TOP as BOARD_TOP, ScreenFocus},
    preview::select_programme,
};

const CANVAS_WIDTH: f32 = 1920.;
const GUTTER: f32 = 40.;
const BAND_BOTTOM: f32 = 444.;
const LANE_HEIGHT: f32 = BOARD_TOP - BAND_BOTTOM;
const CARD_GAP: f32 = 11.;

pub(super) const INK: u32 = 0x0a111e;
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

pub(super) fn surface_height(viewport: Size<Pixels>) -> f32 {
    Layout::new(viewport).height
}

pub(super) struct GuideBand<'a> {
    pub(super) now: DateTime<FixedOffset>,
    pub(super) preview: Option<&'a PreviewSelection>,
    pub(super) description: Option<&'a gpui::SharedString>,
    pub(super) docked: bool,
    pub(super) inline_search: bool,
    pub(super) focus: ScreenFocus,
}

pub(super) fn render(
    screen: &GuideScreen,
    band: GuideBand<'_>,
    lanes: (&mut LaneViewport, &mut RailViewport),
    window: &mut Window,
    icons: &ChannelIcons,
) -> Div {
    let layout = Layout::new(window.viewport_size());
    let now = band.now;
    let guide = div()
        .size_full()
        .relative()
        .overflow_hidden()
        .bg(gpui::rgb(INK))
        .text_color(gpui::rgb(TEXT))
        .font_family(SAIRA)
        .child(band::render(layout, band, icons));
    match screen.mode() {
        GuideMode::Now => guide
            .child(lane::render(layout, screen, lanes.0, window))
            .child(board::render(layout, screen, now, icons)),
        GuideMode::Timeline => guide
            .child(timeline::rail(layout, screen, lanes.1))
            .child(timeline::render(layout, screen, now, icons)),
    }
}

pub(super) struct PreviewSelection {
    pub(super) family: GuideFamily,
    cell: Option<usize>,
    preferences: Arc<VariantPreferences>,
}

impl PreviewSelection {
    pub(super) fn from_screen(screen: &GuideScreen) -> Option<Self> {
        Some(Self {
            family: screen.preview_family()?.clone(),
            cell: screen.preview_cell(),
            preferences: Arc::clone(screen.variant_preferences()),
        })
    }

    pub(super) fn preferred_variant(&self) -> &GuideWindowChannel {
        self.family.preferred_variant(&self.preferences)
    }

    pub(super) fn programme(
        &self,
        now: DateTime<FixedOffset>,
    ) -> Option<(&GuideWindowChannel, &GuideProgramme)> {
        select_programme(&self.family, &self.preferences, self.cell, now.to_utc())
    }
}

fn programme_title(programme: &GuideProgramme) -> gpui::SharedString {
    if programme.title_truncated() {
        format!("{}…", programme.title()).into()
    } else {
        programme.title().into()
    }
}

pub(super) fn clock_time(instant: DateTime<Utc>) -> String {
    instant.with_timezone(&Local).format("%H:%M").to_string()
}
