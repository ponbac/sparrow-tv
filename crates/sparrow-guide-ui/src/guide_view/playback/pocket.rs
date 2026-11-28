//! Pocket playback chrome. Everything here sits outside the picture rectangle,
//! which the native surface covers (ADR 0007).

use sparrow_core::ChannelDirection;

use super::*;
use crate::guide_view::{
    MUTE, MinuteFrame, POCKET_GUTTER, PocketClass, PocketMode, WatchGeometry, board::quality_chip,
};

const BUTTON: f32 = 68.;
const BUTTON_GAP: f32 = 10.;
const UP_NEXT_ROW: f32 = 58.;

pub(super) const PICTURE_IN_PICTURE: &[u8] = icon!(
    r#"<rect x="2" y="4" width="20" height="16" rx="2"/><rect x="12" y="11" width="8" height="6" rx="1"/>"#
);
pub(super) const MORE: &[u8] = icon!(
    r#"<circle cx="5" cy="12" r="1.2"/><circle cx="12" cy="12" r="1.2"/><circle cx="19" cy="12" r="1.2"/>"#
);
const SEARCH: &[u8] = include_bytes!("../../../assets/icons/search.svg");
const GUIDE: &[u8] = include_bytes!("../../../assets/icons/now.svg");

/// Pocket details that only the handheld chrome shows.
#[derive(Default)]
pub(crate) struct PocketDetails<'a> {
    pub(crate) group: &'a str,
    pub(crate) description: Option<&'a str>,
    pub(crate) upcoming: Vec<Upcoming<'a>>,
    /// Quality Variants as family index, quality and whether it plays.
    pub(crate) variants: Vec<(usize, sparrow_core::ChannelQuality, bool)>,
    /// The control bar, left to right.
    pub(crate) actions: Vec<PlaybackControl>,
}

pub(crate) struct Upcoming<'a> {
    pub(crate) starts: String,
    pub(crate) title: &'a str,
    pub(crate) minutes: i64,
}

pub(crate) fn chrome(
    layout: Layout,
    playback: &PlaybackOverlay<'_>,
    details: &PocketDetails<'_>,
    frame: FullscreenFrame,
    minute: MinuteFrame<'_>,
    pointer: Option<&PlayerPointer>,
) -> Option<Div> {
    let pocket = layout.pocket().filter(|pocket| pocket.compact())?;
    let [x, y, w, _] = layout.m.dock;
    let root = div().absolute().inset_0().child(
        layout
            .rect(x, y - 5., w, 5.)
            .bg(rgb(stripe_color(playback.stage)))
            .opacity(frame.stripe),
    );
    let chrome = div().absolute().inset_0().opacity(frame.caption);
    let chrome = match (pocket.mode, pocket.watch, pocket.info) {
        (PocketMode::Watch, Some(watch), _) => {
            watch_screen(layout, playback, details, watch, minute, pointer, chrome)
        }
        (_, _, Some(info)) if pocket.class == PocketClass::Portrait => {
            chrome.child(band_info(layout, playback, info, pointer))
        }
        (_, _, Some([ix, iy, iw, ih])) => {
            let lower = 170_f32.min(ih);
            chrome
                .child(lower_third(
                    layout,
                    playback,
                    details,
                    [ix, iy, iw, lower],
                    0.,
                    pointer,
                ))
                .child(control_bar(
                    layout,
                    playback,
                    &details.actions,
                    [ix, iy + lower, iw, (ih - lower).min(96.)],
                    0.,
                    pointer,
                ))
        }
        _ => chrome,
    };
    Some(root.child(chrome))
}

fn watch_screen(
    layout: Layout,
    playback: &PlaybackOverlay<'_>,
    details: &PocketDetails<'_>,
    watch: WatchGeometry,
    minute: MinuteFrame<'_>,
    pointer: Option<&PlayerPointer>,
    chrome: Div,
) -> Div {
    chrome
        .child(masthead(layout, watch.masthead, minute, pointer))
        .child(lower_third(
            layout,
            playback,
            details,
            watch.lower_third,
            POCKET_GUTTER,
            pointer,
        ))
        .child(control_bar(
            layout,
            playback,
            &details.actions,
            watch.controls,
            POCKET_GUTTER,
            pointer,
        ))
        .child(details_block(layout, playback, details, watch.details))
        .child(channel_bar(layout, playback, watch.channel_bar, pointer))
}

fn masthead(
    layout: Layout,
    [x, y, w, h]: [f32; 4],
    minute: MinuteFrame<'_>,
    pointer: Option<&PlayerPointer>,
) -> Div {
    let mark = signal_mark::steady(
        layout,
        div()
            .relative()
            .flex_none()
            .w(layout.px(42.))
            .h(layout.px(24.)),
        ICE,
    );
    layout
        .rect(x, y, w, h)
        .px(layout.px(POCKET_GUTTER))
        .flex()
        .items_center()
        .gap(layout.px(12.))
        .child(mark)
        .child(
            layout
                .text(SAIRA_TITLE, 32., 36., 700.)
                .flex_1()
                .text_color(rgb(TEXT))
                .child("Sparrow"),
        )
        .child(
            clickable(round_button(layout, 64.), pointer, PlayerClick::Search).child(layout.icon(
                SEARCH,
                26.,
                rgb(TEXT),
            )),
        )
        .child(minute.clock(layout, 28.).ml(layout.px(8.)))
}

fn round_button(layout: Layout, size: f32) -> Div {
    div()
        .size(layout.px(size))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(CELL_ELAPSED))
}

/// The playing Channel and Programme under the picture.
fn lower_third(
    layout: Layout,
    playback: &PlaybackOverlay<'_>,
    details: &PocketDetails<'_>,
    [x, y, w, h]: [f32; 4],
    padding: f32,
    pointer: Option<&PlayerPointer>,
) -> Div {
    let state = match playback.stage {
        PlaybackStage::Playing => None,
        stage => Some(playback.feedback.unwrap_or(stage.label())),
    };
    let meta = state.unwrap_or(details.group);
    let head = div()
        .h(layout.px(68.))
        .flex()
        .items_center()
        .gap(layout.px(16.))
        .child(
            layout
                .text(SAIRA_CLOCK, 30., 30., 700.)
                .size(layout.px(60.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(layout.px(8.))
                .bg(rgb(TALLY))
                .child(zap::number(layout, playback, 60., 30., 18.)),
        )
        .child(zap::identity(layout, playback))
        .when(
            !playback.show_field() || playback.stage == PlaybackStage::Playing,
            |head| head.child(signal_mark::render(layout, playback, false)),
        )
        .when(details.variants.len() > 1, |head| {
            head.child(div().flex_none().flex().gap(layout.px(4.)).children(
                details.variants.iter().map(|&(index, quality, selected)| {
                    clickable(
                        quality_chip(layout, quality, selected, false, false, false),
                        pointer,
                        PlayerClick::Variant(index),
                    )
                }),
            ))
        });
    let next = playback
        .programme
        .as_ref()
        .and_then(|programme| programme.next.as_ref());
    let meta = layout
        .text(SAIRA, 21., 28., 400.)
        .h(layout.px(30.))
        .flex()
        .items_center()
        .gap(layout.px(10.))
        .text_color(rgba(DIM))
        .when(!meta.is_empty(), |line| {
            line.child(
                div()
                    .flex_none()
                    .max_w(layout.px(w * 0.5))
                    .truncate()
                    .when(state.is_some(), |word| {
                        word.font_weight(gpui::FontWeight(600.)).text_color(rgb(
                            if playback.stage == PlaybackStage::Failed {
                                ERROR
                            } else {
                                TEXT
                            },
                        ))
                    })
                    .child(meta.to_owned()),
            )
        })
        .when_some(next, |line, next| {
            line.child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(format!("Next {} {}", next.starts, next.title)),
            )
        });
    layout
        .rect(x, y, w, h)
        .px(layout.px(padding))
        .pt(layout.px(14.))
        .flex()
        .flex_col()
        .gap(layout.px(8.))
        .overflow_hidden()
        .child(head)
        .child(meta)
        .when_some(playback.programme.as_ref(), |block, programme| {
            block.child(meter(layout, programme))
        })
}

fn meter(layout: Layout, programme: &PlaybackProgramme<'_>) -> Div {
    layout
        .text(SAIRA, 20., 24., 400.)
        .h(layout.px(36.))
        .flex()
        .items_center()
        .gap(layout.px(12.))
        .text_color(rgba(DIM))
        .child(div().flex_none().child(programme.starts.to_owned()))
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h(layout.px(6.))
                .rounded_full()
                .bg(rgba(FAINT))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h_full()
                        .w(gpui::relative(programme.elapsed.clamp(0., 1.)))
                        .rounded_full()
                        .bg(rgb(ICE)),
                ),
        )
        .child(div().flex_none().child(programme.ends.to_owned()))
        .child(
            div()
                .flex_none()
                .font_weight(gpui::FontWeight(600.))
                .text_color(rgb(TEXT))
                .child(format!("{} min left", programme.minutes_left)),
        )
}

fn control_icon(action: PlaybackControl) -> &'static [u8] {
    use PlaybackControl as Control;
    match action {
        Control::EnterFullscreen => EXPAND,
        Control::ExitFullscreen => SHRINK,
        Control::Pause => PAUSE,
        Control::Resume => PLAY,
        Control::SetMuted(true) | Control::Volume => VOLUME,
        Control::SetMuted(false) => MUTED,
        Control::Restart => RESTART,
        Control::CopyDiagnostics => CLIPBOARD,
        Control::AudioTrack => AUDIO,
        Control::Quality => QUALITY,
        Control::PictureInPicture => PICTURE_IN_PICTURE,
        Control::More => MORE,
        Control::Stop => STOP,
    }
}

/// An icon-only round control, as the Pocket bars draw them.
pub(super) fn icon_control(
    layout: Layout,
    action: PlaybackControl,
    size: f32,
    focused: bool,
    pointer: Option<&PlayerPointer>,
) -> Div {
    let ink = rgb(if focused { ON_ICE } else { TEXT });
    clickable(
        round_button(layout, size),
        pointer,
        PlayerClick::Control(action),
    )
    .when(focused, |button| button.bg(rgb(ICE)))
    .child(layout.icon(control_icon(action), size * 0.42, ink))
}

/// Pause, sound and Audio on the left, then the feedback line, then the
/// picture and session controls on the right.
fn control_bar(
    layout: Layout,
    playback: &PlaybackOverlay<'_>,
    actions: &[PlaybackControl],
    [x, y, w, h]: [f32; 4],
    padding: f32,
    pointer: Option<&PlayerPointer>,
) -> Div {
    use PlaybackControl as Control;
    let split = actions
        .iter()
        .position(|action| {
            matches!(
                action,
                Control::PictureInPicture
                    | Control::EnterFullscreen
                    | Control::More
                    | Control::Stop
            )
        })
        .unwrap_or(actions.len());
    let (left, right) = actions.split_at(split);
    let button = |action: &Control| {
        icon_control(
            layout,
            *action,
            BUTTON,
            playback.focus.is_some_and(|focus| focus.same_slot(*action)),
            pointer,
        )
    };
    let feedback = playback.feedback.or(playback
        .audio_track
        .filter(|_| left.contains(&Control::AudioTrack)));
    layout
        .rect(x, y, w, h)
        .px(layout.px(padding))
        .flex()
        .items_center()
        .gap(layout.px(BUTTON_GAP))
        .children(left.iter().map(button))
        .child(
            layout
                .text(SAIRA, 19., 24., 400.)
                .flex_1()
                .min_w_0()
                .px(layout.px(4.))
                .text_color(rgba(DIM))
                .truncate()
                .when_some(feedback, |line, text| line.child(text.to_owned())),
        )
        .children(right.iter().map(button))
}

/// The Programme description and what follows it. It scrolls on its own.
fn details_block(
    layout: Layout,
    playback: &PlaybackOverlay<'_>,
    details: &PocketDetails<'_>,
    [x, y, w, h]: [f32; 4],
) -> impl IntoElement {
    let upcoming = details.upcoming.iter().map(|next| {
        layout
            .text(SAIRA, 22., 28., 400.)
            .h(layout.px(UP_NEXT_ROW))
            .flex_none()
            .border_t(layout.px(1.))
            .border_color(rgba(FAINT))
            .flex()
            .items_center()
            .gap(layout.px(14.))
            .child(
                div()
                    .w(layout.px(80.))
                    .flex_none()
                    .font_weight(gpui::FontWeight(600.))
                    .text_color(rgb(ICE))
                    .child(next.starts.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui::FontWeight(600.))
                    .child(next.title.to_owned()),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(rgba(DIM))
                    .child(format!("{} min", next.minutes)),
            )
    });
    let block = layout
        .rect(x, y, w, h)
        .px(layout.px(POCKET_GUTTER))
        .pt(layout.px(12.))
        .flex()
        .flex_col()
        .gap(layout.px(10.))
        .when_some(details.description, |block, text| {
            block.child(
                layout
                    .text(SAIRA, 21., 32., 400.)
                    .flex_none()
                    .text_color(rgba(DIM))
                    .whitespace_normal()
                    .child(text.to_owned()),
            )
        })
        .when(
            details.description.is_none() && playback.programme.is_none(),
            |block| {
                block.child(
                    layout
                        .text(SAIRA, 21., 32., 400.)
                        .flex_none()
                        .text_color(rgba(DIM))
                        .child("No guide data. The channel still plays."),
                )
            },
        )
        .when(!details.upcoming.is_empty(), |block| {
            block
                .child(
                    layout
                        .text(SAIRA, 18., 24., 700.)
                        .flex_none()
                        .mt(layout.px(8.))
                        .text_color(rgba(MUTE))
                        .child("UP NEXT"),
                )
                .child(div().flex_none().flex().flex_col().children(upcoming))
        });
    block.id("pocket-details").overflow_y_scroll()
}

/// Previous Channel, back to the Guide, next Channel. The background runs
/// into the gesture inset; the buttons stay above it.
fn channel_bar(
    layout: Layout,
    playback: &PlaybackOverlay<'_>,
    [x, y, w, h]: [f32; 4],
    pointer: Option<&PlayerPointer>,
) -> Div {
    let reel = playback.reel.cloned().unwrap_or_default();
    let neighbour = |entry: Option<(u32, String)>, direction: ChannelDirection| {
        let previous = direction == ChannelDirection::Previous;
        let label = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .when(!previous, |label| label.items_end())
            .child(
                layout
                    .text(SAIRA_CLOCK, 20., 24., 600.)
                    .text_color(rgba(DIM))
                    .child(entry.as_ref().map_or_else(String::new, |e| e.0.to_string())),
            )
            .child(
                layout
                    .text(SAIRA, 22., 28., 600.)
                    .max_w_full()
                    .truncate()
                    .child(entry.map_or_else(
                        || {
                            if previous {
                                "Previous".to_owned()
                            } else {
                                "Next".to_owned()
                            }
                        },
                        |e| e.1,
                    )),
            );
        let arrow = layout
            .text(SAIRA, 22., 28., 700.)
            .flex_none()
            .text_color(rgb(ICE))
            .child(if previous { "◀" } else { "▶" });
        let button = div()
            .flex_1()
            .min_w_0()
            .h(layout.px(84.))
            .px(layout.px(16.))
            .rounded(layout.px(14.))
            .bg(rgb(CELL_ELAPSED))
            .flex()
            .items_center()
            .gap(layout.px(12.));
        let button = if previous {
            button.child(arrow).child(label)
        } else {
            button.child(label).child(arrow)
        };
        clickable(button, pointer, PlayerClick::Zap(direction))
    };
    let guide = clickable(
        layout
            .text(SAIRA, 22., 28., 650.)
            .h(layout.px(84.))
            .px(layout.px(26.))
            .flex_none()
            .rounded_full()
            .bg(rgb(ICE))
            .text_color(rgb(ON_ICE))
            .flex()
            .items_center()
            .gap(layout.px(10.))
            .child(layout.icon(GUIDE, 24., rgb(ON_ICE)))
            .child("Guide"),
        pointer,
        PlayerClick::Guide,
    );
    let [previous, next] = reel;
    div()
        .absolute()
        .inset_0()
        .child(
            layout
                .rect(0., y, layout.width, (layout.height - y).max(h))
                .bg(rgb(PANEL))
                .border_t(layout.px(1.))
                .border_color(rgba(FAINT)),
        )
        .child(
            layout
                .rect(x, y, w, h)
                .px(layout.px(POCKET_GUTTER))
                .flex()
                .items_center()
                .gap(layout.px(12.))
                .text_color(rgb(TEXT))
                .child(neighbour(previous, ChannelDirection::Previous))
                .child(guide)
                .child(neighbour(next, ChannelDirection::Next)),
        )
}

/// The playing Channel beside the docked picture. A tap returns to Watch.
fn band_info(
    layout: Layout,
    playback: &PlaybackOverlay<'_>,
    [x, y, w, h]: [f32; 4],
    pointer: Option<&PlayerPointer>,
) -> Div {
    let title = playback
        .programme
        .as_ref()
        .map_or(playback.channel_name, |programme| programme.title);
    let identity = layout
        .text(SAIRA, 22., 28., 600.)
        .h(layout.px(34.))
        .flex()
        .items_center()
        .gap(layout.px(10.))
        .text_color(rgb(TEXT))
        .when(
            !playback.show_field() || playback.stage == PlaybackStage::Playing,
            |line| line.child(signal_mark::render(layout, playback, false)),
        )
        .child(
            div()
                .flex_none()
                .text_color(rgba(DIM))
                .child(zap::number(layout, playback, 28., 22., 12.)),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(playback.channel_name.to_owned()),
        )
        .when(playback.stage != PlaybackStage::Playing, |line| {
            line.child(status_tag(layout, playback.stage, false))
        });
    let progress = playback.programme.as_ref().map(|programme| {
        layout
            .text(SAIRA, 19., 24., 500.)
            .h(layout.px(30.))
            .flex()
            .items_center()
            .gap(layout.px(10.))
            .text_color(rgba(DIM))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h(layout.px(5.))
                    .rounded_full()
                    .bg(rgba(FAINT))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .h_full()
                            .w(gpui::relative(programme.elapsed.clamp(0., 1.)))
                            .rounded_full()
                            .bg(rgb(ICE)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .child(format!("{} min left", programme.minutes_left)),
            )
    });
    clickable(
        layout
            .rect(x, y, w, h)
            .flex()
            .flex_col()
            .justify_center()
            .gap(layout.px(6.))
            .overflow_hidden(),
        pointer,
        PlayerClick::Watch,
    )
    .child(identity)
    .child(
        layout
            .text(SAIRA_TITLE, 28., 36., 650.)
            .text_color(rgb(TEXT))
            .truncate()
            .child(title.to_owned()),
    )
    .children(progress)
}
