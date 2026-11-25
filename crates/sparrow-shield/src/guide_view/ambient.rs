//! Light cast by the picture, painted below Guide text and playback controls.
use std::time::{Duration, Instant};

use gpui::{Animation, AnimationExt, BoxShadow, Div, Hsla, Rgba, div, prelude::*, rgb};

use super::{
    CANVAS_WIDTH, Layout,
    playback::{FullscreenFrame, PlaybackLayout, moving_picture, picture_reference},
};

type Palette = [Rgba; 3];
const FADE: Duration = Duration::from_millis(950);

#[derive(Default)]
pub(crate) struct Ambient {
    from: Palette,
    to: Palette,
    changed: Option<Instant>,
    revision: usize,
}

fn mix(from: Palette, to: Palette, t: f32) -> Palette {
    std::array::from_fn(|i| {
        let (a, b) = (from[i], to[i]);
        let lerp = |x: f32, y: f32| x + (y - x) * t;
        Rgba {
            r: lerp(a.r, b.r),
            g: lerp(a.g, b.g),
            b: lerp(a.b, b.b),
            a: lerp(a.a, b.a),
        }
    })
}

impl Ambient {
    pub(crate) fn sample(&mut self, colors: [u32; 3], reduced: bool) -> bool {
        self.target(
            colors.map(|c| {
                let mut color = Hsla::from(rgb(c));
                if color.s < 0.09 {
                    color.h = 0.58;
                }
                color.s = (color.s * 1.16).clamp(0.18, 0.75);
                color.l = color.l.clamp(0.30, 0.49);
                color.into()
            }),
            reduced,
        )
    }

    pub(crate) fn field(&mut self, color: Option<u32>, reduced: bool) {
        self.target([color.map(rgb).unwrap_or_default(); 3], reduced);
    }

    fn target(&mut self, next: Palette, reduced: bool) -> bool {
        if self.to.iter().zip(next).all(|(a, b)| {
            (a.r - b.r)
                .abs()
                .max((a.g - b.g).abs())
                .max((a.b - b.b).abs())
                .max((a.a - b.a).abs())
                < 0.045
        }) {
            return false;
        }
        let t = self.progress();
        self.from = mix(self.from, self.to, t);
        self.to = next;
        self.changed = (!reduced).then(Instant::now);
        self.revision = self.revision.wrapping_add(1);
        true
    }

    fn progress(&self) -> f32 {
        self.changed.map_or(1., |start| {
            (start.elapsed().as_secs_f32() / FADE.as_secs_f32()).min(1.)
        })
    }

    pub(crate) fn layer(&self, layout: Layout, frame: FullscreenFrame) -> Div {
        let [x, y, w, h] = moving_picture(
            layout,
            frame,
            picture_reference(layout, PlaybackLayout::Docked),
        );
        let blobs = match frame.fullscreen >= 1. {
            false => [
                [x - 180., y - 110., 360., 280.],
                [x + w - 180., y + 10., 440., 220.],
                [x + 60., y + h - 70., 450., 155.],
            ],
            true => [
                [-x * 0.5, 0., x * 1.55, h],
                [CANVAS_WIDTH - x * 1.05, 0., x * 1.55, h],
                [x, h - 90., w, 120.],
            ],
        };
        let paint = move |colors: Palette| {
            blobs
                .into_iter()
                .zip(colors)
                .map(move |([x, y, w, h], color)| {
                    let mut ink = Hsla::from(color);
                    ink.a *= 0.58;
                    layout.rect(x, y, w, h).rounded_full().shadow(vec![
                        BoxShadow::new(layout.px(0.), layout.px(0.), ink)
                            .blur_radius(layout.px(150.)),
                    ])
                })
        };
        let root = div().absolute().inset_0().overflow_hidden();
        let (from, to) = (self.from, self.to);
        if self.progress() >= 1. {
            return if to[0].a == 0. {
                root
            } else {
                root.children(paint(to))
            };
        }
        root.child(div().absolute().inset_0().with_animation(
            ("ambient", self.revision),
            Animation::new(FADE).with_max_fps(18.),
            move |layer, t| layer.children(paint(mix(from, to, t))),
        ))
    }
}
