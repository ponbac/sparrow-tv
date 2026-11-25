use super::{Layout, LiftFrame};
use gpui::{
    Bounds, ContentMask, PathBuilder, Pixels, Window, canvas, fill, point, prelude::*, rgb, size,
};

const CORNER_RADIUS: f32 = 8.;
const CLIP_POINT: f32 = 12.;

pub(super) fn paint(
    layout: Layout,
    cut: bool,
    fill_color: u32,
    elapsed: Option<(f32, u32)>,
    lift: Option<LiftFrame>,
) -> impl gpui::IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let radius = layout.px(CORNER_RADIUS);
            let path = cut
                .then(|| left_cut(bounds, layout.px(CLIP_POINT), radius))
                .flatten();
            let paint = |window: &mut Window, color| {
                let base = if lift.is_some() {
                    rgb(color)
                } else {
                    layout.surface(color)
                };
                let fills = std::iter::once(base.into()).chain(
                    lift.and_then(|lift| lift.sheen(color, bounds.size.width / bounds.size.height))
                        .into_iter()
                        .flatten(),
                );
                for fill_color in fills {
                    if let Some(path) = path.as_ref() {
                        window.paint_path(path.clone(), fill_color);
                    } else {
                        window.paint_quad(fill(bounds, fill_color).corner_radii(radius));
                    }
                }
            };
            paint(window, fill_color);
            if let Some((elapsed, elapsed_color)) = elapsed {
                let width = layout.px(elapsed).min(bounds.size.width);
                let mask = ContentMask {
                    bounds: Bounds::new(bounds.origin, size(width, bounds.size.height)),
                };
                window.with_content_mask(Some(mask), |window| paint(window, elapsed_color));
            }
        },
    )
    .size_full()
}

fn left_cut(bounds: Bounds<Pixels>, tip: Pixels, radius: Pixels) -> Option<gpui::Path<Pixels>> {
    let mut path = PathBuilder::fill();
    let width = bounds.size.width;
    let height = bounds.size.height;
    let radius = radius.min(width / 2.);
    let tip = tip.min(width - radius);
    let at = |x, y| bounds.origin + point(x, y);
    let curve = radius * 0.552_284_8;
    let zero = gpui::px(0.);
    path.move_to(at(tip, zero));
    path.line_to(at(width - radius, zero));
    path.cubic_bezier_to(
        at(width, radius),
        at(width - radius + curve, zero),
        at(width, radius - curve),
    );
    path.line_to(at(width, height - radius));
    path.cubic_bezier_to(
        at(width - radius, height),
        at(width, height - radius + curve),
        at(width - radius + curve, height),
    );
    path.line_to(at(tip, height));
    path.line_to(at(zero, height / 2.));
    path.close();
    path.build().ok()
}
