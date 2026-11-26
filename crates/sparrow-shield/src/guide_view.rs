//! The Shield Guide, laid out in 1920-wide reference units.

mod backdrop;
pub(super) use backdrop::BackdropMotion;
pub(crate) mod ambient;
pub(super) mod arrival;
pub(super) mod band;
pub(super) mod board;
mod channel_icon;
mod glow;
mod group_slide;
mod groups;
pub(super) mod lane;
mod minute;
pub(crate) use minute::{MinuteFrame, MinuteSample, MinuteTick};
pub(super) mod playback;
pub(super) mod playing_marker;
mod search_panel;
mod setup;
mod sources;
pub(super) mod timeline;

use std::sync::Arc;

pub(super) use channel_icon::family_icon;
pub(crate) use group_slide::{GroupSlide, SlideFrame};
pub(super) use groups::{GroupViewport, group_filter_rect, group_sheet};
pub(crate) use lane::LaneViewport;
pub(super) use search_panel::{SearchPanel, SearchViewport, search_editor_rect, search_panel};
pub(super) use setup::{SetupPanel, render as setup_screen, setup_editor_rect};
pub(super) use sources::{SourceFocus, SourcePanel, source_editor_rect, sources_sheet};
pub(crate) use timeline::RailViewport;

use chrono::{DateTime, FixedOffset, Local, Utc};
use gpui::{Div, FontWeight, IntoElement, Pixels, Size, Styled, div, px, svg};
use sparrow_core::{GuideProgramme, GuideWindowChannel};
use sparrow_tv_guide::{
    GuideFamily, VariantPreferences,
    guide_screen::{GuideScreen, NOW_BOARD_TOP as BOARD_TOP, ScreenFocus},
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
const WASH: u32 = 0x243858;
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

pub(super) const SAIRA: &str = "Sparrow Guide 92";
const SAIRA_PREVIEW: &str = "Sparrow Guide 72";
const SAIRA_TITLE: &str = "Sparrow Guide 80";
const SAIRA_NOTICE: &str = "Sparrow Guide 84";
const SAIRA_CLOCK: &str = "Sparrow Guide 100";
const SAIRA_NUMBER: &str = "Sparrow Guide 110";

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub(super) translucent: bool,
    scale: f32,
    height: f32,
}

impl Layout {
    fn new(viewport: Size<Pixels>) -> Self {
        let scale = (f32::from(viewport.width) / CANVAS_WIDTH).max(f32::EPSILON);
        Self {
            scale,
            translucent: false,
            height: f32::from(viewport.height) / scale,
        }
    }

    pub(super) fn for_guide(
        viewport: Size<Pixels>,
        choice: sparrow_installed_catalog::GuideBackdrop,
    ) -> Self {
        Self {
            translucent: choice != sparrow_installed_catalog::GuideBackdrop::Flat,
            ..Self::new(viewport)
        }
    }

    pub(super) fn guide_text(self) -> gpui::Rgba {
        gpui::rgb(if self.translucent { 0xffffff } else { TEXT })
    }

    fn secondary(self, color: u32) -> gpui::Rgba {
        let mut ink = gpui::rgba(color);
        if self.translucent {
            ink.a = (ink.a + 0.12).min(1.);
        }
        ink
    }

    fn surface(self, color: u32) -> gpui::Rgba {
        let mut fill = gpui::rgb(color);
        if self.translucent {
            fill.a = 0.85;
        }
        fill
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
    pub(super) minute: MinuteFrame<'a>,
    pub(super) preview: Option<&'a PreviewSelection>,
    pub(super) description: Option<&'a gpui::SharedString>,
    pub(super) docked: bool,
    pub(super) inline_search: bool,
    pub(super) focus: ScreenFocus,
}

#[derive(Clone, Eq, PartialEq)]
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
