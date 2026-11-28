use gpui::{Div, prelude::*, rgba};
use sparrow_tv_guide::guide_screen::{GuideMode, GuideScreen, LaneFocus, ScreenFocus};

use super::super::{FAINT, GUTTER, Layout, MUTE, lane, layout::RowScroll};

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
    scroll: RowScroll,
    ink: super::super::group_slide::GroupInk,
    group_reveal: lane::GroupReveal,
}

struct RailRows {
    height: f32,
    roster_top: f32,
    groups_height: f32,
    rows: usize,
}

impl RailRows {
    fn new(layout: Layout) -> Self {
        let height = (layout.height - layout.band_bottom()).max(0.);
        let roster_top = height - ROSTER_BOTTOM - CONTROL_HEIGHT;
        let groups_height = (roster_top - 12. - GROUPS_TOP).max(0.);
        Self {
            height,
            roster_top,
            groups_height,
            rows: ((groups_height + GROUP_PITCH - GROUP_HEIGHT) / GROUP_PITCH).floor() as usize,
        }
    }
}

/// The rail control under a point in layout units, mirroring `render`.
pub(crate) fn rail_hit(
    layout: Layout,
    screen: &GuideScreen,
    viewport: &RailViewport,
    [x, y]: [f32; 2],
) -> Option<LaneFocus> {
    let (x, y) = (x - GUTTER, y - layout.band_bottom());
    if !(0. ..WIDTH).contains(&x) {
        return None;
    }
    let rails = RailRows::new(layout);
    if (TOP..TOP + CONTROL_HEIGHT).contains(&y) {
        return Some(LaneFocus::Phase);
    }
    if (rails.roster_top..rails.roster_top + CONTROL_HEIGHT).contains(&y) {
        return Some(LaneFocus::ChooseGroups);
    }
    let y = y - GROUPS_TOP;
    if !(0. ..rails.groups_height).contains(&y) {
        return None;
    }
    let clipped = viewport.scroll.clipped(GROUP_PITCH);
    let offset = y + clipped;
    let slot = (offset / GROUP_PITCH) as usize;
    if slot >= rails.rows + usize::from(clipped > 0.) || offset % GROUP_PITCH > GROUP_HEIGHT {
        return None;
    }
    let index = viewport.scroll.first() + slot;
    (index <= screen.groups().len()).then(|| match index {
        0 => LaneFocus::All,
        index => LaneFocus::Group(index - 1),
    })
}

impl RailViewport {
    /// Scrolls the rail's group rows; focus is revealed again on the next key press.
    pub(crate) fn scroll(&mut self, rows: f32) {
        self.scroll.advance(rows);
    }

    fn reveal(&mut self, selected: usize, focused: Option<usize>, count: usize, rows: usize) {
        let target = (rows > 0)
            .then(|| self.group_reveal.target(selected, focused, count))
            .flatten();
        self.scroll.reveal(count, rows, target);
    }
}

pub(crate) fn rail_scroll_hit(layout: Layout, [x, y]: [f32; 2]) -> bool {
    (GUTTER..GUTTER + WIDTH).contains(&x)
        && (0. ..RailRows::new(layout).groups_height)
            .contains(&(y - layout.band_bottom() - GROUPS_TOP))
}

pub(super) fn render(
    layout: Layout,
    screen: &GuideScreen,
    viewport: &mut RailViewport,
    slide: super::super::SlideFrame,
    hover: Option<LaneFocus>,
) -> Div {
    let RailRows {
        height,
        roster_top,
        groups_height,
        rows,
    } = RailRows::new(layout);
    let count = screen.groups().len() + 1;
    let focus = match layout.shown_focus(screen.focus()) {
        ScreenFocus::GroupLane(focus) => Some(focus),
        _ => None,
    };
    let focused_group = focus.and_then(lane::group_pill_index);
    let selected = lane::group_pill_index(screen.selected_lane()).unwrap_or(0);
    viewport.reveal(selected, focused_group, count, rows);
    let first = viewport.scroll.first();
    let clipped = viewport.scroll.clipped(GROUP_PITCH);
    let end = (first + rows + usize::from(clipped > 0.)).min(count);
    let groups = (first..end).map(|index| {
        let y = (index - first) as f32 * GROUP_PITCH - clipped;
        let group = index.checked_sub(1).map(|index| &screen.groups()[index]);
        lane::GroupPill::new(group).render(
            layout,
            layout
                .rect(GROUP_INSET, y, WIDTH - GROUP_INSET * 2., GROUP_HEIGHT)
                .justify_between(),
            selected == index,
            focused_group == Some(index),
            false,
            hover.and_then(lane::group_pill_index) == Some(index),
        )
    });
    let groups = layout
        .rect(0., GROUPS_TOP, WIDTH, groups_height)
        .when(clipped > 0., |groups| groups.overflow_hidden())
        .when((first..end).contains(&selected), |rail| {
            let y = GROUPS_TOP + (selected - first) as f32 * GROUP_PITCH - clipped;
            let [y, w] = viewport
                .ink
                .bounds(selected, [y, WIDTH - GROUP_INSET * 2.], slide);
            rail.child(super::super::group_slide::GroupInk::pill(layout.rect(
                GROUP_INSET,
                y - GROUPS_TOP,
                w,
                GROUP_HEIGHT,
            )))
        })
        .children(groups);
    let roster_focus = focus == Some(LaneFocus::ChooseGroups);
    layout
        .rect(GUTTER, layout.band_bottom(), WIDTH, height)
        .overflow_hidden()
        .child(lane::view_switch(
            layout,
            layout.rect(0., TOP, WIDTH, CONTROL_HEIGHT),
            GuideMode::Timeline,
            focus == Some(LaneFocus::Phase),
        ))
        .child(groups)
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
