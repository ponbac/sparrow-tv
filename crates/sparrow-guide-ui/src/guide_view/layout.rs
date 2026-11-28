//! Profile selection and the geometry each profile lays the Guide out with.
//!
//! Every component draws in layout units. Television keeps the shipped
//! 1920-unit canvas. Desktop and Pocket keep the same type and control sizes in
//! units, choose a legible unit size in logical pixels, and let the canvas width
//! and the band, dock and board geometry follow the window.

use gpui::{Div, FontWeight, IntoElement, Pixels, Size, Styled, div, px, svg};
use sparrow_tv_guide::guide_screen::{BoardGeometry, BoardMetrics, ScreenFocus};

use super::TEXT;

/// The kind of device a Guide window runs on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormFactor {
    Television,
    Handheld,
    Desktop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DesktopClass {
    Compact,
    Regular,
    Wide,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PocketClass {
    Portrait,
    ShortLandscape,
    Tablet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Pointer {
    Coarse,
    Fine,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Profile {
    Tv,
    Desktop(DesktopClass),
    /// Handhelds, and Desktop windows too small for the Desktop band. They use
    /// Compact Desktop geometry until the Pocket layout lands.
    Pocket(PocketClass, Pointer),
}

pub(super) const CANVAS_WIDTH: f32 = 1920.;
pub(super) const GUTTER: f32 = 40.;
pub(super) const BAND_TOP: f32 = 27.;
pub(super) const CARD_GAP: f32 = 11.;
const DOCK_CAPTION: f32 = 44.;
const DOCK_GAP: f32 = 13.;
const LANE_HEIGHT: f32 = 78.;
const DESKTOP_MIN: (f32, f32) = (900., 520.);
/// A window has to cross a class edge by this much, in logical pixels, to change class.
const HYSTERESIS: f32 = 24.;
const MIN_CARD_WIDTH: f32 = 560.;
const MAX_COLUMNS: usize = 6;
const SHEET_MAX_WIDTH: f32 = 760.;

impl DesktopClass {
    /// Window widths, in logical pixels, that select this class.
    fn widths(self) -> (f32, f32) {
        match self {
            Self::Compact => (0., 1280.),
            Self::Regular => (1280., 1800.),
            Self::Wide => (1800., f32::INFINITY),
        }
    }

    fn for_width(width: f32) -> Self {
        [Self::Compact, Self::Regular, Self::Wide]
            .into_iter()
            .find(|class| width < class.widths().1)
            .unwrap_or(Self::Wide)
    }
}

impl Profile {
    /// Picks the profile for a viewport in logical pixels, keeping `previous`
    /// while the viewport stays within the hysteresis margin of its class.
    pub(crate) fn select(form: FormFactor, viewport: (f32, f32), previous: Option<Self>) -> Self {
        let (width, height) = viewport;
        match form {
            FormFactor::Television => return Self::Tv,
            FormFactor::Desktop => {
                let kept = match previous {
                    Some(Self::Desktop(class)) => Some(class),
                    _ => None,
                };
                let slack = kept.map_or(0., |_| HYSTERESIS);
                if width >= DESKTOP_MIN.0 - slack && height >= DESKTOP_MIN.1 - slack {
                    let class = kept
                        .filter(|class| {
                            let (low, high) = class.widths();
                            width >= low - HYSTERESIS && width < high + HYSTERESIS
                        })
                        .unwrap_or_else(|| DesktopClass::for_width(width));
                    return Self::Desktop(class);
                }
            }
            FormFactor::Handheld => {}
        }
        let pointer = if form == FormFactor::Handheld {
            Pointer::Coarse
        } else {
            Pointer::Fine
        };
        let class = if width > height && (height < 520. || (width <= 1050. && height <= 600.)) {
            PocketClass::ShortLandscape
        } else if width.min(height) >= 600. {
            PocketClass::Tablet
        } else {
            PocketClass::Portrait
        };
        Self::Pocket(class, pointer)
    }

    /// Logical pixels per layout unit; Television derives it from the viewport.
    fn unit(self) -> Option<f32> {
        match self {
            Self::Tv => None,
            Self::Desktop(DesktopClass::Compact) => Some(0.65),
            Self::Desktop(DesktopClass::Regular) => Some(0.7),
            Self::Desktop(DesktopClass::Wide) => Some(0.75),
            Self::Pocket(_, Pointer::Fine) => Some(0.62),
            Self::Pocket(PocketClass::Tablet, Pointer::Coarse) => Some(0.66),
            Self::Pocket(_, Pointer::Coarse) => Some(0.6),
        }
    }
}

/// Profile geometry in layout units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Metrics {
    /// The docked picture: x, y, width and height.
    pub(crate) dock: [f32; 4],
    pub(crate) board: BoardMetrics,
    /// The tallest fullscreen picture above the control band.
    pub(crate) band_picture_height: f32,
}

impl Metrics {
    fn tv(height: f32) -> Self {
        Self {
            dock: [GUTTER, BAND_TOP, 640., 360.],
            board: BoardMetrics::tv(height),
            band_picture_height: 876.,
        }
    }

    /// The dock follows the window: about a third of its width, held to the
    /// height that leaves the board room for several rows.
    fn fluid(width: f32, height: f32) -> Self {
        let fit = (width * 0.34).min((height * 0.42 - BAND_TOP - DOCK_CAPTION) * 16. / 9.);
        let dock_width = ((fit / 16.).floor() * 16.).clamp(400., 768.);
        let dock_height = dock_width * 9. / 16.;
        let band_bottom = BAND_TOP + dock_height + DOCK_CAPTION + DOCK_GAP;
        let columns = ((width - 2. * GUTTER + CARD_GAP) / (MIN_CARD_WIDTH + CARD_GAP)).floor();
        Self {
            dock: [GUTTER, BAND_TOP, dock_width, dock_height],
            board: BoardMetrics {
                now: BoardGeometry {
                    top: band_bottom + LANE_HEIGHT,
                    columns: (columns.max(1.) as usize).min(MAX_COLUMNS),
                    ..BoardMetrics::TV_NOW
                },
                timeline: BoardGeometry {
                    top: band_bottom,
                    ..BoardMetrics::TV_TIMELINE
                },
                surface_height: height,
            },
            band_picture_height: f32::INFINITY,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub(crate) profile: Profile,
    pub(crate) translucent: bool,
    /// Whether keyboard and remote focus is drawn. Pointer input hides it
    /// without moving it.
    pub(crate) focus_visible: bool,
    /// Logical pixels per layout unit.
    pub(crate) unit: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) m: Metrics,
}

impl Layout {
    pub(crate) fn new(profile: Profile, viewport: Size<Pixels>) -> Self {
        let (width, height) = (f32::from(viewport.width), f32::from(viewport.height));
        let unit = profile
            .unit()
            .unwrap_or(width / CANVAS_WIDTH)
            .max(f32::EPSILON);
        let (width, height) = (width / unit, height / unit);
        let m = match profile {
            Profile::Tv => Metrics::tv(height),
            Profile::Desktop(_) | Profile::Pocket(..) => Metrics::fluid(width, height),
        };
        Self {
            profile,
            translucent: false,
            focus_visible: true,
            unit,
            width,
            height,
            m,
        }
    }

    pub(crate) fn guide_text(self) -> gpui::Rgba {
        gpui::rgb(if self.translucent { 0xffffff } else { TEXT })
    }

    pub(super) fn secondary(self, color: u32) -> gpui::Rgba {
        let mut ink = gpui::rgba(color);
        if self.translucent {
            ink.a = (ink.a + 0.12).min(1.);
        }
        ink
    }

    pub(super) fn surface(self, color: u32) -> gpui::Rgba {
        let mut fill = gpui::rgb(color);
        if self.translucent {
            fill.a = 0.85;
        }
        fill
    }

    pub(super) fn px(self, length: f32) -> Pixels {
        px(length * self.unit)
    }

    pub(super) fn rect(self, x: f32, y: f32, width: f32, height: f32) -> Div {
        div()
            .absolute()
            .left(self.px(x))
            .top(self.px(y))
            .w(self.px(width.max(0.)))
            .h(self.px(height.max(0.)))
    }

    pub(super) fn text(self, font: &'static str, size: f32, line: f32, weight: f32) -> Div {
        div()
            .font_family(font)
            .font_weight(FontWeight(weight))
            .text_size(self.px(size))
            .line_height(self.px(line))
    }

    pub(super) fn icon(
        self,
        data: &'static [u8],
        size: f32,
        color: gpui::Rgba,
    ) -> impl IntoElement {
        svg()
            .data(data)
            .size(self.px(size))
            .text_color(color)
            .flex_none()
    }

    /// The focus a component draws: pointer input keeps focus but hides it.
    pub(crate) fn shown_focus(self, focus: ScreenFocus) -> ScreenFocus {
        if self.focus_visible {
            focus
        } else {
            ScreenFocus::None
        }
    }

    /// The bottom edge of the band, where the lane and the Timeline rail begin.
    pub(super) fn band_bottom(self) -> f32 {
        self.m.board.timeline.top
    }

    pub(super) fn lane_height(self) -> f32 {
        self.m.board.now.top - self.band_bottom()
    }

    pub(super) fn columns(self) -> usize {
        self.m.board.now.columns
    }

    pub(super) fn column_width(self) -> f32 {
        let columns = self.columns();
        (self.width - GUTTER * 2. - CARD_GAP * (columns - 1) as f32) / columns as f32
    }

    pub(super) fn dock_preview_x(self) -> f32 {
        GUTTER + self.m.dock[2] + 44.
    }

    /// Width of the right-hand sheets: Search desk, Choose groups and Sources.
    pub(crate) fn sheet_width(self) -> f32 {
        (self.width * 0.36).min(SHEET_MAX_WIDTH)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{px, size};

    use super::*;

    fn desktop(width: f32, height: f32) -> Layout {
        let profile = Profile::select(FormFactor::Desktop, (width, height), None);
        Layout::new(profile, size(px(width), px(height)))
    }

    #[test]
    fn television_keeps_the_shipped_canvas_at_any_size() {
        for (width, height) in [(1920., 1080.), (3840., 2160.), (1280., 720.)] {
            let profile = Profile::select(FormFactor::Television, (width, height), None);
            assert_eq!(profile, Profile::Tv);
            let layout = Layout::new(profile, size(px(width), px(height)));
            assert_eq!(layout.width, 1920.);
            assert_eq!(layout.height, 1080.);
            assert_eq!(layout.m.dock, [40., 27., 640., 360.]);
            assert_eq!(layout.m.board, BoardMetrics::tv(1080.));
            assert_eq!(layout.band_bottom(), 444.);
            assert_eq!(layout.lane_height(), 78.);
            assert_eq!(layout.column_width(), (1920. - 80. - 22.) / 3.);
            assert_eq!(layout.dock_preview_x(), 724.);
            assert_eq!(layout.sheet_width(), 1920. * 0.36);
        }
    }

    #[test]
    fn desktop_classes_follow_the_window_width_with_hysteresis() {
        let select = |viewport, previous| Profile::select(FormFactor::Desktop, viewport, previous);
        let compact = Profile::Desktop(DesktopClass::Compact);
        let regular = Profile::Desktop(DesktopClass::Regular);
        let wide = Profile::Desktop(DesktopClass::Wide);
        assert_eq!(select((900., 600.), None), compact);
        assert_eq!(select((950., 1060.), None), compact);
        assert_eq!(select((1440., 900.), None), regular);
        assert_eq!(select((1920., 1080.), None), wide);
        assert_eq!(select((3440., 1440.), None), wide);

        assert_eq!(select((1290., 800.), Some(compact)), compact);
        assert_eq!(select((1310., 800.), Some(compact)), regular);
        assert_eq!(select((1790., 800.), Some(wide)), wide);
        assert_eq!(select((1770., 800.), Some(wide)), regular);
        assert_eq!(select((1270., 800.), Some(regular)), regular);
        assert_eq!(select((1250., 800.), Some(regular)), compact);
        assert_eq!(select((890., 510.), Some(compact)), compact);
        assert_eq!(
            select((890., 510.), None),
            Profile::Pocket(PocketClass::ShortLandscape, Pointer::Fine)
        );
    }

    #[test]
    fn small_desktop_windows_and_handhelds_select_a_pocket_class() {
        let select = Profile::select;
        assert_eq!(
            select(FormFactor::Desktop, (1920., 500.), None),
            Profile::Pocket(PocketClass::ShortLandscape, Pointer::Fine)
        );
        assert_eq!(
            select(FormFactor::Desktop, (800., 1000.), None),
            Profile::Pocket(PocketClass::Tablet, Pointer::Fine)
        );
        assert_eq!(
            select(FormFactor::Handheld, (412., 915.), None),
            Profile::Pocket(PocketClass::Portrait, Pointer::Coarse)
        );
        assert_eq!(
            select(FormFactor::Handheld, (915., 412.), None),
            Profile::Pocket(PocketClass::ShortLandscape, Pointer::Coarse)
        );
    }

    #[test]
    fn desktop_columns_and_dock_scale_with_the_window() {
        for ((width, height), columns, dock_px) in [
            ((900., 600.), 2, 301.6),
            ((1440., 900.), 3, 481.6),
            ((1920., 1080.), 4, 576.),
            ((3440., 1440.), 6, 576.),
        ] {
            let layout = desktop(width, height);
            assert_eq!(layout.columns(), columns, "{width}x{height}");
            assert!((layout.m.dock[2] * layout.unit - dock_px).abs() < 0.5);
            assert_eq!(layout.m.dock[2] * 9., layout.m.dock[3] * 16.);
            assert_eq!(layout.m.board.surface_height, layout.height);
            assert!(layout.column_width() >= MIN_CARD_WIDTH);
            // Text stays at a legible size instead of shrinking with the canvas.
            assert!(layout.unit >= 0.65);
            // The board always keeps room for at least three rows of cards.
            let rows = (layout.height - layout.m.board.now.top) / layout.m.board.now.pitch;
            assert!(rows >= 3., "{width}x{height} fits {rows} rows");
        }
    }
}
