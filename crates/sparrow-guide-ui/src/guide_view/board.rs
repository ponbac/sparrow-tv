use chrono::{DateTime, Utc};
use gpui::{Div, div, linear_color_stop, linear_gradient, prelude::*, rgb, rgba};
use sparrow_core::ChannelQuality;
use sparrow_core::GuideProgramme;
use sparrow_tv_guide::{
    GuideFamily, PageStatus,
    guide_screen::{
        GuideScreen, NOW_MORE_HEIGHT, NOW_RETAINED_RETRY_HEIGHT, ScreenFocus, ScreenNotice,
    },
    now_focus::NowFocus,
    time::{elapsed_fraction, minutes_left, next_after, programme_at},
};

use crate::channel_icons::{ChannelIcons, IconImage};

use super::playing_marker::PlayingMarker;

use super::{
    CARD_GAP, CELL, CELL_ELAPSED, DIM, GUTTER, ICE, ICE_ELAPSED, INK, Layout, ON_ICE, ON_ICE_DIM,
    PANEL, SAIRA, SAIRA_NOTICE, SAIRA_TITLE, TEXT,
    channel_icon::{Plate, family_icon, plate},
    clock_time, programme_title,
};

const SLIDE_TRAVEL: f32 = 80.;
const OVERSCAN_LINES: usize = 0;
const CONTENT_X: f32 = 96.;
const TITLE_HEIGHT: f32 = 35.65;
const LINE_GAP: f32 = 4.;
const QUALITY_CHIP_PITCH: f32 = 52.;

fn content_width(layout: Layout) -> f32 {
    layout.column_width() - CONTENT_X - 21.
}

/// A Now card's surface rectangle in layout units.
pub(crate) fn card_bounds(layout: Layout, index: usize, scroll: f32) -> [f32; 4] {
    let board = layout.m.board.now;
    let columns = board.columns;
    let column_width = layout.column_width();
    [
        GUTTER + (index % columns) as f32 * (column_width + CARD_GAP),
        board.top + board.row_top + (index / columns) as f32 * board.pitch - scroll,
        column_width,
        board.row_height,
    ]
}

/// The Quality Variant chip under a point on a card. Chips sit right-aligned in
/// the caption line; each takes about one chip pitch.
pub(crate) fn card_chip(family: &GuideFamily, card: [f32; 4], point: [f32; 2]) -> Option<usize> {
    let count = family.quality_variants().count();
    let from_right = card[0] + card[2] - 18. - point[0];
    let y = point[1] - card[1];
    if count < 2 || from_right < 0. || !(6. ..42.).contains(&y) {
        return None;
    }
    let slot = (from_right / QUALITY_CHIP_PITCH) as usize;
    (slot < count).then(|| count - 1 - slot)
}

pub(crate) fn render(
    layout: Layout,
    screen: &GuideScreen,
    minute: super::MinuteFrame<'_>,
    icons: &ChannelIcons,
    marker: Option<&PlayingMarker>,
    (slide, wave): (super::SlideFrame, super::arrival::Wave),
    hover: Option<NowFocus>,
) -> Div {
    let top = layout.m.board.now.top;
    let columns = layout.columns();
    let height = (layout.height - top).max(0.);
    let focus = match layout.shown_focus(screen.focus()) {
        ScreenFocus::Board(focus) => Some(focus),
        _ => None,
    };
    let retry_focus = focus == Some(NowFocus::Retry);
    let mut board = layout
        .rect(GUTTER, top, layout.width - 2. * GUTTER, height)
        .overflow_hidden();
    if let Some(notice) = screen.notice() {
        board = board.child(notice_panel(
            layout,
            layout.rect(
                0.,
                layout.m.board.now.row_top,
                layout.width - GUTTER * 2.,
                285.,
            ),
            notice,
            retry_focus,
        ));
    }
    let families = screen.families();
    let scroll = screen.scroll_offset();
    let visible = screen.visible_families(0);
    let first_row = visible.start / columns;
    let last_diagonal =
        (visible.end.saturating_sub(1) / columns).saturating_sub(first_row) + columns - 1;
    for (ordinal, index) in screen.visible_families(OVERSCAN_LINES).enumerate() {
        let focus = focus.filter(|focus| focus.family() == Some(index));
        let (offset, opacity) = slide.item(ordinal / columns, SLIDE_TRAVEL, focus.is_some());
        let [x, y, w, h] = card_bounds(layout, index, scroll);
        let rect = layout
            .rect(x - GUTTER + offset, y - top, w, h)
            .opacity(opacity);
        let family = &families[index];
        let diagonal = (index / columns).saturating_sub(first_row) + index % columns;
        let row = if layout.compact_pocket() {
            pocket_row
        } else {
            card
        };
        board = board.child(row(
            layout,
            wave.card(layout, rect, diagonal, last_diagonal, focus.is_some()),
            marker,
            screen,
            index,
            focus,
            minute,
            family_icon(family).and_then(|icon| icons.get(icon)),
            hover.and_then(NowFocus::family) == Some(index),
        ));
    }
    if !layout.translucent {
        board = board.child(bottom_fade(layout.rect(
            0.,
            height - 12.,
            layout.width,
            12.,
        )));
    }
    let footer_y = screen.footer_top();
    if !families.is_empty() && footer_y < height {
        let width = layout.width - GUTTER * 2.;
        if screen.retry_available() {
            board = board.child(retained_footer(
                layout,
                layout.rect(0., footer_y, width, NOW_RETAINED_RETRY_HEIGHT),
                retry_focus,
            ));
        } else if matches!(
            screen.page_status(),
            Some(PageStatus::Ready | PageStatus::Loading)
        ) {
            board = board.child(more_footer(
                layout,
                layout.rect(0., footer_y, width, NOW_MORE_HEIGHT),
                screen.page_status() == Some(PageStatus::Loading),
            ));
        }
    }
    board
}

pub(super) fn bottom_fade(rect: Div) -> Div {
    rect.bg(linear_gradient(
        180.,
        linear_color_stop(rgba(INK << 8), 0.),
        linear_color_stop(rgb(INK), 1.),
    ))
}

pub(super) fn retained_footer(layout: Layout, rect: Div, focused: bool) -> Div {
    rect.rounded(layout.px(8.))
        .bg(layout.surface(PANEL))
        .px(layout.px(18.))
        .flex()
        .items_center()
        .justify_between()
        .gap(layout.px(18.))
        .child(
            layout
                .text(SAIRA, 18., 26., 400.)
                .text_color(rgba(DIM))
                .child("The guide could not update. These are the last loaded channels."),
        )
        .child(action(layout, "Try again", focused))
}

pub(super) fn more_footer(layout: Layout, rect: Div, loading: bool) -> Div {
    let label = if loading {
        "Loading more channels…"
    } else {
        "More channels"
    };
    rect.flex()
        .justify_center()
        .child(action(layout, label, false))
}

#[allow(clippy::too_many_arguments)]
fn card(
    layout: Layout,
    rect: Div,
    marker: Option<&PlayingMarker>,
    screen: &GuideScreen,
    index: usize,
    focus: Option<NowFocus>,
    minute: super::MinuteFrame<'_>,
    icon: Option<&IconImage>,
    hovered: bool,
) -> Div {
    let family = &screen.families()[index];
    let focused = matches!(focus, Some(NowFocus::Channel(_)));
    let hovered = hovered && focus.is_none();
    let tint = marker.and_then(|marker| marker.family_tint(family));
    let fill = match tint {
        Some(tint) => (tint.base, tint.elapsed),
        None if hovered => (super::CELL_HOVER, CELL_ELAPSED),
        None => (CELL, CELL_ELAPSED),
    };
    let focused_chip = match focus {
        Some(NowFocus::Variant { chip, .. }) => Some(chip),
        _ => None,
    };
    let (preferred, source) = family.preferred_with_schedule(screen.variant_preferences());
    let programmes = source.programmes();
    // A chosen time chip shows each row at that hour, without live progress.
    let viewing = screen.viewing_instant();
    let instant = viewing.unwrap_or(minute.now.to_utc());
    let live = programme_at(programmes, instant);
    let supporting = rgba(if focused { ON_ICE_DIM } else { DIM });
    let rollover = minute.rollover(
        source,
        live.filter(|_| viewing.is_none()),
        index.saturating_sub(screen.visible_families(0).start),
    );
    let caption_height = if live.is_some() || icon.is_some() {
        28.
    } else {
        0.
    };
    let last_height =
        if live.is_some() || next_after(programmes, instant).is_some() || programmes.is_empty() {
            26.
        } else {
            0.
        };
    let content_top = (layout.m.board.now.row_height
        - caption_height
        - TITLE_HEIGHT
        - last_height
        - LINE_GAP * 2.)
        / 2.;
    let title_top = content_top + caption_height + LINE_GAP;
    let quality_count = family.quality_variants().count();
    let mut card = rect
        .rounded(layout.px(8.))
        .overflow_hidden()
        .bg(if focused {
            rgb(ICE)
        } else {
            layout.surface(fill.0)
        });
    if let Some(live) = live.filter(|_| viewing.is_none()) {
        let width = layout.column_width() * elapsed_fraction(live, minute.meter_time()) as f32;
        if width > 0. {
            card = card.child(
                layout
                    .rect(0., 0., width, layout.m.board.now.row_height)
                    .rounded_l(layout.px(8.))
                    .bg(if focused {
                        rgb(ICE_ELAPSED)
                    } else {
                        layout.surface(fill.1)
                    }),
            );
        }
    }
    if let Some(roll) = &rollover {
        if roll.outgoing < 1.
            && let Some(old) = programme_at(roll.prior_programmes, roll.previous)
        {
            let width = layout.column_width() * elapsed_fraction(old, roll.previous) as f32;
            card = card.child(
                layout
                    .rect(0., 0., width, layout.m.board.now.row_height)
                    .rounded_l(layout.px(8.))
                    .bg(if focused {
                        rgb(ICE_ELAPSED)
                    } else {
                        layout.surface(fill.1)
                    })
                    .opacity(1. - roll.outgoing),
            );
        }
        if roll.sweep > 0. && roll.sweep < 1. {
            let x = -180. + (layout.column_width() + 180.) * roll.sweep;
            let peak = if focused { 0xffffff20 } else { 0x9fe0ff18 };
            for (offset, from, to) in [(0., peak & !0xff, peak), (90., peak, peak & !0xff)] {
                card = card.child(
                    layout
                        .rect(x + offset, 0., 90., layout.m.board.now.row_height)
                        .bg(linear_gradient(
                            90.,
                            linear_color_stop(rgba(from), 0.),
                            linear_color_stop(rgba(to), 1.),
                        )),
                );
            }
        }
    }
    if focused && let Some(tint) = tint {
        card = card.child(tint.edge(layout, layout.m.board.now.row_height));
    }
    if caption_height != 0. {
        let caption = layout.text(SAIRA, 18., 28., 400.).text_color(supporting);
        let caption = if icon.is_some() {
            caption
                .flex()
                .items_center()
                .gap(layout.px(10.))
                .child(
                    div()
                        .flex_none()
                        .font_weight(gpui::FontWeight(600.))
                        .text_color(if focused { rgb(ON_ICE) } else { rgba(DIM) })
                        .child(family.number().to_string()),
                )
                .when(live.is_some(), |line| {
                    line.child(div().min_w_0().truncate().child(family.title().to_owned()))
                })
        } else {
            caption.truncate().child(family.title().to_owned())
        };
        card = card.child(
            layout
                .rect(
                    CONTENT_X,
                    content_top,
                    content_width(layout) - (quality_count.max(1) as f32 * QUALITY_CHIP_PITCH + 6.),
                    28.,
                )
                .flex()
                .items_center()
                .gap(layout.px(8.))
                .child(caption.min_w_0())
                .when_some(rollover.as_ref(), |line, roll| {
                    let ink = if focused {
                        rgb(0x0b4a66)
                    } else {
                        rgba(0x9fe0ffdd)
                    };
                    line.child(
                        layout
                            .text(SAIRA, 15., 22., 600.)
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(layout.px(6.))
                            .text_color(ink)
                            .opacity(roll.badge)
                            .child(div().size(layout.px(7.)).rounded_full().bg(ink))
                            .child("Starting now"),
                    )
                }),
        );
    }

    card.child(
        plate(layout, Plate::Now, family.number(), focused, icon)
            .absolute()
            .left(layout.px(14.))
            .top(layout.px(23.)),
    )
    .child({
        let height = TITLE_HEIGHT + LINE_GAP + last_height;
        let block = |programmes: &[GuideProgramme], at, dy| {
            programme_lines(layout, family, programmes, at, focused, viewing.is_some())
                .absolute()
                .top(layout.px(dy))
        };
        let body = layout
            .rect(CONTENT_X, title_top, content_width(layout), height)
            .overflow_hidden();
        match rollover {
            Some(roll) if roll.incoming < 1. => body
                .when(roll.outgoing < 1., |body| {
                    body.child(
                        block(roll.prior_programmes, roll.previous, -28. * roll.outgoing)
                            .opacity(1. - roll.outgoing),
                    )
                })
                .child(
                    block(programmes, instant, 28. * (1. - roll.incoming)).opacity(roll.incoming),
                ),
            _ => body.child(block(programmes, instant, 0.)),
        }
    })
    .when(quality_count > 0, |card| {
        card.child(
            quality_tags(
                layout,
                family,
                preferred.channel().id(),
                focused,
                focused_chip,
            )
            .absolute()
            .top(layout.px(10.))
            .right(layout.px(18.)),
        )
    })
    .when(focused_chip.is_some(), |card| {
        card.child(
            div()
                .absolute()
                .size_full()
                .rounded(layout.px(8.))
                .border(layout.px(2.))
                .border_color(rgb(ICE)),
        )
    })
    .when(hovered, |card| card.child(super::hover_rim(layout, 8.)))
}

fn programme_lines(
    layout: Layout,
    family: &GuideFamily,
    programmes: &[GuideProgramme],
    instant: DateTime<Utc>,
    focused: bool,
    viewing: bool,
) -> Div {
    let live = programme_at(programmes, instant);
    let subtitle = next_after(programmes, instant)
        .map(|next| format!("{} {}", clock_time(next.starts_at()), programme_title(next)))
        .or_else(|| programmes.is_empty().then(|| "No guide data.".into()));
    let supporting = rgba(if focused { ON_ICE_DIM } else { DIM });
    let title = live.map_or_else(|| family.title().into(), programme_title);
    let title_color = if focused {
        rgb(ON_ICE)
    } else if live.is_some() {
        rgb(TEXT)
    } else {
        rgba(DIM)
    };
    div()
        .child(
            layout
                .rect(0., 0., content_width(layout), TITLE_HEIGHT)
                .child(
                    layout
                        .text(
                            SAIRA_TITLE,
                            31.,
                            TITLE_HEIGHT,
                            if live.is_some() { 650. } else { 600. },
                        )
                        .text_color(title_color)
                        .truncate()
                        .child(title),
                ),
        )
        .when(live.is_some() || subtitle.is_some(), |card| {
            card.child(
                layout
                    .rect(0., TITLE_HEIGHT + LINE_GAP, content_width(layout), 26.)
                    .font_family(SAIRA)
                    .text_size(layout.px(18.))
                    .line_height(layout.px(26.))
                    .text_color(supporting)
                    .flex()
                    .gap(layout.px(12.))
                    .when_some(subtitle, |line, subtitle| {
                        line.child(div().min_w_0().truncate().child(subtitle))
                    })
                    .when_some(live, |line, live| {
                        line.child(
                            div()
                                .ml_auto()
                                .flex_none()
                                .child(remaining(live, instant, viewing)),
                        )
                    }),
            )
        })
}

/// Minutes left in a live Programme, or a chosen hour's Programme times.
fn remaining(programme: &GuideProgramme, instant: DateTime<Utc>, viewing: bool) -> String {
    if viewing {
        format!(
            "{}–{}",
            clock_time(programme.starts_at()),
            clock_time(programme.ends_at())
        )
    } else {
        format!("{} min left", minutes_left(programme, instant))
    }
}

/// A one-column Pocket list row. Touch has no focus to show, so the playing
/// row always carries the stage edge.
#[allow(clippy::too_many_arguments)]
fn pocket_row(
    layout: Layout,
    rect: Div,
    marker: Option<&PlayingMarker>,
    screen: &GuideScreen,
    index: usize,
    focus: Option<NowFocus>,
    minute: super::MinuteFrame<'_>,
    icon: Option<&IconImage>,
    hovered: bool,
) -> Div {
    let family = &screen.families()[index];
    let height = layout.m.board.now.row_height;
    let width = layout.column_width();
    let focused = matches!(focus, Some(NowFocus::Channel(_)));
    let focused_chip = match focus {
        Some(NowFocus::Variant { chip, .. }) => Some(chip),
        _ => None,
    };
    let tint = marker.and_then(|marker| marker.family_tint(family));
    let (preferred, source) = family.preferred_with_schedule(screen.variant_preferences());
    let programmes = source.programmes();
    let viewing = screen.viewing_instant();
    let instant = viewing.unwrap_or(minute.now.to_utc());
    let shown = programme_at(programmes, instant);
    let elapsed = shown
        .filter(|_| viewing.is_none())
        .map(|programme| elapsed_fraction(programme, minute.meter_time()) as f32);
    let quality_count = family.quality_variants().count();
    let text_width = width - CONTENT_X - 18.;
    let (title, title_ink) = match shown {
        Some(programme) => (programme_title(programme), layout.guide_text()),
        None if programmes.is_empty() => (
            "No guide data. The channel still plays.".into(),
            layout.secondary(DIM),
        ),
        None => (
            format!("Nothing listed at {}", clock_time(instant)).into(),
            layout.secondary(DIM),
        ),
    };
    let next = next_after(programmes, instant)
        .map(|next| format!("{} {}", clock_time(next.starts_at()), programme_title(next)));
    let mut row = rect.rounded(layout.px(8.)).overflow_hidden();
    row = match tint {
        Some(tint) => row.bg(layout.surface(tint.base)),
        None if hovered => row.bg(super::layout_hover_fill()),
        None => row,
    };
    if let (Some(tint), Some(elapsed)) = (tint, elapsed) {
        row = row.child(
            layout
                .rect(0., 0., width * elapsed, height)
                .bg(layout.surface(tint.elapsed)),
        );
    }
    let meter = layout
        .rect(CONTENT_X, height - 5., text_width, 3.)
        .rounded_full();
    let meter = match elapsed {
        Some(elapsed) => meter.bg(rgba(super::FAINT)).child(
            layout
                .rect(0., 0., text_width * elapsed, 3.)
                .rounded_full()
                .bg(rgb(tint.map_or(ICE_ELAPSED, |tint| tint.edge_color()))),
        ),
        None => meter.bg(rgba(super::FAINT)),
    };
    row.children(tint.map(|tint| tint.edge(layout, height)))
        .child(
            plate(layout, Plate::Now, family.number(), focused, icon)
                .absolute()
                .left(layout.px(14.))
                .top(layout.px((height - 54.) / 2.)),
        )
        .child(
            layout
                .rect(
                    CONTENT_X,
                    12.,
                    text_width - quality_count as f32 * QUALITY_CHIP_PITCH,
                    28.,
                )
                .flex()
                .items_center()
                .gap(layout.px(10.))
                .child(
                    layout
                        .text(SAIRA, 20., 28., 600.)
                        .flex_none()
                        .text_color(layout.secondary(DIM))
                        .child(family.number().to_string()),
                )
                .child(
                    layout
                        .text(SAIRA, 22., 28., 600.)
                        .min_w_0()
                        .truncate()
                        .text_color(layout.guide_text())
                        .child(family.title().to_owned()),
                ),
        )
        .child(
            layout.rect(CONTENT_X, 42., text_width, 36.).child(
                layout
                    .text(SAIRA_TITLE, 29., 36., 650.)
                    .text_color(title_ink)
                    .truncate()
                    .child(title),
            ),
        )
        .child(
            layout
                .rect(CONTENT_X, 80., text_width, 26.)
                .font_family(SAIRA)
                .text_size(layout.px(19.))
                .line_height(layout.px(26.))
                .text_color(layout.secondary(DIM))
                .flex()
                .gap(layout.px(12.))
                .when_some(next, |line, next| {
                    line.child(div().min_w_0().truncate().child(format!("Next {next}")))
                })
                .when_some(shown, |line, programme| {
                    line.child(div().ml_auto().flex_none().child(remaining(
                        programme,
                        instant,
                        viewing.is_some(),
                    )))
                }),
        )
        .child(meter)
        .when(quality_count > 0, |row| {
            row.child(
                quality_tags(
                    layout,
                    family,
                    preferred.channel().id(),
                    false,
                    focused_chip,
                )
                .absolute()
                .top(layout.px(10.))
                .right(layout.px(18.)),
            )
        })
        .when(focused || focused_chip.is_some(), |row| {
            row.child(
                div()
                    .absolute()
                    .size_full()
                    .rounded(layout.px(8.))
                    .border(layout.px(3.))
                    .border_color(rgb(ICE)),
            )
        })
}

pub(super) fn quality_tags(
    layout: Layout,
    family: &GuideFamily,
    preferred: &sparrow_core::ChannelId,
    card_focus: bool,
    chip_focus: Option<usize>,
) -> Div {
    let lone = family.quality_variants().take(2).count() == 1;
    div()
        .flex()
        .gap(layout.px(4.))
        .children(
            family
                .quality_variants()
                .enumerate()
                .filter_map(|(index, row)| {
                    let quality = row.channel().variant()?.quality();
                    let selected = row.channel().id() == preferred;
                    let focused = chip_focus == Some(index);
                    Some(quality_chip(
                        layout, quality, selected, card_focus, focused, lone,
                    ))
                }),
        )
}

pub(super) fn quality_chip(
    layout: Layout,
    quality: ChannelQuality,
    selected: bool,
    card_focus: bool,
    chip_focus: bool,
    lone: bool,
) -> Div {
    let (ink, fill, rim) = match (chip_focus, card_focus, selected) {
        (true, _, _) => (rgb(ON_ICE), rgb(ICE), rgba(0)),
        (false, true, true) => (rgb(ON_ICE), rgba(0x06121c1a), rgba(ON_ICE_DIM)),
        (false, true, false) => (rgba(ON_ICE_DIM), rgba(0), rgba(0x06121c4d)),
        (false, false, true) => (rgb(TEXT), rgba(0xeef3f824), rgba(0xeef3f899)),
        (false, false, false) => (rgba(0xeef3f8bd), rgba(0), rgba(0xeef3f842)),
    };
    let label = match quality {
        ChannelQuality::Sd => "SD",
        ChannelQuality::Hd => "HD",
        ChannelQuality::Fhd => "FHD",
        ChannelQuality::Uhd => "UHD",
    };
    layout
        .text(SAIRA, 16., 16., 700.)
        .h(layout.px(28.))
        .px(layout.px(8.))
        .pt(layout.px(2.))
        .rounded(layout.px(5.))
        .when(!lone, |chip| {
            chip.border(layout.px(1.5)).border_color(rim).bg(fill)
        })
        .text_color(ink)
        .flex()
        .items_center()
        .child(label)
}

pub(super) fn notice_panel(
    layout: Layout,
    rect: Div,
    notice: ScreenNotice,
    retry_focus: bool,
) -> Div {
    use ScreenNotice::*;
    let (title, detail) = match notice {
        Loading => ("Opening the guide", "Loading channels and programme times."),
        NotConfigured => (
            "Add your sources",
            "Open Sources to set up this device before browsing.",
        ),
        CatalogUnavailable => (
            "Waiting for the first catalog",
            "The sources have not loaded yet.",
        ),
        Empty => ("No channels here", "This group has no channels right now."),
        AllGroupsHidden => (
            "Every group is hidden",
            "Open Choose groups and show a group to fill the guide.",
        ),
        ReadFailed => (
            "The guide is unavailable",
            "Sparrow could not reach the guide.",
        ),
    };
    rect.flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(layout.px(12.))
        .child(layout.text(SAIRA_NOTICE, 30., 47.2, 700.).child(title))
        .child(
            layout
                .text(SAIRA, 21., 31.5, 400.)
                .text_color(rgba(DIM))
                .child(detail),
        )
        .when(notice == ReadFailed, |panel| {
            panel.child(action(layout, "Try again", retry_focus).mt(layout.px(12.)))
        })
}

fn action(layout: Layout, label: &'static str, focused: bool) -> Div {
    layout
        .text(SAIRA, 20., 28., 600.)
        .h(layout.px(54.))
        .px(layout.px(24.))
        .rounded_full()
        .bg(if focused {
            rgb(ICE)
        } else {
            layout.surface(CELL_ELAPSED)
        })
        .text_color(rgb(if focused { ON_ICE } else { TEXT }))
        .flex()
        .items_center()
        .child(label)
}

#[cfg(test)]
mod tests {
    use gpui::{px, size};

    use super::{super::layout::DesktopClass, card_bounds};
    use crate::guide_view::{Layout, Profile};

    #[test]
    fn now_cards_keep_the_television_grid_and_tile_desktop_columns() {
        let tv = Layout::new(Profile::Tv, size(px(1920.), px(1080.)));
        assert_eq!(card_bounds(tv, 0, 0.), [40., 526., 606., 100.]);
        assert_eq!(card_bounds(tv, 4, 11.), [657., 626., 606., 100.]);

        let wide = Layout::new(
            Profile::Desktop(DesktopClass::Wide),
            size(px(1920.), px(1080.)),
        );
        let first = card_bounds(wide, 0, 0.);
        let last = card_bounds(wide, 3, 0.);
        assert_eq!(first[1], last[1]);
        assert!((last[0] + last[2] - (wide.width - 40.)).abs() < 0.01);
        assert_eq!(card_bounds(wide, 4, 0.)[1], first[1] + 111.);
    }
}
