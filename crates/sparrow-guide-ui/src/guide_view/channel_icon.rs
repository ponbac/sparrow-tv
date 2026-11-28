use gpui::{Bounds, Corners, Div, canvas, div, point, prelude::*, px, rgb, rgba, size};
use sparrow_core::ChannelIcon;
use sparrow_tv_guide::GuideFamily;

use crate::channel_icons::IconImage;

use super::{DIM, FOCUSED_PLATE, ICE, Layout, PLATE, SAIRA_NUMBER, TEXT};

pub(in super::super) fn family_icon(family: &GuideFamily) -> Option<&ChannelIcon> {
    family.variants().first()?.channel().icon()
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Plate {
    Now,
    Timeline,
    Poster,
    Preview,
    InlineSearch,
    SearchDesk,
}

impl Plate {
    fn geometry(self) -> (f32, f32, f32, f32, f32) {
        match self {
            Self::Now => (72., 54., 8., 30., 2.6667),
            Self::Timeline => (76., 57., 8., 24., 2.),
            Self::Poster => (276., 207., 10., 132., 0.),
            Self::Preview => (120., 90., 10., 42., 0.),
            Self::InlineSearch => (56., 42., 6., 21., 0.),
            Self::SearchDesk => (72., 54., 8., 30., 0.),
        }
    }
}

pub(super) fn plate(
    layout: Layout,
    kind: Plate,
    number: u32,
    focused: bool,
    icon: Option<&IconImage>,
) -> Div {
    let (width, height, radius, number_size, number_padding) = kind.geometry();
    let ground = if !focused {
        rgb(PLATE)
    } else if kind == Plate::Now {
        rgb(FOCUSED_PLATE)
    } else {
        rgba(0x06121c8c)
    };
    let plate = div()
        .w(layout.px(width))
        .h(layout.px(height))
        .rounded(layout.px(radius))
        .flex_none();
    let Some(icon) = icon else {
        let poster = kind == Plate::Poster;
        let number_ink = match (poster, focused) {
            (true, _) => rgb(ICE),
            (_, true) => rgb(TEXT),
            _ => rgba(DIM),
        };
        return plate
            .flex()
            .items_center()
            .pb(layout.px(number_padding))
            .when(!poster, |plate| plate.bg(ground).justify_center())
            .child(
                layout
                    .text(
                        SAIRA_NUMBER,
                        number_size,
                        number_size,
                        if poster { 250. } else { 400. },
                    )
                    .text_color(number_ink)
                    .child(number.to_string()),
            );
    };

    let source = icon.source_dimensions();
    let shape = IconShape::from_dimensions(source);
    let square = shape == IconShape::Square;
    let image = icon.render_image();
    let rounded = layout.px(radius);
    let drawing = canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let fitted = shape.place(
                [f32::from(bounds.size.width), f32::from(bounds.size.height)],
                source,
            );
            let image_bounds = Bounds {
                origin: bounds.origin + point(px(fitted[0]), px(fitted[1])),
                size: size(px(fitted[2]), px(fitted[3])),
            };
            // GPUI intersects the destination with the plate bounds and applies
            // the corner radii to that clipped sprite.
            let _ =
                window.paint_image(bounds, image_bounds, Corners::all(rounded), image, 0, false);
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    plate
        .bg(if square { rgb(0xffffff) } else { ground })
        .child(drawing)
        .child(
            layout
                .rect(0., 0., width, height)
                .rounded(layout.px(radius))
                .border(layout.px(1.))
                .border_color(rgba(if square { 0x0a111e24 } else { 0xeef3f81c }))
                .when(square, |veil| veil.bg(rgba(0x28467821))),
        )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IconShape {
    Square,
    Wide,
    Tall,
}

impl IconShape {
    fn from_dimensions((width, height): (u32, u32)) -> Self {
        let ratio = f64::from(width) / f64::from(height);
        if ratio > 1.05 {
            Self::Wide
        } else if ratio < 1. / 1.05 {
            Self::Tall
        } else {
            Self::Square
        }
    }

    /// The centered image rectangle before the rounded plate clips it.
    fn place(
        self,
        [width, height]: [f32; 2],
        (source_width, source_height): (u32, u32),
    ) -> [f32; 4] {
        let source_width = source_width as f32;
        let source_height = source_height as f32;
        let scale = match self {
            Self::Square => width * 1.15 / source_width.min(source_height),
            Self::Wide | Self::Tall => {
                let (top, side) = if self == Self::Tall {
                    (0.08, 0.24)
                } else {
                    (0.17, 0.09)
                };
                // CSS percentage padding is relative to the plate's width on both axes.
                let image_width = width * (1. - 2. * side);
                let image_height = height - width * 2. * top;
                (image_width / source_width).min(image_height / source_height)
            }
        };
        let image_width = source_width * scale;
        let image_height = source_height * scale;
        [
            (width - image_width) / 2.,
            (height - image_height) / 2.,
            image_width,
            image_height,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::IconShape;

    #[test]
    fn square_tiles_zoom_and_wordmarks_fit_within_their_padding() {
        fn assert_near(actual: [f32; 4], expected: [f32; 4]) {
            for (actual, expected) in actual.into_iter().zip(expected) {
                assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
            }
        }

        let plate = [72., 54.];
        assert_eq!(IconShape::from_dimensions((105, 100)), IconShape::Square);
        assert_eq!(IconShape::from_dimensions((100, 105)), IconShape::Square);
        assert_eq!(IconShape::from_dimensions((106, 100)), IconShape::Wide);
        assert_eq!(IconShape::from_dimensions((100, 106)), IconShape::Tall);
        assert_near(
            IconShape::Square.place(plate, (100, 100)),
            [-5.4, -14.4, 82.8, 82.8],
        );
        assert_near(
            IconShape::Wide.place(plate, (300, 100)),
            [6.48, 17.16, 59.04, 19.68],
        );
        assert_near(
            IconShape::Tall.place(plate, (100, 300)),
            [28.92, 5.76, 14.16, 42.48],
        );
    }
}
