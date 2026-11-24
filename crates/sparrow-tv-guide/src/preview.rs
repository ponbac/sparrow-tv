//! Bounded schedule reads for the Programme preview after its focus dwell.

use chrono::{DateTime, Utc};
use sparrow_core::{
    CatalogGeneration, ChannelId, GuideProgramme, GuideWindowChannel, Page, PageCursor, PageLimit,
    PageRequest, ProgrammeSummary, ScheduleQuery,
};

use crate::{
    GuideFamily, VariantPreferences,
    time::{elapsed_fraction, minutes_left, next_after, programme_at},
};

const DESCRIPTION_LIMIT: usize = 1024;

/// Selects the exact focused Programme, or the live or next one for a Channel.
/// The returned row supplies both the display and its description lookup.
pub fn select_programme<'a>(
    family: &'a GuideFamily,
    preferences: &VariantPreferences,
    cell: Option<usize>,
    now: DateTime<Utc>,
) -> Option<(&'a GuideWindowChannel, &'a GuideProgramme)> {
    let (_, source) = family.preferred_with_schedule(preferences);
    let programmes = source.programmes();
    let programme = cell
        .and_then(|index| programmes.get(index))
        .or_else(|| programme_at(programmes, now))
        .or_else(|| next_after(programmes, now))?;
    Some((source, programme))
}

/// The time and value displayed in the Programme preview's status line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PreviewTiming {
    Live {
        elapsed_fraction: f64,
        minutes_left: i64,
    },
    Upcoming {
        in_minutes: i64,
        duration_minutes: i64,
    },
    Ended {
        duration_minutes: i64,
    },
}

pub fn preview_timing(programme: &GuideProgramme, now: DateTime<Utc>) -> PreviewTiming {
    let start = programme.starts_at();
    let end = programme.ends_at();
    if start <= now && now < end {
        return PreviewTiming::Live {
            elapsed_fraction: elapsed_fraction(programme, now),
            minutes_left: minutes_left(programme, now),
        };
    }
    let duration_minutes = (end - start).num_minutes();
    if now < start {
        PreviewTiming::Upcoming {
            in_minutes: ((start - now).num_seconds() as f64 / 60.).round().max(1.) as i64,
            duration_minutes,
        }
    } else {
        PreviewTiming::Ended { duration_minutes }
    }
}

/// The three successive Programmes on the preview's schedule source.
pub fn following_programmes<'a>(
    source: &'a GuideWindowChannel,
    programme: &GuideProgramme,
) -> impl Iterator<Item = &'a GuideProgramme> {
    let end = programme.ends_at();
    source
        .programmes()
        .iter()
        .filter(move |later| later.starts_at() >= end)
        .take(3)
}

#[derive(Clone, Eq, PartialEq)]
pub struct PreviewProgramme {
    generation: CatalogGeneration,
    channel: ChannelId,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    ordinal: usize,
}

impl PreviewProgramme {
    /// Resolves the exact occurrence from the compact schedule rendered in the Guide.
    pub fn for_row(
        generation: CatalogGeneration,
        row: &GuideWindowChannel,
        programme: &GuideProgramme,
    ) -> Option<Self> {
        let starts_at = programme.starts_at();
        let ends_at = programme.ends_at();
        let ordinal = row
            .programmes()
            .iter()
            .filter(|candidate| {
                candidate.starts_at() == starts_at && candidate.ends_at() == ends_at
            })
            .position(|candidate| std::ptr::eq(candidate, programme))?;
        Some(Self {
            generation,
            channel: row.channel().id().clone(),
            starts_at,
            ends_at,
            ordinal,
        })
    }

    pub fn description(&self, displayed_window_start: DateTime<Utc>) -> DescriptionLookup<'_> {
        DescriptionLookup {
            target: self,
            from: self.starts_at.max(displayed_window_start),
            remaining: self.ordinal,
            cursor: None,
        }
    }
}

pub struct DescriptionLookup<'a> {
    target: &'a PreviewProgramme,
    from: DateTime<Utc>,
    remaining: usize,
    cursor: Option<PageCursor>,
}

pub enum DescriptionPage<'a> {
    Finished(Option<String>),
    Next(DescriptionLookup<'a>),
}

impl<'a> DescriptionLookup<'a> {
    pub fn query(&self) -> ScheduleQuery {
        let limit = PageLimit::new(4).expect("four is a valid schedule page size");
        ScheduleQuery::new(
            self.target.channel.clone(),
            PageRequest::new(self.cursor.clone(), limit),
        )
        .with_from(self.from)
    }

    pub fn accept(mut self, page: &Page<ProgrammeSummary>) -> DescriptionPage<'a> {
        if page.generation() != self.target.generation {
            return DescriptionPage::Finished(None);
        }
        for programme in page.items() {
            if programme.starts_at() > self.target.starts_at {
                return DescriptionPage::Finished(None);
            }
            if programme.starts_at() == self.target.starts_at
                && programme.ends_at() == self.target.ends_at
            {
                if self.remaining == 0 {
                    return DescriptionPage::Finished(programme.description().map(|description| {
                        description[..description.floor_char_boundary(DESCRIPTION_LIMIT)].to_owned()
                    }));
                }
                self.remaining -= 1;
            }
        }
        if let Some(next) = page.next() {
            self.cursor = Some(next.clone());
            DescriptionPage::Next(self)
        } else {
            DescriptionPage::Finished(None)
        }
    }
}
