use chrono::{DateTime, FixedOffset};
use gpui::{Div, div, linear_color_stop, linear_gradient, prelude::*, rgb, rgba, svg};
use sparrow_tv_guide::{
    VariantPreferences,
    now_screen::{NowScreen, ScreenFocus, ToolbarFocus},
    time::{elapsed_fraction, minutes_left},
};

use super::{
    CANVAS_WIDTH, CELL, CELL_ELAPSED, DIM, FAINT, GUTTER, ICE, Layout, ON_ICE, SAIRA, SAIRA_CLOCK,
    SAIRA_NUMBER, SAIRA_PREVIEW, SAIRA_TITLE, TEXT, board::quality_tags, clock_time,
    preview_programme, programme_title,
};

const POSTER_WIDTH: f32 = 336.;
const BAND_TOP: f32 = 27.;
const PREVIEW_X: f32 = GUTTER + POSTER_WIDTH + 44.;
const PREVIEW_WIDTH: f32 = CANVAS_WIDTH - GUTTER - PREVIEW_X;
const NO_GUIDE_DATA: &str = "No guide data. The channel still plays.";

pub(super) fn render(
    layout: Layout,
    screen: &NowScreen,
    now: DateTime<FixedOffset>,
    description: Option<&gpui::SharedString>,
) -> Div {
    div()
        .child(poster(layout, screen))
        .child(toolbar(layout, screen.focus(), now))
        .when_some(screen.preview_family(), |band, family| {
            band.child(preview(layout, screen, family, now, description))
        })
}

fn toolbar(layout: Layout, focus: ScreenFocus, now: DateTime<FixedOffset>) -> Div {
    let search_focus = focus == ScreenFocus::Toolbar(ToolbarFocus::Search);
    let sources_focus = focus == ScreenFocus::Toolbar(ToolbarFocus::Sources);
    let search = div()
        .h(layout.px(58.))
        .flex_1()
        .min_w_0()
        .rounded_full()
        .bg(rgb(CELL))
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
            rgba(DIM),
        ))
        .child(
            layout
                .text(SAIRA, 20., 28., 400.)
                .text_color(rgba(DIM))
                .truncate()
                .child("Search channels and programmes"),
        );
    let sources_ink = if sources_focus { ON_ICE } else { TEXT };
    let sources = layout
        .text(SAIRA, 20., 28., 500.)
        .text_color(rgb(sources_ink))
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
            rgb(sources_ink),
        ))
        .child("Sources");
    layout
        .rect(PREVIEW_X, BAND_TOP, PREVIEW_WIDTH, 58.)
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

fn poster(layout: Layout, screen: &NowScreen) -> Div {
    let poster = layout
        .rect(GUTTER, BAND_TOP, POSTER_WIDTH, 404.)
        .rounded(layout.px(8.))
        .bg(linear_gradient(
            160.,
            linear_color_stop(rgb(CELL_ELAPSED), 0.),
            linear_color_stop(rgb(CELL), 1.),
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
    let Some(family) = screen.preview_family() else {
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
            layout
                .rect(30., 30., 276., 207.)
                .flex()
                .items_center()
                .child(
                    layout
                        .text(SAIRA_NUMBER, 132., 132., 250.)
                        .text_color(rgb(ICE))
                        .child(family.number().to_string()),
                ),
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
    screen: &NowScreen,
    family: &sparrow_tv_guide::GuideFamily,
    now: DateTime<FixedOffset>,
    description: Option<&gpui::SharedString>,
) -> Div {
    let highlighted = preview_programme(screen, now);
    let preferred = family.preferred_variant(&VariantPreferences::default());
    let metadata = layout
        .text(SAIRA, 20., 30., 400.)
        .w_full()
        .text_color(rgba(DIM))
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
                .text_color(rgb(TEXT))
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
    let preview = layout
        .rect(PREVIEW_X, 101., PREVIEW_WIDTH, 331.)
        .flex()
        .flex_col()
        .child(metadata)
        .child(
            layout
                .text(SAIRA_PREVIEW, 66., 68.64, 700.)
                .mt(layout.px(4.))
                .truncate()
                .child(title),
        );
    let meter = layout
        .text(SAIRA, 20., 24., 400.)
        .h(layout.px(30.))
        .mt(layout.px(10.))
        .text_color(rgba(DIM))
        .flex()
        .items_center()
        .gap(layout.px(14.));
    let Some((source, programme)) = highlighted else {
        return preview.child(meter.child(NO_GUIDE_DATA));
    };
    let starts = programme.starts_at();
    let ends = programme.ends_at();
    let meter = if starts <= now {
        let elapsed = elapsed_fraction(programme, now.to_utc()) as f32;
        meter
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
                            .rect(0., 0., 264. * elapsed, 6.)
                            .rounded_l(layout.px(3.))
                            .bg(rgb(ICE)),
                    ),
            )
            .child(clock_time(ends))
            .child(
                div()
                    .font_weight(gpui::FontWeight(600.))
                    .text_color(rgb(TEXT))
                    .child(format!(
                        "{} min left",
                        minutes_left(programme, now.to_utc())
                    )),
            )
    } else {
        let minutes = ((starts - now.to_utc()).num_seconds() as f64 / 60.)
            .round()
            .max(1.) as i64;
        meter
            .child(
                div()
                    .flex()
                    .gap(layout.px(5.))
                    .child("Starts")
                    .child(div().text_color(rgb(TEXT)).child(clock_time(starts))),
            )
            .child(div().text_color(rgba(super::MUTE)).child("·"))
            .child(
                div()
                    .font_weight(gpui::FontWeight(600.))
                    .text_color(rgb(TEXT))
                    .child(format!("in {}", duration(minutes))),
            )
            .child(
                div()
                    .text_color(rgba(super::MUTE))
                    .child(format!("· {}", duration((ends - starts).num_minutes()))),
            )
    };
    let later = source
        .programmes()
        .iter()
        .filter(|later| later.starts_at() >= programme.ends_at())
        .take(3)
        .map(|later| {
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
                .child(div().text_color(rgba(DIM)).child(duration(
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
                .text_color(rgba(DIM))
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
