use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use serde::Serialize;
use sparrow_core::{
    ChannelDetails, ChannelGroupView, ChannelSummary, GuideProgramme, GuideWindowChannel, Page,
    ProgrammeSearchHit, ProgrammeSummary, SearchResults,
};

/// A generation-bound page shared by HTTP and installed IPC clients.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageDto<T> {
    generation: u64,
    items: Vec<T>,
    next: Option<String>,
}

impl PageDto<ChannelGroupDto> {
    pub fn groups(page: &Page<ChannelGroupView>) -> Self {
        Self::new(page, |group| ChannelGroupDto {
            name: group.name().to_owned(),
            channel_count: group.channel_count(),
        })
    }
}

impl PageDto<ChannelSummaryDto> {
    pub fn channels(page: &Page<ChannelSummary>) -> Self {
        Self::new(page, |channel| ChannelSummaryDto::from(channel))
    }
}

impl PageDto<ProgrammeDto> {
    pub fn programmes(page: &Page<ProgrammeSummary>) -> Self {
        Self::new(page, |programme| ProgrammeDto::from(programme))
    }
}

impl PageDto<ProgrammeSearchHitDto> {
    pub fn programme_search_hits(page: &Page<ProgrammeSearchHit>) -> Self {
        Self::new(page, |hit| ProgrammeSearchHitDto::from(hit))
    }
}

impl PageDto<GuideWindowChannelDto> {
    pub fn guide_window(page: &Page<GuideWindowChannel>) -> Self {
        Self::new(page, |row| GuideWindowChannelDto::from(row))
    }
}

impl<T> PageDto<T> {
    fn new<U>(page: &Page<U>, project: impl Fn(&U) -> T) -> Self {
        Self {
            generation: page.generation().get(),
            items: page.items().iter().map(project).collect(),
            next: page.next().map(|cursor| cursor.as_str().to_owned()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelGroupDto {
    name: String,
    channel_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSummaryDto {
    id: String,
    name: String,
    group: String,
}

impl From<&ChannelSummary> for ChannelSummaryDto {
    fn from(channel: &ChannelSummary) -> Self {
        Self {
            id: channel.id().as_str().to_owned(),
            name: channel.name().to_owned(),
            group: channel.group().to_owned(),
        }
    }
}

pub type ChannelDetailsDto = ChannelSummaryDto;

impl From<&ChannelDetails> for ChannelSummaryDto {
    fn from(channel: &ChannelDetails) -> Self {
        Self {
            id: channel.id().as_str().to_owned(),
            name: channel.name().to_owned(),
            group: channel.group().to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgrammeDto {
    channel_id: String,
    title: String,
    description: Option<String>,
    starts_at: String,
    ends_at: String,
}

impl From<&ProgrammeSummary> for ProgrammeDto {
    fn from(programme: &ProgrammeSummary) -> Self {
        Self {
            channel_id: programme.channel_id().as_str().to_owned(),
            title: programme.title().to_owned(),
            description: programme.description().map(str::to_owned),
            starts_at: browser_instant(programme.starts_at()),
            ends_at: browser_instant(programme.ends_at()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgrammeSearchHitDto {
    channel: ChannelSummaryDto,
    #[serde(flatten)]
    programme: CompactProgrammeDto,
}

impl From<&ProgrammeSearchHit> for ProgrammeSearchHitDto {
    fn from(hit: &ProgrammeSearchHit) -> Self {
        Self {
            channel: ChannelSummaryDto::from(hit.channel()),
            programme: CompactProgrammeDto::new(
                hit.title(),
                hit.title_truncated(),
                hit.starts_at(),
                hit.ends_at(),
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideWindowChannelDto {
    channel: ChannelSummaryDto,
    programmes: Vec<CompactProgrammeDto>,
    programmes_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompactProgrammeDto {
    title: String,
    title_truncated: bool,
    starts_at: String,
    ends_at: String,
}

impl CompactProgrammeDto {
    fn new(
        title: &str,
        title_truncated: bool,
        starts_at: DateTime<Utc>,
        ends_at: DateTime<Utc>,
    ) -> Self {
        Self {
            title: title.to_owned(),
            title_truncated,
            starts_at: browser_instant(starts_at),
            ends_at: browser_instant(ends_at),
        }
    }
}

impl From<&GuideProgramme> for CompactProgrammeDto {
    fn from(programme: &GuideProgramme) -> Self {
        Self::new(
            programme.title(),
            programme.title_truncated(),
            programme.starts_at(),
            programme.ends_at(),
        )
    }
}

impl From<&GuideWindowChannel> for GuideWindowChannelDto {
    fn from(row: &GuideWindowChannel) -> Self {
        Self {
            channel: ChannelSummaryDto::from(row.channel()),
            programmes: row
                .programmes()
                .iter()
                .map(CompactProgrammeDto::from)
                .collect(),
            programmes_truncated: row.programmes_truncated(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResultsDto {
    generation: u64,
    channels: PageDto<ChannelSummaryDto>,
    programmes: PageDto<ProgrammeSearchHitDto>,
}

impl From<&SearchResults> for SearchResultsDto {
    fn from(results: &SearchResults) -> Self {
        Self {
            generation: results.generation().get(),
            channels: PageDto::channels(results.channels()),
            programmes: PageDto::programme_search_hits(results.programmes()),
        }
    }
}

/// Projects a core UTC instant into the browser's four-digit RFC 3339 range.
pub fn browser_instant(value: DateTime<Utc>) -> String {
    match value.year() {
        ..=-1 => "0000-01-01T00:00:00Z".to_owned(),
        10_000.. => "9999-12-31T23:59:59Z".to_owned(),
        _ => value.to_rfc3339_opts(SecondsFormat::AutoSi, true),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::browser_instant;

    #[test]
    fn browser_instant_clamps_years_and_preserves_nanoseconds() {
        assert_eq!(
            browser_instant(DateTime::<Utc>::MIN_UTC),
            "0000-01-01T00:00:00Z"
        );
        assert_eq!(
            browser_instant(DateTime::<Utc>::MAX_UTC),
            "9999-12-31T23:59:59Z"
        );
        assert_eq!(
            browser_instant(
                DateTime::parse_from_rfc3339("2026-08-30T12:00:00.123456789Z")
                    .expect("fixture instant is valid")
                    .with_timezone(&Utc)
            ),
            "2026-08-30T12:00:00.123456789Z"
        );
    }
}
