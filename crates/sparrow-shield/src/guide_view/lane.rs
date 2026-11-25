use gpui::{Div, FontWeight, Rgba, SharedString, TextRun, Window, font, prelude::*, rgb, rgba};
use sparrow_core::{CatalogGeneration, ChannelGroupView};
use sparrow_tv_guide::guide_screen::{GuideMode, GuideScreen, LaneFocus, ScreenFocus};

use super::{
    BAND_BOTTOM, CANVAS_WIDTH, CELL_ELAPSED, DIM, FAINT, GUTTER, ICE, LANE_HEIGHT, Layout, MUTE,
    ON_ICE, ON_ICE_DIM, SAIRA, TEXT,
};

const CONTROL_HEIGHT: f32 = 50.;
const CONTROL_Y: f32 = (LANE_HEIGHT - CONTROL_HEIGHT) / 2.;
const GROUP_GAP: f32 = 4.;
const REVEAL_MARGIN: f32 = 48.;

#[derive(Default)]
pub(super) struct GroupReveal {
    selected: Option<usize>,
    count: usize,
}

impl GroupReveal {
    pub(super) fn target(
        &mut self,
        selected: usize,
        focused: Option<usize>,
        count: usize,
    ) -> Option<usize> {
        let focused = focused.filter(|&index| index < count);
        let selection_changed = self.selected.replace(selected) != Some(selected);
        let groups_changed = std::mem::replace(&mut self.count, count) != count;
        // Outside the group pills, make the checked group visible as groups arrive.
        if selection_changed || (groups_changed && focused.is_none()) {
            Some(selected).filter(|&index| index < count)
        } else {
            focused
        }
    }
}

#[derive(Default)]
pub(crate) struct LaneViewport {
    generation: Option<CatalogGeneration>,
    first: usize,
    clipped: f32,
    group_reveal: GroupReveal,
    ink: super::group_slide::GroupInk,
}

impl LaneViewport {
    fn update(
        &mut self,
        generation: Option<CatalogGeneration>,
        selected: usize,
        focused: Option<usize>,
        count: usize,
        available: f32,
        width: impl Fn(usize) -> f32,
    ) {
        if self.generation != generation {
            *self = Self {
                generation,
                ..Self::default()
            };
        }
        if self.first >= count {
            self.first = 0;
            self.clipped = 0.;
        }
        if available <= 0. {
            return;
        }
        if let Some(index) = self.group_reveal.target(selected, focused, count) {
            self.reveal(index, count - 1, available, width);
        }
    }

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
    screen: &GuideScreen,
    viewport: &mut LaneViewport,
    window: &Window,
    (slide, wave): (super::SlideFrame, super::arrival::Wave),
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
            wave.enter(
                layout,
                layout.rect(GUTTER, CONTROL_Y, switch_width, CONTROL_HEIGHT),
                102.,
                0.,
                16.,
            ),
            GuideMode::Now,
            lane_focus == Some(LaneFocus::Phase),
        ))
        .child(groups(
            layout,
            screen,
            viewport,
            window,
            (groups_x, available),
            lane_focus,
            (slide, wave),
        ))
        .child(choose_groups(
            layout,
            wave.enter(
                layout,
                layout.rect(roster_x, CONTROL_Y, roster_width, CONTROL_HEIGHT),
                216.,
                0.,
                16.,
            ),
            lane_focus == Some(LaneFocus::ChooseGroups),
        ))
}

pub(super) fn view_switch(layout: Layout, rect: Div, mode: GuideMode, focused: bool) -> Div {
    let now_icon = include_bytes!("../../assets/icons/now.svg");
    let timeline_icon = include_bytes!("../../assets/icons/timeline.svg");
    let mut switch = rect
        .rounded_full()
        .px(layout.px(if focused { 4. } else { 2.5 }))
        .py(layout.px(if focused { 4. } else { 2.5 }))
        .when(focused, |control| control.bg(rgb(ICE)))
        .when(!focused, |control| {
            control.border(layout.px(1.5)).border_color(rgba(FAINT))
        })
        .flex()
        .gap(layout.px(2.));
    for (label, icon, active) in [
        ("Now", now_icon.as_slice(), mode == GuideMode::Now),
        (
            "Timeline",
            timeline_icon.as_slice(),
            mode == GuideMode::Timeline,
        ),
    ] {
        let (ink, fill) = match (active, focused) {
            (true, true) => (rgb(ICE), rgb(ON_ICE)),
            (true, false) => (rgb(TEXT), layout.surface(CELL_ELAPSED)),
            (false, true) => (rgb(ON_ICE), rgba(0)),
            (false, false) => (layout.secondary(DIM), rgba(0)),
        };
        switch = switch.child(
            segment(layout, label, icon, ink, fill)
                .when(mode == GuideMode::Timeline, |segment| {
                    segment.flex_1().justify_center()
                })
                .when(mode == GuideMode::Now, |segment| segment.flex_none()),
        );
    }
    switch
}

fn segment(layout: Layout, label: &'static str, icon: &'static [u8], ink: Rgba, fill: Rgba) -> Div {
    layout
        .text(SAIRA, 18., 26., 600.)
        .px(layout.px(14.))
        .rounded_full()
        .bg(fill)
        .text_color(ink)
        .flex()
        .items_center()
        .gap(layout.px(8.))
        .child(layout.icon(icon, 18., ink))
        .child(label)
}

pub(super) fn choose_groups(layout: Layout, rect: Div, focused: bool) -> Div {
    let ink = rgb(if focused { ON_ICE } else { TEXT });
    rect.font_weight(FontWeight(600.))
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
    screen: &GuideScreen,
    viewport: &mut LaneViewport,
    window: &Window,
    bounds: (f32, f32),
    focus: Option<LaneFocus>,
    (slide, wave): (super::SlideFrame, super::arrival::Wave),
) -> Div {
    let (x, available) = bounds;
    let groups = screen.groups();
    let selected = group_pill_index(screen.selected_lane()).unwrap_or(0);
    let focused = focus.and_then(group_pill_index);
    let pill = |index: usize| GroupPill::new(index.checked_sub(1).map(|index| &groups[index]));

    let mut row = layout
        .rect(x, CONTROL_Y, available, CONTROL_HEIGHT)
        .overflow_hidden();
    viewport.update(
        screen.generation(),
        selected,
        focused,
        groups.len() + 1,
        available,
        |index| pill(index).width(layout, window).min(available),
    );
    let mut left = -viewport.clipped;
    let mut pills = Vec::new();
    for index in viewport.first..=groups.len() {
        let pill = pill(index);
        let content_width = pill.width(layout, window);
        let width = content_width.min(available);
        if selected == index {
            let [x, w] = viewport.ink.bounds(selected, [left, width], slide);
            row = row.child(super::group_slide::GroupInk::pill(layout.rect(
                x,
                0.,
                w,
                CONTROL_HEIGHT,
            )));
        }
        pills.push(pill.render(
            layout,
            wave.enter(
                layout,
                layout.rect(left, 0., width, CONTROL_HEIGHT),
                118. + 20. * (index - viewport.first) as f32,
                0.,
                16.,
            ),
            selected == index,
            focused == Some(index),
            content_width > available,
        ));
        left += width + GROUP_GAP;
        if left >= available {
            break;
        }
    }
    row.children(pills)
}

pub(super) fn group_pill_index(focus: LaneFocus) -> Option<usize> {
    match focus {
        LaneFocus::All => Some(0),
        LaneFocus::Group(index) => Some(index + 1),
        LaneFocus::Phase | LaneFocus::ChooseGroups => None,
    }
}

pub(super) struct GroupPill {
    label: SharedString,
    count: Option<SharedString>,
}

impl GroupPill {
    pub(super) fn new(group: Option<&ChannelGroupView>) -> Self {
        let Some(group) = group else {
            return Self {
                label: "All".into(),
                count: None,
            };
        };
        let label = match group.name() {
            "" => "Ungrouped",
            name => name,
        };
        Self {
            label: label.into(),
            count: Some(group.channel_count().to_string().into()),
        }
    }

    fn width(&self, layout: Layout, window: &Window) -> f32 {
        let label_width = measure(layout, window, &self.label, 20., 600.);
        let count_width = self
            .count
            .as_ref()
            .map_or(0., |count| 8. + measure(layout, window, count, 15., 500.));
        28. + label_width + count_width
    }

    pub(super) fn render(
        self,
        layout: Layout,
        rect: Div,
        selected: bool,
        focused: bool,
        constrained: bool,
    ) -> Div {
        let (text, count, fill) = if focused {
            (rgb(ON_ICE), rgba(ON_ICE_DIM), rgb(ICE))
        } else if selected {
            (rgb(TEXT), layout.secondary(DIM), rgba(0))
        } else {
            (layout.secondary(DIM), layout.secondary(MUTE), rgba(0))
        };
        rect.rounded_full()
            .bg(fill)
            .px(layout.px(14.))
            .gap(layout.px(8.))
            .flex()
            .items_center()
            .child(
                layout
                    .text(SAIRA, 20., 28., 600.)
                    .whitespace_nowrap()
                    .when(constrained, |label| label.min_w_0().truncate())
                    .when(!constrained, |label| label.flex_none())
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

#[cfg(test)]
mod tests {
    use super::{GROUP_GAP, LaneViewport};

    #[test]
    fn horizontal_lane_reveals_the_selected_group_then_keeps_the_focused_group_visible() {
        let mut viewport = LaneViewport::default();
        let width = |_| 80.;
        let available = 256.;
        let scroll =
            |viewport: &LaneViewport| viewport.first as f32 * (80. + GROUP_GAP) + viewport.clipped;

        viewport.update(None, 12, None, 20, available, width);
        assert_eq!(scroll(&viewport), 880.);
        viewport.update(None, 12, Some(11), 20, available, width);
        assert_eq!(scroll(&viewport), 880.);

        viewport.update(None, 12, Some(16), 20, available, width);
        assert_eq!(scroll(&viewport), 1216.);
        viewport.update(None, 12, Some(16), 21, available, width);
        assert_eq!(scroll(&viewport), 1216.);
        viewport.update(None, 12, None, 21, available, width);
        assert_eq!(scroll(&viewport), 1216.);
        viewport.update(None, 12, None, 22, available, width);
        assert_eq!(scroll(&viewport), 960.);
    }
}
