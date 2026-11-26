use sparrow_core::ChannelDirection;

use super::*;

pub(crate) type Reel = [Option<(u32, String)>; 2];

const DURATION: f32 = 0.34;

pub(crate) struct ZapRoll {
    number: u32,
    name: String,
    title: String,
    reel: Reel,
    direction: ChannelDirection,
    digits: Option<Instant>,
    subject: Option<Instant>,
}

impl ZapRoll {
    pub(crate) fn new(
        number: u32,
        name: &str,
        title: Option<&str>,
        reel: Reel,
        direction: ChannelDirection,
    ) -> Self {
        Self {
            number,
            name: name.to_owned(),
            title: headline(name, title).to_owned(),
            reel,
            direction,
            digits: None,
            subject: None,
        }
    }

    pub(crate) fn show_number(&mut self) {
        self.digits.get_or_insert_with(Instant::now);
    }

    pub(crate) fn show_subject(&mut self) {
        self.subject.get_or_insert_with(Instant::now);
    }

    pub(crate) fn active(&self, number: u32) -> bool {
        self.number != number
            && [self.digits, self.subject]
                .into_iter()
                .flatten()
                .any(|t| t.elapsed().as_secs_f32() < DURATION)
    }

    fn positions(&self, start: Instant, distance: f32) -> (f32, f32, f32) {
        let p = (start.elapsed().as_secs_f32() / DURATION).min(1.);
        let eased = gpui::ease_out_quint()(p);
        let travel = match self.direction {
            ChannelDirection::Next => -distance,
            ChannelDirection::Previous => distance,
        };
        (travel * eased, travel * (eased - 1.), p)
    }
}

pub(super) fn number(
    layout: Layout,
    playback: &PlaybackOverlay<'_>,
    height: f32,
    font: f32,
    slot: f32,
) -> Div {
    let new = playback.channel_number.to_string();
    let roll = playback.roll.and_then(|r| Some((r, r.digits?)));
    let old = roll
        .map(|(r, _)| r.number.to_string())
        .unwrap_or_else(|| new.clone());
    let (out_y, in_y, p) = roll.map_or((0., 0., 1.), |(r, start)| r.positions(start, height));
    let width = |value: &str| (slot * value.len() as f32).min(60.);
    let w = width(&old) + (width(&new) - width(&old)) * gpui::ease_out_quint()(p);
    let row = div()
        .relative()
        .h(layout.px(height))
        .w(layout.px(w))
        .overflow_hidden();
    let label = |value: String, y, x, width, text_size| {
        div()
            .absolute()
            .top(layout.px(y))
            .left(layout.px(x))
            .w(layout.px(width))
            .h_full()
            .text_size(layout.px(text_size))
            .flex()
            .items_center()
            .justify_center()
            .child(value)
    };
    if old.len() != new.len() {
        let layer = |s: &str, y| {
            label(
                s.to_owned(),
                y,
                (w - width(s)) / 2.,
                width(s),
                font.min(100. / s.len() as f32),
            )
        };
        return row.child(layer(&old, out_y)).child(layer(&new, in_y));
    }
    let slot = w / new.len() as f32;
    let size = font.min(100. / new.len() as f32);
    old.chars()
        .zip(new.chars())
        .enumerate()
        .fold(row, |row, (i, (a, b))| {
            let x = i as f32 * slot;
            if a == b {
                row.child(label(b.to_string(), 0., x, slot, size))
            } else {
                row.child(label(a.to_string(), out_y, x, slot, size))
                    .child(label(b.to_string(), in_y, x, slot, size))
            }
        })
}

fn reel(layout: Layout, entries: &Reel) -> Div {
    let mut rows = div().size_full().flex().flex_col().justify_center();
    for (arrow, entry) in ["▲", "▼"].into_iter().zip(entries) {
        let mut row = layout
            .text(SAIRA, 20., 26., 500.)
            .h(layout.px(26.))
            .flex()
            .gap(layout.px(6.))
            .items_center()
            .text_color(rgba(DIM));
        if let Some((number, name)) = entry {
            row = row
                .child(
                    div()
                        .w(layout.px(16.))
                        .flex_none()
                        .text_color(rgb(ICE))
                        .child(arrow),
                )
                .child(
                    layout
                        .text(SAIRA_CLOCK, 20., 26., 500.)
                        .w(layout.px(40.))
                        .flex_none()
                        .child(number.to_string()),
                )
                .child(div().flex_1().min_w_0().truncate().child(name.clone()));
        }
        rows = rows.child(row);
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
    if let Some(roll) = roll
        && let Some(start) = roll.subject
    {
        let (out_y, in_y, p) = roll.positions(start, 80.);
        let layer = |y, opacity| {
            div()
                .absolute()
                .top(layout.px(y))
                .size_full()
                .opacity(opacity)
        };
        container
            .child(layer(out_y, (1. - p / 0.35).clamp(0., 1.)).child(outgoing(roll)))
            .child(layer(in_y, ((p - 0.15) / 0.55).clamp(0., 1.)).child(current))
    } else {
        container.child(current)
    }
}

pub(super) fn channel_reel(layout: Layout, entries: &Reel, roll: Option<&ZapRoll>) -> Div {
    roll_rows(
        layout,
        div().w(layout.px(250.)).flex_none(),
        reel(layout, entries),
        |old| reel(layout, &old.reel),
        roll,
    )
}

pub(super) fn identity(layout: Layout, playback: &PlaybackOverlay<'_>) -> Div {
    let label = |name: &str, title: &str| {
        div()
            .child(
                layout
                    .text(SAIRA, 22., 26., 600.)
                    .truncate()
                    .child(name.to_owned()),
            )
            .child(
                layout
                    .text(SAIRA_TITLE, 36., 42., 700.)
                    .truncate()
                    .child(title.to_owned()),
            )
    };
    roll_rows(
        layout,
        div().flex_1().min_w_0(),
        label(
            playback.channel_name,
            headline(
                playback.channel_name,
                playback.programme.as_ref().map(|p| p.title),
            ),
        ),
        |old| label(&old.name, &old.title),
        playback.roll,
    )
}

fn headline<'a>(name: &'a str, title: Option<&'a str>) -> &'a str {
    title.unwrap_or(name)
}
