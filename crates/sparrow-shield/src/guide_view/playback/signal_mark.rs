//! Signal waves share one clock so their phase stays aligned on a slow frame.
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Div, Transformation, div, prelude::*, rgb, size, svg,
};

use super::{ERROR, ICE, Layout, PANEL, PlaybackOverlay, PlaybackStage, TALLY, TEXT};

const LAND: Duration = Duration::from_millis(550);
const INNER: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 24"><path d="M15.5 7.5a8 8 0 0 0 0 9m17-9a8 8 0 0 1 0 9" fill="none" stroke="white" stroke-width="2.6" stroke-linecap="round"/></svg>"#;
const OUTER: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 24"><path d="M9 3.5a14 14 0 0 0 0 17m30-17a14 14 0 0 1 0 17" fill="none" stroke="white" stroke-width="2.6" stroke-linecap="round"/></svg>"#;

pub(super) fn render(layout: Layout, playback: &PlaybackOverlay<'_>, focused: bool) -> AnyElement {
    let frame = div()
        .relative()
        .flex_none()
        .w(layout.px(42.))
        .h(layout.px(24.))
        .when(focused, |frame| frame.rounded_full().bg(rgb(PANEL)));
    let stage = playback.stage;
    match stage {
        PlaybackStage::Tuning | PlaybackStage::Reconnecting => {
            let (color, period, id) = if stage == PlaybackStage::Tuning {
                (ICE, 1100, "signal-tuning")
            } else {
                (0xffb45d, 1600, "signal-reconnecting")
            };
            frame
                .with_animation(
                    id,
                    Animation::new(Duration::from_millis(period))
                        .repeat()
                        .with_max_fps(24.),
                    move |frame, t| {
                        let inner = pulse(t);
                        let outer = pulse((t + 0.8).fract());
                        frame
                            .child(arcs(layout, INNER, color, 1., 0.32 + 0.68 * inner))
                            .child(arcs(layout, OUTER, color, 1., 0.32 + 0.68 * outer))
                            .child(dot(layout, color, 7. + 2. * inner))
                    },
                )
                .into_any_element()
        }
        PlaybackStage::Playing => {
            if let Some(start) = playback
                .playing_since
                .filter(|start| start.elapsed() < LAND)
            {
                return frame
                    .with_animation(
                        "signal-landing",
                        Animation::new(LAND),
                        move |frame, delta| {
                            // Tie the landing to the stage change across chrome remounts.
                            let t = if delta >= 1. {
                                1.
                            } else {
                                (start.elapsed().as_secs_f32() / LAND.as_secs_f32()).min(1.)
                            };
                            let fold = (1. - t).powi(3);
                            frame
                                .child(arcs(layout, INNER, TALLY, 0.35 + 0.65 * fold, fold))
                                .child(arcs(layout, OUTER, TALLY, 0.35 + 0.65 * fold, fold))
                                .child(dot(layout, TALLY, 9. + 5. * pulse(t)))
                        },
                    )
                    .into_any_element();
            }
            frame.child(dot(layout, TALLY, 9.)).into_any_element()
        }
        PlaybackStage::Paused | PlaybackStage::Stopping => {
            frame.child(dot(layout, TEXT, 9.)).into_any_element()
        }
        PlaybackStage::Failed => frame.child(dot(layout, ERROR, 9.)).into_any_element(),
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
