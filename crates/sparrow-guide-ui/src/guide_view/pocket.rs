//! The compact Pocket Guide's toolbar and its idle picture band.

use gpui::{Div, div, linear_color_stop, linear_gradient, prelude::*, rgb, rgba};
use sparrow_tv_guide::guide_screen::{ScreenFocus, ToolbarFocus};

use super::{
    BAND_TOP, CELL, CELL_ELAPSED, DIM, FAINT, ICE, Layout, ON_ICE, SAIRA, SAIRA_TITLE, TEXT,
    band::TOOLBAR_HEIGHT, layout::POCKET_GUTTER, playback::signal_mark,
};

const BUTTON: f32 = TOOLBAR_HEIGHT;
const GAP: f32 = 12.;
const CLOCK_WIDTH: f32 = 80.;
/// Room the toolbar keeps right of the Search pill.
pub(super) const TRAILING_WIDTH: f32 = 3. * (BUTTON + GAP) + GAP + CLOCK_WIDTH;

const APPEARANCE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="13.5" cy="6.5" r="1.5"/><circle cx="17.5" cy="10.5" r="1.5"/><circle cx="8.5" cy="7.5" r="1.5"/><circle cx="6.5" cy="12.5" r="1.5"/><path d="M12 2a10 10 0 0 0 0 20c1.1 0 2-.9 2-2 0-.5-.2-1-.5-1.3-.3-.4-.5-.8-.5-1.3 0-1.1.9-2 2-2h2.3A5.7 5.7 0 0 0 22 9.7C22 5.4 17.5 2 12 2z"/></svg>"#;

/// A compact toolbar control.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PocketTool {
    Search,
    Groups,
    Appearance,
    Sources,
}

const TOOLS: [PocketTool; 3] = [
    PocketTool::Groups,
    PocketTool::Appearance,
    PocketTool::Sources,
];

fn bounds(layout: Layout) -> [f32; 4] {
    [
        POCKET_GUTTER,
        BAND_TOP,
        (layout.width - 2. * POCKET_GUTTER).max(0.),
        TOOLBAR_HEIGHT,
    ]
}

/// The toolbar control under a Guide-region point, mirroring `toolbar`.
pub(crate) fn toolbar_hit(layout: Layout, [x, y]: [f32; 2]) -> Option<PocketTool> {
    let [left, top, width, height] = bounds(layout);
    if !(top..top + height).contains(&y) || !(left..left + width).contains(&x) {
        return None;
    }
    let buttons = left + width - TRAILING_WIDTH + GAP;
    if x < buttons - GAP {
        return Some(PocketTool::Search);
    }
    let slot = ((x - buttons) / (BUTTON + GAP)).floor();
    TOOLS
        .get(slot.max(0.) as usize)
        .copied()
        .filter(|_| x - buttons - slot * (BUTTON + GAP) < BUTTON)
}

pub(crate) fn toolbar(
    layout: Layout,
    focus: ScreenFocus,
    hover: Option<ToolbarFocus>,
    minute: super::MinuteFrame<'_>,
    inline_search: bool,
) -> Div {
    let [left, top, width, height] = bounds(layout);
    let search_focus = focus == ScreenFocus::Toolbar(ToolbarFocus::Search);
    let sources_focus = focus == ScreenFocus::Toolbar(ToolbarFocus::Sources);
    let search = div()
        .h(layout.px(height))
        .flex_1()
        .min_w_0()
        .when(!inline_search, |search| {
            search
                .rounded_full()
                .bg(layout.surface(CELL))
                .when(search_focus, |search| {
                    search.border(layout.px(3.)).border_color(rgb(ICE))
                })
                .when(
                    !search_focus && hover == Some(ToolbarFocus::Search),
                    |search| search.child(super::hover_rim(layout, height / 2.)),
                )
                .flex()
                .items_center()
                .pl(layout.px(20.))
                .pr(layout.px(8.))
                .gap(layout.px(12.))
                .child(layout.icon(
                    include_bytes!("../../assets/icons/search.svg"),
                    24.,
                    layout.secondary(DIM),
                ))
                .child(
                    layout
                        .text(SAIRA, 22., 28., 400.)
                        .text_color(layout.secondary(DIM))
                        .truncate()
                        .child("Search"),
                )
        });
    let button = |tool: PocketTool| {
        let focused = tool == PocketTool::Sources && sources_focus;
        let ink = if focused {
            rgb(ON_ICE)
        } else {
            layout.guide_text()
        };
        let icon: &'static [u8] = match tool {
            PocketTool::Groups => include_bytes!("../../assets/icons/choose-groups.svg"),
            PocketTool::Appearance => APPEARANCE,
            PocketTool::Sources | PocketTool::Search => {
                include_bytes!("../../assets/icons/sources.svg")
            }
        };
        div()
            .size(layout.px(BUTTON))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(if focused {
                rgb(ICE)
            } else {
                layout.surface(CELL_ELAPSED)
            })
            .when(
                !focused && tool == PocketTool::Sources && hover == Some(ToolbarFocus::Sources),
                |button| button.child(super::hover_rim(layout, BUTTON / 2.)),
            )
            .child(layout.icon(icon, 26., ink))
    };
    layout
        .rect(left, top, width, height)
        .flex()
        .items_center()
        .gap(layout.px(GAP))
        .child(search)
        .children(TOOLS.map(button))
        .child(
            div()
                .w(layout.px(CLOCK_WIDTH))
                .flex_none()
                .flex()
                .justify_end()
                .child(minute.clock(layout, 26.)),
        )
}

/// The picture band before anything plays: the wordmark where the picture
/// will dock, and a prompt beside or under it.
pub(crate) fn idle_band(layout: Layout) -> Option<Div> {
    let pocket = layout.pocket().filter(|pocket| pocket.compact())?;
    let [x, y, w, h] = layout.m.dock;
    let [ix, iy, iw, ih] = pocket.info?;
    let mark = signal_mark::steady(
        layout,
        div()
            .relative()
            .flex_none()
            .w(layout.px(42.))
            .h(layout.px(24.)),
        ICE,
    );
    Some(
        div()
            .absolute()
            .inset_0()
            .child(
                layout
                    .rect(x, y, w, h)
                    .rounded(layout.px(8.))
                    .overflow_hidden()
                    .bg(linear_gradient(
                        160.,
                        linear_color_stop(layout.surface(CELL_ELAPSED), 0.),
                        linear_color_stop(layout.surface(CELL), 1.),
                    ))
                    .border(layout.px(1.))
                    .border_color(rgba(FAINT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(layout.px(10.))
                    .child(mark)
                    .child(
                        layout
                            .text(SAIRA_TITLE, 30., 36., 700.)
                            .text_color(rgb(TEXT))
                            .child("Sparrow"),
                    ),
            )
            .child(
                layout
                    .rect(ix, iy, iw, ih.min(h))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(layout.px(6.))
                    .child(
                        layout
                            .text(SAIRA_TITLE, 28., 34., 650.)
                            .text_color(layout.guide_text())
                            .truncate()
                            .child("Pick a channel"),
                    )
                    .child(
                        layout
                            .text(SAIRA, 21., 28., 400.)
                            .text_color(layout.secondary(DIM))
                            .whitespace_normal()
                            .line_clamp(2)
                            .child("Tap a row to start watching."),
                    ),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(width: f32) -> Layout {
        let mut layout = Layout::new(
            super::super::Profile::Tv,
            gpui::size(gpui::px(1920.), gpui::px(1080.)),
        );
        layout.width = width;
        layout
    }

    #[test]
    fn toolbar_presses_map_to_the_control_drawn_there() {
        let layout = region(686.);
        let y = BAND_TOP + 20.;
        let right = 686. - POCKET_GUTTER;
        let first = right - TRAILING_WIDTH + GAP;
        assert_eq!(
            toolbar_hit(layout, [POCKET_GUTTER + 5., y]),
            Some(PocketTool::Search)
        );
        assert_eq!(
            toolbar_hit(layout, [first + 5., y]),
            Some(PocketTool::Groups)
        );
        assert_eq!(
            toolbar_hit(layout, [first + BUTTON + GAP + 5., y]),
            Some(PocketTool::Appearance)
        );
        assert_eq!(
            toolbar_hit(layout, [first + 2. * (BUTTON + GAP) + 5., y]),
            Some(PocketTool::Sources)
        );
        // The gaps, the clock and the rows outside the toolbar take no press.
        assert_eq!(toolbar_hit(layout, [first + BUTTON + 2., y]), None);
        assert_eq!(toolbar_hit(layout, [right - 10., y]), None);
        assert_eq!(
            toolbar_hit(layout, [POCKET_GUTTER + 5., BAND_TOP - 2.]),
            None
        );
    }
}
