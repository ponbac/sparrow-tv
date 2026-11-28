//! Clock windows, Programme timing, and the Timeline's time-axis navigation.

use chrono::{
    DateTime, Datelike, FixedOffset, MappedLocalTime, Offset, TimeDelta, TimeZone, Utc, Weekday,
};
use sparrow_core::GuideProgramme;

const HALF_HOUR_SECONDS: i64 = 30 * 60;
const TIMELINE_SPAN: TimeDelta = TimeDelta::minutes(120);
const RIGHT_EDGE: TimeDelta = TimeDelta::minutes(15);
const LEFT_EDGE_SLIVER: TimeDelta = TimeDelta::seconds(8 * 60 + 30);

/// A stable query window anchored to the local half hour.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuideWindow {
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
}

/// Up to six future full local hours in the extended Guide window, in UTC order.
/// A repeated clock hour contributes both instants and a skipped hour contributes none.
pub fn next_full_local_hours(now: DateTime<Utc>, zone: &impl TimeZone) -> Vec<DateTime<Utc>> {
    const HOUR: i64 = 3600;
    const MAX_OFFSET: i64 = 24 * HOUR;
    let offset = zone.offset_from_utc_datetime(&now.naive_utc()).fix();
    let end = GuideWindow::at(now, offset, true).ends_at();
    // Search wall-clock hours on both sides of the UTC window, since the
    // offset at a future hour can differ from the current offset.
    let first = (now.timestamp() - MAX_OFFSET).div_euclid(HOUR) + 1;
    let last = (end.timestamp() + MAX_OFFSET).div_euclid(HOUR);
    let mut hours = Vec::new();
    for hour in first..=last {
        let Some(wall_clock) = DateTime::from_timestamp(hour * HOUR, 0) else {
            continue;
        };
        let mut add = |instant: DateTime<Utc>| {
            if now < instant && instant < end {
                hours.push(instant);
            }
        };
        match zone.from_local_datetime(&wall_clock.naive_utc()) {
            MappedLocalTime::Single(instant) => add(instant.to_utc()),
            MappedLocalTime::Ambiguous(first, second) => {
                add(first.to_utc());
                add(second.to_utc());
            }
            MappedLocalTime::None => {}
        }
    }
    hours.sort_unstable();
    hours.truncate(6);
    hours
}

impl GuideWindow {
    /// The ordinary read lasts three hours. The extended read lasts eight hours.
    pub fn at(now: DateTime<Utc>, offset: FixedOffset, extended: bool) -> Self {
        let utc_seconds = now.timestamp();
        let local_seconds = utc_seconds + i64::from(offset.local_minus_utc());
        let into_half_hour = local_seconds.rem_euclid(HALF_HOUR_SECONDS);
        let starts_at = DateTime::from_timestamp(utc_seconds - into_half_hour, 0)
            .unwrap_or(DateTime::<Utc>::MIN_UTC);
        let span = TimeDelta::hours(if extended { 8 } else { 3 });
        let ends_at = starts_at
            .checked_add_signed(span)
            .unwrap_or(DateTime::<Utc>::MAX_UTC);
        Self { starts_at, ends_at }
    }

    pub const fn starts_at(self) -> DateTime<Utc> {
        self.starts_at
    }

    pub const fn ends_at(self) -> DateTime<Utc> {
        self.ends_at
    }

    fn overlaps(self, programme: &GuideProgramme) -> bool {
        programme.starts_at() < self.ends_at && self.starts_at < programme.ends_at()
    }
}

/// The first Programme airing at an instant, with a half-open time interval.
pub fn programme_at(
    programmes: &[GuideProgramme],
    instant: DateTime<Utc>,
) -> Option<&GuideProgramme> {
    programmes
        .iter()
        .find(|programme| is_live(programme, instant))
}

/// The first later Programme in a start-ordered schedule, including after a gap.
pub fn next_after(
    programmes: &[GuideProgramme],
    instant: DateTime<Utc>,
) -> Option<&GuideProgramme> {
    programmes
        .iter()
        .find(|programme| programme.starts_at() > instant)
}

/// The time left in minutes, rounded up with a minimum of one.
pub fn minutes_left(programme: &GuideProgramme, now: DateTime<Utc>) -> i64 {
    let remaining = programme.ends_at() - now;
    let seconds = remaining.num_seconds();
    let rounded_up = seconds / 60 + i64::from(seconds % 60 > 0 || remaining.subsec_nanos() > 0);
    rounded_up.max(1)
}

/// How much of the Programme has aired, clamped between zero and one.
pub fn elapsed_fraction(programme: &GuideProgramme, now: DateTime<Utc>) -> f64 {
    let elapsed = now - programme.starts_at();
    let duration = programme.ends_at() - programme.starts_at();
    (elapsed.as_seconds_f64() / duration.as_seconds_f64()).clamp(0.0, 1.0)
}

fn is_live(programme: &GuideProgramme, at: DateTime<Utc>) -> bool {
    programme.starts_at() <= at && at < programme.ends_at()
}

/// The 120 minutes of the query window currently shown in the Timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timeline {
    window: GuideWindow,
    starts_at: DateTime<Utc>,
}

/// A horizontal step from an existing Programme cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgrammeStep {
    Earlier,
    Later,
}

/// A Programme's position and widths as fractions of the Timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgrammePlacement {
    pub left_fraction: f64,
    /// Width visible inside the track.
    pub width_fraction: f64,
    /// Width from the clipped-left start to the actual end, before right clipping.
    pub full_width_fraction: f64,
    pub cut_at_start: bool,
}

/// The visible copy inside a Timeline Programme cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgrammeCellContent {
    Text,
    Chevron,
}

/// A drawable Programme cell, measured before the track clips the right edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgrammeCellAppearance {
    pub width: f32,
    pub content: ProgrammeCellContent,
}

impl ProgrammePlacement {
    /// Classifies the cell using the full span after its six-unit gap.
    /// At 90 reference units or less, only a cut tail or a focused cell draws.
    pub fn cell_appearance(
        self,
        track_width: f32,
        focused: bool,
    ) -> Option<ProgrammeCellAppearance> {
        const CELL_GAP: f64 = 6.;
        const TEXT_CUTOFF: f64 = 90.;

        let width = self.full_width_fraction * f64::from(track_width) - CELL_GAP;
        let content = if width <= 0. {
            return None;
        } else if width > TEXT_CUTOFF {
            ProgrammeCellContent::Text
        } else if self.cut_at_start || focused {
            ProgrammeCellContent::Chevron
        } else {
            return None;
        };
        Some(ProgrammeCellAppearance {
            width: width as f32,
            content,
        })
    }
}

/// Labels the first visible day relative to the current local date.
/// Localize each instant separately so a change in UTC offset is reflected.
pub fn timeline_day_label(
    visible: DateTime<FixedOffset>,
    now: DateTime<FixedOffset>,
) -> &'static str {
    let visible_date = visible.date_naive();
    match (visible_date - now.date_naive()).num_days() {
        0 => "Today",
        1 => "Tomorrow",
        _ => match visible_date.weekday() {
            Weekday::Mon => "Monday",
            Weekday::Tue => "Tuesday",
            Weekday::Wed => "Wednesday",
            Weekday::Thu => "Thursday",
            Weekday::Fri => "Friday",
            Weekday::Sat => "Saturday",
            Weekday::Sun => "Sunday",
        },
    }
}

impl Timeline {
    pub const fn new(window: GuideWindow) -> Self {
        Self {
            window,
            starts_at: window.starts_at,
        }
    }

    pub const fn starts_at(self) -> DateTime<Utc> {
        self.starts_at
    }

    /// First half-hour ruler mark inside the visible span.
    pub fn first_tick(self) -> DateTime<Utc> {
        let elapsed = self.starts_at - self.window.starts_at;
        let step = TimeDelta::seconds(HALF_HOUR_SECONDS);
        let previous =
            TimeDelta::seconds(elapsed.num_seconds() / HALF_HOUR_SECONDS * HALF_HOUR_SECONDS);
        self.window.starts_at
            + if previous == elapsed {
                previous
            } else {
                previous + step
            }
    }

    pub fn ends_at(self) -> DateTime<Utc> {
        self.starts_at
            .checked_add_signed(TIMELINE_SPAN)
            .unwrap_or(DateTime::<Utc>::MAX_UTC)
            .min(self.window.ends_at)
    }

    /// Keeps the absolute time in view as the clock window advances.
    pub fn reanchor(&mut self, window: GuideWindow) {
        self.window = window;
        self.set_start(self.starts_at);
    }

    /// Focus can reveal several half-hour steps at once, inside the query window.
    pub fn reveal(&mut self, programme: &GuideProgramme) {
        let start = programme.starts_at();
        let end = programme.ends_at();
        if !self.window.overlaps(programme) {
            return;
        }
        let lead_in = if self.ends_at() - start <= RIGHT_EDGE {
            TimeDelta::seconds(HALF_HOUR_SECONDS)
        } else if end - self.starts_at <= LEFT_EDGE_SLIVER {
            TimeDelta::zero()
        } else {
            return;
        };
        let elapsed = start - self.window.starts_at;
        let half_hour = elapsed
            .num_milliseconds()
            .div_euclid(HALF_HOUR_SECONDS * 1_000);
        let aligned = self.window.starts_at + TimeDelta::seconds(half_hour * HALF_HOUR_SECONDS);
        self.set_start(aligned - lead_in);
    }

    /// The time that Up or Down preserves when changing Channel.
    pub fn anchor(self, now: DateTime<Utc>, focused: Option<&GuideProgramme>) -> DateTime<Utc> {
        focused
            .map_or(now, GuideProgramme::starts_at)
            .max(self.starts_at)
    }

    /// A Programme for an adjacent row or from the Channel Group lane.
    /// The row's Channel is the stop when there is no Programme in the query window.
    pub fn cell_at(self, programmes: &[GuideProgramme], anchor: DateTime<Utc>) -> Option<usize> {
        programmes
            .iter()
            .enumerate()
            .filter(|(_, programme)| self.window.overlaps(programme))
            .min_by_key(|(_, programme)| {
                let distance = if is_live(programme, anchor) {
                    TimeDelta::seconds(-1)
                } else {
                    (programme.starts_at() - anchor)
                        .abs()
                        .min((programme.ends_at() - anchor).abs())
                };
                (self.is_left_sliver(programme), distance)
            })
            .map(|(index, _)| index)
    }

    /// Moves chronologically, including to cells that can be revealed by scrolling.
    pub fn step_programme(
        self,
        programmes: &[GuideProgramme],
        focused: usize,
        step: ProgrammeStep,
    ) -> Option<usize> {
        let (earlier, rest) = programmes.split_at_checked(focused)?;
        let (_, later) = rest.split_first()?;
        let can_focus = |programme: &GuideProgramme| {
            self.window.overlaps(programme)
                && (self.starts_at > self.window.starts_at || !self.is_left_sliver(programme))
        };
        match step {
            ProgrammeStep::Earlier => earlier.iter().rposition(can_focus),
            ProgrammeStep::Later => later
                .iter()
                .position(can_focus)
                .map(|index| focused + index + 1),
        }
    }

    /// Restores focus in the same row after its Programmes have changed.
    /// If the focused Programme has left, move to its successor or the Channel.
    pub fn restore_focus(
        &mut self,
        programmes: &[GuideProgramme],
        focus: &ProgrammeFocus,
    ) -> Option<usize> {
        let index = self.exact_focus(programmes, focus).or_else(|| {
            programmes.iter().position(|programme| {
                self.window.overlaps(programme)
                    && programme.starts_at() >= focus.programme.ends_at()
            })
        })?;
        self.reveal(&programmes[index]);
        Some(index)
    }

    /// Looks up a remembered cell in its row without moving focus or panning.
    pub fn exact_focus(
        self,
        programmes: &[GuideProgramme],
        focus: &ProgrammeFocus,
    ) -> Option<usize> {
        programmes
            .iter()
            .enumerate()
            .filter(|(_, programme)| {
                self.window.overlaps(programme) && same_programme(programme, &focus.programme)
            })
            .nth(focus.occurrence)
            .map(|(index, _)| index)
    }

    pub fn placement(self, programme: &GuideProgramme) -> Option<ProgrammePlacement> {
        let viewport_end = self.ends_at();
        let start = programme.starts_at().max(self.starts_at);
        let end = programme.ends_at().min(viewport_end);
        if end <= start {
            return None;
        }
        let span = (viewport_end - self.starts_at).as_seconds_f64();
        Some(ProgrammePlacement {
            left_fraction: (start - self.starts_at).as_seconds_f64() / span,
            width_fraction: (end - start).as_seconds_f64() / span,
            full_width_fraction: (programme.ends_at() - start).as_seconds_f64() / span,
            cut_at_start: programme.starts_at() < self.starts_at,
        })
    }

    /// Progress from the visible cell's left edge to the Programme's actual end.
    pub fn elapsed_fraction(self, programme: &GuideProgramme, now: DateTime<Utc>) -> Option<f64> {
        let start = programme.starts_at().max(self.starts_at);
        let end = programme.ends_at();
        if start >= end.min(self.ends_at()) {
            return None;
        }
        Some(((now - start).as_seconds_f64() / (end - start).as_seconds_f64()).clamp(0.0, 1.0))
    }

    /// Position of the current time, if the Timeline shows it.
    pub fn playhead_fraction(self, now: DateTime<Utc>) -> Option<f64> {
        let end = self.ends_at();
        if now < self.starts_at || now > end {
            return None;
        }
        Some((now - self.starts_at).as_seconds_f64() / (end - self.starts_at).as_seconds_f64())
    }

    /// Moves the visible span by whole half hours, clamped to the Guide window.
    pub fn pan(&mut self, half_hours: i64) {
        if let Some(delta) = TimeDelta::try_seconds(half_hours.saturating_mul(HALF_HOUR_SECONDS)) {
            self.pan_by(delta);
        }
    }

    /// Moves the visible span by elapsed time inside the Guide window.
    pub fn pan_by(&mut self, delta: TimeDelta) {
        let wanted = self
            .starts_at
            .checked_add_signed(delta)
            .unwrap_or(self.starts_at);
        self.set_start(wanted);
    }

    fn set_start(&mut self, wanted: DateTime<Utc>) {
        let latest = self.window.ends_at - TIMELINE_SPAN;
        self.starts_at = wanted.min(latest).max(self.window.starts_at);
    }

    fn is_left_sliver(self, programme: &GuideProgramme) -> bool {
        programme.starts_at() < self.starts_at
            && programme.ends_at() - self.starts_at < LEFT_EDGE_SLIVER
    }
}

/// Programme identity and its occurrence among identical entries in a row.
#[derive(Clone)]
pub struct ProgrammeFocus {
    programme: GuideProgramme,
    occurrence: usize,
}

impl ProgrammeFocus {
    pub fn at(programmes: &[GuideProgramme], index: usize) -> Option<Self> {
        let programme = programmes.get(index)?;
        let occurrence = programmes[..index]
            .iter()
            .filter(|earlier| same_programme(earlier, programme))
            .count();
        Some(Self {
            programme: programme.clone(),
            occurrence,
        })
    }
}

fn same_programme(left: &GuideProgramme, right: &GuideProgramme) -> bool {
    left.starts_at() == right.starts_at()
        && left.ends_at() == right.ends_at()
        && left.title() == right.title()
}
