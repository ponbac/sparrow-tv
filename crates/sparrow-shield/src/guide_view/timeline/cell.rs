use gpui::{Div, prelude::*, rgb, rgba};
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
use super::super::card_face::{CLIP_POINT, CORNER_RADIUS};
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
        (true, _) => ICE,
        (false, true) => CELL_ELAPSED,
        (false, false) => CELL,
    };
    // Geometry past the track's clip cannot affect its image unless it rounds
    // back into view. Bound the painted shape even for very long programmes.
    let paint_width = full_width.min(TRACK_WIDTH - left + CORNER_RADIUS);
    let fade = if layout.translucent {
        0.
    } else {
        RIGHT_FADE_WIDTH
    };
    let text_width = full_width.min(TRACK_WIDTH - left + fade);
    let lift = focused.then_some(frame.lift);
    let grow = lift.map_or(0., |lift| lift.amount * 2.5);
    let left_grow = grow.min(left.max(0.));
    let progress = frame
        .axis
        .elapsed_fraction(programme, frame.meter_time)
        .filter(|_| live)
        .map(|fraction| {
            let elapsed = (fraction * placement.full_width_fraction) as f32 * TRACK_WIDTH;
            (
                elapsed + left_grow,
                if focused { ICE_ELAPSED } else { WASH },
            )
        });
    let width = paint_width + left_grow + grow;
    let face_height = height + 2. * grow;
    let cell = layout.rect(left, top, text_width, height).child(
        layout
            .rect(-left_grow, -grow, width, face_height)
            .when_some(lift, |face, lift| {
                let inset = if cut { CLIP_POINT.min(width / 2.) } else { 0. };
                face.child(
                    layout
                        .rect(inset, 0., width - inset, face_height)
                        .rounded(layout.px(CORNER_RADIUS))
                        .shadow(vec![lift.shadow(layout, 9., 20., 204)]),
                )
            })
            .child(super::super::card_face::paint(
                layout, cut, fill_color, progress, lift,
            )),
    );
    if content == ProgrammeCellContent::Chevron {
        let chevron_ink = if focused { rgb(ON_ICE) } else { rgba(DIM) };
        let viewport = layout
            .rect(0., 0., text_width, CELL_HEIGHT)
            .overflow_hidden();
        return Some(
            cell.child(
                viewport.child(
                    layout
                        .rect(
                            (full_width - 21.) / 2. + 4.,
                            (CELL_HEIGHT - 21.) / 2.,
                            21.,
                            21.,
                        )
                        .child(layout.icon(
                            include_bytes!("../../../assets/icons/chevron-left.svg"),
                            21.,
                            chevron_ink,
                        )),
                ),
            ),
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
