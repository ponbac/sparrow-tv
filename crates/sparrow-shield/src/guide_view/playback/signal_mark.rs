//! Playback signal mark for tuning, reconnecting, and the landing into live playback.
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Div, Transformation, div, prelude::*, rgb, size, svg,
};

use super::{AMBER, ERROR, ICE, Layout, PANEL, PlaybackOverlay, PlaybackStage, TALLY, TEXT};

const LAND: Duration = Duration::from_millis(550);
const INNER: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 24"><path d="M15.5 7.5a8 8 0 0 0 0 9m17-9a8 8 0 0 1 0 9" fill="none" stroke="white" stroke-width="2.6" stroke-linecap="round"/></svg>"#;
const OUTER: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 24"><path d="M9 3.5a14 14 0 0 0 0 17m30-17a14 14 0 0 1 0 17" fill="none" stroke="white" stroke-width="2.6" stroke-linecap="round"/></svg>"#;

pub(super) fn render(layout: Layout, playback: &PlaybackOverlay<'_>, focused: bool) -> AnyElement {
    let frame = div()
        .relative()
        .flex_none()
        .w(layout.px(42.))
        .h(layout.px(24.));
    let backing = move |frame: Div, fold: f32| {
        frame.when(focused, |frame| {
            let width = 20. + 22. * fold;
            let height = 20. + 4. * fold;
            frame.child(
                layout
                    .rect((42. - width) / 2., (24. - height) / 2., width, height)
                    .rounded_full()
                    .bg(rgb(PANEL)),
            )
        })
    };
    let stage = playback.stage;
    match stage {
        PlaybackStage::Tuning | PlaybackStage::Reconnecting => {
            let color = signal_color(stage);
            let (period, id) = if stage == PlaybackStage::Tuning {
                (1100, "signal-tuning")
            } else {
                (1600, "signal-reconnecting")
            };
            let frame = backing(frame, 1.);
            if playback.reduce_motion {
                return steady(layout, frame, color).into_any_element();
            }
            searching(layout, frame, color, period, id)
        }
        PlaybackStage::Playing => {
            if let Some((start, outgoing)) = playback
                .signal_landing
                .filter(|(start, _)| start.elapsed() < LAND)
            {
                return frame
                    .with_animation(
                        "signal-landing",
                        Animation::new(LAND),
                        move |frame, delta| {
                            // Reduced motion supplies the end delta. Otherwise keep the original
                            // stage-change clock across chrome remounts.
                            let t = if delta >= 1. {
                                1.
                            } else {
                                (start.elapsed().as_secs_f32() / LAND.as_secs_f32()).min(1.)
                            };
                            let fold = (1. - (t / 0.65).min(1.)).powi(2);
                            let color = signal_color(outgoing);
                            backing(frame, fold)
                                .child(arcs(layout, INNER, color, 0.35 + 0.65 * fold, 0.75 * fold))
                                .child(arcs(layout, OUTER, color, 0.35 + 0.65 * fold, 0.75 * fold))
                                .child(dot(layout, TALLY, 10. + 5. * pulse(t)))
                        },
                    )
                    .into_any_element();
            }
            backing(frame, 0.)
                .child(dot(layout, TALLY, 10.))
                .into_any_element()
        }
        PlaybackStage::Paused | PlaybackStage::Stopping => backing(frame, 0.)
            .child(dot(layout, TEXT, 10.))
            .into_any_element(),
        PlaybackStage::Failed => backing(frame, 0.)
            .child(dot(layout, ERROR, 10.))
            .into_any_element(),
    }
}

pub(super) fn waves(frame: Div, layout: Layout, color: u32, t: f32) -> Div {
    let inner = pulse(t);
    let outer = pulse((t + 0.8).fract());
    frame
        .child(arcs(layout, INNER, color, 1., 0.32 + 0.68 * inner))
        .child(arcs(layout, OUTER, color, 1., 0.32 + 0.68 * outer))
        .child(dot(layout, color, 7.5 + 2.5 * inner))
}

fn signal_color(stage: PlaybackStage) -> u32 {
    if stage == PlaybackStage::Reconnecting {
        AMBER
    } else {
        ICE
    }
}

fn pulse(t: f32) -> f32 {
    let value = if t < 0.35 { t / 0.35 } else { (1. - t) / 0.65 };
    value * value * (3. - 2. * value)
}

fn dot(layout: Layout, color: u32, diameter: f32) -> Div {
    layout
        .rect(21. - diameter / 2., 12. - diameter / 2., diameter, diameter)
        .rounded_full()
        .bg(rgb(color))
}

fn arcs(
    layout: Layout,
    path: &'static [u8],
    color: u32,
    scale: f32,
    opacity: f32,
) -> impl IntoElement {
    svg()
        .data(path)
        .absolute()
        .left(layout.px(1.))
        .top(layout.px(2.))
        .w(layout.px(40.))
        .h(layout.px(20.))
        .with_transformation(Transformation::scale(size(scale, scale)))
        .text_color(rgb(color))
        .opacity(opacity)
}

fn searching(layout: Layout, frame: Div, color: u32, period: u64, id: &'static str) -> AnyElement {
    frame
        .with_animation(
            id,
            Animation::new(Duration::from_millis(period)).repeat(),
            move |frame, t| {
                let inner = pulse(t);
                let outer = pulse((t + 0.8).fract());
                frame
                    .child(arcs(layout, INNER, color, 1., 0.32 + 0.68 * inner))
                    .child(arcs(layout, OUTER, color, 1., 0.32 + 0.68 * outer))
                    .child(dot(layout, color, 7.5 + 2.5 * inner))
            },
        )
        .into_any_element()
}

pub(in crate::guide_view) fn steady(layout: Layout, frame: Div, color: u32) -> Div {
    frame
        .child(arcs(layout, INNER, color, 1., 0.85))
        .child(arcs(layout, OUTER, color, 1., 0.55))
        .child(dot(layout, color, 10.))
}
