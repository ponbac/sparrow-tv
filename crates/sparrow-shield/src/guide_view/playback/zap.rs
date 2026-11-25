use sparrow_core::ChannelDirection;

use super::*;

pub(crate) struct ZapRoll {
    number: u32,
    name: String,
    title: String,
    reel: [Option<String>; 2],
    direction: ChannelDirection,
    started: Instant,
}

impl ZapRoll {
    pub(crate) fn new(
        playback: &PlaybackOverlay<'_>,
        direction: ChannelDirection,
        started: Instant,
    ) -> Self {
        Self {
            number: playback.channel_number,
            name: playback.channel_name.to_owned(),
            title: playback
                .programme
                .as_ref()
                .map_or(playback.channel_name, |p| p.title)
                .to_owned(),
            reel: playback.reel.cloned().unwrap_or_default(),
            direction,
            started,
        }
    }

    pub(crate) fn active(&self, number: u32) -> bool {
        self.number != number && self.started.elapsed().as_millis() < 420
    }

    fn positions(&self, distance: f32) -> (f32, f32, f32) {
        let t = (self.started.elapsed().as_secs_f32() / 0.42).min(1.);
        let t = 1. - (1. - t).powi(3);
        let travel = match self.direction {
            ChannelDirection::Next => -distance,
            ChannelDirection::Previous => distance,
        };
        (travel * t, travel * (t - 1.), t)
    }
}

pub(super) fn number(layout: Layout, playback: &PlaybackOverlay<'_>, height: f32) -> Div {
    let new = playback.channel_number.to_string();
    let old = playback
        .roll
        .map(|r| r.number.to_string())
        .unwrap_or_default();
    let columns = new.len().max(old.len());
    let mut row = div()
        .h(layout.px(height))
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden();
    if height == 68. {
        row = row.text_size(layout.px((100. / columns as f32).min(34.)));
    }
    for i in 0..columns {
        let digit = |value: &str| {
            i.checked_sub(columns - value.len())
                .map(|n| char::from(value.as_bytes()[n]))
        };
        let new_digit = digit(&new);
        let old_digit = digit(&old);
        let slot = div().relative().h_full().w(layout.px(if height == 68. {
            (60. / columns as f32).min(21.)
        } else {
            13.
        }));
        let label = |digit: char, y| {
            div()
                .absolute()
                .top(layout.px(y))
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(digit.to_string())
        };
        row = row.child(
            if let Some(roll) = playback.roll
                && old_digit != new_digit
            {
                let (outgoing, incoming, _) = roll.positions(height);
                slot.children(old_digit.map(|d| label(d, outgoing)))
                    .children(new_digit.map(|d| label(d, incoming)))
            } else {
                slot.children(new_digit.map(|d| label(d, 0.)))
            },
        );
    }
    row
}

fn reel(layout: Layout, entries: Option<&[Option<String>; 2]>) -> Div {
    let mut rows = div().size_full().flex().flex_col().justify_center();
    if let Some(entries) = entries {
        for (arrow, text) in ["▲", "▼"].into_iter().zip(entries) {
            if let Some(text) = text {
                rows = rows.child(
                    layout
                        .text(SAIRA, 20., 26., 500.)
                        .flex()
                        .gap(layout.px(9.))
                        .child(div().text_color(rgb(ICE)).child(arrow))
                        .child(div().truncate().text_color(rgba(DIM)).child(text.clone())),
                );
            }
        }
    }
    rows
}

fn roll_rows(
    layout: Layout,
    container: Div,
    current: Div,
    outgoing: impl FnOnce(&ZapRoll) -> Div,
    roll: Option<&ZapRoll>,
) -> Div {
    let container = container.relative().h(layout.px(68.)).overflow_hidden();
    if let Some(roll) = roll {
        let (out_y, in_y, t) = roll.positions(60.);
        let layer = |y, opacity| {
            div()
                .absolute()
                .top(layout.px(y))
                .size_full()
                .opacity(opacity)
        };
        container
            .child(layer(out_y, 1. - t).child(outgoing(roll)))
            .child(layer(in_y, t).child(current))
    } else {
        container.child(current)
    }
}

pub(super) fn channel_reel(layout: Layout, playback: &PlaybackOverlay<'_>) -> Div {
    roll_rows(
        layout,
        div().w(layout.px(278.)).flex_none(),
        reel(layout, playback.reel),
        |old| reel(layout, Some(&old.reel)),
        playback.roll,
    )
}

pub(super) fn identity(layout: Layout, current: Div, roll: Option<&ZapRoll>) -> Div {
    roll_rows(
        layout,
        div().flex_1().min_w_0(),
        current,
        |old| {
            div()
                .child(
                    layout
                        .text(SAIRA, 22., 26., 600.)
                        .truncate()
                        .child(old.name.clone()),
                )
                .child(
                    layout
                        .text(SAIRA_TITLE, 36., 42., 700.)
                        .truncate()
                        .child(old.title.clone()),
                )
        },
        roll,
    )
}
