//! Shield picture placement and the controls that fit around the native surface.

mod signal_mark;
mod tune;

use std::time::Instant;

use gpui::{Bounds, Div, Pixels, Size, div, point, prelude::*, rgb, rgba, size};

use super::{
    CANVAS_WIDTH, CELL_ELAPSED, DIM, FAINT, GUTTER, ICE, INK, Layout, ON_ICE, ON_ICE_DIM, PANEL,
    SAIRA, SAIRA_CLOCK, SAIRA_TITLE, TEXT,
};

const DOCK_TOP: f32 = 27.;
const DOCK_WIDTH: f32 = 640.;
const DOCK_HEIGHT: f32 = 360.;
const DOCK_CAPTION: f32 = 44.;
pub(crate) const DOCK_PREVIEW_X: f32 = GUTTER + DOCK_WIDTH + 44.;

const FULLSCREEN_PICTURE_HEIGHT: f32 = 876.;
const FULLSCREEN_BAND_HEIGHT: f32 = 204.;
const FULLSCREEN_GUTTER: f32 = 56.;
const FULLSCREEN_CONTROL_HEIGHT: f32 = 76.;

const AMBER: u32 = 0xffb45d;
const TALLY: u32 = 0xff5d5d;
const ERROR: u32 = 0xff8a75;
const ERROR_FILL: u32 = 0xff8a7524;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlaybackLayout {
    Docked,
    Fullscreen { controls_visible: bool },
}

/// An action also identifies its button in the D-pad focus path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlaybackControl {
    EnterFullscreen,
    ExitFullscreen,
    Pause,
    Resume,
    SetMuted(bool),
    Restart,
    CopyDiagnostics,
    AudioTrack,
    Quality,
    Stop,
}

impl PlaybackControl {
    pub(crate) fn same_slot(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Pause | Self::Resume, Self::Pause | Self::Resume)
                | (Self::SetMuted(_), Self::SetMuted(_))
        ) || self == other
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlaybackStage {
    Tuning,
    Playing,
    Paused,
    Reconnecting,
    Stopping,
    Failed,
}

impl PlaybackOverlay<'_> {
    fn show_field(&self) -> bool {
        self.tuning_field() || self.still.is_some() || self.stage == PlaybackStage::Reconnecting
    }
}

impl PlaybackStage {
    fn label(self) -> &'static str {
        match self {
            Self::Tuning => "Tuning",
            Self::Playing => "Live",
            Self::Paused => "Paused",
            Self::Reconnecting => "Reconnecting",
            Self::Stopping => "Stopping",
            Self::Failed => "Playback failed",
        }
    }
}

pub(crate) struct PlaybackOverlay<'a> {
    pub(crate) channel_number: u32,
    pub(crate) channel_name: &'a str,
    pub(crate) programme: Option<PlaybackProgramme<'a>>,
    pub(crate) clock: &'a str,
    pub(crate) stage: PlaybackStage,
    pub(crate) first_picture: Option<Instant>,
    pub(crate) signal_landing: Option<(Instant, PlaybackStage)>,
    pub(crate) reduce_motion: bool,
    pub(crate) hold_started: Option<Instant>,
    pub(crate) still: Option<&'a std::sync::Arc<gpui::RenderImage>>,
    pub(crate) attempt: u8,
    pub(crate) audio_track: Option<&'a str>,
    pub(crate) quality: Option<&'a str>,
    pub(crate) copy_label: &'static str,
    /// The runner's display message, filtered before reaching the view.
    pub(crate) feedback: Option<&'a str>,
    pub(crate) focus: Option<PlaybackControl>,
}

pub(crate) struct PlaybackProgramme<'a> {
    pub(crate) title: &'a str,
    pub(crate) starts: &'a str,
    pub(crate) ends: &'a str,
    pub(crate) minutes_left: i64,
    pub(crate) elapsed: f32,
    pub(crate) next: Option<PlaybackNext<'a>>,
}

pub(crate) struct PlaybackNext<'a> {
    pub(crate) starts: &'a str,
    pub(crate) title: &'a str,
}

impl PlaybackOverlay<'_> {
    fn tuning_field(&self) -> bool {
        match self.first_picture {
            Some(first) => {
                self.stage == PlaybackStage::Playing
                    && first.elapsed() < crate::picture_reveal_duration()
            }
            None => matches!(
                self.stage,
                PlaybackStage::Tuning | PlaybackStage::Stopping | PlaybackStage::Playing
            ),
        }
    }
}

/// Bounds in the logical coordinates used by GPUI. The Activity gets the
/// corresponding physical rectangle via `picture_rect`.
pub(crate) fn picture_bounds(viewport: Size<Pixels>, placement: PlaybackLayout) -> Bounds<Pixels> {
    let layout = Layout::new(viewport);
    let [left, top, width, height] = picture_reference(layout, placement);
    Bounds::new(
        point(layout.px(left), layout.px(top)),
        size(layout.px(width), layout.px(height)),
    )
}

/// Physical pixels in the Activity's `[left, top, width, height]` order.
pub(crate) fn picture_rect(
    viewport: Size<Pixels>,
    scale_factor: f32,
    placement: PlaybackLayout,
) -> [i32; 4] {
    let bounds = picture_bounds(viewport, placement);
    let physical = |value: Pixels| (f32::from(value) * scale_factor).round() as i32;
    let left = physical(bounds.origin.x);
    let top = physical(bounds.origin.y);
    [
        left,
        top,
        physical(bounds.origin.x + bounds.size.width) - left,
        physical(bounds.origin.y + bounds.size.height) - top,
    ]
}

fn picture_reference(layout: Layout, placement: PlaybackLayout) -> [f32; 4] {
    match placement {
        PlaybackLayout::Docked => [GUTTER, DOCK_TOP, DOCK_WIDTH, DOCK_HEIGHT],
        PlaybackLayout::Fullscreen {
            controls_visible: false,
        } => [0., 0., CANVAS_WIDTH, layout.height],
        PlaybackLayout::Fullscreen {
            controls_visible: true,
        } => {
            let height =
                (layout.height - FULLSCREEN_BAND_HEIGHT).clamp(0., FULLSCREEN_PICTURE_HEIGHT);
            let width = height * 16. / 9.;
            [(CANVAS_WIDTH - width) / 2., 0., width, height]
        }
    }
}

pub(crate) fn picker(
    viewport: Size<Pixels>,
    focused: usize,
    options: impl IntoIterator<Item = String>,
) -> Div {
    let layout = Layout::new(viewport);
    let first = focused.saturating_sub(5);
    div()
        .absolute()
        .left(layout.px(42.))
        .right(layout.px(42.))
        .bottom(layout.px(8.))
        .h(layout.px(72.))
        .px(layout.px(8.))
        .flex()
        .items_center()
        .gap(layout.px(8.))
        .bg(rgb(PANEL))
        .children(
            options
                .into_iter()
                .enumerate()
                .skip(first)
                .take(6)
                .map(|(index, name)| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .rounded(layout.px(8.))
                        .px(layout.px(12.))
                        .py(layout.px(12.))
                        .when(index == focused, |item| {
                            item.bg(rgb(ICE)).text_color(rgb(ON_ICE))
                        })
                        .child(name)
                }),
        )
}

pub(crate) fn fullscreen_actions(
    stage: PlaybackStage,
    muted: bool,
    show_audio_track: bool,
    can_change_quality: bool,
    can_restart: bool,
) -> Vec<PlaybackControl> {
    use PlaybackControl as Control;
    let mut controls = match stage {
        PlaybackStage::Playing | PlaybackStage::Paused => {
            let first = if stage == PlaybackStage::Paused {
                Control::Resume
            } else {
                Control::Pause
            };
            let mut controls = vec![first, Control::SetMuted(!muted), Control::Restart];
            if show_audio_track {
                controls.push(Control::AudioTrack);
            }
            if can_change_quality {
                controls.push(Control::Quality);
            }
            controls
        }
        PlaybackStage::Failed => {
            let mut controls = Vec::new();
            if can_restart {
                controls.push(Control::Restart);
            }
            controls.push(Control::CopyDiagnostics);
            controls
        }
        PlaybackStage::Tuning | PlaybackStage::Reconnecting | PlaybackStage::Stopping => Vec::new(),
    };
    controls.push(Control::ExitFullscreen);
    if stage != PlaybackStage::Stopping {
        controls.push(Control::Stop);
    }
    controls
}

pub(crate) fn docked_playback(viewport: Size<Pixels>, playback: &PlaybackOverlay<'_>) -> Div {
    let layout = Layout::new(viewport);
    let [left, top, width, height] = picture_reference(layout, PlaybackLayout::Docked);
    let focused = playback.focus == Some(PlaybackControl::EnterFullscreen);
    let primary = if focused { ON_ICE } else { TEXT };
    let secondary = if focused { ON_ICE_DIM } else { DIM };
    let bounds = [left, top, width, height];
    let caption = layout
        .rect(left, top + height, width, DOCK_CAPTION)
        .rounded_b(layout.px(8.))
        .pl(layout.px(10.))
        .pr(layout.px(12.))
        .flex()
        .items_center()
        .gap(layout.px(10.))
        .bg(rgb(PANEL))
        .when(focused, |caption| caption.bg(rgb(ICE)))
        .when(
            !playback.tuning_field() || playback.stage == PlaybackStage::Playing,
            |caption| caption.child(signal_mark::render(layout, playback, focused)),
        )
        .child(
            layout
                .text(SAIRA, 20., 20., 600.)
                .flex_none()
                .text_color(rgb(primary))
                .child(playback.channel_number.to_string()),
        )
        .child(
            layout
                .text(SAIRA, 20., 20., 600.)
                .max_w(layout.px(216.))
                .text_color(rgb(primary))
                .truncate()
                .child(playback.channel_name.to_owned()),
        )
        .when(playback.stage != PlaybackStage::Playing, |caption| {
            caption.child(status_tag(layout, playback.stage, false))
        })
        .child(
            layout
                .text(SAIRA, 20., 20., 400.)
                .flex_1()
                .min_w_0()
                .text_color(rgba(secondary))
                .truncate()
                .when_some(playback.programme.as_ref(), |title, programme| {
                    title.child(programme.title.to_owned())
                }),
        )
        .when_some(playback.programme.as_ref(), |caption, programme| {
            caption.child(
                layout
                    .text(SAIRA, 20., 20., 500.)
                    .mr(layout.px(8.))
                    .flex_none()
                    .text_color(rgba(secondary))
                    .child(format!("{} min left", programme.minutes_left)),
            )
        })
        .child(layout.icon(EXPAND, 20., rgba(secondary)));
    div()
        .absolute()
        .inset_0()
        .when_some(
            playback.first_picture.filter(|since| {
                playback.stage == PlaybackStage::Playing
                    && !playback.reduce_motion
                    && since.elapsed() < tune::LOCK
            }),
            |root, since| root.child(tune::lock(layout, [left, top, width, height], since)),
        )
        .when(playback.show_field(), |root| {
            root.child(tune::field(
                layout,
                bounds,
                tune::FieldPlacement::Docked,
                playback,
            ))
        })
        .child(
            layout
                .rect(left, top - 5., width, 5.)
                .bg(rgb(match playback.stage {
                    PlaybackStage::Reconnecting => AMBER,
                    PlaybackStage::Failed => ERROR,
                    PlaybackStage::Paused | PlaybackStage::Stopping => FAINT,
                    PlaybackStage::Playing if !playback.tuning_field() => TALLY,
                    _ => ICE,
                })),
        )
        .child(caption)
}

pub(crate) fn fullscreen_playback(
    viewport: Size<Pixels>,
    playback: &PlaybackOverlay<'_>,
    actions: &[PlaybackControl],
) -> Div {
    let layout = Layout::new(viewport);
    let root = div()
        .size_full()
        .relative()
        .overflow_hidden()
        .font_family(SAIRA)
        .text_color(rgb(TEXT))
        .bg(rgb(INK))
        .when(playback.show_field(), |root| {
            root.child(tune::field(
                layout,
                picture_reference(
                    layout,
                    PlaybackLayout::Fullscreen {
                        controls_visible: !actions.is_empty(),
                    },
                ),
                tune::FieldPlacement::Fullscreen,
                playback,
            ))
        });
    if actions.is_empty() {
        return root;
    }
    let [_, _, _, picture_height] = picture_reference(
        layout,
        PlaybackLayout::Fullscreen {
            controls_visible: true,
        },
    );
    root.child(
        layout
            .rect(
                0.,
                picture_height,
                CANVAS_WIDTH,
                layout.height - picture_height,
            )
            .bg(rgb(PANEL))
            .border_t(layout.px(2.))
            .border_color(rgba(FAINT))
            .child(fullscreen_head(layout, playback))
            .child(fullscreen_controls(
                layout,
                layout.height - picture_height,
                playback,
                actions,
            )),
    )
}

fn fullscreen_head(layout: Layout, playback: &PlaybackOverlay<'_>) -> Div {
    let identity = div()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .h(layout.px(26.))
                .min_w_0()
                .flex()
                .items_center()
                .gap(layout.px(12.))
                .when(
                    !playback.tuning_field() || playback.stage == PlaybackStage::Playing,
                    |head| head.child(signal_mark::render(layout, playback, false)),
                )
                .child(
                    layout
                        .text(SAIRA, 22., 26., 600.)
                        .min_w_0()
                        .truncate()
                        .child(playback.channel_name.to_owned()),
                )
                .child(status_tag(layout, playback.stage, true))
                .when_some(playback.quality, |channel, quality| {
                    channel.child(quality_tag(layout, quality))
                }),
        )
        .child(
            layout.text(SAIRA_TITLE, 36., 42., 700.).truncate().child(
                playback
                    .programme
                    .as_ref()
                    .map_or(playback.channel_name, |item| item.title)
                    .to_owned(),
            ),
        );
    layout
        .rect(
            FULLSCREEN_GUTTER,
            16.,
            CANVAS_WIDTH - 2. * FULLSCREEN_GUTTER,
            68.,
        )
        .flex()
        .items_center()
        .gap(layout.px(24.))
        .child(
            layout
                .text(SAIRA_CLOCK, 34., 34., 700.)
                .size(layout.px(68.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(layout.px(10.))
                .bg(rgb(TALLY))
                .child(playback.channel_number.to_string()),
        )
        .child(identity)
        .when_some(playback.programme.as_ref(), |head, programme| {
            head.child(progress(layout, programme, playback.stage))
        })
        .child(
            layout
                .text(SAIRA_CLOCK, 40., 40., 600.)
                .pl(layout.px(8.))
                .flex_none()
                .child(playback.clock.to_owned()),
        )
}

fn progress(layout: Layout, programme: &PlaybackProgramme<'_>, stage: PlaybackStage) -> Div {
    let live_picture = matches!(stage, PlaybackStage::Playing | PlaybackStage::Paused);
    let thickness = if live_picture { 8. } else { 4. };
    let position = programme.elapsed.clamp(0., 1.);
    let bar = div()
        .relative()
        .flex_1()
        .min_w_0()
        .h(layout.px(thickness))
        .rounded_full()
        .bg(if live_picture {
            rgba(0xeef3f838)
        } else {
            rgba(FAINT)
        })
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w(gpui::relative(position))
                .h_full()
                .rounded_full()
                .bg(if live_picture {
                    rgba(0x9fe0ffff)
                } else {
                    rgba(DIM)
                }),
        )
        .when(live_picture, |bar| {
            bar.child(
                div()
                    .absolute()
                    .left(gpui::relative(position))
                    .mt(layout.px(-6.))
                    .ml(layout.px(-10.))
                    .size(layout.px(20.))
                    .rounded_full()
                    .bg(rgb(TEXT)),
            )
        });

    div()
        .w(layout.px(552.))
        .flex_none()
        .child(
            div()
                .h(layout.px(26.))
                .flex()
                .items_center()
                .gap(layout.px(10.))
                .child(
                    layout
                        .text(SAIRA, 22., 26., 600.)
                        .flex_none()
                        .child(format!("{} min left", programme.minutes_left)),
                )
                .when_some(programme.next.as_ref(), |line, next| {
                    line.child(
                        layout
                            .text(SAIRA, 22., 26., 400.)
                            .min_w_0()
                            .text_color(rgba(DIM))
                            .truncate()
                            .child(format!("· Next {} {}", next.starts, next.title)),
                    )
                }),
        )
        .child(
            div()
                .h(layout.px(42.))
                .flex()
                .items_center()
                .gap(layout.px(16.))
                .text_color(rgba(DIM))
                .child(
                    layout
                        .text(SAIRA, 22., 22., 400.)
                        .flex_none()
                        .child(programme.starts.to_owned()),
                )
                .child(bar)
                .child(
                    layout
                        .text(SAIRA, 22., 22., 400.)
                        .flex_none()
                        .child(programme.ends.to_owned()),
                ),
        )
}

fn fullscreen_controls(
    layout: Layout,
    height: f32,
    playback: &PlaybackOverlay<'_>,
    controls: &[PlaybackControl],
) -> Div {
    let leading = controls
        .iter()
        .position(|action| *action == PlaybackControl::ExitFullscreen)
        .unwrap_or(controls.len());
    let (left, right) = controls.split_at(leading);
    let feedback = playback.feedback.or(match playback.stage {
        PlaybackStage::Playing | PlaybackStage::Paused | PlaybackStage::Tuning => None,
        stage => Some(stage.label()),
    });
    layout
        .rect(
            FULLSCREEN_GUTTER,
            (height - FULLSCREEN_CONTROL_HEIGHT - 20.).max(86.),
            CANVAS_WIDTH - 2. * FULLSCREEN_GUTTER,
            FULLSCREEN_CONTROL_HEIGHT,
        )
        .flex()
        .items_center()
        .gap(layout.px(12.))
        .children(left.iter().map(|action| control(layout, *action, playback)))
        .child(
            layout
                .text(SAIRA, 21., 26., 400.)
                .flex_1()
                .min_w_0()
                .px(layout.px(12.))
                .text_color(rgba(DIM))
                .truncate()
                .when_some(feedback, |line, feedback| line.child(feedback.to_owned())),
        )
        .children(
            right
                .iter()
                .map(|action| control(layout, *action, playback)),
        )
}

fn control(layout: Layout, action: PlaybackControl, playback: &PlaybackOverlay<'_>) -> Div {
    use PlaybackControl as Control;
    let (label, icon, detail) = match action {
        Control::EnterFullscreen => ("Full screen", EXPAND, None),
        Control::ExitFullscreen if playback.stage == PlaybackStage::Failed => {
            ("Back to Guide", SHRINK, None)
        }
        Control::ExitFullscreen => ("Exit fullscreen", SHRINK, None),
        Control::Pause => ("Pause", PAUSE, None),
        Control::Resume => ("Resume", PLAY, None),
        Control::SetMuted(true) => ("Mute", VOLUME, None),
        Control::SetMuted(false) => ("Unmute", MUTED, None),
        Control::Restart => ("Restart", RESTART, None),
        Control::CopyDiagnostics => (playback.copy_label, CLIPBOARD, None),
        Control::AudioTrack => ("Audio", AUDIO, playback.audio_track),
        Control::Quality => ("Quality", QUALITY, playback.quality),
        Control::Stop => ("Stop", STOP, None),
    };
    let focused = playback.focus.is_some_and(|focus| focus.same_slot(action));
    let ink = if focused { ON_ICE } else { TEXT };
    layout
        .text(SAIRA, 27., 28., 650.)
        .h(layout.px(FULLSCREEN_CONTROL_HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .gap(layout.px(10.))
        .pl(layout.px(20.))
        .pr(layout.px(24.))
        .rounded_full()
        .bg(rgb(if focused { ICE } else { CELL_ELAPSED }))
        .text_color(rgb(ink))
        .child(layout.icon(icon, 27.36, rgb(ink)))
        .child(label)
        .when_some(detail, |button, value| {
            button
                .child(
                    layout
                        .text(SAIRA, 24., 28., 600.)
                        .max_w(layout.px(152.))
                        .truncate()
                        .child(value.to_owned()),
                )
                .child(layout.icon(CHEVRON, 18., rgb(ink)))
        })
}

fn status_tag(layout: Layout, stage: PlaybackStage, fullscreen: bool) -> Div {
    if fullscreen && stage == PlaybackStage::Tuning {
        return div();
    }

    let (fill, ink) = match stage {
        PlaybackStage::Playing => (0xff5d5dff, TEXT),
        PlaybackStage::Reconnecting => (0x6b4817ff, 0xffe2bd),
        PlaybackStage::Failed => (ERROR_FILL, ERROR),
        _ => (0x243858ff, TEXT),
    };
    let (height, font_size) = if fullscreen { (26., 16.) } else { (21., 15.) };
    layout
        .text(SAIRA, font_size, font_size, 700.)
        .h(layout.px(height))
        .px(layout.px(8.))
        .flex_none()
        .flex()
        .items_center()
        .rounded(layout.px(4.))
        .bg(rgba(fill))
        .text_color(rgb(ink))
        .child(stage.label().to_ascii_uppercase())
}

fn quality_tag(layout: Layout, value: &str) -> Div {
    layout
        .text(SAIRA, 16., 16., 700.)
        .h(layout.px(26.))
        .px(layout.px(8.))
        .flex_none()
        .flex()
        .items_center()
        .rounded(layout.px(5.))
        .bg(rgba(0xeef3f824))
        .border(layout.px(1.5))
        .border_color(rgba(0xeef3f899))
        .child(value.to_owned())
}

macro_rules! icon {
    ($path:literal) => {
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#,
            $path,
            "</svg>"
        )
        .as_bytes()
    };
}

const EXPAND: &[u8] = icon!(r#"<path d="M15 3h6v6m0-6-7 7m-5 11H3v-6m0 6 7-7"/>"#);
const SHRINK: &[u8] = icon!(r#"<path d="M21 3l-7 7m6 0h-6V4M3 21l7-7m-6 0h6v6"/>"#);
const PAUSE: &[u8] = icon!(
    r#"<rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/>"#
);
const PLAY: &[u8] = icon!(r#"<path d="m8 5 12 7-12 7z"/>"#);
const VOLUME: &[u8] =
    icon!(r#"<path d="M11 5 6 9H3v6h3l5 4V5m4 4a4 4 0 0 1 0 6m2.8-8.8a8 8 0 0 1 0 11.6"/>"#);
const MUTED: &[u8] = icon!(r#"<path d="M11 5 6 9H3v6h3l5 4V5m5 4 6 6m0-6-6 6"/>"#);
const RESTART: &[u8] = icon!(r#"<path d="M3 12a9 9 0 1 1 2.64 6.36M3 12V6m0 6h6"/>"#);
const AUDIO: &[u8] =
    icon!(r#"<path d="M3 14v-3a9 9 0 0 1 18 0v3m-18 0v4h5v-6H3zm18 0v4h-5v-6h5z"/>"#);
const QUALITY: &[u8] = icon!(r#"<path d="M5 19v-4m7 4v-8m7 8V5"/>"#);
const CLIPBOARD: &[u8] = icon!(
    r#"<rect x="8" y="2" width="8" height="4" rx="1"/><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/>"#
);
const STOP: &[u8] = icon!(r#"<rect x="6" y="6" width="12" height="12" rx="2"/>"#);
const CHEVRON: &[u8] = include_bytes!("../../assets/icons/chevron-down.svg");
