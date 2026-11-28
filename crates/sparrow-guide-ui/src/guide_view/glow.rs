//! A bilinearly stretched sprite replaces a full-screen analytical shadow.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use gpui::{Div, RenderImage, Rgba, bounds, canvas, point, prelude::*, size};
use image::{Frame, RgbaImage};

use super::Layout;

const WIDTH: u32 = 96;
const HEIGHT: u32 = 54;
const PAD: u32 = 2;

#[derive(Clone, Default)]
pub(super) struct Glow(Rc<RefCell<Option<Sprite>>>);

#[derive(PartialEq)]
struct Shape {
    width: u16,
    height: u16,
    blur: u16,
}

struct Sprite {
    shape: Shape,
    mask: Vec<u8>,
    color: [u8; 3],
    image: Arc<RenderImage>,
}

impl Shape {
    // Integrate the rounded rectangle with the same four Gaussian samples as
    // the GPUI shadow shader. Only geometry changes require a new alpha mask.
    fn mask(&self) -> Vec<u8> {
        let (w, h, sigma) = (
            f32::from(self.width),
            f32::from(self.height),
            f32::from(self.blur),
        );
        let r = w.min(h) * 0.5;
        let mut mask = Vec::with_capacity((WIDTH * HEIGHT) as usize);
        let scale = std::f32::consts::FRAC_1_SQRT_2 / sigma;
        for row in 0..HEIGHT {
            let y = ((row as f32 + 0.5) / HEIGHT as f32 - 0.5) * (h + 6. * sigma);
            let start = (y - h / 2.).clamp(-3. * sigma, 3. * sigma);
            let step = ((y + h / 2.).clamp(-3. * sigma, 3. * sigma) - start) / 4.;
            let samples: [(f32, f32); 4] = std::array::from_fn(|i| {
                let z = start + (i as f32 + 0.5) * step;
                let delta = (h / 2. - r - (y - z).abs()).min(0.);
                let edge = w / 2. - r + (r * r - delta * delta).max(0.).sqrt();
                let weight = (-0.5 * (z / sigma).powi(2)).exp() * step
                    / (sigma * (2. * std::f32::consts::PI).sqrt());
                (edge * scale, weight)
            });
            for col in 0..WIDTH {
                let x = ((col as f32 + 0.5) / WIDTH as f32 - 0.5) * (w + 6. * sigma) * scale;
                let alpha: f32 = samples
                    .iter()
                    .map(|&(edge, weight)| 0.5 * weight * (erf(x + edge) - erf(x - edge)))
                    .sum();
                mask.push((255. * alpha).round() as u8);
            }
        }
        mask
    }
}

fn erf(v: f32) -> f32 {
    let a = v.abs();
    let q = 1. + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
    v.signum() * (1. - 1. / q.powi(4))
}

fn sprite_pixels(mask: &[u8], tint: [u8; 3]) -> RgbaImage {
    let mut pixels = RgbaImage::new(WIDTH + 2 * PAD, HEIGHT + 2 * PAD);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.put_pixel(
                x + PAD,
                y + PAD,
                image::Rgba([tint[0], tint[1], tint[2], mask[(y * WIDTH + x) as usize]]),
            );
        }
    }
    pixels
}

impl Glow {
    pub(super) fn paint(
        &self,
        layout: Layout,
        [x, y, w, h]: [f32; 4],
        blur: f32,
        color: Rgba,
    ) -> Div {
        let tint = [color.b, color.g, color.r].map(|v| (255. * v).round() as u8);
        let shape = Shape {
            width: w.round() as u16,
            height: h.round() as u16,
            blur: blur.round() as u16,
        };
        let cache = self.clone();
        layout
            .rect(x - 3. * blur, y - 3. * blur, w + 6. * blur, h + 6. * blur)
            .overflow_hidden()
            .opacity(color.a)
            .child(
                canvas(
                    |_, _, _| (),
                    move |clip, (), window, _| {
                        let mut cache = cache.0.borrow_mut();
                        if cache
                            .as_ref()
                            .is_none_or(|sprite| sprite.shape != shape || sprite.color != tint)
                        {
                            let mask = match cache.take() {
                                Some(old) => {
                                    let _ = window.drop_image(old.image);
                                    if old.shape == shape {
                                        old.mask
                                    } else {
                                        shape.mask()
                                    }
                                }
                                None => shape.mask(),
                            };
                            let pixels = sprite_pixels(&mask, tint);
                            let image = Arc::new(RenderImage::new([Frame::new(pixels)]));
                            *cache = Some(Sprite {
                                shape,
                                mask,
                                color: tint,
                                image,
                            });
                        }
                        let image = &cache.as_ref().unwrap().image;
                        // Linear sampling must stay inside GPUI's unpadded atlas tile.
                        let pad_x = clip.size.width * (PAD as f32 / WIDTH as f32);
                        let pad_y = clip.size.height * (PAD as f32 / HEIGHT as f32);
                        let padded = bounds(
                            clip.origin - point(pad_x, pad_y),
                            clip.size + size(pad_x * 2., pad_y * 2.),
                        );
                        let _ = window.paint_image(
                            clip,
                            padded,
                            Default::default(),
                            image.clone(),
                            0,
                            false,
                        );
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprite_has_two_transparent_texels_on_every_edge() {
        let pixels = sprite_pixels(&vec![180; (WIDTH * HEIGHT) as usize], [10, 20, 30]);
        assert_eq!(pixels.dimensions(), (100, 58));
        for (x, y, pixel) in pixels.enumerate_pixels() {
            let expected = if (2..98).contains(&x) && (2..56).contains(&y) {
                [10, 20, 30, 180]
            } else {
                [0; 4]
            };
            assert_eq!(pixel.0, expected, "texel ({x}, {y})");
        }
    }
}
