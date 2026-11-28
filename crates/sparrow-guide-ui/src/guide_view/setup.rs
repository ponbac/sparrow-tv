//! First-launch Setup, one question per screen, in profile layout units.

use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use gpui::{
    Animation, AnimationExt, Div, ElementId, FontWeight, IntoElement, TextRun, Window, div,
    ease_out_quint, font, linear_color_stop, linear_gradient, prelude::*, rgb, rgba,
};
use sparrow_tv_guide::setup::{
    DraftView, FailureReason, FoundSummary, Setup, SetupControl, SetupMessage, SetupStep,
};

use super::{
    CELL, DIM, FAINT, GUTTER, ICE, ICE_ELAPSED, INK, Layout, MUTE, ON_ICE, PLATE, SAIRA,
    SAIRA_CLOCK, SAIRA_NUMBER, SAIRA_PREVIEW, SAIRA_TITLE, TEXT,
    band::{BAND_TOP, TOOLBAR_HEIGHT},
    clock_time,
};

const CONTENT_MAX_WIDTH: f32 = 1600.;
const MIN_LEFT: f32 = 80.;
const PROGRESS_Y: f32 = 96.;
const HEADLINE_Y: f32 = 150.;
const FIELD_Y: f32 = 298.;
const FIELD_HEIGHT: f32 = 96.;
const FIELD_BORDER: f32 = 3.;
const FIELD_PADDING: f32 = 32.;
const CLEAR_WIDTH: f32 = 170.;
const CLEAR_GAP: f32 = 20.;
const COPY_WIDTH: f32 = 1000.;
const BUTTON_HEIGHT: f32 = 62.;
const SWEEP_Y: f32 = 470.;
const SWEEP_WIDTH: f32 = 640.;
const ERROR: u32 = 0xff8a75;
/// Four names, their counts and "and K more" fit the content width.
const GROUP_NAME_WIDTH: f32 = 280.;

pub(in super::super) struct SetupPanel<'a> {
    pub(in super::super) setup: &'a Setup,
    pub(in super::super) draft: DraftView,
    /// An Android editor sits on the field and paints its own text.
    pub(in super::super) editing: bool,
    /// A save is waiting for the catalog to accept the draft.
    pub(in super::super) saving: bool,
    pub(in super::super) now: DateTime<FixedOffset>,
}

/// The centred column: 160 units in from each edge on television.
fn left(layout: Layout) -> f32 {
    ((layout.width - CONTENT_MAX_WIDTH) / 2.).max(MIN_LEFT)
}

fn content_width(layout: Layout) -> f32 {
    layout.width - 2. * left(layout)
}

fn field_width(layout: Layout, filled: bool) -> f32 {
    if filled {
        content_width(layout) - CLEAR_WIDTH - CLEAR_GAP
    } else {
        content_width(layout)
    }
}

/// The control under a point in layout units, mirroring `render`.
pub(in super::super) fn setup_hit(
    layout: Layout,
    window: &Window,
    panel: &SetupPanel<'_>,
    [x, y]: [f32; 2],
) -> Option<SetupControl> {
    let [field_row, actions] = panel.setup.rows(panel.draft);
    let x = x - left(layout);
    let filled = field_row.contains(&SetupControl::Clear);
    let within = |top: f32, height: f32| (top..top + height).contains(&y);
    if !field_row.is_empty() && within(FIELD_Y, FIELD_HEIGHT) {
        let width = field_width(layout, filled);
        if (0. ..width).contains(&x) {
            return Some(SetupControl::Field);
        }
        let clear = width + CLEAR_GAP;
        return (filled && (clear..clear + CLEAR_WIDTH).contains(&x))
            .then_some(SetupControl::Clear);
    }
    if !within(buttons_top(panel)?, BUTTON_HEIGHT) {
        return None;
    }
    let mut left = if panel.setup.step() == SetupStep::Found {
        10.
    } else {
        0.
    };
    for &control in actions {
        let label: gpui::SharedString = action_label(control, panel.setup.guide_ready()).into();
        let mut run_font = font(SAIRA);
        run_font.weight = FontWeight(600.);
        let text = window.text_system().shape_line(
            label.clone(),
            layout.px(24.),
            &[TextRun {
                font: run_font,
                len: label.len(),
                ..TextRun::default()
            }],
            None,
        );
        let width = f32::from(text.width()) / layout.unit + 60.;
        if (left..left + width).contains(&x) {
            return Some(control);
        }
        left += width + 18.;
    }
    None
}

fn buttons_top(panel: &SetupPanel<'_>) -> Option<f32> {
    match panel.setup.step() {
        SetupStep::Channels | SetupStep::Guide => Some(
            if panel.setup.message().is_some() {
                470.
            } else {
                430.
            } + 130.,
        ),
        SetupStep::Failed(_) => Some(420.),
        SetupStep::Found => panel.setup.summary().map(|_| 700.),
        SetupStep::Loading => None,
    }
}

/// Physical x, y, width and height of the text inside the Setup field.
pub(in super::super) fn setup_editor_rect(
    layout: Layout,
    pixel_scale: f32,
    filled: bool,
) -> Option<[i32; 4]> {
    let height = FIELD_HEIGHT.min(layout.height - FIELD_Y);
    if height <= 0. {
        return None;
    }
    let scale = layout.unit * pixel_scale;
    let inset = FIELD_BORDER + FIELD_PADDING;
    Some([
        ((left(layout) + inset) * scale).round() as i32,
        (FIELD_Y * scale).round() as i32,
        ((field_width(layout, filled) - 2. * inset) * scale).round() as i32,
        (height * scale).round() as i32,
    ])
}

pub(in super::super) fn render(layout: Layout, panel: SetupPanel<'_>) -> Div {
    let screen = div()
        .size_full()
        .relative()
        .overflow_hidden()
        .bg(rgb(INK))
        .text_color(rgb(TEXT))
        .font_family(SAIRA)
        .child(clock(layout, panel.now));
    match panel.setup.step() {
        SetupStep::Channels => screen.children(question(
            layout,
            &panel,
            Question {
                step: 0,
                headline: "Where do your channels come from?",
                placeholder: "Enter the M3U address",
                copy: "Your TV provider gives you this as an M3U link. Sparrow loads \
                       your channels from it and keeps the address on this device.",
                filled: panel.draft.has_m3u,
            },
        )),
        SetupStep::Guide => screen.children(question(
            layout,
            &panel,
            Question {
                step: 1,
                headline: "Add programme times?",
                placeholder: "Enter the XMLTV address",
                copy: "A guide source shows what is on now and next. \
                       You can add it later in Sources.",
                filled: panel.draft.has_epg,
            },
        )),
        SetupStep::Loading => screen.children(loading(layout)),
        SetupStep::Failed(reason) => screen
            .child(headline(layout, HEADLINE_Y, "Couldn’t load your channels"))
            .child(copy(layout, 300., failure_copy(reason)))
            .child(buttons(layout, 420., &panel))
            .children(
                panel
                    .setup
                    .message()
                    .map(|message| message_line(layout, 510., message)),
            ),
        SetupStep::Found => match panel.setup.summary() {
            None => screen.children(loading(layout)),
            Some(summary) => screen.children(found(layout, summary, &panel)),
        },
    }
}

fn clock(layout: Layout, now: DateTime<FixedOffset>) -> Div {
    layout
        .rect(layout.width - GUTTER - 200., BAND_TOP, 200., TOOLBAR_HEIGHT)
        .flex()
        .items_center()
        .justify_end()
        .child(
            layout
                .text(SAIRA_CLOCK, 30., 30., 600.)
                .child(clock_time(now.to_utc())),
        )
}

struct Question {
    step: usize,
    headline: &'static str,
    placeholder: &'static str,
    copy: &'static str,
    filled: bool,
}

fn question(layout: Layout, panel: &SetupPanel<'_>, question: Question) -> Vec<Div> {
    let focus = panel.setup.focus();
    let message = panel.setup.message();
    let copy_y = if message.is_some() { 470. } else { 430. };
    let width = field_width(layout, question.filled);
    let field_focus = focus == SetupControl::Field;
    let mut parts = vec![
        progress(layout, question.step),
        headline(layout, HEADLINE_Y, question.headline),
        layout
            .rect(left(layout), FIELD_Y, width, FIELD_HEIGHT)
            .rounded(layout.px(12.))
            .bg(rgb(CELL))
            .border(layout.px(if field_focus { FIELD_BORDER } else { 1.5 }))
            .border_color(if field_focus { rgb(ICE) } else { rgba(FAINT) })
            .flex()
            .items_center()
            .px(layout.px(FIELD_PADDING + if field_focus { 0. } else { 1.5 }))
            .when(!panel.editing, |field| {
                field.child(
                    layout
                        .text(SAIRA, 32., 40., 400.)
                        .text_color(if question.filled {
                            rgb(TEXT)
                        } else {
                            rgba(MUTE)
                        })
                        .truncate()
                        .child(if question.filled {
                            "Address entered"
                        } else {
                            question.placeholder
                        }),
                )
            }),
    ];
    if question.filled {
        let clear_focus = focus == SetupControl::Clear;
        parts.push(
            layout
                .rect(
                    left(layout) + width + CLEAR_GAP,
                    FIELD_Y,
                    CLEAR_WIDTH,
                    FIELD_HEIGHT,
                )
                .rounded(layout.px(12.))
                .bg(rgb(if clear_focus { ICE } else { PLATE }))
                .text_color(rgb(if clear_focus { ON_ICE } else { TEXT }))
                .flex()
                .items_center()
                .justify_center()
                .child(layout.text(SAIRA, 26., 32., 600.).child("Clear")),
        );
    }
    if let Some(message) = message {
        parts.push(message_line(layout, 412., message));
    }
    parts.push(copy(layout, copy_y, question.copy));
    parts.push(buttons(layout, copy_y + 130., panel));
    parts
}

fn progress(layout: Layout, step: usize) -> Div {
    let segment = |index: usize| {
        let fill = match index.cmp(&step) {
            std::cmp::Ordering::Less => ICE_ELAPSED,
            std::cmp::Ordering::Equal => ICE,
            std::cmp::Ordering::Greater => PLATE,
        };
        div()
            .w(layout.px(150.))
            .h(layout.px(6.))
            .rounded_full()
            .bg(rgb(fill))
    };
    layout
        .rect(left(layout), PROGRESS_Y - 12., 600., 30.)
        .flex()
        .items_center()
        .gap(layout.px(10.))
        .child(segment(0))
        .child(segment(1))
        .child(
            layout
                .text(SAIRA, 22., 30., 500.)
                .text_color(rgba(MUTE))
                .ml(layout.px(18.))
                .child(format!("{} of 2", step + 1)),
        )
}

fn headline(layout: Layout, top: f32, text: &'static str) -> Div {
    layout
        .rect(left(layout), top, content_width(layout), 104.)
        .child(layout.text(SAIRA_PREVIEW, 96., 104., 700.).child(text))
}

fn copy(layout: Layout, top: f32, text: &'static str) -> Div {
    layout.rect(left(layout), top, COPY_WIDTH, 120.).child(
        layout
            .text(SAIRA, 27., 40., 400.)
            .text_color(rgba(DIM))
            .child(text),
    )
}

fn message_line(layout: Layout, top: f32, message: SetupMessage) -> Div {
    layout
        .rect(left(layout), top, content_width(layout), 40.)
        .child(
            layout
                .text(SAIRA, 24., 34., 600.)
                .text_color(rgb(ERROR))
                .child(message_copy(message)),
        )
}

/// The step's actions, as the Setup state machine lists them.
fn buttons(layout: Layout, top: f32, panel: &SetupPanel<'_>) -> Div {
    let focus = panel.setup.focus();
    let ready = panel.setup.guide_ready();
    let enabled = !panel.saving && (panel.setup.step() != SetupStep::Found || ready);
    layout
        .rect(left(layout), top, content_width(layout), BUTTON_HEIGHT)
        .flex()
        .gap(layout.px(18.))
        .children(panel.setup.rows(panel.draft)[1].iter().map(|&control| {
            let focused = control == focus;
            let label = if panel.saving && focused {
                "Checking…"
            } else {
                action_label(control, ready)
            };
            button(layout, label, focused, enabled)
        }))
}

const fn action_label(control: SetupControl, guide_ready: bool) -> &'static str {
    match control {
        SetupControl::Next => "Next",
        SetupControl::Skip => "Skip for now",
        SetupControl::Load => "Load channels",
        SetupControl::TryAgain => "Try again",
        SetupControl::ChangeSources => "Change sources",
        SetupControl::OpenGuide if guide_ready => "Open the guide",
        SetupControl::OpenGuide => "Finishing the guide…",
        SetupControl::Field | SetupControl::Clear => "",
    }
}

fn button(layout: Layout, label: &'static str, focused: bool, enabled: bool) -> Div {
    layout
        .text(SAIRA, 24., 32., 600.)
        .h(layout.px(BUTTON_HEIGHT))
        .px(layout.px(30.))
        .rounded_full()
        .flex()
        .items_center()
        .bg(rgb(if focused { ICE } else { PLATE }))
        .text_color(rgb(if focused { ON_ICE } else { TEXT }))
        .when(!enabled, |button| button.opacity(0.6))
        .child(label)
}

fn sweep(layout: Layout) -> impl IntoElement {
    let half = |left: bool| {
        let (from, to) = if left {
            (rgba(ICE << 8), rgb(ICE))
        } else {
            (rgb(ICE), rgba(ICE << 8))
        };
        div()
            .w(layout.px(SWEEP_WIDTH / 2.))
            .h_full()
            .bg(linear_gradient(
                90.,
                linear_color_stop(from, 0.),
                linear_color_stop(to, 1.),
            ))
    };
    layout
        .rect(-SWEEP_WIDTH, SWEEP_Y, SWEEP_WIDTH, 2.)
        .flex()
        .child(half(true))
        .child(half(false))
        .with_animation(
            ElementId::Name("setup-sweep".into()),
            Animation::new(Duration::from_millis(1600))
                .repeat()
                .with_max_fps(30.),
            move |line, delta| {
                line.left(layout.px(-SWEEP_WIDTH + delta * (layout.width + SWEEP_WIDTH)))
            },
        )
}

fn loading(layout: Layout) -> Vec<gpui::AnyElement> {
    vec![
        headline(layout, 330., "Loading your channels").into_any_element(),
        sweep(layout).into_any_element(),
        copy(
            layout,
            520.,
            "Sparrow is reading your channel source for the first time. \
             Large lists can take a minute.",
        )
        .into_any_element(),
    ]
}

fn found(layout: Layout, summary: &FoundSummary, panel: &SetupPanel<'_>) -> Vec<gpui::AnyElement> {
    let action = buttons(layout, 700., panel).ml(layout.px(10.));
    let rule = layout
        .rect(0., SWEEP_Y, layout.width, 2.)
        .bg(linear_gradient(
            90.,
            linear_color_stop(rgba(ICE << 8), 0.),
            linear_color_stop(rgba(ICE << 8 | 0x99), 1.),
        ));
    let FoundSummary::Counted {
        channels,
        groups,
        first,
    } = summary
    else {
        return vec![
            rule.into_any_element(),
            headline(layout, 330., "Your channels are ready").into_any_element(),
            action.into_any_element(),
        ];
    };
    let total = *channels;
    // Rolls up once per counted generation. The id keeps the finished state
    // across later frames.
    let generation = panel
        .setup
        .summary_generation()
        .map_or(0, |generation| generation.get());
    let count = layout
        .rect(left(layout), 170., content_width(layout), 300.)
        .with_animation(
            ElementId::Name(format!("setup-found-count-{generation}").into()),
            Animation::new(Duration::from_millis(1200)).with_easing(ease_out_quint()),
            move |count, delta| {
                let shown = (total as f64 * f64::from(delta)).round() as u64;
                count.child(
                    layout
                        .text(SAIRA_NUMBER, 300., 300., 250.)
                        .child(shown.to_string()),
                )
            },
        );
    let more = groups.saturating_sub(first.len());
    let names = layout
        .rect(left(layout) + 10., 590., content_width(layout), 40.)
        .flex()
        .gap(layout.px(34.))
        .text_size(layout.px(26.))
        .line_height(layout.px(36.))
        .children(first.iter().map(|(name, count)| {
            // Provider names can be long. Each keeps a bounded, single-line slot.
            div()
                .flex()
                .flex_none()
                .gap(layout.px(8.))
                .font_weight(gpui::FontWeight(600.))
                .text_color(rgba(DIM))
                .child(div().max_w(layout.px(GROUP_NAME_WIDTH)).truncate().child(
                    if name.is_empty() {
                        "Ungrouped".to_owned()
                    } else {
                        name.clone()
                    },
                ))
                .child(
                    div()
                        .font_weight(gpui::FontWeight(400.))
                        .text_color(rgba(MUTE))
                        .child(count.to_string()),
                )
        }))
        .when(more > 0, |names| {
            names.child(
                div()
                    .text_color(rgba(MUTE))
                    .child(format!("and {more} more")),
            )
        });
    vec![
        rule.into_any_element(),
        count.into_any_element(),
        layout
            .rect(left(layout) + 10., 500., content_width(layout), 70.)
            .child(
                layout
                    .text(SAIRA_TITLE, 56., 68., 650.)
                    .child(found_line(total, *groups)),
            )
            .into_any_element(),
        names.into_any_element(),
        action.into_any_element(),
    ]
}

/// The line under the big count, which supplies the number of channels.
fn found_line(channel_count: u64, group_count: usize) -> String {
    let channels = if channel_count == 1 {
        "channel"
    } else {
        "channels"
    };
    let groups = if group_count == 1 { "group" } else { "groups" };
    format!("{channels} in {group_count} {groups}")
}

const fn failure_copy(reason: FailureReason) -> &'static str {
    match reason {
        FailureReason::Unreachable => {
            "Sparrow couldn’t reach the channel source. Check the address and this TV’s network connection."
        }
        FailureReason::Interrupted => {
            "The download stopped before it finished. Try again in a moment."
        }
        FailureReason::Refused => {
            "The channel source refused the request. Check the address with your TV provider."
        }
        FailureReason::TimedOut => {
            "The channel source took too long to answer. Try again in a moment."
        }
        FailureReason::NotChannelList => {
            "The address didn’t return an M3U channel list. Check that it is the M3U link from your provider."
        }
        FailureReason::NoChannels => "The channel list has no channels Sparrow can play.",
        FailureReason::TooLarge => "The channel list is too large for this TV.",
        FailureReason::Storage => {
            "Sparrow couldn’t store the channels on this TV. Try again, or restart the TV if it keeps happening."
        }
    }
}

const fn message_copy(message: SetupMessage) -> &'static str {
    match message {
        SetupMessage::InvalidM3u | SetupMessage::InvalidEpg => {
            "Enter a full web address that starts with http:// or https://."
        }
        SetupMessage::SaveFailed => "Couldn’t save the sources. Try again.",
        SetupMessage::KeyboardUnavailable => "Couldn’t open the keyboard.",
        SetupMessage::SavedAddressesUnavailable => {
            "Couldn’t read the saved addresses. Enter them again."
        }
        SetupMessage::RetryUnavailable => "Couldn’t start loading. Try again.",
        SetupMessage::DraftUnavailable => "Couldn’t open the address editor.",
    }
}
