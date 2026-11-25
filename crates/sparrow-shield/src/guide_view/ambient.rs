//! Light cast by the picture, painted below Guide text and playback controls.
use std::time::{Duration, Instant};

use gpui::{Animation, AnimationExt, Div, Hsla, Rgba, div, prelude::*, rgb};

use super::{
    Layout,
    backdrop::mix_hue,
    playback::{FullscreenFrame, PlaybackLayout, moving_picture, picture_reference},
};

type Palette = [Rgba; 3];
const FADE: Duration = Duration::from_millis(450);

#[derive(Default)]
pub(crate) struct Ambient {
    sprites: [super::glow::Glow; 3],
    from: Palette,
    to: Palette,
    changed: Option<Instant>,
    revision: usize,
}

fn mix(from: Palette, to: Palette, t: f32) -> Palette {
    std::array::from_fn(|i| {
        let (a, b) = (from[i], to[i]);
        let mut color = if a.a == 0. {
            b
        } else if b.a == 0. {
            a
        } else {
            mix_hue(a.into(), b.into(), t).into()
        };
        color.a = a.a + (b.a - a.a) * t;
        color
    })
}

impl Ambient {
    pub(crate) fn sample(&mut self, colors: [u32; 3], reduced: bool) -> bool {
        self.target(
            colors.map(|c| {
                let mut color = Hsla::from(rgb(c));
                let neutral = (1. - color.s / 0.09).clamp(0., 1.);
                color.a = ((color.l - 0.03) / 0.12).clamp(0., 1.);
                color.s = (color.s * 1.25).min(0.80);
                color.l = color.l.clamp(0.32, 0.56);
                // A trace of cool ink in neutral scenes, continuous across the grey threshold.
                let mut glow = Rgba::from(color);
                glow.b += 0.024 * neutral;
                glow.r -= 0.012 * neutral;
                glow
            }),
            reduced,
            if reduced { 0.15 } else { 0.09 },
        )
    }

    pub(crate) fn field(&mut self, color: Option<u32>, reduced: bool) {
        self.target(
            [color.map(rgb).unwrap_or_default(); 3],
            reduced,
            f32::EPSILON,
        );
    }

    fn target(&mut self, next: Palette, reduced: bool, deadband: f32) -> bool {
        if self.to.iter().zip(next).all(|(a, b)| {
            (a.r - b.r)
                .abs()
                .max((a.g - b.g).abs())
                .max((a.b - b.b).abs())
                .max((a.a - b.a).abs())
                < deadband
        }) {
            return false;
        }
        let t = progress(self.changed);
        self.from = mix(self.from, self.to, t);
        self.to = next;
        self.changed = (!reduced).then(Instant::now);
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub(crate) fn layer(
        &self,
        viewport: gpui::Size<gpui::Pixels>,
        frame: FullscreenFrame,
        reduced: bool,
    ) -> Div {
        let layout = Layout::new(viewport);
        let [x, y, w, h] = moving_picture(
            layout,
            frame,
            picture_reference(layout, PlaybackLayout::Docked),
        );
        let t = frame.fullscreen;
        let dock = [
            [x - 200., y - 180., 620., 400.],
            [x + w - 160., y - 120., 600., 500.],
            [x - 80., y + h - 80., 760., 340.],
        ];
        let full = [
            [x - 40., y - 130., 300., h + 260.],
            [x + w - 260., y - 130., 300., h + 260.],
            dock[2],
        ];
        let sprites = self.sprites.clone();
        let paint = move |colors: Palette| {
            let sprites = sprites.clone();
            colors
                .into_iter()
                .enumerate()
                .filter_map(move |(i, color)| {
                    let alpha = color.a * 0.48 * if i == 2 { 1. - t } else { 1. };
                    if alpha <= 0. {
                        return None;
                    }
                    let [x, y, w, h] =
                        std::array::from_fn(|j| dock[i][j] + (full[i][j] - dock[i][j]) * t);
                    Some(sprites[i].paint(
                        layout,
                        [x, y, w, h],
                        210. - 20. * t,
                        Rgba { a: alpha, ..color },
                    ))
                })
        };
        let root = div().absolute().inset_0().overflow_hidden();
        let (from, to) = (self.from, self.to);
        let changed = self.changed;
        if reduced || progress(changed) >= 1. {
            return root.children(paint(to));
        }
        root.child(div().absolute().inset_0().with_animation(
            ("ambient", self.revision),
            Animation::new(FADE),
            move |layer, _| layer.children(paint(mix(from, to, progress(changed)))),
        ))
    }
}

fn progress(changed: Option<Instant>) -> f32 {
    changed.map_or(1., |start| {
        let t = (start.elapsed().as_secs_f32() / FADE.as_secs_f32()).min(1.);
        t * t * (3. - 2. * t)
    })
}
