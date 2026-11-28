//! The shared Guide, laid out in profile layout units (see `layout`).

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
mod layout;
pub use layout::FormFactor;
use layout::{BAND_TOP, CARD_GAP, GUTTER, POCKET_GUTTER, WatchGeometry};
pub(crate) use layout::{Layout, PocketClass, PocketMode, Pointer, Profile, contains};
pub(super) mod lane;
mod minute;
mod pocket;
pub(crate) use minute::{MinuteFrame, MinuteSample, MinuteTick};
pub(crate) use pocket::{
    PocketTool, idle_band, toolbar as pocket_toolbar, toolbar_hit as pocket_toolbar_hit,
};
pub(super) mod playback;
pub(super) mod playing_marker;
mod search_panel;
mod setup;
mod sources;
pub(super) mod timeline;

use std::sync::Arc;

pub(super) use channel_icon::family_icon;
pub(crate) use group_slide::{GroupSlide, SlideFrame};
pub(super) use groups::{GroupViewport, group_filter_rect, group_sheet, group_sheet_hit};
pub(crate) use lane::LaneViewport;
pub(super) use search_panel::{
    SearchHit, SearchPanel, SearchViewport, search_editor_rect, search_hit, search_panel,
};
pub(super) use setup::{SetupPanel, render as setup_screen, setup_editor_rect, setup_hit};
pub(super) use sources::{
    SourceFocus, SourcePanel, scroll_source_sheet, source_editor_rect, source_sheet_hit,
    sources_sheet,
};
pub(crate) use timeline::RailViewport;

use chrono::{DateTime, FixedOffset, Local, Utc};
use sparrow_core::{GuideProgramme, GuideWindowChannel};
use sparrow_tv_guide::{
    GuideFamily, VariantPreferences,
    guide_screen::{GuideScreen, ScreenFocus},
    preview::select_programme,
};

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
/// A control under the pointer on Desktop.
const CELL_HOVER: u32 = 0x17233a;
const HOVER_EDGE: u32 = 0x9fe0ff47;

pub(super) const SAIRA: &str = "Sparrow Guide 92";
const SAIRA_PREVIEW: &str = "Sparrow Guide 72";
const SAIRA_TITLE: &str = "Sparrow Guide 80";
const SAIRA_NOTICE: &str = "Sparrow Guide 84";
const SAIRA_CLOCK: &str = "Sparrow Guide 100";
const SAIRA_NUMBER: &str = "Sparrow Guide 110";

fn layout_hover_fill() -> gpui::Rgba {
    gpui::rgb(CELL_HOVER)
}

/// The 1px rim on a control under the pointer.
fn hover_rim(layout: Layout, radius: f32) -> gpui::Div {
    use gpui::{Styled, div};
    div()
        .absolute()
        .size_full()
        .rounded(layout.px(radius))
        .border(gpui::px(1.))
        .border_color(gpui::rgba(HOVER_EDGE))
}

pub(super) struct GuideBand<'a> {
    pub(super) minute: MinuteFrame<'a>,
    pub(super) hover: Option<sparrow_tv_guide::guide_screen::ToolbarFocus>,
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
