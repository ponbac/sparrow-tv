use gpui::{
    Bounds, ContentMask, Div, PathBuilder, Pixels, Window, canvas, fill, point, prelude::*, rgb,
    rgba, size,
};
use sparrow_core::GuideProgramme;
use sparrow_tv_guide::{
    guide_screen::TIMELINE_ROW_HEIGHT,
    time::{ProgrammeCellContent, ProgrammePlacement},
};

use super::super::{
    CELL, CELL_ELAPSED, DIM, ICE, ICE_ELAPSED, Layout, ON_ICE, ON_ICE_DIM, SAIRA, TEXT, clock_time,
    programme_title,
};
use super::{RIGHT_FADE_WIDTH, TRACK_WIDTH, TimelineFrame};

const TITLE_FAMILY: &str = "Sparrow Guide 82";
const CELL_HEIGHT: f32 = TIMELINE_ROW_HEIGHT - 8.;
const INSET_Y: f32 = 4.;
const CLIP_POINT: f32 = 12.;
const CORNER_RADIUS: f32 = 8.;
const WASH: u32 = 0x243858;

fn geometry(
    placement: ProgrammePlacement,
    focused: bool,
) -> Option<([f32; 4], ProgrammeCellContent)> {
    let left = placement.left_fraction as f32 * TRACK_WIDTH;
    let appearance = placement.cell_appearance(TRACK_WIDTH, focused)?;
    Some((
        [left, INSET_Y, appearance.width, CELL_HEIGHT],
        appearance.content,
    ))
}

pub(super) fn render(
    layout: Layout,
    frame: TimelineFrame,
    programme: &GuideProgramme,
    focused: bool,
) -> Option<Div> {
    let placement = frame.axis.placement(programme)?;
    let ([left, top, full_width, height], content) = geometry(placement, focused)?;
    let cut = placement.cut_at_start;
    let live = programme.starts_at() <= frame.now && frame.now < programme.ends_at();
    let fill_color = match (focused, live) {
        (true, _) => rgb(ICE),
        (false, true) => layout.surface(CELL_ELAPSED),
        (false, false) => layout.surface(CELL),
    };
    let progress = live
        .then(|| frame.axis.elapsed_fraction(programme, frame.now))
        .flatten()
        .map(|fraction| {
            (
                (fraction as f32 * full_width).clamp(0., full_width),
                if focused {
                    rgb(ICE_ELAPSED)
                } else {
                    layout.surface(WASH)
                },
            )
        });
    // Geometry past the track's clip cannot affect its image unless it rounds
    // back into view. Bound the painted shape even for very long programmes.
    let paint_width = full_width.min(TRACK_WIDTH - left + CORNER_RADIUS);
    let fade = if layout.translucent {
        0.
    } else {
        RIGHT_FADE_WIDTH
    };
    let text_width = full_width.min(TRACK_WIDTH - left + fade);
    let cell = layout.rect(left, top, text_width, height).child(
        layout
            .rect(0., 0., paint_width, CELL_HEIGHT)
            .child(cell_fill(layout, cut, fill_color, progress)),
    );
    if content == ProgrammeCellContent::Chevron {
        let chevron_ink = if focused { rgb(ON_ICE) } else { rgba(DIM) };
        return Some(
            cell.child(
                layout
                    .rect(
                        (full_width - 21.) / 2. + 4.,
                        (CELL_HEIGHT - 21.) / 2.,
                        21.,
                        21.,
                    )
                    .overflow_hidden()
                    .child(layout.icon(
                        include_bytes!("../../../assets/icons/chevron-left.svg"),
                        21.,
                        chevron_ink,
                    )),
            )
            .overflow_hidden(),
        );
    }

    Some(
        cell.child(
            layout
                .rect(14., 0., text_width - 28., CELL_HEIGHT)
                .flex()
                .flex_col()
                .justify_center()
                .gap(layout.px(4.))
                .child(
                    layout
                        .text(SAIRA, 15., 15., 400.)
                        .text_color(rgba(if focused { ON_ICE_DIM } else { DIM }))
                        .truncate()
                        .child(clock_time(programme.starts_at())),
                )
                .child(
                    layout
                        .text(TITLE_FAMILY, 22., 25.3, 650.)
                        .text_color(rgb(if focused { ON_ICE } else { TEXT }))
                        .truncate()
                        .child(programme_title(programme)),
                ),
        ),
    )
}

fn cell_fill(
    layout: Layout,
    cut: bool,
    fill_color: gpui::Rgba,
    elapsed: Option<(f32, gpui::Rgba)>,
) -> impl gpui::IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let radius = layout.px(CORNER_RADIUS);
            let path = cut.then(|| left_cut(bounds, layout.px(CLIP_POINT), radius));
            let paint = |window: &mut Window, color| {
                if let Some(path) = path.as_ref().and_then(|path| path.as_ref()) {
                    window.paint_path(path.clone(), color);
                } else {
                    window.paint_quad(fill(bounds, color).corner_radii(radius));
                }
            };
            paint(window, fill_color);
            if let Some((elapsed, elapsed_color)) = elapsed {
                let width = layout.px(elapsed).min(bounds.size.width);
                let mask = ContentMask {
                    bounds: Bounds::new(bounds.origin, size(width, bounds.size.height)),
                };
                window.with_content_mask(Some(mask), |window| paint(window, elapsed_color));
            }
        },
    )
    .size_full()
}

fn left_cut(bounds: Bounds<Pixels>, tip: Pixels, radius: Pixels) -> Option<gpui::Path<Pixels>> {
    let mut path = PathBuilder::fill();
    let width = bounds.size.width;
    let height = bounds.size.height;
    let radius = radius.min(width / 2.);
    let tip = tip.min(width - radius);
    let at = |x, y| bounds.origin + point(x, y);
    let curve = radius * 0.552_284_8;
    let zero = gpui::px(0.);
    path.move_to(at(tip, zero));
    path.line_to(at(width - radius, zero));
    path.cubic_bezier_to(
        at(width, radius),
        at(width - radius + curve, zero),
        at(width, radius - curve),
    );
    path.line_to(at(width, height - radius));
    path.cubic_bezier_to(
        at(width - radius, height),
        at(width, height - radius + curve),
        at(width - radius + curve, height),
    );
    path.line_to(at(tip, height));
    path.line_to(at(zero, height / 2.));
    path.close();
    path.build().ok()
}
