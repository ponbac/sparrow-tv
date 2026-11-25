use gpui::{Div, prelude::*, rgba};
use sparrow_tv_guide::guide_screen::{GuideMode, GuideScreen, LaneFocus, ScreenFocus};

use super::super::{BAND_BOTTOM, FAINT, GUTTER, Layout, MUTE, lane};

pub(super) const WIDTH: f32 = 270.;

const CONTROL_HEIGHT: f32 = 50.;
const TOP: f32 = 6.;
const ROSTER_BOTTOM: f32 = 30.;
const GROUPS_TOP: f32 = TOP + CONTROL_HEIGHT + 12.;
const GROUP_HEIGHT: f32 = 46.;
const GROUP_PITCH: f32 = 48.;
const GROUP_INSET: f32 = 7.5;

#[derive(Default)]
pub(crate) struct RailViewport {
    first: usize,
    group_reveal: lane::GroupReveal,
}

impl RailViewport {
    fn reveal(&mut self, selected: usize, focused: Option<usize>, count: usize, rows: usize) {
        self.first = self.first.min(count.saturating_sub(rows));
        if rows == 0 {
            return;
        }
        if let Some(index) = self.group_reveal.target(selected, focused, count) {
            self.first = self.first.clamp((index + 1).saturating_sub(rows), index);
        }
    }
}

pub(super) fn render(layout: Layout, screen: &GuideScreen, viewport: &mut RailViewport) -> Div {
    let height = (layout.height - BAND_BOTTOM).max(0.);
    let roster_top = height - ROSTER_BOTTOM - CONTROL_HEIGHT;
    let groups_height = (roster_top - 12. - GROUPS_TOP).max(0.);
    let rows = ((groups_height + GROUP_PITCH - GROUP_HEIGHT) / GROUP_PITCH).floor() as usize;
    let count = screen.groups().len() + 1;
    let focus = match screen.focus() {
        ScreenFocus::GroupLane(focus) => Some(focus),
        _ => None,
    };
    let focused_group = focus.and_then(lane::group_pill_index);
    let selected = lane::group_pill_index(screen.selected_lane()).unwrap_or(0);
    viewport.reveal(selected, focused_group, count, rows);
    let end = (viewport.first + rows).min(count);
    let groups = (viewport.first..end).map(|index| {
        let y = GROUPS_TOP + (index - viewport.first) as f32 * GROUP_PITCH;
        let group = index.checked_sub(1).map(|index| &screen.groups()[index]);
        lane::GroupPill::new(group).render(
            layout,
            layout
                .rect(GROUP_INSET, y, WIDTH - GROUP_INSET * 2., GROUP_HEIGHT)
                .justify_between(),
            selected == index,
            focused_group == Some(index),
            false,
        )
    });
    let roster_focus = focus == Some(LaneFocus::ChooseGroups);
    layout
        .rect(GUTTER, BAND_BOTTOM, WIDTH, height)
        .overflow_hidden()
        .child(lane::view_switch(
            layout,
            layout.rect(0., TOP, WIDTH, CONTROL_HEIGHT),
            GuideMode::Timeline,
            focus == Some(LaneFocus::Phase),
        ))
        .children(groups)
        .when(end < count, |rail| {
            let used = (rows as f32 * GROUP_PITCH - (GROUP_PITCH - GROUP_HEIGHT)).max(0.);
            let cue_y = GROUPS_TOP + (used + groups_height - 21.) / 2.;
            rail.child(
                layout
                    .rect(21., cue_y.max(GROUPS_TOP), 21., 21.)
                    .child(layout.icon(
                        include_bytes!("../../../assets/icons/chevron-down.svg"),
                        21.,
                        rgba(MUTE),
                    )),
            )
        })
        .child(lane::choose_groups(
            layout,
            layout.rect(0., roster_top, WIDTH, CONTROL_HEIGHT),
            roster_focus,
        ))
        .when(!roster_focus, |rail| {
            rail.child(
                layout
                    .rect(0., roster_top, WIDTH, CONTROL_HEIGHT)
                    .rounded_full()
                    .border(layout.px(1.5))
                    .border_color(rgba(FAINT)),
            )
        })
}
