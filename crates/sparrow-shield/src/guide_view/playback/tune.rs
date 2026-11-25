use std::time::{Duration, Instant};

use super::super::CELL_ELAPSED;
use super::PlaybackStage;
use super::{ICE, INK, Layout, SAIRA, signal_mark};
use gpui::{
    Animation, AnimationExt, AnyElement, BoxShadow, Div, StyledImage, Transformation, div,
    linear_color_stop, linear_gradient, percentage, prelude::*, rgb, rgba, svg,
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
    playback: &super::PlaybackOverlay,
) -> Div {
    let [x, y, w, h] = bounds;
    let (diameter, scale, label) = match placement {
        FieldPlacement::Fullscreen => (196., 3.2, 28.),
        FieldPlacement::Docked => (136., 2.2, 22.),
    };
    let reduce_motion = playback.reduce_motion;
    let image = playback.still.cloned();
    let stage = playback.stage;
    let landing = playback.signal_landing.filter(|(_, outgoing)| {
        *outgoing == PlaybackStage::Reconnecting
            && stage == PlaybackStage::Playing
            && image.is_some()
    });
    let hold =
        matches!(stage, PlaybackStage::Failed | PlaybackStage::Reconnecting) || landing.is_some();
    let color = match stage {
        PlaybackStage::Failed => super::ERROR,
        _ if hold => super::AMBER,
        _ => ICE,
    };
    let started = playback.hold_started;
    let attempt = playback.attempt;
    let fullscreen = matches!(placement, FieldPlacement::Fullscreen);
    let frame = layout.rect(x, y, w, h).overflow_hidden();
    let frame = match placement {
        FieldPlacement::Fullscreen => frame,
        FieldPlacement::Docked => frame.rounded_t(layout.px(8.)),
    };
    let draw = move |field: Div, phase: f32| {
        // Three turns of the arc per sweep of the background bands.
        let entrance = if reduce_motion {
            1.
        } else {
            started.map_or(1., |t| {
                1. - (1. - (t.elapsed().as_secs_f32() / 0.45).min(1.)).powi(5)
            })
        };
        let hud = entrance
            * landing.map_or(1., |(since, _)| {
                (1. - since.elapsed().as_secs_f32() / 0.16).max(0.)
            });
        let turn = (phase * 3.).fract();
        let mid = (0.46 + (phase * std::f32::consts::TAU).sin() * 0.12) * h;
        let mark_layout = Layout {
            scale: layout.scale
                * if hold {
                    if fullscreen { 3.4 } else { 2.6 }
                } else {
                    scale
                },
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
                    .text_color(rgb(color))
                    .with_transformation(Transformation::rotate(percentage(turn))),
            )
            .child(signal_mark::waves(
                mark,
                mark_layout,
                color,
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
            .when_some(image.clone(), |field, image| {
                field
                    .child(gpui::img(image.clone()).absolute().size_full())
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .opacity(entrance)
                            .child(gpui::img(image).absolute().size_full().grayscale(true))
                            .child(div().absolute().inset_0().bg(rgba(0x00000099))),
                    )
            })
            .when(
                (stage == PlaybackStage::Reconnecting || landing.is_some()) && !reduce_motion,
                |field| {
                    let height = (h * 0.10).clamp(44., 88.);
                    let feather = 10.;
                    let y = (h + height) * phase - height;
                    field.child(
                        layout
                            .rect(0., y, w, height)
                            .opacity(hud)
                            .child(layout.rect(0., 0., w, height - feather).bg(linear_gradient(
                                180.,
                                linear_color_stop(rgba(ICE << 8), 0.),
                                linear_color_stop(rgba(ICE << 8 | 41), 1.),
                            )))
                            .child(layout.rect(0., height - feather, w, feather).bg(
                                linear_gradient(
                                    180.,
                                    linear_color_stop(rgba(ICE << 8 | 54), 0.),
                                    linear_color_stop(rgba(ICE << 8), 1.),
                                ),
                            )),
                    )
                },
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(layout.px(16.))
                    .opacity(hud)
                    .child(ring)
                    .child(
                        layout
                            .text(SAIRA, label, label + 6., if hold { 600. } else { 500. })
                            .when(hold, |text| {
                                text.bg(rgba(INK << 8 | 204))
                                    .rounded(layout.px(20.))
                                    .px(layout.px(if fullscreen { 22. } else { 16. }))
                                    .py(layout.px(if fullscreen { 8. } else { 6. }))
                            })
                            .text_color(rgb(super::TEXT))
                            .child(match stage {
                                _ if hold && stage != PlaybackStage::Failed => {
                                    format!("Reconnecting · attempt {attempt}")
                                }
                                super::PlaybackStage::Failed => stage.label().to_owned(),
                                _ => "Tuning".to_owned(),
                            }),
                    ),
            )
    };
    let field = div().relative().size_full();
    if reduce_motion || stage == PlaybackStage::Failed {
        frame.child(draw(field, 0.))
    } else {
        frame.child(field.with_animation(
            "tuning-field",
            Animation::new(Duration::from_millis(if hold { 1700 } else { 4800 })).repeat(),
            draw,
        ))
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
            Animation::new(LOCK),
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
