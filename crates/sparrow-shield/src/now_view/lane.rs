use gpui::{Div, FontWeight, Rgba, SharedString, TextRun, Window, font, prelude::*, rgb, rgba};
use sparrow_core::CatalogGeneration;
use sparrow_tv_guide::now_screen::{LaneFocus, NowScreen, ScreenFocus};

use super::{
    BAND_BOTTOM, CANVAS_WIDTH, CELL_ELAPSED, DIM, FAINT, GUTTER, ICE, LANE_HEIGHT, Layout, MUTE,
    ON_ICE, ON_ICE_DIM, SAIRA, TEXT,
};

const CONTROL_HEIGHT: f32 = 50.;
const CONTROL_Y: f32 = (LANE_HEIGHT - CONTROL_HEIGHT) / 2.;
const GROUP_GAP: f32 = 4.;
const REVEAL_MARGIN: f32 = 48.;

#[derive(Default)]
pub(crate) struct LaneViewport {
    generation: Option<CatalogGeneration>,
    first: usize,
    clipped: f32,
}

impl LaneViewport {
    fn reveal(&mut self, index: usize, last: usize, available: f32, width: impl Fn(usize) -> f32) {
        if index < self.first || (index == self.first && self.clipped > 0.) {
            self.first = index;
            self.clipped = 0.;
            let mut margin = REVEAL_MARGIN;
            while self.first > 0 && margin > 0. {
                self.first -= 1;
                let step = width(self.first) + GROUP_GAP;
                self.clipped = (step - margin).max(0.);
                margin -= step;
            }
            return;
        }
        let right = (self.first..index)
            .map(|index| width(index) + GROUP_GAP)
            .sum::<f32>()
            + width(index);
        let needed = right - available;
        if needed <= self.clipped {
            return;
        }
        let mut margin = 0.;
        for following in (index + 1)..=last {
            margin += width(following) + GROUP_GAP;
            if margin >= REVEAL_MARGIN {
                break;
            }
        }
        self.clipped = needed + margin.min(REVEAL_MARGIN);
        while self.first < index {
            let step = width(self.first) + GROUP_GAP;
            if self.clipped < step {
                break;
            }
            self.clipped -= step;
            self.first += 1;
        }
    }
}

pub(super) fn render(
    layout: Layout,
    screen: &NowScreen,
    viewport: &mut LaneViewport,
    window: &Window,
) -> Div {
    let lane_focus = match screen.focus() {
        ScreenFocus::GroupLane(focus) => Some(focus),
        _ => None,
    };
    let switch_width = 118.
        + measure(layout, window, &"Now".into(), 18., 600.)
        + measure(layout, window, &"Timeline".into(), 18., 600.);
    let roster_width = 62. + measure(layout, window, &"Choose groups".into(), 20., 600.);
    let roster_x = CANVAS_WIDTH - GUTTER - roster_width;
    let groups_x = GUTTER + switch_width + 25.5;
    let available = (roster_x - 25.5 - groups_x).max(0.);

    layout
        .rect(0., BAND_BOTTOM, CANVAS_WIDTH, LANE_HEIGHT)
        .child(view_switch(
            layout,
            switch_width,
            lane_focus == Some(LaneFocus::Phase),
        ))
        .child(groups(
            layout,
            screen,
            viewport,
            window,
            (groups_x, available),
            lane_focus,
        ))
        .child(choose_groups(
            layout,
            roster_x,
            roster_width,
            lane_focus == Some(LaneFocus::ChooseGroups),
        ))
}

fn view_switch(layout: Layout, width: f32, focused: bool) -> Div {
    let now_icon = include_bytes!("../../assets/icons/now.svg");
    let timeline_icon = include_bytes!("../../assets/icons/timeline.svg");
    layout
        .rect(GUTTER, CONTROL_Y, width, CONTROL_HEIGHT)
        .rounded_full()
        .px(layout.px(if focused { 4. } else { 2.5 }))
        .py(layout.px(if focused { 4. } else { 2.5 }))
        .when(focused, |control| control.bg(rgb(ICE)))
        .when(!focused, |control| {
            control.border(layout.px(1.5)).border_color(rgba(FAINT))
        })
        .flex()
        .gap(layout.px(2.))
        .child(segment(
            layout,
            "Now",
            now_icon,
            if focused { rgb(ICE) } else { rgb(TEXT) },
            if focused {
                rgb(ON_ICE)
            } else {
                rgb(CELL_ELAPSED)
            },
        ))
        .child(segment(
            layout,
            "Timeline",
            timeline_icon,
            if focused { rgb(ON_ICE) } else { rgba(DIM) },
            rgba(0),
        ))
}

fn segment(layout: Layout, label: &'static str, icon: &'static [u8], ink: Rgba, fill: Rgba) -> Div {
    layout
        .text(SAIRA, 18., 26., 600.)
        .px(layout.px(14.))
        .rounded_full()
        .bg(fill)
        .text_color(ink)
        .flex_none()
        .flex()
        .items_center()
        .gap(layout.px(8.))
        .child(layout.icon(icon, 18., ink))
        .child(label)
}

fn choose_groups(layout: Layout, x: f32, width: f32, focused: bool) -> Div {
    let ink = rgb(if focused { ON_ICE } else { TEXT });
    layout
        .rect(x, CONTROL_Y, width, CONTROL_HEIGHT)
        .font_weight(FontWeight(600.))
        .text_size(layout.px(20.))
        .line_height(layout.px(28.))
        .text_color(ink)
        .rounded_full()
        .when(focused, |control| control.bg(rgb(ICE)))
        .flex()
        .items_center()
        .px(layout.px(16.))
        .gap(layout.px(9.))
        .child(layout.icon(
            include_bytes!("../../assets/icons/choose-groups.svg"),
            21.,
            ink,
        ))
        .child("Choose groups")
}

fn groups(
    layout: Layout,
    screen: &NowScreen,
    viewport: &mut LaneViewport,
    window: &Window,
    bounds: (f32, f32),
    focus: Option<LaneFocus>,
) -> Div {
    let (x, available) = bounds;
    let groups = screen.groups();
    let selected = group_pill_index(screen.selected_lane());
    let focused = focus
        .and_then(group_pill_index)
        .filter(|&index| index <= groups.len());
    if viewport.generation != screen.generation() || viewport.first > groups.len() {
        *viewport = LaneViewport {
            generation: screen.generation(),
            ..LaneViewport::default()
        };
    }
    let pill = |index| {
        if index == 0 {
            Pill::new(layout, window, "All", None, available)
        } else {
            let group: &sparrow_core::ChannelGroupView = &groups[index - 1];
            let name = if group.name().is_empty() {
                "Ungrouped"
            } else {
                group.name()
            };
            Pill::new(layout, window, name, Some(group.channel_count()), available)
        }
    };

    let mut row = layout
        .rect(x, CONTROL_Y, available, CONTROL_HEIGHT)
        .overflow_hidden();
    if let Some(index) = focused {
        viewport.reveal(index, groups.len(), available, |index| pill(index).width);
    }
    let mut left = -viewport.clipped;
    for index in viewport.first..=groups.len() {
        let pill = pill(index);
        let width = pill.width;
        row = row.child(pill.render(
            layout,
            left,
            selected == Some(index),
            focused == Some(index),
        ));
        left += width + GROUP_GAP;
        if left >= available {
            break;
        }
    }
    row
}

fn group_pill_index(focus: LaneFocus) -> Option<usize> {
    match focus {
        LaneFocus::All => Some(0),
        LaneFocus::Group(index) => Some(index + 1),
        LaneFocus::Phase | LaneFocus::ChooseGroups => None,
    }
}

struct Pill {
    label: SharedString,
    count: Option<SharedString>,
    width: f32,
    constrained: bool,
}

impl Pill {
    fn new(
        layout: Layout,
        window: &Window,
        label: &str,
        count: Option<u32>,
        available: f32,
    ) -> Self {
        let label: SharedString = label.into();
        let count: Option<SharedString> = count.map(|count| count.to_string().into());
        let label_width = measure(layout, window, &label, 20., 600.);
        let count_width = count
            .as_ref()
            .map_or(0., |count| 8. + measure(layout, window, count, 15., 500.));
        let content_width = 28. + label_width + count_width;
        Self {
            label,
            count,
            width: content_width.min(available),
            constrained: content_width > available,
        }
    }

    fn render(self, layout: Layout, x: f32, selected: bool, focused: bool) -> Div {
        let (text, count, fill) = if focused {
            (rgb(ON_ICE), rgba(ON_ICE_DIM), rgb(ICE))
        } else if selected {
            (rgb(TEXT), rgba(DIM), rgb(CELL_ELAPSED))
        } else {
            (rgba(DIM), rgba(MUTE), rgba(0))
        };
        layout
            .rect(x, 0., self.width, CONTROL_HEIGHT)
            .rounded_full()
            .bg(fill)
            .px(layout.px(14.))
            .gap(layout.px(8.))
            .flex()
            .items_center()
            .child(
                layout
                    .text(SAIRA, 20., 28., 600.)
                    .whitespace_nowrap()
                    .when(self.constrained, |label| label.min_w_0().truncate())
                    .when(!self.constrained, |label| label.flex_none())
                    .text_color(text)
                    .child(self.label),
            )
            .when_some(self.count, |pill, value| {
                pill.child(
                    layout
                        .text(SAIRA, 15., 22., 500.)
                        .flex_none()
                        .text_color(gpui::Rgba {
                            a: count.a * 0.82,
                            ..count
                        })
                        .child(value),
                )
            })
    }
}

fn measure(layout: Layout, window: &Window, label: &SharedString, size: f32, weight: f32) -> f32 {
    let mut font = font(SAIRA);
    font.weight = FontWeight(weight);
    let line = window.text_system().shape_line(
        label.clone(),
        layout.px(size),
        &[TextRun {
            font,
            len: label.len(),
            ..TextRun::default()
        }],
        None,
    );
    f32::from(line.width()) / layout.scale
}
