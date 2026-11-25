//! Light cast by the picture, painted below Guide text and playback controls.
use std::time::{Duration, Instant};

use gpui::{Div, Hsla, Rgba, div, prelude::*, rgb};

use super::{
    Layout,
    backdrop::mix_hue,
    playback::{FullscreenFrame, PlaybackLayout, moving_picture, picture_reference},
};

type Palette = [Rgba; 3];
const FADE: Duration = Duration::from_millis(450);

#[derive(Default)]
pub(crate) struct Ambient {
    dock_sprites: [super::glow::Glow; 3],
    full_sprites: [super::glow::Glow; 2],
    from: Palette,
    to: Palette,
    changed: Option<Instant>,
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
        true
    }

    pub(crate) fn layer(
        &self,
        window: &gpui::Window,
        frame: FullscreenFrame,
        reduced: bool,
    ) -> impl Fn() -> Div + use<> {
        let layout = Layout::new(window.viewport_size());
        let fade = if reduced { 1. } else { progress(self.changed) };
        let colors = if fade < 1. {
            if frame.fullscreen < 1. || frame.band > 0. {
                window.request_animation_frame();
            }
            mix(self.from, self.to, fade)
        } else {
            self.to
        };
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
            [x - 40., y - 130., 300., layout.height + 260.],
            [x + w - 260., y - 130., 300., layout.height + 260.],
        ];
        let dock_sprites = self.dock_sprites.clone();
        let full_sprites = self.full_sprites.clone();
        move || {
            let mut layer = div().absolute().inset_0().overflow_hidden();
            for (i, color) in colors.into_iter().enumerate() {
                let alpha = color.a * 0.48;
                if alpha > 0. && t < 1. {
                    layer = layer.child(dock_sprites[i].paint(
                        layout,
                        dock[i],
                        210.,
                        Rgba {
                            a: alpha * (1. - t),
                            ..color
                        },
                    ));
                }
                if alpha > 0. && t > 0. && i < full.len() {
                    layer = layer.child(full_sprites[i].paint(
                        layout,
                        full[i],
                        190.,
                        Rgba {
                            a: alpha * t,
                            ..color
                        },
                    ));
                }
            }
            layer
        }
    }
}

fn progress(changed: Option<Instant>) -> f32 {
    changed.map_or(1., |start| {
        let t = (start.elapsed().as_secs_f32() / FADE.as_secs_f32()).min(1.);
        t * t * (3. - 2. * t)
    })
}
