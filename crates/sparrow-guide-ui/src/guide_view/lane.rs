use gpui::{Div, FontWeight, Rgba, SharedString, TextRun, Window, font, prelude::*, rgb, rgba};
use sparrow_core::{CatalogGeneration, ChannelGroupView};
use sparrow_tv_guide::guide_screen::{GuideMode, GuideScreen, LaneFocus, ScreenFocus};

use super::{CELL_ELAPSED, DIM, FAINT, GUTTER, ICE, Layout, MUTE, ON_ICE, ON_ICE_DIM, SAIRA, TEXT};

const CONTROL_HEIGHT: f32 = 50.;
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
    widths: std::collections::HashMap<(SharedString, Option<SharedString>), f32>,
    measured_scale: f32,
    scroll: LaneScroll,
    ink: super::group_slide::GroupInk,
}

#[derive(Default)]
struct LaneScroll {
    first: usize,
    clipped: f32,
    group_reveal: GroupReveal,
}

impl LaneScroll {
    fn update(
        &mut self,
        selected: usize,
        focused: Option<usize>,
        count: usize,
        available: f32,
        width: impl FnMut(usize) -> f32,
    ) {
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

    /// Scrolls the pills sideways by `delta` units, keeping the last pill reachable.
    fn scroll_by(
        &mut self,
        delta: f32,
        count: usize,
        available: f32,
        mut width: impl FnMut(usize) -> f32,
    ) {
        if count == 0 {
            return;
        }
        let total = (0..count)
            .map(|index| width(index) + GROUP_GAP)
            .sum::<f32>()
            - GROUP_GAP;
        let start = (0..self.first)
            .map(|index| width(index) + GROUP_GAP)
            .sum::<f32>();
        let offset = (start + self.clipped + delta).clamp(0., (total - available).max(0.));
        self.first = 0;
        self.clipped = offset;
        while self.first + 1 < count {
            let step = width(self.first) + GROUP_GAP;
            if self.clipped < step {
                break;
            }
            self.clipped -= step;
            self.first += 1;
        }
    }

    fn reveal(
        &mut self,
        index: usize,
        last: usize,
        available: f32,
        mut width: impl FnMut(usize) -> f32,
    ) {
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

struct LaneBounds {
    switch_width: f32,
    roster_x: f32,
    roster_width: f32,
    groups_x: f32,
    available: f32,
}

impl LaneBounds {
    fn new(layout: Layout, window: &Window) -> Self {
        let switch_width = 118.
            + measure(layout, window, &"Now".into(), 18., 600.)
            + measure(layout, window, &"Timeline".into(), 18., 600.);
        let roster_width = 62. + measure(layout, window, &"Choose groups".into(), 20., 600.);
        let roster_x = layout.width - GUTTER - roster_width;
        let groups_x = GUTTER + switch_width + 25.5;
        Self {
            switch_width,
            roster_x,
            roster_width,
            groups_x,
            available: (roster_x - 25.5 - groups_x).max(0.),
        }
    }
}

fn control_y(layout: Layout) -> f32 {
    (layout.lane_height() - CONTROL_HEIGHT) / 2.
}

/// The lane control under a point in layout units, mirroring `render`.
pub(crate) fn lane_hit(
    layout: Layout,
    screen: &GuideScreen,
    viewport: &mut LaneViewport,
    window: &Window,
    [x, y]: [f32; 2],
) -> Option<LaneFocus> {
    let y = y - layout.band_bottom() - control_y(layout);
    if !(0. ..CONTROL_HEIGHT).contains(&y) {
        return None;
    }
    let bounds = LaneBounds::new(layout, window);
    if (GUTTER..GUTTER + bounds.switch_width).contains(&x) {
        return Some(LaneFocus::Phase);
    }
    if (bounds.roster_x..bounds.roster_x + bounds.roster_width).contains(&x) {
        return Some(LaneFocus::ChooseGroups);
    }
    let x = x - bounds.groups_x;
    if !(0. ..bounds.available).contains(&x) {
        return None;
    }
    pill_slots(layout, screen, viewport, window, bounds.available)
        .into_iter()
        .find(|slot| (slot.left..slot.left + slot.width).contains(&x))
        .map(|slot| match slot.index {
            0 => LaneFocus::All,
            index => LaneFocus::Group(index - 1),
        })
}

/// Scrolls the Channel Group pills sideways; a vertical wheel lands here too.
pub(crate) fn scroll_lane(
    layout: Layout,
    screen: &GuideScreen,
    viewport: &mut LaneViewport,
    window: &Window,
    delta: f32,
) {
    let available = LaneBounds::new(layout, window).available;
    let groups = screen.groups();
    sync_measurements(layout, screen, viewport);
    let LaneViewport { widths, scroll, .. } = viewport;
    scroll.scroll_by(delta, groups.len() + 1, available, |index| {
        let pill = GroupPill::new(index.checked_sub(1).map(|index| &groups[index]));
        measured(widths, &pill, layout, window).min(available)
    });
}

struct PillSlot {
    index: usize,
    left: f32,
    width: f32,
    content_width: f32,
}

fn sync_measurements(layout: Layout, screen: &GuideScreen, viewport: &mut LaneViewport) {
    if viewport.generation != screen.generation() {
        *viewport = LaneViewport {
            generation: screen.generation(),
            ..Default::default()
        };
    }
    if viewport.measured_scale != layout.unit {
        viewport.widths.clear();
        viewport.measured_scale = layout.unit;
    }
}

fn measured(
    widths: &mut std::collections::HashMap<(SharedString, Option<SharedString>), f32>,
    pill: &GroupPill,
    layout: Layout,
    window: &Window,
) -> f32 {
    *widths
        .entry((pill.label.clone(), pill.count.clone()))
        .or_insert_with(|| pill.width(layout, window))
}

/// The pills that fit after the scroll position, in row coordinates.
fn pill_slots(
    layout: Layout,
    screen: &GuideScreen,
    viewport: &mut LaneViewport,
    window: &Window,
    available: f32,
) -> Vec<PillSlot> {
    sync_measurements(layout, screen, viewport);
    let groups = screen.groups();
    let mut slots = Vec::new();
    let mut left = -viewport.scroll.clipped;
    for index in viewport.scroll.first..=groups.len() {
        let pill = GroupPill::new(index.checked_sub(1).map(|index| &groups[index]));
        let content_width = measured(&mut viewport.widths, &pill, layout, window);
        let width = content_width.min(available);
        slots.push(PillSlot {
            index,
            left,
            width,
            content_width,
        });
        left += width + GROUP_GAP;
        if left >= available {
            break;
        }
    }
    slots
}

pub(crate) fn render(
    layout: Layout,
    screen: &GuideScreen,
    viewport: &mut LaneViewport,
    window: &Window,
    (slide, wave): (super::SlideFrame, super::arrival::Wave),
    hover: Option<LaneFocus>,
) -> Div {
    let lane_focus = match layout.shown_focus(screen.focus()) {
        ScreenFocus::GroupLane(focus) => Some(focus),
        _ => None,
    };
    let LaneBounds {
        switch_width,
        roster_x,
        roster_width,
        groups_x,
        available,
    } = LaneBounds::new(layout, window);
    let control_y = control_y(layout);

    layout
        .rect(0., layout.band_bottom(), layout.width, layout.lane_height())
        .child(
            view_switch(
                layout,
                wave.enter(
                    layout,
                    layout.rect(GUTTER, control_y, switch_width, CONTROL_HEIGHT),
                    102.,
                    0.,
                    16.,
                ),
                GuideMode::Now,
                lane_focus == Some(LaneFocus::Phase),
            )
            .when(hover == Some(LaneFocus::Phase), |switch| {
                switch.child(super::hover_rim(layout, CONTROL_HEIGHT / 2.))
            }),
        )
        .child(groups(
            layout,
            screen,
            viewport,
            window,
            (groups_x, available),
            (lane_focus, hover),
            (slide, wave),
        ))
        .child(
            choose_groups(
                layout,
                wave.enter(
                    layout,
                    layout.rect(roster_x, control_y, roster_width, CONTROL_HEIGHT),
                    216.,
                    0.,
                    16.,
                ),
                lane_focus == Some(LaneFocus::ChooseGroups),
            )
            .when(hover == Some(LaneFocus::ChooseGroups), |roster| {
                roster.bg(super::layout_hover_fill())
            }),
        )
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
    (focus, hover): (Option<LaneFocus>, Option<LaneFocus>),
    (slide, wave): (super::SlideFrame, super::arrival::Wave),
) -> Div {
    let (x, available) = bounds;
    let hovered = hover.and_then(group_pill_index);
    let groups = screen.groups();
    let selected = group_pill_index(screen.selected_lane()).unwrap_or(0);
    let focused = focus.and_then(group_pill_index);
    let pill = |index: usize| GroupPill::new(index.checked_sub(1).map(|index| &groups[index]));

    let mut row = layout
        .rect(x, control_y(layout), available, CONTROL_HEIGHT + 16.)
        .overflow_hidden();
    sync_measurements(layout, screen, viewport);
    {
        let LaneViewport { widths, scroll, .. } = &mut *viewport;
        scroll.update(selected, focused, groups.len() + 1, available, |index| {
            measured(widths, &pill(index), layout, window).min(available)
        });
    }
    let mut pills = Vec::new();
    for slot in pill_slots(layout, screen, viewport, window, available) {
        let PillSlot {
            index,
            left,
            width,
            content_width,
        } = slot;
        let pill = pill(index);
        let enter = |rect| {
            wave.enter(
                layout,
                rect,
                118. + 20. * (index - viewport.scroll.first) as f32,
                0.,
                16.,
            )
        };
        if selected == index {
            let [x, w] = viewport.ink.bounds(selected, [left, width], slide);
            row = row.child(enter(super::group_slide::GroupInk::pill(layout.rect(
                x,
                0.,
                w,
                CONTROL_HEIGHT,
            ))));
        }
        pills.push(pill.render(
            layout,
            enter(layout.rect(left, 0., width, CONTROL_HEIGHT)),
            selected == index,
            focused == Some(index),
            content_width > available,
            hovered == Some(index),
        ));
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
        hovered: bool,
    ) -> Div {
        let (text, count, fill) = if focused {
            (rgb(ON_ICE), rgba(ON_ICE_DIM), rgb(ICE))
        } else if selected {
            (rgb(TEXT), layout.secondary(DIM), rgba(0))
        } else if hovered {
            (rgb(TEXT), layout.secondary(DIM), super::layout_hover_fill())
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
    measure_in(layout, window, SAIRA, label, size, weight)
}

/// A single line's advance in layout units.
pub(super) fn measure_in(
    layout: Layout,
    window: &Window,
    family: &'static str,
    label: &SharedString,
    size: f32,
    weight: f32,
) -> f32 {
    let mut font = font(family);
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
    f32::from(line.width()) / layout.unit
}

#[cfg(test)]
mod tests {
    use super::{GROUP_GAP, LaneScroll};

    #[test]
    fn horizontal_lane_reveals_the_selected_group_then_keeps_the_focused_group_visible() {
        let mut viewport = LaneScroll::default();
        let width = |_| 80.;
        let available = 256.;
        let scroll =
            |viewport: &LaneScroll| viewport.first as f32 * (80. + GROUP_GAP) + viewport.clipped;

        viewport.update(12, None, 20, available, width);
        assert_eq!(scroll(&viewport), 880.);
        viewport.update(12, Some(11), 20, available, width);
        assert_eq!(scroll(&viewport), 880.);

        viewport.update(12, Some(16), 20, available, width);
        assert_eq!(scroll(&viewport), 1216.);
        viewport.update(12, Some(16), 21, available, width);
        assert_eq!(scroll(&viewport), 1216.);
        viewport.update(12, None, 21, available, width);
        assert_eq!(scroll(&viewport), 1216.);
        viewport.update(12, None, 22, available, width);
        assert_eq!(scroll(&viewport), 960.);
    }
}
