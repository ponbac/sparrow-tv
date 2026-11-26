mod cell;
mod rail;

pub(crate) use rail::RailViewport;

use chrono::{DateTime, FixedOffset, Local, TimeDelta, Utc};
use gpui::{Div, div, linear_color_stop, linear_gradient, prelude::*, rgb, rgba};
use sparrow_tv_guide::{
    GuideFamily, PageStatus, VariantPreferences,
    guide_screen::{
        GuideScreen, NOW_MORE_HEIGHT, NOW_RETAINED_RETRY_HEIGHT, ScreenFocus, TIMELINE_BOARD_TOP,
        TIMELINE_ROW_HEIGHT, TIMELINE_ROW_PITCH, TIMELINE_ROW_TOP,
    },
    time::{Timeline, timeline_day_label},
    timeline_focus::TimelineFocus,
};

use crate::channel_icons::{ChannelIcons, IconImage};

use super::playing_marker::{PlayingMarker, Tint, mark_row};
use super::{
    CANVAS_WIDTH, DIM, FAINT, GUTTER, ICE, INK, Layout, MUTE, ON_ICE, SAIRA, TEXT, board,
    channel_icon::{Plate, family_icon, plate},
    clock_time,
};

const SLIDE_TRAVEL: f32 = 64.;
const BOARD_X: f32 = GUTTER + rail::WIDTH + 16.;
const BOARD_WIDTH: f32 = CANVAS_WIDTH - BOARD_X;
const CHANNEL_WIDTH: f32 = 336.;
const TRACK_WIDTH: f32 = BOARD_WIDTH - CHANNEL_WIDTH - GUTTER;
const RULER_HEIGHT: f32 = TIMELINE_ROW_TOP;
const OVERSCAN_LINES: usize = 0;
const RIGHT_FADE_WIDTH: f32 = 84.;
const CHANNEL_CONTROL_X: f32 = 52.;

#[derive(Clone, Copy)]
struct TimelineFrame {
    axis: Timeline,
    meter_time: DateTime<Utc>,
    now: DateTime<Utc>,
}

pub(crate) fn rail(
    layout: Layout,
    screen: &GuideScreen,
    viewport: &mut RailViewport,
    slide: super::SlideFrame,
) -> Div {
    rail::render(layout, screen, viewport, slide)
}

fn channel_bounds() -> [f32; 4] {
    [
        CHANNEL_CONTROL_X,
        (TIMELINE_ROW_HEIGHT - 70.) / 2.,
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
) -> Div {
    let now = minute.now;
    let axis = screen
        .timeline()
        .unwrap_or_else(|| Timeline::new(screen.displayed_window()));
    let frame = TimelineFrame {
        axis,
        meter_time: minute.meter_time(),
        now: now.to_utc(),
    };
    let height = (layout.height - TIMELINE_BOARD_TOP).max(0.);
    let focus = match screen.focus() {
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
                    TIMELINE_ROW_TOP + index as f32 * TIMELINE_ROW_PITCH - scroll - RULER_HEIGHT,
                    CHANNEL_WIDTH + TRACK_WIDTH,
                    TIMELINE_ROW_HEIGHT,
                ),
                focus,
                family_icon(family).and_then(|icon| icons.get(icon)),
            );
            row.opacity(opacity)
        });
    let rows_bottom = TIMELINE_ROW_TOP + families.len() as f32 * TIMELINE_ROW_PITCH - scroll;
    let rows_height = (rows_bottom.min(height) - RULER_HEIGHT).max(0.);
    let mut board = layout
        .rect(
            BOARD_X,
            TIMELINE_BOARD_TOP,
            CHANNEL_WIDTH + TRACK_WIDTH,
            height,
        )
        .overflow_hidden()
        .child(
            layout
                .rect(
                    0.,
                    RULER_HEIGHT,
                    BOARD_WIDTH,
                    (height - RULER_HEIGHT).max(0.),
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
                        playhead(layout, at as f32 * TRACK_WIDTH, rows_height)
                            .opacity(slide.item(0, SLIDE_TRAVEL, false).1),
                    )
                },
            )
            .when(!layout.translucent, |board| {
                board.child(
                    layout
                        .rect(
                            CHANNEL_WIDTH + TRACK_WIDTH - RIGHT_FADE_WIDTH,
                            RULER_HEIGHT,
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
    if let Some(notice) = screen.notice() {
        board = board.child(board::notice_panel(
            layout,
            layout.rect(0., RULER_HEIGHT, BOARD_WIDTH - GUTTER, 285.),
            notice,
            retry_focus,
        ));
    }
    if !layout.translucent {
        board = board.child(board::bottom_fade(layout.rect(
            0.,
            height - 12.,
            BOARD_WIDTH,
            12.,
        )));
    }
    let footer_y = screen.footer_top();
    if !families.is_empty() && footer_y < height {
        if screen.retry_available() {
            board = board.child(board::retained_footer(
                layout,
                layout.rect(
                    0.,
                    footer_y,
                    BOARD_WIDTH - GUTTER,
                    NOW_RETAINED_RETRY_HEIGHT,
                ),
                retry_focus,
            ));
        } else if matches!(
            screen.page_status(),
            Some(PageStatus::Ready | PageStatus::Loading)
        ) {
            board = board.child(board::more_footer(
                layout,
                layout.rect(0., footer_y, BOARD_WIDTH - GUTTER, NOW_MORE_HEIGHT),
                screen.page_status() == Some(PageStatus::Loading),
            ));
        }
    }
    board.child(ruler(layout, frame, now))
}

fn ruler(layout: Layout, frame: TimelineFrame, now: DateTime<FixedOffset>) -> Div {
    let start = frame.axis.starts_at();
    let day = timeline_day_label(start.with_timezone(&Local).fixed_offset(), now);
    let ticks = (0..4).map(|index| {
        let instant = start + TimeDelta::minutes(index * 30);
        layout
            .rect(index as f32 * TRACK_WIDTH / 4., 12., TRACK_WIDTH / 4., 17.)
            .border_l(layout.px(1.))
            .border_color(rgba(FAINT))
            .pl(layout.px(7.))
            .child(
                layout
                    .text(SAIRA, 17., 17., 400.)
                    .text_color(rgba(DIM))
                    .child(clock_time(instant)),
            )
    });
    layout
        .rect(0., 0., BOARD_WIDTH, RULER_HEIGHT)
        .child(
            layout
                .rect(6., 12., CHANNEL_WIDTH - 6., 24.)
                .child(layout.text(SAIRA, 18., 24., 600.).child(day)),
        )
        .child(
            layout
                .rect(CHANNEL_WIDTH, 0., TRACK_WIDTH, RULER_HEIGHT)
                .overflow_hidden()
                .children(ticks)
                .when_some(
                    frame.axis.playhead_fraction(frame.meter_time),
                    |ticks, at| {
                        ticks.child(
                            div()
                                .absolute()
                                .left(layout.px(at as f32 * TRACK_WIDTH))
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

fn playhead(layout: Layout, x: f32, height: f32) -> Div {
    layout
        .rect(CHANNEL_WIDTH, RULER_HEIGHT, TRACK_WIDTH, height)
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
    let track = layout
        .rect(CHANNEL_WIDTH, 0., TRACK_WIDTH, TIMELINE_ROW_HEIGHT)
        .overflow_hidden();
    let track = if programmes.is_empty() {
        track.child(no_guide_data(layout))
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
    };
    rect.when(icon.is_some(), |row| {
        row.child(
            layout
                .rect(0., 0., 42., TIMELINE_ROW_HEIGHT)
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
    .child(channel(
        layout,
        family,
        preferred.channel().id(),
        channel_focus,
        chip_focus,
        icon,
        tint,
    ))
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
    let [x, y, width, height] = channel_bounds();
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
                    .rect(-CHANNEL_CONTROL_X, -6.5, CHANNEL_WIDTH, TIMELINE_ROW_HEIGHT)
                    .border(layout.px(2.))
                    .border_color(rgb(ICE)),
            )
        })
}

fn no_guide_data(layout: Layout) -> Div {
    layout
        .rect(0., 4., TRACK_WIDTH - 6., TIMELINE_ROW_HEIGHT - 8.)
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
