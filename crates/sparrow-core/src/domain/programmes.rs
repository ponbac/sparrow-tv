use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::{ChannelId, ChannelQuery, ChannelSummary, CoreError, InputField, InputReason};

/// One source-derived Programme associated with a Channel in this catalog generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProgrammeSummary {
    channel_id: ChannelId,
    title: Arc<str>,
    description: Option<Arc<str>>,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
}

impl ProgrammeSummary {
    pub(crate) fn new(
        channel_id: ChannelId,
        title: Arc<str>,
        description: Option<Arc<str>>,
        starts_at: DateTime<Utc>,
        ends_at: DateTime<Utc>,
    ) -> Self {
        Self {
            channel_id,
            title,
            description,
            starts_at,
            ends_at,
        }
    }

    /// Returns the opaque Channel Identifier associated with this Programme.
    pub fn channel_id(&self) -> &ChannelId {
        &self.channel_id
    }

    /// Returns the normalized Programme title supplied by the EPG Source.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the optional normalized Programme description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Returns the Programme start instant normalized to UTC.
    pub const fn starts_at(&self) -> DateTime<Utc> {
        self.starts_at
    }

    /// Returns the Programme end instant normalized to UTC.
    pub const fn ends_at(&self) -> DateTime<Utc> {
        self.ends_at
    }
}

const MAX_COMPACT_PROGRAMME_TITLE_BYTES: usize = 256;

/// One compact Programme search match paired with its owning Channel.
///
/// Search hits deliberately omit the source description so the bounded result
/// lane cannot retain or serialize arbitrarily large XMLTV fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProgrammeSearchHit {
    channel: ChannelSummary,
    title: Arc<str>,
    title_truncated: bool,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
}

impl ProgrammeSearchHit {
    /// The largest normalized Programme title included in a search response.
    pub const MAX_TITLE_BYTES: usize = MAX_COMPACT_PROGRAMME_TITLE_BYTES;

    pub(crate) fn new(
        channel: ChannelSummary,
        title: Arc<str>,
        starts_at: DateTime<Utc>,
        ends_at: DateTime<Utc>,
    ) -> Self {
        let (title, title_truncated) = bounded_programme_title(title);
        Self {
            channel,
            title,
            title_truncated,
            starts_at,
            ends_at,
        }
    }

    /// Returns the browser-safe Channel that owns this Programme.
    pub const fn channel(&self) -> &ChannelSummary {
        &self.channel
    }

    /// Returns the bounded normalized Programme title supplied by the EPG Source.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Reports whether the source title exceeded the search projection bound.
    pub const fn title_truncated(&self) -> bool {
        self.title_truncated
    }

    /// Returns the Programme start instant normalized to UTC.
    pub const fn starts_at(&self) -> DateTime<Utc> {
        self.starts_at
    }

    /// Returns the Programme end instant normalized to UTC.
    pub const fn ends_at(&self) -> DateTime<Utc> {
        self.ends_at
    }
}

/// Bounded Programme metadata projected specifically for one guide row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuideProgramme {
    title: Arc<str>,
    title_truncated: bool,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
}

impl GuideProgramme {
    /// The largest normalized Programme title included in a guide response.
    pub const MAX_TITLE_BYTES: usize = MAX_COMPACT_PROGRAMME_TITLE_BYTES;

    pub(crate) fn new(title: Arc<str>, starts_at: DateTime<Utc>, ends_at: DateTime<Utc>) -> Self {
        let (title, title_truncated) = bounded_programme_title(title);
        Self {
            title,
            title_truncated,
            starts_at,
            ends_at,
        }
    }

    /// Returns the bounded normalized title supplied by the EPG Source.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Reports whether the source title exceeded the guide projection bound.
    pub const fn title_truncated(&self) -> bool {
        self.title_truncated
    }

    /// Returns the Programme start instant normalized to UTC.
    pub const fn starts_at(&self) -> DateTime<Utc> {
        self.starts_at
    }

    /// Returns the Programme end instant normalized to UTC.
    pub const fn ends_at(&self) -> DateTime<Utc> {
        self.ends_at
    }
}

fn bounded_programme_title(title: Arc<str>) -> (Arc<str>, bool) {
    if title.len() <= MAX_COMPACT_PROGRAMME_TITLE_BYTES {
        return (title, false);
    }
    let mut end = MAX_COMPACT_PROGRAMME_TITLE_BYTES;
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    (Arc::from(&title[..end]), true)
}

/// One Channel and its Programmes overlapping a bounded guide window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuideWindowChannel {
    channel: ChannelSummary,
    programmes: Arc<[GuideProgramme]>,
    programmes_truncated: bool,
}

impl GuideWindowChannel {
    /// The largest number of overlapping Programmes returned for one Channel.
    pub const MAX_PROGRAMMES: usize = 100;

    pub(crate) fn new(channel: ChannelSummary, mut programmes: Vec<GuideProgramme>) -> Self {
        let programmes_truncated = programmes.len() > Self::MAX_PROGRAMMES;
        programmes.truncate(Self::MAX_PROGRAMMES);
        Self {
            channel,
            programmes: Arc::from(programmes),
            programmes_truncated,
        }
    }

    /// Returns the Channel represented by this guide row.
    pub const fn channel(&self) -> &ChannelSummary {
        &self.channel
    }

    /// Returns the start-ordered Programmes that overlap the requested window.
    pub fn programmes(&self) -> &[GuideProgramme] {
        &self.programmes
    }

    /// Reports whether additional overlapping Programmes exceeded the row cap.
    pub const fn programmes_truncated(&self) -> bool {
        self.programmes_truncated
    }
}

/// Selects a bounded page of Channels and their Programmes in one UTC window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuideWindowQuery {
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    channels: ChannelQuery,
}

impl GuideWindowQuery {
    /// The largest time span accepted by one guide query.
    pub const MAX_HOURS: i64 = 24;
    /// The largest encoded RFC 3339 instant accepted at a public boundary.
    pub const MAX_INSTANT_BYTES: usize = 64;

    /// Parses two untrusted RFC 3339 instants and validates one guide window.
    pub fn parse(
        starts_at: String,
        ends_at: String,
        channels: ChannelQuery,
    ) -> Result<Self, CoreError> {
        let starts_at = parse_guide_instant(starts_at, InputField::GuideWindowStartsAt)?;
        let ends_at = parse_guide_instant(ends_at, InputField::GuideWindowEndsAt)?;
        Self::new(starts_at, ends_at, channels)
    }

    /// Creates a guide query after enforcing a non-empty, bounded UTC interval.
    pub fn new(
        starts_at: DateTime<Utc>,
        ends_at: DateTime<Utc>,
        channels: ChannelQuery,
    ) -> Result<Self, CoreError> {
        if ends_at <= starts_at
            || ends_at.signed_duration_since(starts_at) > chrono::Duration::hours(Self::MAX_HOURS)
        {
            return Err(CoreError::InvalidInput {
                field: InputField::GuideWindowEndsAt,
                reason: InputReason::OutOfRange,
            });
        }
        Ok(Self {
            starts_at,
            ends_at,
            channels,
        })
    }

    /// Returns the inclusive start of the half-open guide interval.
    pub const fn starts_at(&self) -> DateTime<Utc> {
        self.starts_at
    }

    /// Returns the exclusive end of the half-open guide interval.
    pub const fn ends_at(&self) -> DateTime<Utc> {
        self.ends_at
    }

    /// Returns the Channel selection and page request for this window.
    pub const fn channels(&self) -> &ChannelQuery {
        &self.channels
    }
}

fn parse_guide_instant(value: String, field: InputField) -> Result<DateTime<Utc>, CoreError> {
    if value.len() > GuideWindowQuery::MAX_INSTANT_BYTES {
        return Err(CoreError::InvalidInput {
            field,
            reason: InputReason::TooLong {
                max_bytes: GuideWindowQuery::MAX_INSTANT_BYTES,
            },
        });
    }
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| CoreError::InvalidInput {
            field,
            reason: InputReason::InvalidFormat,
        })
}
