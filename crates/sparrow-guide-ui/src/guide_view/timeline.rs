mod cell;
mod rail;

pub(crate) use rail::{RailViewport, rail_hit, rail_scroll_hit};

use chrono::{DateTime, FixedOffset, Local, TimeDelta, Utc};
use gpui::{Div, div, linear_color_stop, linear_gradient, prelude::*, rgb, rgba};
use sparrow_tv_guide::{
    GuideFamily, VariantPreferences,
    guide_screen::{GuideScreen, LaneFocus, ScreenFocus},
    time::{Timeline, timeline_day_label},
    timeline_focus::TimelineFocus,
};

use crate::channel_icons::{ChannelIcons, IconImage};

use super::playing_marker::{PlayingMarker, Tint, mark_row};
use super::{
    DIM, FAINT, GUTTER, ICE, INK, Layout, MUTE, ON_ICE, Profile, SAIRA, TEXT, board,
    channel_icon::{Plate, family_icon, plate},
    clock_time,
};

const SLIDE_TRAVEL: f32 = 64.;
const BOARD_X: f32 = GUTTER + rail::WIDTH + 16.;
const CHANNEL_WIDTH: f32 = 336.;
const OVERSCAN_LINES: usize = 0;
const RIGHT_FADE_WIDTH: f32 = 84.;
const CHANNEL_CONTROL_X: f32 = 52.;

#[derive(Clone, Copy)]
struct TimelineFrame {
    axis: Timeline,
    meter_time: DateTime<Utc>,
    now: DateTime<Utc>,
    /// Width of the programme track.
    track: f32,
}

fn board_width(layout: Layout) -> f32 {
    layout.width - BOARD_X
}

/// The width the notice and footers span, and the board's height.
fn board_size(layout: Layout) -> [f32; 2] {
    [
        board_width(layout) - GUTTER,
        (layout.height - layout.m.board.timeline.top).max(0.),
    ]
}

/// The track spans the Timeline's two hours.
pub(crate) fn track_width(layout: Layout) -> f32 {
    board_width(layout) - CHANNEL_WIDTH - GUTTER
}

/// The Timeline control under a point in layout units, mirroring `render`.
pub(crate) fn timeline_hit(
    layout: Layout,
    screen: &GuideScreen,
    [x, y]: [f32; 2],
) -> Option<TimelineFocus> {
    let geometry = layout.m.board.timeline;
    let board_y = y - geometry.top;
    let x = x - BOARD_X;
    if board::retry_bounds(screen, board_size(layout), geometry.row_top)
        .is_some_and(|button| super::contains(button, [x, board_y]))
    {
        return Some(TimelineFocus::Retry);
    }
    if board_y < geometry.row_top || !(0. ..CHANNEL_WIDTH + track_width(layout)).contains(&x) {
        return None;
    }
    let content_y = board_y + screen.scroll_offset() - geometry.row_top;
    let family = (content_y / geometry.pitch) as usize;
    let row_y = content_y - family as f32 * geometry.pitch;
    let row = screen.families().get(family)?;
    if row_y > geometry.row_height {
        return None;
    }
    if x < CHANNEL_WIDTH {
        let [left, top, width, height] = channel_bounds(layout);
        let chip = (x >= left + 94. && (top + 37. ..top + 65.).contains(&row_y))
            .then(|| ((x - left - 94.) / 52.) as usize)
            .filter(|&chip| {
                row.quality_variants().count() > 1 && chip < row.quality_variants().count()
            });
        return Some(match chip {
            Some(chip) => TimelineFocus::Variant { family, chip },
            None if (left..left + width).contains(&x) && (top..top + height).contains(&row_y) => {
                TimelineFocus::Channel(family)
            }
            None => return None,
        });
    }
    let axis = screen.timeline()?;
    let track_x = x - CHANNEL_WIDTH;
    let programmes = row.programmes(screen.variant_preferences());
    programmes
        .iter()
        .position(|programme| {
            axis.placement(programme)
                .and_then(|placement| cell::bounds(placement, track_width(layout)))
                .is_some_and(|[left, _, width, _]| (left..left + width).contains(&track_x))
        })
        .map(|cell| TimelineFocus::Programme { family, cell })
}

pub(crate) fn rail(
    layout: Layout,
    screen: &GuideScreen,
    viewport: &mut RailViewport,
    slide: super::SlideFrame,
    hover: Option<LaneFocus>,
) -> Div {
    rail::render(layout, screen, viewport, slide, hover)
}

fn channel_bounds(layout: Layout) -> [f32; 4] {
    [
        CHANNEL_CONTROL_X,
        (layout.m.board.timeline.row_height - 70.) / 2.,
        CHANNEL_WIDTH - CHANNEL_CONTROL_X - 16.,
        70.,
    ]
}

pub(crate) fn render(
    layout: Layout,
    screen: &GuideScreen,
    minute: super::MinuteFrame<'_>,
    icons: &ChannelIcons,
    marker: Option<&PlayingMarker>,
    slide: super::SlideFrame,
    hover: Option<TimelineFocus>,
) -> Div {
    let now = minute.now;
    let axis = screen
        .timeline()
        .unwrap_or_else(|| Timeline::new(screen.displayed_window()));
    let frame = TimelineFrame {
        axis,
        meter_time: minute.meter_time(),
        now: now.to_utc(),
        track: track_width(layout),
    };
    let geometry = layout.m.board.timeline;
    let ruler_height = geometry.row_top;
    let height = (layout.height - geometry.top).max(0.);
    let focus = match layout.shown_focus(screen.focus()) {
        ScreenFocus::TimelineBoard(focus) => Some(focus),
        _ => None,
    };
    let retry_focus = focus == Some(TimelineFocus::Retry);
    let families = screen.families();
    let scroll = screen.scroll_offset();
    let rows = screen
        .visible_families(OVERSCAN_LINES)
        .enumerate()
        .map(|(ordinal, index)| {
            let focus = focus.filter(|focus| focus.family() == Some(index));
            let (offset, opacity) = slide.item(ordinal, SLIDE_TRAVEL, focus.is_some());
            let family = &families[index];
            let row = render_row(
                layout,
                frame,
                marker,
                family,
                screen.variant_preferences(),
                layout.rect(
                    offset,
                    geometry.row_top + index as f32 * geometry.pitch - scroll - ruler_height,
                    CHANNEL_WIDTH + frame.track,
                    geometry.row_height,
                ),
                focus,
                family_icon(family).and_then(|icon| icons.get(icon)),
                hover.filter(|hover| focus.is_none() && hover.family() == Some(index)),
            );
            row.opacity(opacity)
        });
    let rows_bottom = geometry.row_top + families.len() as f32 * geometry.pitch - scroll;
    let rows_height = (rows_bottom.min(height) - ruler_height).max(0.);
    let mut board = layout
        .rect(BOARD_X, geometry.top, CHANNEL_WIDTH + frame.track, height)
        .overflow_hidden()
        .child(
            layout
                .rect(
                    0.,
                    ruler_height,
                    board_width(layout),
                    (height - ruler_height).max(0.),
                )
                .overflow_hidden()
                .children(rows),
        );
    if rows_height > 0. {
        board = board
            .when_some(
                frame.axis.playhead_fraction(frame.meter_time),
                |board, at| {
                    board.child(
                        playhead(layout, frame, at as f32 * frame.track, rows_height)
                            .opacity(slide.item(0, SLIDE_TRAVEL, false).1),
                    )
                },
            )
            .when(!layout.translucent, |board| {
                board.child(
                    layout
                        .rect(
                            CHANNEL_WIDTH + frame.track - RIGHT_FADE_WIDTH,
                            ruler_height,
                            RIGHT_FADE_WIDTH,
                            rows_height,
                        )
                        .bg(linear_gradient(
                            90.,
                            linear_color_stop(rgba(INK << 8), 0.),
                            linear_color_stop(rgb(INK), 0.92),
                        )),
                )
            });
    }
    let size = board_size(layout);
    if let Some(notice) = screen.notice() {
        board = board.child(board::notice_panel(
            layout,
            board::notice_bounds(size[0], ruler_height),
            notice,
            retry_focus,
        ));
    }
    if !layout.translucent {
        board = board.child(board::bottom_fade(layout.rect(
            0.,
            height - 12.,
            board_width(layout),
            12.,
        )));
    }
    board
        .children(board::footer(layout, screen, size, retry_focus))
        .child(ruler(layout, frame, now))
}

fn ruler(layout: Layout, frame: TimelineFrame, now: DateTime<FixedOffset>) -> Div {
    let ruler_height = layout.m.board.timeline.row_top;
    let start = frame.axis.starts_at();
    let end = frame.axis.ends_at();
    let day = timeline_day_label(start.with_timezone(&Local).fixed_offset(), now);
    let first_tick = if layout.profile == Profile::Tv {
        start
    } else {
        frame.axis.first_tick()
    };
    let ticks = (0..4).filter_map(|index| {
        let instant = first_tick.checked_add_signed(TimeDelta::minutes(index * 30))?;
        if instant >= end {
            return None;
        }
        let x = (instant - start).as_seconds_f64() / (end - start).as_seconds_f64()
            * f64::from(frame.track);
        Some(
            layout
                .rect(x as f32, 12., frame.track / 4., 17.)
                .border_l(layout.px(1.))
                .border_color(rgba(FAINT))
                .pl(layout.px(7.))
                .child(
                    layout
                        .text(SAIRA, 17., 17., 400.)
                        .text_color(rgba(DIM))
                        .child(clock_time(instant)),
                ),
        )
    });
    layout
        .rect(0., 0., board_width(layout), ruler_height)
        .child(
            layout
                .rect(6., 12., CHANNEL_WIDTH - 6., 24.)
                .child(layout.text(SAIRA, 18., 24., 600.).child(day)),
        )
        .child(
            layout
                .rect(CHANNEL_WIDTH, 0., frame.track, ruler_height)
                .overflow_hidden()
                .children(ticks)
                .when_some(
                    frame.axis.playhead_fraction(frame.meter_time),
                    |ticks, at| {
                        ticks.child(
                            div()
                                .absolute()
                                .left(layout.px(at as f32 * frame.track))
                                .top(layout.px(7.))
                                .h(layout.px(30.))
                                .px(layout.px(10.))
                                .rounded(layout.px(4.))
                                .bg(rgb(ICE))
                                .flex()
                                .items_center()
                                .whitespace_nowrap()
                                .child(
                                    layout
                                        .text(SAIRA, 15., 30., 700.)
                                        .text_color(rgb(ON_ICE))
                                        .child(format!("Now {}", clock_time(frame.now))),
                                ),
                        )
                    },
                ),
        )
}

fn playhead(layout: Layout, frame: TimelineFrame, x: f32, height: f32) -> Div {
    layout
        .rect(
            CHANNEL_WIDTH,
            layout.m.board.timeline.row_top,
            frame.track,
            height,
        )
        .overflow_hidden()
        .child(
            layout
                .rect(x, 0., 28., height)
                .bg(rgba(0x9fe0ff1f))
                .child(layout.rect(0., 0., 2., height).bg(rgba(0x9fe0ff99))),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_row(
    layout: Layout,
    frame: TimelineFrame,
    marker: Option<&PlayingMarker>,
    family: &GuideFamily,
    preferences: &VariantPreferences,
    rect: Div,
    focus: Option<TimelineFocus>,
    icon: Option<&IconImage>,
    hover: Option<TimelineFocus>,
) -> Div {
    let (preferred, source) = family.preferred_with_schedule(preferences);
    let channel_focus = matches!(focus, Some(TimelineFocus::Channel(_)));
    let chip_focus = match focus {
        Some(TimelineFocus::Variant { chip, .. }) => Some(chip),
        _ => None,
    };
    let programme_focus = match focus {
        Some(TimelineFocus::Programme { cell, .. }) => Some(cell),
        _ => None,
    };
    let programmes = source.programmes();
    let tint = marker.and_then(|marker| marker.family_tint(family));
    let row_height = layout.m.board.timeline.row_height;
    let track = layout
        .rect(CHANNEL_WIDTH, 0., frame.track, row_height)
        .overflow_hidden();
    let track = if programmes.is_empty() {
        track.child(no_guide_data(layout, frame.track))
    } else {
        track
            .children(
                programmes
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| Some(*index) != programme_focus)
                    .filter_map(|(_, programme)| cell::render(layout, frame, programme, false)),
            )
            .when_some(
                programme_focus.and_then(|index| programmes.get(index)),
                |track, programme| track.children(cell::render(layout, frame, programme, true)),
            )
            .children(match hover {
                Some(TimelineFocus::Programme { cell, .. }) => programmes
                    .get(cell)
                    .and_then(|programme| frame.axis.placement(programme))
                    .and_then(|placement| cell::bounds(placement, frame.track))
                    .map(|[x, y, w, h]| {
                        layout.rect(x, y, w, h).child(super::hover_rim(layout, 8.))
                    }),
                _ => None,
            })
    };
    rect.when(icon.is_some(), |row| {
        row.child(
            layout
                .rect(0., 0., 42., row_height)
                .flex()
                .items_center()
                .justify_end()
                .child(
                    layout
                        .text(SAIRA, 24., 24., 400.)
                        .text_color(rgba(DIM))
                        .child(family.number().to_string()),
                ),
        )
    })
    .child(
        channel(
            layout,
            family,
            preferred.channel().id(),
            channel_focus,
            chip_focus,
            icon,
            tint,
        )
        .when(
            matches!(
                hover,
                Some(TimelineFocus::Channel(_) | TimelineFocus::Variant { .. })
            ),
            |channel| channel.child(super::hover_rim(layout, 8.)),
        ),
    )
    .child(track)
}

fn channel(
    layout: Layout,
    family: &GuideFamily,
    preferred: &sparrow_core::ChannelId,
    focused: bool,
    focused_chip: Option<usize>,
    icon: Option<&IconImage>,
    tint: Option<Tint>,
) -> Div {
    let [x, y, width, height] = channel_bounds(layout);
    let variants = family.quality_variants().next().is_some();
    layout
        .rect(x, y, width, height)
        .rounded(layout.px(8.))
        .map(|channel| {
            mark_row(channel, layout, tint, focused, height, |base| {
                layout.surface(base)
            })
        })
        .child(
            plate(layout, Plate::Timeline, family.number(), focused, icon)
                .absolute()
                .left(layout.px(6.))
                .top(layout.px(6.5)),
        )
        .child(
            layout
                .rect(94., if variants { 8.8 } else { 22.9 }, width - 106., 24.2)
                .child(
                    layout
                        .text(SAIRA, 22., 24.2, 600.)
                        .text_color(rgb(if focused { ON_ICE } else { TEXT }))
                        .truncate()
                        .child(family.title().to_owned()),
                ),
        )
        .when(variants, |channel| {
            channel.child(
                div()
                    .absolute()
                    .top(layout.px(37.))
                    .left(layout.px(94.))
                    .child(board::quality_tags(
                        layout,
                        family,
                        preferred,
                        focused,
                        focused_chip,
                    )),
            )
        })
        .when(focused_chip.is_some(), |channel| {
            // Tauri rims the Channel column while one of its quality chips holds focus.
            channel.child(
                layout
                    .rect(
                        -CHANNEL_CONTROL_X,
                        -6.5,
                        CHANNEL_WIDTH,
                        layout.m.board.timeline.row_height,
                    )
                    .border(layout.px(2.))
                    .border_color(rgb(ICE)),
            )
        })
}

fn no_guide_data(layout: Layout, track: f32) -> Div {
    layout
        .rect(0., 4., track - 6., layout.m.board.timeline.row_height - 8.)
        .rounded(layout.px(8.))
        .border(layout.px(1.))
        .border_color(rgba(FAINT))
        .pl(layout.px(13.))
        .flex()
        .items_center()
        .child(
            layout
                .text(SAIRA, 18., 26., 400.)
                .text_color(rgba(MUTE))
                .child("No guide data. The channel still plays."),
        )
}
