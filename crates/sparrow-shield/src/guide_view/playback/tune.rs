use std::time::{Duration, Instant};

use super::super::CELL_ELAPSED;
use super::{ICE, INK, Layout, SAIRA, signal_mark};
use gpui::{
    Animation, AnimationExt, AnyElement, BoxShadow, Div, Transformation, div, linear_color_stop,
    linear_gradient, percentage, prelude::*, rgb, rgba, svg,
};

pub(super) const LOCK: Duration = Duration::from_millis(800);

pub(super) enum FieldPlacement {
    Docked,
    Fullscreen,
}

pub(super) fn field(
    layout: Layout,
    bounds: [f32; 4],
    placement: FieldPlacement,
    reduce_motion: bool,
) -> Div {
    let [x, y, w, h] = bounds;
    let (diameter, scale, label) = match placement {
        FieldPlacement::Fullscreen => (196., 3.2, 28.),
        FieldPlacement::Docked => (136., 2.2, 22.),
    };
    let frame = layout.rect(x, y, w, h).overflow_hidden();
    let frame = match placement {
        FieldPlacement::Fullscreen => frame,
        FieldPlacement::Docked => frame.rounded_t(layout.px(8.)),
    };
    let draw = move |field: Div, phase: f32| {
        // Three turns of the arc per sweep of the background bands.
        let turn = (phase * 3.).fract();
        let mid = (0.46 + (phase * std::f32::consts::TAU).sin() * 0.12) * h;
        let mark_layout = Layout {
            scale: layout.scale * scale,
            ..layout
        };
        let mark = div()
            .relative()
            .w(mark_layout.px(42.))
            .h(mark_layout.px(24.));
        let ring = div()
            .relative()
            .size(layout.px(diameter))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .border(layout.px(2.5))
                    .border_color(rgba(0xeef3f81f)),
            )
            .child(
                svg()
                    .data(RING)
                    .absolute()
                    .size_full()
                    .text_color(rgb(ICE))
                    .with_transformation(Transformation::rotate(percentage(turn))),
            )
            .child(signal_mark::waves(
                mark,
                mark_layout,
                ICE,
                if reduce_motion {
                    0.35
                } else {
                    (phase * 4.).fract()
                },
            ));
        field
            .child(layout.rect(0., 0., w, mid).bg(linear_gradient(
                180.,
                linear_color_stop(rgb(INK), 0.),
                linear_color_stop(rgb(CELL_ELAPSED), 1.),
            )))
            .child(layout.rect(0., mid, w, h - mid).bg(linear_gradient(
                180.,
                linear_color_stop(rgb(CELL_ELAPSED), 0.),
                linear_color_stop(rgb(INK), 1.),
            )))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(layout.px(16.))
                    .child(ring)
                    .child(
                        layout
                            .text(SAIRA, label, label + 6., 500.)
                            .text_color(rgb(super::TEXT))
                            .child("Tuning"),
                    ),
            )
    };
    let field = div().relative().size_full();
    if reduce_motion {
        frame.child(draw(field, 0.))
    } else {
        frame.child(
            field.with_animation(
                "tuning-field",
                Animation::new(Duration::from_millis(4800))
                    .repeat()
                    .with_max_fps(24.),
                draw,
            ),
        )
    }
}

const RING: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 136 136"><path d="M68 1.5 A66.5 66.5 0 0 1 134.5 68" fill="none" stroke="white" stroke-width="2.5" stroke-linecap="round"/></svg>"#;

pub(super) fn lock(layout: Layout, bounds: [f32; 4], since: Instant) -> AnyElement {
    let [x, y, w, h] = bounds;
    layout
        .rect(x, y, w, h)
        .rounded(layout.px(8.))
        .bg(rgb(INK))
        .with_animation(
            gpui::ElementId::Name(format!("dock-lock-{since:?}").into()),
            Animation::new(LOCK).with_max_fps(30.),
            move |frame, delta| {
                let t = (since.elapsed().as_secs_f32() / LOCK.as_secs_f32()).min(1.);
                let alpha = if delta >= 1. {
                    0
                } else {
                    ((1. - t).powi(2) * 110.) as u32
                };
                frame.shadow(vec![BoxShadow {
                    color: rgba(ICE << 8 | alpha).into(),
                    offset: Default::default(),
                    blur_radius: layout.px(36.),
                    spread_radius: layout.px(6.),
                    inset: false,
                }])
            },
        )
        .into_any_element()
}
