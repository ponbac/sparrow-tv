//! A bilinearly stretched sprite replaces a full-screen analytical shadow.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use gpui::{Div, RenderImage, Rgba, canvas, prelude::*};
use image::{Frame, RgbaImage};

use super::Layout;

#[derive(Clone, Default)]
pub(super) struct Glow(Rc<RefCell<Option<Sprite>>>);

#[derive(PartialEq)]
struct Shape {
    color: [u8; 3],
    width: u16,
    height: u16,
    blur: u16,
}

struct Sprite {
    shape: Shape,
    image: Arc<RenderImage>,
}

impl Shape {
    fn raster(&self) -> Arc<RenderImage> {
        let (w, h, blur) = (
            f32::from(self.width),
            f32::from(self.height),
            f32::from(self.blur),
        );
        let radius = w.min(h) * 0.5;
        let pixels = RgbaImage::from_fn(128, 72, |i, j| {
            let x = (((i as f32 + 0.5) / 128. - 0.5) * (w + 3. * blur)).abs();
            let y = (((j as f32 + 0.5) / 72. - 0.5) * (h + 3. * blur)).abs();
            let dx = x - (w * 0.5 - radius);
            let dy = y - (h * 0.5 - radius);
            let distance = dx.max(0.).hypot(dy.max(0.)) + dx.max(dy).min(0.) - radius;
            let t = (0.5 - distance / (2. * blur)).clamp(0., 1.);
            let alpha = t * t * (3. - 2. * t);
            let [b, g, r] = self.color;
            image::Rgba([b, g, r, (255. * alpha).round() as u8])
        });
        Arc::new(RenderImage::new([Frame::new(pixels)]))
    }
}

impl Glow {
    pub(super) fn paint(
        &self,
        layout: Layout,
        [x, y, w, h]: [f32; 4],
        blur: f32,
        color: Rgba,
    ) -> Div {
        let shape = Shape {
            color: [color.b, color.g, color.r].map(|v| (255. * v).round() as u8),
            width: w.round() as u16,
            height: h.round() as u16,
            blur: blur.round() as u16,
        };
        let cache = self.clone();
        layout
            .rect(x - 1.5 * blur, y - 1.5 * blur, w + 3. * blur, h + 3. * blur)
            .opacity(color.a)
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| {
                        let mut cache = cache.0.borrow_mut();
                        if cache.as_ref().is_none_or(|sprite| sprite.shape != shape) {
                            let image = shape.raster();
                            if let Some(old) = cache.replace(Sprite { shape, image }) {
                                let _ = window.drop_image(old.image);
                            }
                        }
                        let image = &cache.as_ref().unwrap().image;
                        let _ = window.paint_image(
                            bounds,
                            bounds,
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
