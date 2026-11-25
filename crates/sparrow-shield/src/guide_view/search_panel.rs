//! Read-only presentation of the two television Search surfaces.

use std::ops::Range;

use chrono::{DateTime, Utc};
use gpui::{
    BoxShadow, Div, Pixels, Size, div, linear_color_stop, linear_gradient, prelude::*, rgb, rgba,
};
use sparrow_core::{ChannelIcon, ProgrammeSearchHit};
use sparrow_tv_guide::{
    GuideFamily,
    search::{SearchChannelRow, SearchState, SearchStatus, SearchView},
    time::minutes_left,
};

use crate::{channel_icons::ChannelIcons, search_flow::SearchFocus};

use super::{
    CANVAS_WIDTH, CELL, CELL_ELAPSED, DIM, FAINT, ICE, INK, Layout, MUTE, ON_ICE, ON_ICE_DIM,
    PANEL, SAIRA, TEXT,
    band::{BAND_TOP, TOOLBAR_HEIGHT, panel_bounds},
    board::quality_chip,
    channel_icon::{Plate, plate},
    clock_time, programme_title,
};

const SEARCH: &[u8] = include_bytes!("../../assets/icons/search.svg");
const CLOSE: &[u8] = include_bytes!("../../assets/icons/close.svg");
const SEARCH_AGAIN: &[u8] = include_bytes!("../../assets/icons/search-again.svg");
const RESULT_TITLE: &str = "Sparrow Guide 84";
const DESK_TITLE: &str = "Sparrow Guide 76";

// The toolbar's Sources control, tabular clock, and gaps.
const INLINE_TRAILING_WIDTH: f32 = 259.4;
const INLINE_TEXT_LEFT: f32 = 54.;
const INLINE_CLEAR_INSET: f32 = 50.;
const INLINE_POPUP_Y: f32 = BAND_TOP + TOOLBAR_HEIGHT + 14.;
const INLINE_ROW_HEIGHT: f32 = 56.;
const INLINE_PITCH: f32 = INLINE_ROW_HEIGHT + 2.;
const INLINE_PADDING: f32 = 8.;

const DESK_WIDTH: f32 = CANVAS_WIDTH * 0.36;
const DESK_X: f32 = CANVAS_WIDTH - DESK_WIDTH;
const DESK_PADDING: f32 = 30.;
const DESK_CONTENT_WIDTH: f32 = DESK_WIDTH - DESK_PADDING * 2.;
const DESK_FILTER_Y: f32 = 126.2;
const DESK_FILTER_HEIGHT: f32 = 56.;
const DESK_LIST_Y: f32 = DESK_FILTER_Y + DESK_FILTER_HEIGHT + 18.;
const DESK_ROW_HEIGHT: f32 = 78.;
const DESK_PITCH: f32 = DESK_ROW_HEIGHT + 4.;
const DESK_FOOTER_SPACE: f32 = 96.;
const INCLUDE_WIDTH: f32 = 140.6;

pub(in super::super) struct SearchPanel<'a> {
    pub(in super::super) choice: sparrow_installed_catalog::GuideBackdrop,
    pub(in super::super) state: &'a SearchState,
    pub(in super::super) display_term: &'a str,
    pub(in super::super) focus: SearchFocus,
    pub(in super::super) editing: bool,
    pub(in super::super) has_exclusions: bool,
    pub(in super::super) docked: bool,
    pub(in super::super) families: &'a [GuideFamily],
    pub(in super::super) now: DateTime<Utc>,
}

#[derive(Default)]
pub(in super::super) struct SearchViewport {
    inline_first: usize,
    desk_first: usize,
}

pub(in super::super) fn search_panel<'a>(
    size: Size<Pixels>,
    panel: SearchPanel<'a>,
    viewport: &mut SearchViewport,
    icons: &ChannelIcons,
) -> (Option<Div>, Vec<&'a ChannelIcon>) {
    let layout = Layout::for_guide(size, panel.choice);
    let (surface, visible_icons) = match panel.state.view() {
        SearchView::Closed => {
            *viewport = SearchViewport::default();
            return (None, Vec::new());
        }
        SearchView::Inline => {
            viewport.desk_first = 0;
            inline(layout, &panel, &mut viewport.inline_first, icons)
        }
        SearchView::Desk => {
            viewport.inline_first = 0;
            desk(layout, &panel, &mut viewport.desk_first, icons)
        }
    };
    (Some(surface), visible_icons)
}

struct InlineGeometry {
    x: f32,
    width: f32,
    field_height: f32,
}

impl InlineGeometry {
    fn new(layout: Layout, docked: bool) -> Self {
        let (x, toolbar_width) = panel_bounds(docked);
        Self {
            x,
            width: (toolbar_width - INLINE_TRAILING_WIDTH).max(0.),
            field_height: (layout.height - BAND_TOP).clamp(0., TOOLBAR_HEIGHT),
        }
    }

    fn editor_rect(&self, scale: f32) -> [i32; 4] {
        let physical = |length: f32| (length * scale).round() as i32;
        let left = physical(self.x + INLINE_TEXT_LEFT);
        let top = physical(BAND_TOP);
        [
            left,
            top,
            (physical(self.x + self.width - INLINE_CLEAR_INSET) - left).max(0),
            physical(BAND_TOP + self.field_height) - top,
        ]
    }
}

/// The native editor takes the text area while GPUI keeps the surrounding field.
pub(in super::super) fn search_editor_rect(
    size: Size<Pixels>,
    pixel_scale: f32,
    state: &SearchState,
    has_exclusions: bool,
    docked: bool,
) -> [i32; 4] {
    let layout = Layout::new(size);
    let scale = layout.scale * pixel_scale;
    match state.view() {
        SearchView::Closed => [0; 4],
        SearchView::Inline => InlineGeometry::new(layout, docked).editor_rect(scale),
        SearchView::Desk => [
            DESK_X + DESK_PADDING + 55.,
            DESK_FILTER_Y,
            (desk_field_width(state, has_exclusions) - 76.).max(0.),
            DESK_FILTER_HEIGHT,
        ]
        .map(|length| (length * scale).round() as i32),
    }
}

fn inline<'a>(
    layout: Layout,
    panel: &SearchPanel<'a>,
    first: &mut usize,
    icons: &ChannelIcons,
) -> (Div, Vec<&'a ChannelIcon>) {
    let mut overlay = div().absolute().inset_0();
    let geometry = InlineGeometry::new(layout, panel.docked);
    let rows = inline_rows(panel.state, panel.display_term);
    let visible_icons = if rows.is_empty() {
        *first = 0;
        Vec::new()
    } else {
        let slice = inline_slice(layout, panel.focus, &rows, *first);
        *first = slice.rendered.start;
        let visible_icons = rows[slice.visible.clone()]
            .iter()
            .filter_map(InlineRow::icon)
            .collect();
        overlay = overlay.bg(rgba(0x060a12b8)).child(inline_popup(
            layout,
            &geometry,
            panel.focus,
            &rows,
            slice,
            icons,
        ));
        visible_icons
    };
    let field_focus = panel.focus == SearchFocus::Field || panel.editing;
    let rim = if field_focus { 3. } else { 0. };
    let field = field(
        layout,
        layout.rect(geometry.x, BAND_TOP, geometry.width, geometry.field_height),
        panel,
        field_focus,
        false,
    )
    .when(!panel.display_term.is_empty(), |field| {
        let clear_focus = panel.focus == SearchFocus::Clear;
        field.child(
            layout
                .rect(
                    geometry.width - INLINE_CLEAR_INSET - rim,
                    8. - rim,
                    42.,
                    42.,
                )
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .when(clear_focus, |clear| clear.bg(rgb(ICE)))
                .child(layout.icon(
                    CLOSE,
                    20.,
                    if clear_focus { rgb(ON_ICE) } else { rgba(DIM) },
                )),
        )
    });
    (overlay.child(field), visible_icons)
}

enum InlineRow<'a> {
    Desk,
    Retry,
    Channel(&'a SearchChannelRow),
    Programme(usize, &'a ProgrammeSearchHit),
    Notice(&'static str),
}

impl<'a> InlineRow<'a> {
    fn focus(&self) -> Option<SearchFocus> {
        match self {
            Self::Desk => Some(SearchFocus::Desk),
            Self::Retry => Some(SearchFocus::Retry),
            Self::Channel(row) => Some(SearchFocus::Channel(row.channel().number())),
            Self::Programme(index, _) => Some(SearchFocus::Programme(*index)),
            Self::Notice(_) => None,
        }
    }

    fn icon(&self) -> Option<&'a ChannelIcon> {
        match self {
            Self::Channel(hit) => hit.channel().icon(),
            Self::Programme(_, hit) => hit.channel().icon(),
            _ => None,
        }
    }
}

fn inline_rows<'a>(state: &'a SearchState, term: &str) -> Vec<InlineRow<'a>> {
    use SearchStatus::*;
    let status = state.status();
    if term.trim().is_empty() || status == Idle {
        return Vec::new();
    }
    if matches!(status, Invalid | Unavailable) {
        return status_text(status, SearchView::Inline)
            .map(InlineRow::Notice)
            .into_iter()
            .collect();
    }

    let mut rows = Vec::with_capacity(18);
    if status == ReconcileGeneration {
        rows.push(InlineRow::Retry);
    }
    rows.push(InlineRow::Desk);
    if status == Ready {
        rows.extend(state.channels().iter().map(InlineRow::Channel));
        rows.extend(
            state
                .programmes()
                .iter()
                .enumerate()
                .map(|(index, hit)| InlineRow::Programme(index, hit)),
        );
    } else if let Some(message) = status_text(status, SearchView::Inline) {
        rows.push(InlineRow::Notice(message));
    }
    rows
}

struct SearchSlice {
    height: f32,
    visible: Range<usize>,
    rendered: Range<usize>,
}

fn inline_slice(
    layout: Layout,
    focus: SearchFocus,
    rows: &[InlineRow<'_>],
    mut first: usize,
) -> SearchSlice {
    let all_rows = rows.len() as f32 * INLINE_PITCH - 2.;
    let available = (layout.height - INLINE_POPUP_Y).clamp(0., 720.);
    let height = (all_rows + INLINE_PADDING * 2.).min(available);
    let capacity = ((height - INLINE_PADDING * 2. + 2.) / INLINE_PITCH)
        .floor()
        .max(1.) as usize;
    let target = rows.iter().position(|row| row.focus() == Some(focus));
    reveal(&mut first, rows.len(), capacity, target);
    let visible_rows = ((height - (INLINE_PADDING - 1.)) / INLINE_PITCH).ceil() as usize;
    let rendered_end = (first + capacity + 1).min(rows.len());
    SearchSlice {
        height,
        visible: first..(first + visible_rows).min(rendered_end),
        rendered: first..rendered_end,
    }
}

fn inline_popup(
    layout: Layout,
    geometry: &InlineGeometry,
    focus: SearchFocus,
    rows: &[InlineRow<'_>],
    slice: SearchSlice,
    icons: &ChannelIcons,
) -> Div {
    layout
        .rect(geometry.x, INLINE_POPUP_Y, geometry.width, slice.height)
        .rounded(layout.px(12.))
        .bg(layout.surface(PANEL))
        .shadow(vec![
            BoxShadow::new(layout.px(0.), layout.px(24.), rgba(0x00000073).into())
                .blur_radius(layout.px(60.)),
        ])
        .border(layout.px(1.))
        .border_color(rgba(FAINT))
        .overflow_hidden()
        .children(rows[slice.rendered].iter().enumerate().map(|(row, item)| {
            let y = INLINE_PADDING - 1. + row as f32 * INLINE_PITCH;
            let row_rect = layout.rect(
                INLINE_PADDING - 1.,
                y,
                geometry.width - INLINE_PADDING * 2.,
                INLINE_ROW_HEIGHT,
            );
            inline_row(layout, row_rect, item, item.focus() == Some(focus), icons)
        }))
}

fn inline_row(
    layout: Layout,
    row: Div,
    item: &InlineRow<'_>,
    focused: bool,
    icons: &ChannelIcons,
) -> Div {
    let ink = if focused { rgb(ON_ICE) } else { rgb(TEXT) };
    let supporting = if focused { rgba(ON_ICE_DIM) } else { rgba(DIM) };
    let icon = item.icon().and_then(|location| icons.get(location));
    let (lead, name, detail) = match item {
        InlineRow::Desk => (
            action_icon(layout, SEARCH, supporting),
            result_title(layout, "Open full channel search", ink),
            "All matches".to_owned(),
        ),
        InlineRow::Retry => (
            action_icon(layout, SEARCH_AGAIN, supporting),
            result_title(layout, "Search again", ink),
            "Reload channels".to_owned(),
        ),
        InlineRow::Channel(hit) => (
            plate(
                layout,
                Plate::InlineSearch,
                hit.channel().number(),
                focused,
                icon,
            ),
            channel_title(layout, hit, focused, ink, icon.is_some()),
            group_name(hit.channel().group()).to_owned(),
        ),
        InlineRow::Programme(_, hit) => (
            plate(
                layout,
                Plate::InlineSearch,
                hit.channel().number(),
                focused,
                icon,
            ),
            result_title(layout, hit.title(), ink),
            format!(
                "{}, {} to {}",
                hit.channel().name(),
                clock_time(hit.starts_at()),
                clock_time(hit.ends_at()),
            ),
        ),
        InlineRow::Notice(message) => {
            return row.flex().items_center().px(layout.px(18.)).child(
                layout
                    .text(SAIRA, 18., 24., 400.)
                    .text_color(rgba(DIM))
                    .child(*message),
            );
        }
    };
    row.rounded(layout.px(8.))
        .when(focused, |row| row.bg(rgb(ICE)))
        .pl(layout.px(7.))
        .pr(layout.px(18.))
        .gap(layout.px(16.))
        .flex()
        .items_center()
        .child(lead)
        .child(div().min_w_0().flex_1().child(name))
        .child(
            layout
                .text(SAIRA, 16., 22., 400.)
                .max_w(layout.px(336.))
                .text_color(supporting)
                .truncate()
                .child(detail),
        )
}

fn desk<'a>(
    layout: Layout,
    panel: &SearchPanel<'a>,
    first: &mut usize,
    icons: &ChannelIcons,
) -> (Div, Vec<&'a ChannelIcon>) {
    let close_focus = panel.focus == SearchFocus::Close;
    let filter_width = desk_field_width(panel.state, panel.has_exclusions);
    let toolbar = layout
        .rect(
            DESK_PADDING,
            DESK_FILTER_Y,
            DESK_CONTENT_WIDTH,
            DESK_FILTER_HEIGHT,
        )
        .flex()
        .items_center()
        .gap(layout.px(18.))
        .pl(layout.px(filter_width + 18.))
        .child(desk_tally(layout, panel.state))
        .when(panel.has_exclusions, |toolbar| {
            let include = panel.state.include_hidden();
            toolbar.child(desk_button(
                layout,
                div().w(layout.px(INCLUDE_WIDTH)),
                "Include hidden",
                panel.focus == SearchFocus::IncludeHidden,
                include,
            ))
        });

    let (list, visible_icons) = desk_list(layout, panel, first, icons);
    let sheet = layout
        .rect(DESK_X, 0., DESK_WIDTH, layout.height)
        .bg(layout.surface(PANEL))
        .text_color(rgb(TEXT))
        .shadow(vec![
            BoxShadow::new(layout.px(-30.), layout.px(0.), rgba(0x00000066).into())
                .blur_radius(layout.px(60.)),
        ])
        .child(
            layout
                .rect(DESK_PADDING, 30., DESK_CONTENT_WIDTH - 72., 80.)
                .child(
                    layout
                        .text(DESK_TITLE, 40., 42., 700.)
                        .child("Find a channel"),
                )
                .child(
                    layout
                        .text(SAIRA, 18., 25.2, 400.)
                        .mt(layout.px(8.))
                        .text_color(rgba(DIM))
                        .child("Search every channel by name. Hidden groups are left out."),
                ),
        )
        .child(
            layout
                .rect(DESK_WIDTH - 78., 30., 48., 48.)
                .rounded_full()
                .bg(rgb(if close_focus { ICE } else { CELL }))
                .flex()
                .items_center()
                .justify_center()
                .child(layout.icon(CLOSE, 22., rgb(if close_focus { ON_ICE } else { TEXT }))),
        )
        .child(toolbar)
        .child(field(
            layout,
            layout.rect(
                DESK_PADDING,
                DESK_FILTER_Y,
                filter_width,
                DESK_FILTER_HEIGHT,
            ),
            panel,
            panel.focus == SearchFocus::Field || panel.editing,
            true,
        ))
        .child(list);
    (
        div().absolute().inset_0().bg(rgba(0x050910b3)).child(sheet),
        visible_icons,
    )
}

fn desk_field_width(state: &SearchState, has_exclusions: bool) -> f32 {
    // Advances measured from the bundled Saira 92 at 16 units. Digits are tabular.
    let digits = |count: usize| {
        count
            .checked_ilog10()
            .map_or(1., |length| (length + 1) as f32)
    };
    let mut tally = 43.84 + 9.25 * digits(state.channels().len());
    if state.hidden_count() != 0 {
        tally += 57.6 + 9.25 * digits(state.hidden_count());
    }
    let controls = if has_exclusions {
        18. + INCLUDE_WIDTH
    } else {
        0.
    };
    (DESK_CONTENT_WIDTH - tally - 18. - controls).max(0.)
}

fn desk_tally(layout: Layout, state: &SearchState) -> Div {
    let count = |value: usize| {
        layout
            .text(SAIRA, 16., 24., 600.)
            .text_color(rgb(TEXT))
            .child(value.to_string())
    };
    layout
        .text(SAIRA, 16., 24., 400.)
        .text_color(rgba(DIM))
        .flex_none()
        .flex()
        .items_center()
        .child(count(state.channels().len()))
        .child(" found")
        .when(state.hidden_count() != 0, |tally| {
            tally
                .child(", ")
                .child(count(state.hidden_count()))
                .child(" hidden")
        })
}

fn desk_slice(
    layout: Layout,
    focus: SearchFocus,
    rows: &[SearchChannelRow],
    has_more: bool,
    mut first: usize,
) -> SearchSlice {
    let height = (layout.height - DESK_LIST_Y).max(0.);
    let capacity = ((height - DESK_FOOTER_SPACE + 4.) / DESK_PITCH)
        .floor()
        .max(1.) as usize;
    let target = match focus {
        SearchFocus::Channel(number) => {
            rows.iter().position(|row| row.channel().number() == number)
        }
        SearchFocus::More if has_more => Some(rows.len()),
        _ => None,
    };
    reveal(&mut first, rows.len() + 1, capacity, target);
    let visible_rows = (height / DESK_PITCH).ceil() as usize;
    SearchSlice {
        height,
        visible: first..(first + visible_rows).min(rows.len()),
        rendered: first..(first + visible_rows + 1).min(rows.len()),
    }
}

fn desk_list<'a>(
    layout: Layout,
    panel: &SearchPanel<'a>,
    first: &mut usize,
    icons: &ChannelIcons,
) -> (Div, Vec<&'a ChannelIcon>) {
    let height = (layout.height - DESK_LIST_Y).max(0.);
    let list = layout
        .rect(DESK_PADDING, DESK_LIST_Y, DESK_CONTENT_WIDTH, height)
        .overflow_hidden();
    let status = panel.state.status();
    if status != SearchStatus::Ready {
        *first = 0;
        let message = status_text(status, SearchView::Desk).unwrap_or_default();
        let retry = match status {
            SearchStatus::Failed => Some("Try again"),
            SearchStatus::ReconcileGeneration => Some("Search again"),
            _ => None,
        };
        let notice = list.child(
            div()
                .flex()
                .flex_col()
                .items_start()
                .pt(layout.px(12.))
                .gap(layout.px(10.))
                .child(
                    layout
                        .text(SAIRA, 17., 24., 500.)
                        .text_color(rgba(DIM))
                        .child(message),
                )
                .when_some(retry, |notice, label| {
                    notice.child(desk_button(
                        layout,
                        div(),
                        label,
                        panel.focus == SearchFocus::Retry,
                        false,
                    ))
                }),
        );
        return (notice, Vec::new());
    }
    let rows = panel.state.channels();
    let slice = desk_slice(layout, panel.focus, rows, panel.state.has_more(), *first);
    *first = slice.rendered.start;
    let footer_y = (rows.len() - *first) as f32 * DESK_PITCH + 8.;
    let visible_icons = rows[slice.visible]
        .iter()
        .filter_map(|hit| hit.channel().icon())
        .collect();
    let list = list.children(rows[slice.rendered].iter().enumerate().map(|(index, hit)| {
        desk_row(layout, panel, hit, index as f32 * DESK_PITCH, icons)
    }))
    .when(footer_y < height, |list| {
        list.child(
            layout.rect(0., footer_y, DESK_CONTENT_WIDTH, 44.).child(
                if panel.state.has_more() {
                    desk_button(
                        layout,
                        div(),
                        if panel.state.loading_more() { "Loading more channels…" } else { "More channels" },
                        panel.focus == SearchFocus::More,
                        false,
                    )
                } else {
                    layout
                        .text(SAIRA, 16., 22.4, 400.)
                        .px(layout.px(10.))
                        .text_color(rgba(MUTE))
                        .child("That is every channel found. To search again, go up to the search field and press OK.")
                },
            ),
        )
    })
    .child(
        layout.rect(0., (height - 84.).max(0.), DESK_CONTENT_WIDTH, 84.)
            .bg(linear_gradient(
                180.,
                linear_color_stop(rgba(PANEL << 8), 0.),
                linear_color_stop(rgb(PANEL), 0.7),
            )),
    );
    (list, visible_icons)
}

fn desk_row(
    layout: Layout,
    panel: &SearchPanel<'_>,
    hit: &SearchChannelRow,
    y: f32,
    icons: &ChannelIcons,
) -> Div {
    let number = hit.channel().number();
    let focused = panel.focus == SearchFocus::Channel(number);
    let supporting = if focused { rgba(ON_ICE_DIM) } else { rgba(DIM) };
    let icon = hit
        .channel()
        .icon()
        .and_then(|location| icons.get(location));
    let now = panel
        .families
        .binary_search_by_key(&number, GuideFamily::number)
        .ok()
        .and_then(|index| hit.programme_on_now(&panel.families[index], panel.now));
    let ink = rgb(if focused { ON_ICE } else { TEXT });
    layout
        .rect(0., y, DESK_CONTENT_WIDTH, DESK_ROW_HEIGHT)
        .rounded(layout.px(8.))
        .bg(rgb(if focused { ICE } else { CELL }))
        .flex()
        .items_center()
        .pl(layout.px(10.))
        .pr(layout.px(21.))
        .gap(layout.px(12.))
        .child(plate(layout, Plate::SearchDesk, number, focused, icon).mr(layout.px(4.)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(layout.px(8.))
                .child(channel_title(layout, hit, focused, ink, icon.is_some()))
                .when_some(now, |body, programme| {
                    body.child(
                        layout
                            .text(SAIRA, 16., 18.4, 400.)
                            .text_color(supporting)
                            .flex()
                            .gap(layout.px(12.))
                            .child(div().min_w_0().truncate().child(programme_title(programme)))
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .gap(layout.px(12.))
                                    .text_color(if focused { supporting } else { rgba(MUTE) })
                                    .child("·")
                                    .child(format!(
                                        "{} min left",
                                        minutes_left(programme, panel.now)
                                    )),
                            ),
                    )
                }),
        )
        .when(hit.is_hidden(), |row| {
            row.child(
                layout
                    .text(SAIRA, 16., 22., 500.)
                    .flex_none()
                    .text_color(supporting)
                    .child("Hidden"),
            )
        })
        .child(
            layout
                .text(SAIRA, 16., 16., 400.)
                .max_w(layout.px(172.))
                .pl(layout.px(12.))
                .flex_none()
                .truncate()
                .text_color(supporting)
                .child(group_name(hit.channel().group()).to_owned()),
        )
}

fn field(layout: Layout, field: Div, panel: &SearchPanel<'_>, focused: bool, desk: bool) -> Div {
    let rim = if focused {
        3.
    } else if desk {
        2.
    } else {
        0.
    };
    let label = if panel.display_term.is_empty() {
        if desk {
            "Search channels"
        } else {
            "Search channels and programmes"
        }
    } else {
        panel.display_term
    };
    let text_color = if panel.display_term.is_empty() {
        rgba(if desk { MUTE } else { DIM })
    } else {
        rgb(TEXT)
    };
    field
        .rounded_full()
        .bg(rgb(if desk { INK } else { CELL }))
        .flex()
        .items_center()
        .pl(layout.px(if desk { 21. - rim } else { 20. - rim }))
        .pr(layout.px(if desk { 21. - rim } else { 50. - rim }))
        .gap(layout.px(12.))
        .when(rim != 0., |field| {
            field
                .border(layout.px(rim))
                .border_color(if focused { rgb(ICE) } else { rgba(FAINT) })
        })
        .child(layout.icon(SEARCH, 22., rgba(DIM)))
        .when(!panel.editing, |field| {
            field.child(
                layout
                    .text(SAIRA, if desk { 21. } else { 20. }, 28., 400.)
                    .text_color(text_color)
                    .min_w_0()
                    .truncate()
                    .child(label.to_owned()),
            )
        })
}

fn desk_button(
    layout: Layout,
    rect: Div,
    label: &'static str,
    focused: bool,
    pressed: bool,
) -> Div {
    rect.h(layout.px(44.))
        .px(layout.px(20.))
        .rounded_full()
        .bg(rgb(if focused {
            ICE
        } else if pressed {
            0x243858
        } else {
            CELL_ELAPSED
        }))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            layout
                .text(SAIRA, 16., 22., 600.)
                .text_color(rgb(if focused { ON_ICE } else { TEXT }))
                .child(label),
        )
}

fn channel_title(
    layout: Layout,
    hit: &SearchChannelRow,
    focused: bool,
    ink: gpui::Rgba,
    has_icon: bool,
) -> Div {
    let current = hit.channel().id();
    div()
        .flex()
        .items_center()
        .min_w_0()
        .gap(layout.px(10.))
        .when(has_icon, |name| {
            name.child(
                layout
                    .text(SAIRA, 20., 24., 600.)
                    .flex_none()
                    .text_color(if focused { rgba(ON_ICE_DIM) } else { rgba(DIM) })
                    .child(hit.channel().number().to_string()),
            )
        })
        .child(result_title(layout, hit.title(), ink))
        .when(hit.channel().variant().is_some(), |name| {
            name.child(div().flex_none().flex().gap(layout.px(4.)).children(
                hit.variants().iter().filter_map(|variant| {
                    Some(quality_chip(
                        layout,
                        variant.variant()?.quality(),
                        variant.id() == current,
                        focused,
                        false,
                        false,
                    ))
                }),
            ))
        })
}

fn result_title(layout: Layout, title: &str, ink: gpui::Rgba) -> Div {
    layout
        .text(RESULT_TITLE, 21., 24., 650.)
        .min_w_0()
        .truncate()
        .text_color(ink)
        .child(title.to_owned())
}

fn action_icon(layout: Layout, icon: &'static [u8], ink: gpui::Rgba) -> Div {
    div()
        .w(layout.px(56.))
        .flex_none()
        .flex()
        .justify_center()
        .child(layout.icon(icon, 21., ink))
}

fn reveal(first: &mut usize, count: usize, capacity: usize, target: Option<usize>) {
    *first = (*first).min(count.saturating_sub(capacity));
    if let Some(target) = target {
        *first = (*first).clamp((target + 1).saturating_sub(capacity), target);
    }
}

fn group_name(source_name: &str) -> &str {
    if source_name.is_empty() {
        "Ungrouped"
    } else {
        source_name
    }
}

fn status_text(status: SearchStatus, view: SearchView) -> Option<&'static str> {
    use SearchStatus::*;
    let desk = view == SearchView::Desk;
    match status {
        Idle if desk => Some("Type a channel name to search."),
        Invalid => Some("That search is too long. Shorten it and try again."),
        Unavailable => Some("Search is ready once the channels have loaded."),
        Loading => Some("Searching…"),
        ReconcileGeneration => Some("The channels changed while you searched."),
        Failed if desk => Some("Search is not available right now."),
        Failed => Some("Search is not available right now. Try again in a moment."),
        Hidden if desk => {
            Some("The only matches are in hidden groups. Choose Include hidden to see them.")
        }
        Hidden => Some("The only matches are in hidden groups."),
        Empty if desk => Some("No channel matches that search."),
        Empty => Some("Nothing matches that search."),
        Idle | Ready => None,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{px, size};
    use sparrow_tv_guide::search::{SearchState, SearchView};

    use crate::guide_view::playback::{FullscreenFrame, picture_rect};

    use super::{
        INLINE_POPUP_Y, InlineGeometry, InlineRow, Layout, SearchFocus, inline_slice,
        search_editor_rect,
    };

    #[test]
    fn inline_search_keeps_the_docked_picture_clear_even_on_a_short_viewport() {
        let mut state = SearchState::new(None);
        let _ = state.set_view(SearchView::Inline);
        let rows = [InlineRow::Desk];
        for (viewport, pixel_scale, has_results, idle_editor, docked_editor) in [
            (
                size(px(1920.), px(1080.)),
                1.,
                true,
                [474, 27, 1097, 58],
                [778, 27, 793, 58],
            ),
            (
                size(px(480.), px(20.)),
                2.,
                false,
                [237, 14, 548, 26],
                [389, 14, 396, 26],
            ),
        ] {
            let layout = Layout::new(viewport);
            let idle = InlineGeometry::new(layout, false);
            let docked = InlineGeometry::new(layout, true);
            let [picture_x, _, picture_width, _] =
                picture_rect(viewport, pixel_scale, FullscreenFrame::default());
            assert_eq!(idle.x, 420.);
            assert_eq!(docked.x, 724.);
            assert_eq!(idle.x + idle.width, docked.x + docked.width);
            assert!(docked.x * layout.scale * pixel_scale > (picture_x + picture_width) as f32);
            assert_eq!(
                !inline_slice(layout, SearchFocus::Desk, &rows, 0)
                    .visible
                    .is_empty(),
                has_results,
            );
            assert_eq!(
                search_editor_rect(viewport, pixel_scale, &state, false, false),
                idle_editor
            );
            assert_eq!(
                search_editor_rect(viewport, pixel_scale, &state, false, true),
                docked_editor
            );
        }
    }

    #[test]
    fn inline_icon_requests_stay_inside_the_rendered_clip() {
        let rows = [
            InlineRow::Desk,
            InlineRow::Retry,
            InlineRow::Notice("Three"),
            InlineRow::Notice("Four"),
        ];
        let short = Layout {
            scale: 1.,
            height: INLINE_POPUP_Y + 124.,
        };
        let slice = inline_slice(short, SearchFocus::Retry, &rows, 0);
        assert_eq!(slice.visible, 1..3);
        assert_eq!(slice.rendered, 1..3);

        let clipped = Layout {
            height: INLINE_POPUP_Y,
            ..short
        };
        let slice = inline_slice(clipped, SearchFocus::Retry, &rows, 0);
        assert!(slice.visible.is_empty());
        assert_eq!(slice.rendered, 1..3);
    }
}
