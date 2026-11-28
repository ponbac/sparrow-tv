//! Profile selection and the geometry each profile lays the Guide out with.
//!
//! Every component draws in layout units. Television keeps the shipped
//! 1920-unit canvas. Desktop and Pocket keep the same type and control sizes in
//! units, choose a legible unit size in logical pixels, and let the canvas width
//! and the band, dock and board geometry follow the window.

use gpui::{Div, FontWeight, IntoElement, ParentElement, Pixels, Size, Styled, div, px, svg};
use sparrow_tv_guide::guide_screen::{BoardGeometry, BoardMetrics, ScreenFocus};

use super::TEXT;
use crate::Insets;

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

/// Where a handheld shows the picture outside fullscreen: on the Watch stage
/// or docked in the Guide's picture band.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PocketMode {
    #[default]
    Guide,
    Watch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Profile {
    Tv,
    Desktop(DesktopClass),
    /// Handhelds, and Desktop windows too small for the Desktop band. Portrait
    /// and short landscape lay out the compact Pocket list; tablets keep the
    /// band model inside the safe area.
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
/// A Desktop window has to cross a class edge by this much, in logical pixels,
/// to change class. Pocket classes follow the viewport directly.
const HYSTERESIS: f32 = 24.;
const MIN_CARD_WIDTH: f32 = 560.;
const MAX_COLUMNS: usize = 6;
const SHEET_MAX_WIDTH: f32 = 760.;
const FULLSCREEN_BAND: f32 = 204.;

/// Side padding inside the safe area on compact Pocket layouts.
pub(super) const POCKET_GUTTER: f32 = 27.;
/// The compact Pocket Guide: the band row holding the docked picture, then a
/// toolbar, lane and list inside the Guide region.
const POCKET_DOCK: [f32; 2] = [256., 144.];
const POCKET_BAND_PAD: f32 = 16.;
const POCKET_BAND_ROW: f32 = POCKET_BAND_PAD + 144.;
pub(super) const POCKET_TOOLBAR_GAP: f32 = 8.;
pub(super) const POCKET_ROW_HEIGHT: f32 = 116.;
const POCKET_ROW_PITCH: f32 = 118.;
const POCKET_FULLSCREEN_BAND: f32 = 200.;
const TABLET_FULLSCREEN_BAND: f32 = 220.;
const TABLET_SHEET_SHARE: f32 = 0.45;
/// Short landscape gives the picture column this share of the safe width.
const SPLIT_SHARE: f32 = 0.42;
/// Room the short-landscape column keeps under its picture for the
/// information block and the control row.
const SPLIT_BELOW_PICTURE: f32 = 270.;
/// Landscape tablets this wide, in logical pixels, can switch the board to Timeline.
const TIMELINE_TABLET_WIDTH: f32 = 960.;
const WATCH_MASTHEAD: f32 = 80.;
const WATCH_STRIPE: f32 = 5.;
const WATCH_LOWER_THIRD: f32 = 190.;
const WATCH_CONTROLS: f32 = 96.;
const WATCH_CHANNEL_BAR: f32 = 112.;

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

    pub(crate) fn pointer(self) -> Option<Pointer> {
        match self {
            Self::Tv => None,
            Self::Desktop(_) => Some(Pointer::Fine),
            Self::Pocket(_, pointer) => Some(pointer),
        }
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

/// Profile geometry in layout units. Rectangles are x, y, width and height in
/// window units, except `board`, which is relative to the `guide` region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Metrics {
    /// The picture outside fullscreen: the dock, or the Pocket Watch stage.
    pub(crate) dock: [f32; 4],
    pub(crate) board: BoardMetrics,
    /// The tallest fullscreen picture above the control band.
    pub(crate) band_picture_height: f32,
    pub(crate) fullscreen_band: f32,
    /// Where the band, lane and board sections are laid out.
    pub(crate) guide: [f32; 4],
    /// Where the Search, Choose groups and Sources sheets are laid out.
    pub(crate) sheet: [f32; 4],
    pub(crate) sheet_width: f32,
    /// The window inside the safe-area insets.
    pub(crate) safe: [f32; 4],
    pub(crate) pocket: Option<Pocket>,
}

/// Handheld and small-window geometry beyond the shared band model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pocket {
    pub(crate) class: PocketClass,
    pub(crate) mode: PocketMode,
    /// Playback details beside the docked picture (Portrait) or under it
    /// (short landscape). Tablets keep the shared dock caption.
    pub(crate) info: Option<[f32; 4]>,
    pub(crate) watch: Option<WatchGeometry>,
    /// The lane starts with a time chip instead of the board view switch.
    pub(crate) time_chips: bool,
}

impl Pocket {
    /// Compact layouts draw their own toolbar and leave Choose groups out of the lane.
    pub(crate) fn compact(self) -> bool {
        self.class != PocketClass::Tablet
    }
}

/// The Portrait Watch screen, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WatchGeometry {
    pub(crate) masthead: [f32; 4],
    pub(crate) stage: [f32; 4],
    pub(crate) lower_third: [f32; 4],
    pub(crate) controls: [f32; 4],
    pub(crate) details: [f32; 4],
    pub(crate) channel_bar: [f32; 4],
}

impl WatchGeometry {
    fn new([x, y, width, height]: [f32; 4]) -> Self {
        let bottom = y + height;
        let stage_top = y + WATCH_MASTHEAD + WATCH_STRIPE;
        let stage_height = (width * 9. / 16.).min(0.52 * height).max(0.);
        let stage_width = stage_height * 16. / 9.;
        let lower = stage_top + stage_height;
        let controls = lower + WATCH_LOWER_THIRD;
        let details = controls + WATCH_CONTROLS;
        let channel_bar = (bottom - WATCH_CHANNEL_BAR).max(details);
        Self {
            masthead: [x, y, width, WATCH_MASTHEAD],
            stage: [
                x + (width - stage_width) / 2.,
                stage_top,
                stage_width,
                stage_height,
            ],
            lower_third: [x, lower, width, WATCH_LOWER_THIRD],
            controls: [x, controls, width, WATCH_CONTROLS],
            details: [x, details, width, channel_bar - details],
            channel_bar: [x, channel_bar, width, WATCH_CHANNEL_BAR],
        }
    }
}

impl Metrics {
    fn tv(width: f32, height: f32) -> Self {
        let window = [0., 0., width, height];
        Self {
            dock: [GUTTER, BAND_TOP, 640., 360.],
            board: BoardMetrics::tv(height),
            band_picture_height: 876.,
            fullscreen_band: FULLSCREEN_BAND,
            guide: window,
            sheet: window,
            sheet_width: (width * 0.36).min(SHEET_MAX_WIDTH),
            safe: window,
            pocket: None,
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
        let window = [0., 0., width, height];
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
            fullscreen_band: FULLSCREEN_BAND,
            guide: window,
            sheet: window,
            sheet_width: (width * 0.36).min(SHEET_MAX_WIDTH),
            safe: window,
            pocket: None,
        }
    }

    /// Handheld geometry inside the safe area. The Guide region, its board
    /// and the sheets stay put across Watch and Guide; only the picture moves.
    fn pocket(class: PocketClass, mode: PocketMode, safe: [f32; 4], timeline: bool) -> Self {
        let [left, top, width, height] = safe;
        let pocket = |info, watch: Option<WatchGeometry>, time_chips| Pocket {
            class,
            mode,
            info,
            watch,
            time_chips,
        };
        match class {
            PocketClass::Tablet => {
                let mut metrics = Self::fluid(width, height);
                metrics.dock[0] += left;
                metrics.dock[1] += top;
                Self {
                    fullscreen_band: TABLET_FULLSCREEN_BAND,
                    guide: safe,
                    sheet: safe,
                    sheet_width: (width * TABLET_SHEET_SHARE).min(SHEET_MAX_WIDTH),
                    safe,
                    pocket: Some(pocket(None, None, !timeline)),
                    ..metrics
                }
            }
            PocketClass::Portrait => {
                let dock = [
                    left + POCKET_GUTTER,
                    top + POCKET_BAND_PAD,
                    POCKET_DOCK[0],
                    POCKET_DOCK[1],
                ];
                let info_x = dock[0] + dock[2] + 20.;
                let guide = [
                    left,
                    top + POCKET_BAND_ROW,
                    width,
                    (height - POCKET_BAND_ROW).max(0.),
                ];
                let watch = WatchGeometry::new(safe);
                Self::compact(
                    match mode {
                        PocketMode::Guide => dock,
                        PocketMode::Watch => watch.stage,
                    },
                    guide,
                    safe,
                    pocket(
                        Some([
                            info_x,
                            dock[1],
                            (left + width - POCKET_GUTTER - info_x).max(0.),
                            dock[3],
                        ]),
                        Some(watch),
                        true,
                    ),
                )
            }
            PocketClass::ShortLandscape => {
                let column = (width * SPLIT_SHARE).floor();
                let fit = (column - POCKET_GUTTER - 16.)
                    .min((height - 2. * POCKET_BAND_PAD - SPLIT_BELOW_PICTURE) * 16. / 9.)
                    .max(0.);
                let dock = [
                    left + POCKET_GUTTER,
                    top + POCKET_BAND_PAD,
                    fit,
                    fit * 9. / 16.,
                ];
                let below = dock[1] + dock[3] + 12.;
                let guide = [left + column, top, (width - column).max(0.), height];
                Self::compact(
                    dock,
                    guide,
                    safe,
                    pocket(
                        Some([dock[0], below, fit, (top + height - below).max(0.)]),
                        None,
                        true,
                    ),
                )
            }
        }
    }

    /// The compact Guide region holds a toolbar, the lane and one-column list rows.
    fn compact(dock: [f32; 4], guide: [f32; 4], safe: [f32; 4], pocket: Pocket) -> Self {
        let band_bottom = BAND_TOP + super::band::TOOLBAR_HEIGHT + POCKET_TOOLBAR_GAP;
        Self {
            dock,
            board: BoardMetrics {
                now: BoardGeometry {
                    top: band_bottom + LANE_HEIGHT,
                    row_top: 0.,
                    row_height: POCKET_ROW_HEIGHT,
                    pitch: POCKET_ROW_PITCH,
                    columns: 1,
                    ..BoardMetrics::TV_NOW
                },
                timeline: BoardGeometry {
                    top: band_bottom,
                    ..BoardMetrics::TV_TIMELINE
                },
                surface_height: guide[3],
            },
            band_picture_height: f32::INFINITY,
            fullscreen_band: POCKET_FULLSCREEN_BAND,
            guide,
            sheet: guide,
            sheet_width: guide[2].min(SHEET_MAX_WIDTH),
            safe,
            pocket: Some(pocket),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub(crate) profile: Profile,
    /// Where this layout's origin sits in the window, in units. Regions
    /// such as the Pocket Guide and sheets lay out from their own corner.
    pub(crate) origin: [f32; 2],
    pub(crate) insets: Insets,
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
    #[cfg(test)]
    pub(crate) fn new(profile: Profile, viewport: Size<Pixels>) -> Self {
        Self::placed(profile, viewport, Insets::default(), PocketMode::Guide)
    }

    /// A window layout. Only Pocket profiles honour the safe-area insets.
    pub(crate) fn placed(
        profile: Profile,
        viewport: Size<Pixels>,
        insets: Insets,
        mode: PocketMode,
    ) -> Self {
        let (logical_width, logical_height) =
            (f32::from(viewport.width), f32::from(viewport.height));
        let unit = profile
            .unit()
            .unwrap_or(logical_width / CANVAS_WIDTH)
            .max(f32::EPSILON);
        // Television's canvas is exactly the shipped width at any viewport.
        let width = match profile {
            Profile::Tv => CANVAS_WIDTH,
            Profile::Desktop(_) | Profile::Pocket(..) => logical_width / unit,
        };
        let height = logical_height / unit;
        let insets = match profile {
            Profile::Pocket(..) => insets,
            Profile::Tv | Profile::Desktop(_) => Insets::default(),
        };
        let m = match profile {
            Profile::Tv => Metrics::tv(width, height),
            Profile::Desktop(_) => Metrics::fluid(width, height),
            Profile::Pocket(class, _) => {
                let edge = |value: f32| (value / unit).clamp(0., width.max(height));
                let (left, top) = (edge(insets.left), edge(insets.top));
                let (right, bottom) = (edge(insets.right), edge(insets.bottom.max(insets.ime)));
                let safe = [
                    left,
                    top,
                    (width - left - right).max(0.),
                    (height - top - bottom).max(0.),
                ];
                let timeline =
                    logical_width > logical_height && logical_width >= TIMELINE_TABLET_WIDTH;
                Metrics::pocket(class, mode, safe, timeline)
            }
        };
        Self {
            profile,
            origin: [0., 0.],
            insets,
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

    pub(super) fn bottom_occlusion(self) -> f32 {
        self.insets.bottom.max(self.insets.ime) / self.unit
    }

    /// A layout for a window rectangle, laid out from that rectangle's corner.
    pub(crate) fn region(self, [x, y, width, height]: [f32; 4]) -> Self {
        Self {
            origin: [self.origin[0] + x, self.origin[1] + y],
            width,
            height,
            ..self
        }
    }

    /// Positions a region's content in the window.
    pub(crate) fn place(self, content: impl IntoElement) -> Div {
        div()
            .absolute()
            .left(self.px(self.origin[0]))
            .top(self.px(self.origin[1]))
            .w(self.px(self.width))
            .h(self.px(self.height))
            .child(content)
    }

    /// Converts a region point in window units to this region's units.
    pub(crate) fn local(self, [x, y]: [f32; 2]) -> [f32; 2] {
        [x - self.origin[0], y - self.origin[1]]
    }

    /// Moves a native editor rectangle from this region into window pixels.
    pub(crate) fn to_window(self, rect: [i32; 4], pixel_scale: f32) -> [i32; 4] {
        let physical = |value: f32| (value * self.unit * pixel_scale).round() as i32;
        let [x, y, width, height] = rect;
        [
            x + physical(self.origin[0]),
            y + physical(self.origin[1]),
            width,
            height,
        ]
    }

    pub(crate) fn pocket(self) -> Option<Pocket> {
        self.m.pocket
    }

    /// The compact Pocket layout: toolbar, list rows and the picture band.
    pub(crate) fn compact_pocket(self) -> bool {
        self.pocket().is_some_and(Pocket::compact)
    }

    /// The lane starts with a time chip, so Timeline is unavailable and a
    /// chosen hour can be shown.
    pub(crate) fn time_chips(self) -> bool {
        self.pocket().is_some_and(|pocket| pocket.time_chips)
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
        self.m.sheet_width.min(self.width)
    }
}

/// Whether a point lies inside an x, y, width and height rectangle.
pub(crate) fn contains([x, y, width, height]: [f32; 4], [px, py]: [f32; 2]) -> bool {
    (x..x + width).contains(&px) && (y..y + height).contains(&py)
}

/// A virtualized row list's position, including a partially visible first row.
#[derive(Clone, Copy, Default)]
pub(super) struct RowScroll(f32);

impl RowScroll {
    pub(super) fn advance(&mut self, rows: f32) {
        self.0 = (self.0 + rows).max(0.);
    }

    pub(super) fn reveal(&mut self, count: usize, capacity: usize, target: Option<usize>) {
        self.0 = self.0.min(count.saturating_sub(capacity) as f32);
        // With no room for a row, no offset can show the target.
        if let Some(row) = target.filter(|_| capacity > 0) {
            self.0 = self
                .0
                .clamp((row + 1).saturating_sub(capacity) as f32, row as f32);
        }
    }

    pub(super) fn first(self) -> usize {
        self.0 as usize
    }

    pub(super) fn clipped(self, pitch: f32) -> f32 {
        self.0.fract() * pitch
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

    fn handheld(width: f32, height: f32, insets: Insets, mode: PocketMode) -> Layout {
        let profile = Profile::select(FormFactor::Handheld, (width, height), None);
        Layout::placed(profile, size(px(width), px(height)), insets, mode)
    }

    fn within(inner: [f32; 4], outer: [f32; 4]) -> bool {
        let slack = 0.01;
        inner[0] >= outer[0] - slack
            && inner[1] >= outer[1] - slack
            && inner[0] + inner[2] <= outer[0] + outer[2] + slack
            && inner[1] + inner[3] <= outer[1] + outer[3] + slack
    }

    fn bottom(rect: [f32; 4]) -> f32 {
        rect[1] + rect[3]
    }

    const PHONE_INSETS: Insets = Insets {
        top: 32.,
        right: 0.,
        bottom: 24.,
        left: 0.,
        ime: 0.,
    };

    #[test]
    fn insets_shape_only_pocket_layouts() {
        let insets = Insets {
            top: 40.,
            right: 30.,
            bottom: 50.,
            left: 20.,
            ime: 300.,
        };
        for (profile, width, height) in [
            (Profile::Tv, 1920., 1080.),
            (Profile::Desktop(DesktopClass::Regular), 1440., 900.),
        ] {
            let viewport = size(px(width), px(height));
            let placed = Layout::placed(profile, viewport, insets, PocketMode::Watch);
            let plain = Layout::new(profile, viewport);
            assert_eq!(placed.m, plain.m);
            assert_eq!(placed.insets, Insets::default());
            assert_eq!(placed.m.guide, [0., 0., placed.width, placed.height]);
            assert_eq!(placed.m.sheet, placed.m.guide);
            assert_eq!(placed.sheet_width(), (placed.width * 0.36).min(760.));
        }
    }

    #[test]
    fn portrait_guide_docks_the_picture_in_a_band_above_the_list() {
        let layout = handheld(412., 915., PHONE_INSETS, PocketMode::Guide);
        let pocket = layout.pocket().unwrap();
        assert_eq!(pocket.class, PocketClass::Portrait);
        let [left, top, width, height] = layout.m.safe;
        assert_eq!(left, 0.);
        assert!((top - 32. / layout.unit).abs() < 0.01);
        assert!((top + height - (915. - 24.) / layout.unit).abs() < 0.01);
        assert_eq!(width, layout.width);

        let dock = layout.m.dock;
        assert_eq!(dock, [POCKET_GUTTER, top + 16., 256., 144.]);
        let info = pocket.info.unwrap();
        assert!(info[0] >= dock[0] + dock[2]);
        assert!(within(info, layout.m.safe));
        // The Guide region starts under the band and ends at the gesture inset.
        let guide = layout.m.guide;
        assert!(guide[1] >= bottom(dock));
        assert!((bottom(guide) - bottom(layout.m.safe)).abs() < 0.01);
        assert_eq!(layout.m.sheet, guide);
        assert_eq!(layout.region(guide).sheet_width(), guide[2]);
        assert_eq!(layout.m.board.surface_height, guide[3]);
        assert_eq!(layout.m.board.now.columns, 1);
        assert!(pocket.time_chips);
        // List rows stay comfortable touch targets.
        assert!(layout.m.board.now.row_height * layout.unit >= 68.);
        let rows = (guide[3] - layout.m.board.now.top) / layout.m.board.now.pitch;
        assert!(rows >= 8., "fits {rows} rows");
    }

    #[test]
    fn portrait_watch_stacks_chrome_around_a_full_width_stage() {
        let guide = handheld(412., 915., PHONE_INSETS, PocketMode::Guide);
        let layout = handheld(412., 915., PHONE_INSETS, PocketMode::Watch);
        let watch = layout.pocket().unwrap().watch.unwrap();
        let safe = layout.m.safe;
        assert_eq!(layout.m.dock, watch.stage);
        assert_eq!(watch.stage[2], safe[2]);
        assert!((watch.stage[2] * 9. - watch.stage[3] * 16.).abs() < 0.01);
        // Top to bottom without overlap, and no chrome on the picture.
        let order = [
            watch.masthead,
            watch.stage,
            watch.lower_third,
            watch.controls,
            watch.details,
            watch.channel_bar,
        ];
        for pair in order.windows(2) {
            assert!(bottom(pair[0]) <= pair[1][1] + 0.01, "{pair:?}");
        }
        assert_eq!(watch.masthead[1], safe[1]);
        assert!((bottom(watch.channel_bar) - bottom(safe)).abs() < 0.01);
        assert!(order.iter().all(|rect| within(*rect, safe)));
        assert!(watch.details[3] > 200.);
        // The Guide region, its board and the sheets keep still across modes.
        assert_eq!(layout.m.guide, guide.m.guide);
        assert_eq!(layout.m.board, guide.m.board);
        assert_eq!(layout.m.sheet, guide.m.sheet);
    }

    #[test]
    fn the_keyboard_raises_the_pocket_bottom_edge() {
        let typing = Insets {
            ime: 300.,
            ..PHONE_INSETS
        };
        let layout = handheld(412., 915., typing, PocketMode::Guide);
        let safe = layout.m.safe;
        assert!((bottom(safe) - (915. - 300.) / layout.unit).abs() < 0.01);
        assert!((bottom(layout.m.sheet) - bottom(safe)).abs() < 0.01);
    }

    #[test]
    fn short_landscape_splits_the_picture_from_the_list_inside_the_cutout() {
        let insets = Insets {
            top: 24.,
            right: 0.,
            bottom: 0.,
            left: 32.,
            ime: 0.,
        };
        let layout = handheld(915., 412., insets, PocketMode::Guide);
        let pocket = layout.pocket().unwrap();
        assert_eq!(pocket.class, PocketClass::ShortLandscape);
        assert!(pocket.watch.is_none());
        let [left, top, _, _] = layout.m.safe;
        assert!((left - 32. / layout.unit).abs() < 0.01);
        let dock = layout.m.dock;
        assert_eq!(dock[0], left + POCKET_GUTTER);
        assert_eq!(dock[1], top + 16.);
        assert!((dock[2] * 9. - dock[3] * 16.).abs() < 0.01);
        let info = pocket.info.unwrap();
        assert!(info[1] >= bottom(dock));
        assert!(info[3] >= 260.);
        assert!(within(info, layout.m.safe));
        let guide = layout.m.guide;
        assert!(guide[0] >= dock[0] + dock[2]);
        assert!(within(guide, layout.m.safe));
        assert_eq!(layout.m.sheet, guide);
        assert_eq!(layout.m.board.now.columns, 1);
        // Watch has no landscape screen; the dock stays put.
        assert_eq!(
            handheld(915., 412., insets, PocketMode::Watch).m.dock,
            layout.m.dock
        );
    }

    #[test]
    fn tablets_keep_the_band_model_inside_the_safe_area() {
        let insets = Insets {
            top: 24.,
            right: 0.,
            bottom: 48.,
            left: 0.,
            ime: 0.,
        };
        let landscape = handheld(1280., 800., insets, PocketMode::Guide);
        let pocket = landscape.pocket().unwrap();
        assert_eq!(pocket.class, PocketClass::Tablet);
        assert!(!pocket.time_chips);
        assert!(pocket.info.is_none() && pocket.watch.is_none());
        let safe = landscape.m.safe;
        assert_eq!(landscape.m.guide, safe);
        assert_eq!(landscape.m.dock[1], safe[1] + BAND_TOP);
        assert!(within(landscape.m.dock, safe));
        assert!(landscape.m.board.now.columns >= 2);
        assert_eq!(landscape.m.board.surface_height, safe[3]);
        assert!(landscape.region(safe).sheet_width() <= safe[2] * 0.45 + 0.01);

        let portrait = handheld(800., 1280., insets, PocketMode::Guide);
        assert!(portrait.pocket().unwrap().time_chips);
        // A portrait tablet keeps the list even when it is wide enough for Timeline.
        let wide_portrait = handheld(1000., 1400., insets, PocketMode::Guide);
        assert_eq!(wide_portrait.pocket().unwrap().class, PocketClass::Tablet);
        assert!(wide_portrait.time_chips());
    }

    #[test]
    fn regions_convert_points_and_native_editor_rects() {
        let layout = handheld(412., 915., PHONE_INSETS, PocketMode::Guide);
        let guide = layout.region(layout.m.guide);
        assert_eq!(guide.origin, [layout.m.guide[0], layout.m.guide[1]]);
        assert_eq!(guide.local([10., guide.origin[1] + 5.]), [10., 5.]);
        let scale = 2.625;
        let rect = guide.to_window([4, 6, 100, 40], scale);
        let top = (guide.origin[1] * layout.unit * scale).round() as i32;
        assert_eq!(rect, [4, 6 + top, 100, 40]);
    }

    #[test]
    fn a_row_list_with_no_room_keeps_its_offset() {
        let mut scroll = RowScroll::default();
        scroll.advance(3.);
        scroll.reveal(10, 0, Some(5));
        assert_eq!(scroll.first(), 3);
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
