use gpui::{Div, Pixels, Size, Styled, div, prelude::*, rgb, rgba};
use sparrow_tv_guide::{
    PageStatus,
    group_chooser::{GroupChooserFocus, GroupRosterCounts},
    guide_screen::GuideScreen,
};

use super::{
    CANVAS_WIDTH, CELL, DIM, FAINT, ICE, INK, Layout, MUTE, ON_ICE, PANEL, PLATE, SAIRA,
    SAIRA_TITLE, TEXT,
};

const WIDTH: f32 = CANVAS_WIDTH * 0.36;
const LEFT: f32 = CANVAS_WIDTH - WIDTH;
const PADDING: f32 = 30.;
const FILTER_Y: f32 = 116.;
const FILTER_HEIGHT: f32 = 56.;
const FILTER_PADDING: f32 = 20.;
const FILTER_ICON_SIZE: f32 = 22.;
const FILTER_GAP: f32 = 12.;
const LIST_Y: f32 = 190.;
const ROW_HEIGHT: f32 = 60.;
const ROW_PITCH: f32 = ROW_HEIGHT + 4.;

#[derive(Default)]
pub(in super::super) struct GroupViewport {
    first: usize,
}

fn filter_width(can_restore: bool) -> f32 {
    WIDTH - PADDING * 2. - if can_restore { 248. } else { 136. }
}

/// Android's editor occupies the field's text area.
pub(in super::super) fn group_filter_rect(
    size: Size<Pixels>,
    pixel_scale: f32,
    can_restore: bool,
) -> [i32; 4] {
    let scale = Layout::new(size).scale * pixel_scale;
    let text_inset = FILTER_PADDING + FILTER_ICON_SIZE + FILTER_GAP;
    [
        LEFT + PADDING + text_inset,
        FILTER_Y,
        filter_width(can_restore) - text_inset - FILTER_PADDING,
        FILTER_HEIGHT,
    ]
    .map(|length| (length * scale).round() as i32)
}

pub(in super::super) fn group_sheet(
    viewport_size: Size<Pixels>,
    screen: &GuideScreen,
    viewport: &mut GroupViewport,
    editing: bool,
) -> Option<Div> {
    let Some(chooser) = screen.group_chooser() else {
        viewport.first = 0;
        return None;
    };
    let layout = Layout::new(viewport_size);
    let focus = chooser.focus();
    let rows = chooser.matches().len() + 1;
    let capacity = ((layout.height - LIST_Y - 60.) / ROW_PITCH).floor().max(1.) as usize;
    viewport.first = viewport.first.min(rows.saturating_sub(capacity));
    if let Some(row) = focus.row() {
        viewport.first = viewport
            .first
            .min(row)
            .max((row + 1).saturating_sub(capacity));
    }
    let end = (viewport.first + capacity + 1).min(rows);
    let roster = screen.group_roster();
    let mut list = layout
        .rect(
            PADDING,
            LIST_Y,
            WIDTH - 2. * PADDING,
            layout.height - LIST_Y,
        )
        .overflow_hidden()
        .children((viewport.first..end).map(|row| {
            let group = row
                .checked_sub(1)
                .and_then(|index| chooser.matches().get(index))
                .and_then(|index| roster.get(*index));
            let (label, count, selected, hidden) = match group {
                Some(group) => (
                    if group.name().is_empty() {
                        "Ungrouped"
                    } else {
                        group.name()
                    },
                    Some(group.channel_count()),
                    screen
                        .selected_group()
                        .is_some_and(|selected| selected.as_str() == group.name()),
                    screen.exclusions().contains(group.name()),
                ),
                None => (
                    "All channels",
                    None,
                    screen.selected_group().is_none(),
                    false,
                ),
            };
            let row_width = WIDTH - 2. * PADDING;
            let pick_width = row_width - if group.is_some() { 108. } else { 0. };
            let picked = focus == GroupChooserFocus::Pick(row);
            layout
                .rect(
                    0.,
                    (row - viewport.first) as f32 * ROW_PITCH,
                    row_width,
                    ROW_HEIGHT,
                )
                .rounded(layout.px(8.))
                .bg(rgb(CELL))
                .overflow_hidden()
                .when(selected && !picked, |tile| {
                    tile.child(layout.rect(0., 0., 4., ROW_HEIGHT).bg(rgb(ICE)))
                })
                .child(
                    layout
                        .rect(0., 0., pick_width, ROW_HEIGHT)
                        .flex()
                        .items_center()
                        .px(layout.px(16.))
                        .gap(layout.px(16.))
                        .when(picked, |pick| pick.bg(rgb(ICE)))
                        .child(
                            layout
                                .text(SAIRA, 20., 24., 700.)
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(if picked {
                                    rgb(ON_ICE)
                                } else if hidden {
                                    rgba(MUTE)
                                } else {
                                    rgb(TEXT)
                                })
                                .child(label.to_owned()),
                        )
                        .when_some(count, |pick, count| {
                            pick.child(
                                layout
                                    .text(SAIRA, 16., 22., 500.)
                                    .flex_none()
                                    .text_color(if picked { rgb(ON_ICE) } else { rgba(DIM) })
                                    .child(count.to_string()),
                            )
                        }),
                )
                .when(group.is_some(), |tile| {
                    tile.child(pill(
                        layout,
                        layout.rect(row_width - 100., 8., 92., 44.),
                        if hidden { "Show" } else { "Hide" },
                        focus == GroupChooserFocus::Toggle(row),
                    ))
                })
        }));
    if rows == 1 {
        let message = if matches!(
            screen.group_roster_status(),
            Some(PageStatus::Loading | PageStatus::Ready)
        ) {
            Some("Loading channel groups…")
        } else if !chooser.filter().trim().is_empty() {
            Some("No group matches that search.")
        } else {
            None
        };
        if let Some(message) = message {
            list = list.child(
                layout.rect(16., ROW_PITCH + 12., WIDTH - 94., 32.).child(
                    layout
                        .text(SAIRA, 18., 24., 400.)
                        .text_color(rgba(DIM))
                        .child(message),
                ),
            );
        }
    }
    let show_all = screen.group_chooser_can_show_all();
    let filter_width = filter_width(show_all);
    let GroupRosterCounts { shown, hidden } = screen.group_roster_counts();
    let tally = if hidden == 0 {
        format!("{shown} shown")
    } else {
        format!("{shown} shown, {hidden} hidden")
    };
    let filter_focused = focus == GroupChooserFocus::Filter || editing;
    let filter_border = if filter_focused { 3. } else { 2. };
    let sheet = layout
        .rect(LEFT, 0., WIDTH, layout.height)
        .bg(rgb(PANEL))
        .text_color(rgb(TEXT))
        .overflow_hidden()
        .child(
            layout.rect(PADDING, 30., WIDTH - 130., 48.).child(
                layout
                    .text(SAIRA_TITLE, 40., 42., 700.)
                    .child("Channel groups"),
            ),
        )
        .child(
            layout.rect(PADDING, 81., WIDTH - 130., 28.).child(
                layout
                    .text(SAIRA, 17., 24., 400.)
                    .text_color(rgba(DIM))
                    .truncate()
                    .child("Pick a group to show in the guide, or hide the ones you never watch."),
            ),
        )
        .child(pill(
            layout,
            layout.rect(WIDTH - 78., 30., 48., 48.),
            "×",
            focus == GroupChooserFocus::Close,
        ))
        .child(
            layout
                .rect(PADDING, FILTER_Y, filter_width, FILTER_HEIGHT)
                .rounded_full()
                .bg(rgb(INK))
                .border(layout.px(filter_border))
                .border_color(if filter_focused {
                    rgb(ICE)
                } else {
                    rgba(FAINT)
                })
                .flex()
                .items_center()
                .px(layout.px(FILTER_PADDING - filter_border))
                .gap(layout.px(FILTER_GAP))
                .child(layout.icon(
                    include_bytes!("../../assets/icons/search.svg"),
                    FILTER_ICON_SIZE,
                    rgba(DIM),
                ))
                .when(!editing, |filter| {
                    filter.child(
                        layout
                            .text(SAIRA, 21., 28., 400.)
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if chooser.filter().is_empty() {
                                rgba(MUTE)
                            } else {
                                rgb(TEXT)
                            })
                            .child(if chooser.filter().is_empty() {
                                "Find a group".to_owned()
                            } else {
                                chooser.filter().to_owned()
                            }),
                    )
                }),
        )
        .child(
            layout
                .rect(PADDING + filter_width + 16., FILTER_Y + 17., 128., 24.)
                .child(
                    layout
                        .text(SAIRA, 16., 22., 500.)
                        .text_color(rgba(DIM))
                        .child(tally),
                ),
        )
        .when(show_all, |sheet| {
            sheet.child(pill(
                layout,
                layout.rect(WIDTH - 126., FILTER_Y + 6., 96., 44.),
                "Show all",
                focus == GroupChooserFocus::ShowAll,
            ))
        })
        .child(list);
    Some(div().absolute().inset_0().bg(rgba(0x050910b3)).child(sheet))
}

fn pill(layout: Layout, bounds: Div, label: &'static str, focused: bool) -> Div {
    bounds
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(if focused { ICE } else { PLATE }))
        .text_color(rgb(if focused { ON_ICE } else { TEXT }))
        .child(layout.text(SAIRA, 18., 24., 600.).child(label))
}
