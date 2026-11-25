use chrono::{DateTime, FixedOffset};
use gpui::{Div, div, linear_color_stop, linear_gradient, prelude::*, rgb, rgba, svg};
use sparrow_tv_guide::{
    guide_screen::{ScreenFocus, ToolbarFocus},
    preview::{PreviewTiming, following_programmes, preview_timing},
};

use crate::channel_icons::ChannelIcons;

use super::{
    CANVAS_WIDTH, CELL, CELL_ELAPSED, DIM, FAINT, GUTTER, GuideBand, ICE, Layout, ON_ICE,
    PreviewSelection, SAIRA, SAIRA_CLOCK, SAIRA_PREVIEW, SAIRA_TITLE,
    board::quality_tags,
    channel_icon::{Plate, family_icon, plate},
    clock_time,
    playback::DOCK_PREVIEW_X,
    programme_title,
};

const POSTER_WIDTH: f32 = 336.;
pub(super) const BAND_TOP: f32 = 27.;
pub(super) const TOOLBAR_HEIGHT: f32 = 58.;
const IDLE_PREVIEW_X: f32 = GUTTER + POSTER_WIDTH + 44.;
const NO_GUIDE_DATA: &str = "No guide data. The channel still plays.";

pub(super) fn render(layout: Layout, band: GuideBand<'_>, icons: &ChannelIcons) -> Div {
    let GuideBand {
        focus,
        now,
        preview: shown,
        description,
        docked,
        inline_search,
        ..
    } = band;
    div()
        .when(!docked, |band| {
            band.child(poster(layout, shown.map(|preview| &preview.family), icons))
        })
        .child(toolbar(layout, focus, now, docked, inline_search))
        .when_some(shown, |band, shown| {
            band.child(preview(layout, shown, now, description, icons, docked))
        })
}

pub(super) fn panel_bounds(docked: bool) -> (f32, f32) {
    let left = if docked {
        DOCK_PREVIEW_X
    } else {
        IDLE_PREVIEW_X
    };
    (left, CANVAS_WIDTH - GUTTER - left)
}

fn toolbar(
    layout: Layout,
    focus: ScreenFocus,
    now: DateTime<FixedOffset>,
    docked: bool,
    inline_search: bool,
) -> Div {
    let (left, width) = panel_bounds(docked);
    let search_focus = focus == ScreenFocus::Toolbar(ToolbarFocus::Search);
    let sources_focus = focus == ScreenFocus::Toolbar(ToolbarFocus::Sources);
    let search =
        div()
            .h(layout.px(TOOLBAR_HEIGHT))
            .flex_1()
            .min_w_0()
            .when(!inline_search, |search| {
                search
                    .rounded_full()
                    .bg(layout.surface(CELL))
                    .when(search_focus, |search| {
                        search.border(layout.px(3.)).border_color(rgb(ICE))
                    })
                    .flex()
                    .items_center()
                    .pl(layout.px(if search_focus { 17. } else { 20. }))
                    .pr(layout.px(8.))
                    .gap(layout.px(12.))
                    .child(layout.icon(
                        include_bytes!("../../assets/icons/search.svg"),
                        22.,
                        layout.secondary(DIM),
                    ))
                    .child(
                        layout
                            .text(SAIRA, 20., 28., 400.)
                            .text_color(layout.secondary(DIM))
                            .truncate()
                            .child("Search channels and programmes"),
                    )
            });
    let sources_ink = if sources_focus {
        rgb(ON_ICE)
    } else {
        layout.guide_text()
    };
    let sources = layout
        .text(SAIRA, 20., 28., 500.)
        .text_color(sources_ink)
        .h(layout.px(48.))
        .px(layout.px(16.))
        .rounded_full()
        .flex_none()
        .flex()
        .items_center()
        .gap(layout.px(9.))
        .when(sources_focus, |sources| sources.bg(rgb(ICE)))
        .child(layout.icon(
            include_bytes!("../../assets/icons/sources.svg"),
            22.,
            sources_ink,
        ))
        .child("Sources");
    layout
        .rect(left, BAND_TOP, width, TOOLBAR_HEIGHT)
        .flex()
        .items_center()
        .gap(layout.px(20.))
        .child(search)
        .child(sources)
        .child(
            layout
                .text(SAIRA_CLOCK, 30., 30., 600.)
                .ml(layout.px(6.))
                .flex_none()
                .child(clock_time(now.to_utc())),
        )
}

fn poster(
    layout: Layout,
    family: Option<&sparrow_tv_guide::GuideFamily>,
    icons: &ChannelIcons,
) -> Div {
    let poster = layout
        .rect(GUTTER, BAND_TOP, POSTER_WIDTH, 404.)
        .rounded(layout.px(8.))
        .bg(linear_gradient(
            160.,
            linear_color_stop(layout.surface(CELL_ELAPSED), 0.),
            linear_color_stop(layout.surface(CELL), 1.),
        ))
        .overflow_hidden()
        .child(
            svg()
                .data(include_bytes!("../../assets/poster-flare.svg"))
                .absolute()
                .size_full()
                .text_color(rgb(ICE)),
        )
        .child(
            div()
                .absolute()
                .size_full()
                .rounded(layout.px(8.))
                .border(layout.px(1.))
                .border_color(rgba(FAINT)),
        );
    let Some(family) = family else {
        return poster.child(
            layout
                .rect(30., 290., 276., 84.)
                .flex()
                .flex_col()
                .justify_end()
                .gap(layout.px(4.))
                .child(layout.text(SAIRA_TITLE, 36., 52., 700.).child("Sparrow")),
        );
    };
    poster
        .child(
            plate(
                layout,
                Plate::Poster,
                family.number(),
                false,
                family_icon(family).and_then(|icon| icons.get(icon)),
            )
            .absolute()
            .left(layout.px(30.))
            .top(layout.px(30.)),
        )
        .child(
            layout.rect(30., 268., 276., 52.).child(
                layout
                    .text(SAIRA_TITLE, 36., 52., 700.)
                    .truncate()
                    .child(family.title().to_owned()),
            ),
        )
}

fn preview(
    layout: Layout,
    shown: &PreviewSelection,
    now: DateTime<FixedOffset>,
    description: Option<&gpui::SharedString>,
    icons: &ChannelIcons,
    docked: bool,
) -> Div {
    let family = &shown.family;
    let highlighted = shown.programme(now);
    let preferred = shown.preferred_variant();
    let metadata = layout
        .text(SAIRA, 20., 30., 400.)
        .w_full()
        .text_color(layout.secondary(DIM))
        .flex()
        .items_center()
        .gap(layout.px(12.))
        .child(
            div()
                .font_weight(gpui::FontWeight(600.))
                .flex_none()
                .child(family.number().to_string()),
        )
        .child(
            div()
                .font_weight(gpui::FontWeight(600.))
                .text_color(layout.guide_text())
                .min_w_0()
                .truncate()
                .child(family.title().to_owned()),
        )
        .when(family.quality_variants().next().is_some(), |metadata| {
            metadata.child(
                quality_tags(layout, family, preferred.channel().id(), false, None).flex_none(),
            )
        })
        .when(!family.group().is_empty(), |metadata| {
            metadata.child(div().min_w_0().truncate().child(family.group().to_owned()))
        });
    let title = highlighted.map_or_else(
        || family.title().into(),
        |(_, programme)| programme_title(programme),
    );
    let heading = layout
        .text(SAIRA_PREVIEW, 66., 68.64, 700.)
        .mt(layout.px(4.))
        .truncate()
        .child(title);
    let (left, width) = panel_bounds(docked);
    let preview = layout.rect(left, 101., width, 331.).flex().flex_col();
    let preview = if docked {
        preview.child(
            div()
                .flex()
                .items_center()
                .gap(layout.px(24.))
                .child(plate(
                    layout,
                    Plate::Preview,
                    family.number(),
                    false,
                    family_icon(family).and_then(|icon| icons.get(icon)),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(metadata)
                        .child(heading),
                ),
        )
    } else {
        preview.child(metadata).child(heading)
    };
    let meter = layout
        .text(SAIRA, 20., 24., 400.)
        .h(layout.px(30.))
        .mt(layout.px(10.))
        .text_color(layout.secondary(DIM))
        .flex()
        .items_center()
        .gap(layout.px(14.));
    let Some((source, programme)) = highlighted else {
        return preview.child(meter.child(NO_GUIDE_DATA));
    };
    let starts = programme.starts_at();
    let ends = programme.ends_at();
    let meter = match preview_timing(programme, now.to_utc()) {
        PreviewTiming::Live {
            elapsed_fraction,
            minutes_left,
        } => meter
            .child(clock_time(starts))
            .child(
                div()
                    .relative()
                    .w(layout.px(264.))
                    .h(layout.px(6.))
                    .rounded_full()
                    .bg(rgba(FAINT))
                    .child(
                        layout
                            .rect(0., 0., 264. * elapsed_fraction as f32, 6.)
                            .rounded_l(layout.px(3.))
                            .bg(rgb(ICE)),
                    ),
            )
            .child(clock_time(ends))
            .child(
                div()
                    .font_weight(gpui::FontWeight(600.))
                    .text_color(layout.guide_text())
                    .child(format!("{minutes_left} min left")),
            ),
        PreviewTiming::Upcoming {
            in_minutes,
            duration_minutes,
        } => meter
            .child(
                div().flex().gap(layout.px(5.)).child("Starts").child(
                    div()
                        .text_color(layout.guide_text())
                        .child(clock_time(starts)),
                ),
            )
            .child(div().text_color(layout.secondary(super::MUTE)).child("·"))
            .child(
                div()
                    .font_weight(gpui::FontWeight(600.))
                    .text_color(layout.guide_text())
                    .child(format!("in {}", duration(in_minutes))),
            )
            .child(
                div()
                    .text_color(layout.secondary(super::MUTE))
                    .child(format!("· {}", duration(duration_minutes))),
            ),
        PreviewTiming::Ended { duration_minutes } => meter
            .child(
                div().flex().gap(layout.px(5.)).child("Ended").child(
                    div()
                        .text_color(layout.guide_text())
                        .child(clock_time(ends)),
                ),
            )
            .child(
                div()
                    .text_color(layout.secondary(super::MUTE))
                    .child(format!("· {}", duration(duration_minutes))),
            ),
    };
    let later = following_programmes(source, programme).map(|later| {
        layout
            .text(SAIRA, 20., 20., 400.)
            .h(layout.px(34.))
            .border_t(layout.px(1.))
            .border_color(rgba(FAINT))
            .flex()
            .items_center()
            .gap(layout.px(12.))
            .child(
                div()
                    .w(layout.px(72.))
                    .flex_none()
                    .font_weight(gpui::FontWeight(600.))
                    .text_color(rgb(ICE))
                    .child(clock_time(later.starts_at())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui::FontWeight(600.))
                    .child(programme_title(later)),
            )
            .child(div().text_color(layout.secondary(DIM)).child(duration(
                (later.ends_at() - later.starts_at()).num_minutes(),
            )))
    });
    preview
        .child(meter)
        .child(
            layout
                .text(SAIRA, 20., 26., 400.)
                .h(layout.px(52.))
                .w(layout.px(768.))
                .mt(layout.px(12.))
                .text_color(layout.secondary(DIM))
                .whitespace_normal()
                .line_clamp(2)
                .child(description.cloned().unwrap_or_default()),
        )
        .child(div().mt(layout.px(18.)).children(later))
}

fn duration(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}
