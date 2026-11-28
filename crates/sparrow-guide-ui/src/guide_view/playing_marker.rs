//! The playing Channel's mark in the Guide: its card, Timeline Channel pill or
//! search row takes a wash in the colour of the playback stage.

use gpui::{Div, Rgba, prelude::*, rgb};
use sparrow_core::{ChannelFamily, ChannelId, ChannelSummary};
use sparrow_tv_guide::GuideFamily;

use super::{ICE, Layout, playback::PlaybackStage};

// Narrow enough to clear the icon plate in Timeline pills and inline Search rows.
const EDGE_WIDTH: f32 = 5.;

/// The Channel the dock shows, with every Quality Variant of its family when the
/// family is known, so any variant's row carries the mark.
#[derive(Clone, PartialEq)]
pub(crate) struct PlayingMarker {
    channels: Vec<ChannelId>,
    tint: Tint,
}

/// Colours for a marked surface. Unfocused surfaces take the `base` wash;
/// focused ones keep the ice fill with an `edge` bar. Text keeps its usual ink.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Tint {
    pub(super) base: u32,
    pub(super) elapsed: u32,
    edge: u32,
}

impl PlayingMarker {
    pub(crate) fn new(
        stage: PlaybackStage,
        shown: &ChannelId,
        family: Option<&ChannelFamily>,
    ) -> Option<Self> {
        let tint = Tint::for_stage(stage)?;
        let channels = family.map_or_else(
            || vec![shown.clone()],
            |family| family.variants().iter().map(|c| c.id().clone()).collect(),
        );
        Some(Self { channels, tint })
    }

    /// The tint for a Guide row, matched across all of its Quality Variants.
    pub(super) fn family_tint(&self, family: &GuideFamily) -> Option<Tint> {
        self.tint(family.variants().iter().map(|row| row.channel()))
    }

    /// The tint for a row whose variants include the playing Channel.
    pub(super) fn tint<'a>(
        &self,
        mut variants: impl Iterator<Item = &'a ChannelSummary>,
    ) -> Option<Tint> {
        variants
            .any(|channel| self.channels.contains(channel.id()))
            .then_some(self.tint)
    }
}

impl Tint {
    /// Washes stay dark enough for TEXT. Stopping and Failed carry no mark: the
    /// dock shows its own state then.
    fn for_stage(stage: PlaybackStage) -> Option<Self> {
        Some(match stage {
            PlaybackStage::Playing => Self {
                base: 0x2e1622,
                elapsed: 0x4d1f2b,
                edge: 0xff5d5d,
            },
            PlaybackStage::Tuning => Self {
                base: 0x12283d,
                elapsed: 0x1b3a55,
                edge: 0x0b4a66,
            },
            PlaybackStage::Reconnecting => Self {
                base: 0x2e2213,
                elapsed: 0x4a3419,
                edge: 0xc06a00,
            },
            PlaybackStage::Paused => Self {
                base: 0x242c3e,
                elapsed: 0x343d53,
                edge: 0x5b6b80,
            },
            PlaybackStage::Stopping | PlaybackStage::Failed => return None,
        })
    }

    /// A bar along the left edge of a focused surface of `height` reference px.
    pub(super) fn edge(self, layout: Layout, height: f32) -> Div {
        layout
            .rect(0., 0., EDGE_WIDTH, height)
            .rounded_l(layout.px(8.))
            .bg(rgb(self.edge))
    }
}

/// Fills a Channel row: ice when focused (with the stage edge when marked),
/// else the stage wash through `wash` when marked. Unmarked rows keep their fill.
pub(super) fn mark_row(
    row: Div,
    layout: Layout,
    tint: Option<Tint>,
    focused: bool,
    height: f32,
    wash: impl FnOnce(u32) -> Rgba,
) -> Div {
    match (focused, tint) {
        (true, tint) => row
            .bg(rgb(ICE))
            .children(tint.map(|tint| tint.edge(layout, height))),
        (false, Some(tint)) => row.bg(wash(tint.base)),
        (false, None) => row,
    }
}
