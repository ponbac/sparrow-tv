use std::{
    fmt::{self, Debug, Display, Formatter},
    mem::size_of,
    num::NonZeroU64,
    ops::Range,
    sync::Arc,
    time::Duration,
};

use blake3::Hasher;
use chrono::{DateTime, Utc};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;
use url::Url;

const MAX_SOURCE_LOCATION_BYTES: usize = 16 * 1024;
const PAGE_CURSOR_PREFIX: &str = "pc1";
const CATALOG_GENERATION_DOMAIN: &[u8] = b"sparrow-catalog-generation-v1\0";
pub(crate) const CHANNEL_ID_PREFIX: &str = "ch1_";
const CHANNEL_ID_DIGEST_HEX_BYTES: usize = 64;
const MAX_SEARCH_TERM_BYTES: usize = 256;

/// Untrusted source locations as entered at a configuration boundary.
///
/// This type intentionally implements neither `Debug` nor `Display` because it
/// can contain provider credentials.
pub struct SourceConfigurationInput {
    m3u: String,
    epg: Option<String>,
}

impl SourceConfigurationInput {
    pub fn new(m3u: impl Into<String>, epg: Option<impl Into<String>>) -> SourceConfigurationInput {
        SourceConfigurationInput {
            m3u: m3u.into(),
            epg: epg.map(Into::into),
        }
    }
}

/// A validated Source Configuration whose provider locations remain private.
pub struct SourceConfiguration {
    pub(crate) m3u: ConfiguredSource,
    pub(crate) epg: Option<ConfiguredSource>,
    pub(crate) fingerprint: SourceConfigurationFingerprint,
}

impl SourceConfiguration {
    pub(crate) fn parse(input: SourceConfigurationInput) -> Result<Self, CoreError> {
        let m3u_location = SecretSourceLocation::parse_required(InputField::M3u, input.m3u)?;
        let epg_location = SecretSourceLocation::parse_optional(InputField::Epg, input.epg)?;
        let fingerprint =
            SourceConfigurationFingerprint::for_configuration(&m3u_location, epg_location.as_ref());

        Ok(Self {
            m3u: ConfiguredSource::new(SourceKind::M3u, m3u_location),
            epg: epg_location.map(|location| ConfiguredSource::new(SourceKind::Epg, location)),
            fingerprint,
        })
    }

    pub(crate) fn has_epg(&self) -> bool {
        self.epg.is_some()
    }

    pub(crate) fn redacted(&self) -> RedactedSourceConfiguration {
        RedactedSourceConfiguration {
            configured: true,
            epg_configured: self.has_epg(),
        }
    }

    /// Derives a restart-stable generation from every input that contributes
    /// to one immutable Channel Catalog.
    pub(crate) fn catalog_generation(
        &self,
        m3u_checksum: &[u8; 32],
        epg_checksum: Option<&[u8; 32]>,
    ) -> CatalogGeneration {
        CatalogGeneration::for_content(&self.fingerprint, m3u_checksum, epg_checksum)
    }
}

impl Debug for SourceConfiguration {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceConfiguration")
            .field("m3u", &Redacted)
            .field("epg_configured", &self.has_epg())
            .field("fingerprint", &Redacted)
            .finish()
    }
}

#[derive(Clone)]
pub(crate) struct SecretSourceLocation(String);

impl SecretSourceLocation {
    fn parse_required(field: InputField, input: String) -> Result<Self, CoreError> {
        Self::parse_non_empty(field, input).and_then(|location| {
            location.ok_or(CoreError::InvalidInput {
                field,
                reason: InputReason::Required,
            })
        })
    }

    fn parse_optional(field: InputField, input: Option<String>) -> Result<Option<Self>, CoreError> {
        match input {
            Some(input) => Self::parse_non_empty(field, input),
            None => Ok(None),
        }
    }

    fn parse_non_empty(field: InputField, input: String) -> Result<Option<Self>, CoreError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        if trimmed.len() > MAX_SOURCE_LOCATION_BYTES {
            return Err(CoreError::InvalidInput {
                field,
                reason: InputReason::TooLong {
                    max_bytes: MAX_SOURCE_LOCATION_BYTES,
                },
            });
        }
        if trimmed.chars().any(char::is_control) {
            return Err(CoreError::InvalidInput {
                field,
                reason: InputReason::ContainsControlCharacter,
            });
        }
        let supported_location = Url::parse(trimmed).ok().is_some_and(|location| {
            matches!(location.scheme(), "http" | "https") && location.has_host()
        });
        if !supported_location {
            return Err(CoreError::InvalidInput {
                field,
                reason: InputReason::UnsupportedLocation,
            });
        }

        Ok(Some(Self(trimmed.to_owned())))
    }

    pub(crate) fn expose_for_access(&self) -> &str {
        &self.0
    }
}

impl Debug for SecretSourceLocation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Redacted.fmt(formatter)
    }
}

#[derive(Clone)]
pub(crate) struct ConfiguredSource {
    location: SecretSourceLocation,
    fingerprint: SourceFingerprint,
}

impl ConfiguredSource {
    fn new(kind: SourceKind, location: SecretSourceLocation) -> Self {
        let fingerprint = SourceFingerprint::for_source(kind, &location);
        Self {
            location,
            fingerprint,
        }
    }

    pub(crate) fn location(&self) -> &SecretSourceLocation {
        &self.location
    }

    pub(crate) fn fingerprint(&self) -> SourceFingerprint {
        self.fingerprint
    }
}

impl Debug for ConfiguredSource {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfiguredSource")
            .field("location", &Redacted)
            .field("fingerprint", &Redacted)
            .finish()
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) struct SourceFingerprint([u8; 32]);

impl SourceFingerprint {
    fn for_source(kind: SourceKind, location: &SecretSourceLocation) -> Self {
        let mut hasher = Hasher::new();
        hasher.update(b"sparrow-source-v1\0");
        hasher.update(&[match kind {
            SourceKind::M3u => 0,
            SourceKind::Epg => 1,
        }]);
        hash_field(&mut hasher, location.expose_for_access().as_bytes());
        Self(*hasher.finalize().as_bytes())
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Debug for SourceFingerprint {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Redacted.fmt(formatter)
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) struct SourceConfigurationFingerprint([u8; 32]);

impl SourceConfigurationFingerprint {
    fn for_configuration(m3u: &SecretSourceLocation, epg: Option<&SecretSourceLocation>) -> Self {
        let mut hasher = Hasher::new();
        hasher.update(b"sparrow-source-configuration-v1\0");
        hash_field(&mut hasher, m3u.expose_for_access().as_bytes());
        match epg {
            Some(epg) => {
                hasher.update(&[1]);
                hash_field(&mut hasher, epg.expose_for_access().as_bytes());
            }
            None => {
                hasher.update(&[0]);
            }
        };
        Self(*hasher.finalize().as_bytes())
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Debug for SourceConfigurationFingerprint {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Redacted.fmt(formatter)
    }
}

fn hash_field(hasher: &mut Hasher, field: &[u8]) {
    hasher.update(&(field.len() as u64).to_le_bytes());
    hasher.update(field);
}

struct Redacted;

impl Debug for Redacted {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputField {
    M3u,
    Epg,
    ChannelId,
    SearchTerm,
    PageLimit,
    PageCursor,
}

impl Display for InputField {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            InputField::M3u => "m3u",
            InputField::Epg => "epg",
            InputField::ChannelId => "channel ID",
            InputField::SearchTerm => "search term",
            InputField::PageLimit => "page limit",
            InputField::PageCursor => "page cursor",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InputReason {
    #[error("a value is required")]
    Required,
    #[error("the value exceeds the {max_bytes}-byte limit")]
    TooLong { max_bytes: usize },
    #[error("control characters are not allowed")]
    ContainsControlCharacter,
    #[error("the source location is unsupported")]
    UnsupportedLocation,
    #[error("the value is outside the supported range")]
    OutOfRange,
    #[error("the value has an invalid format")]
    InvalidFormat,
    #[error("the cursor belongs to a different query")]
    CursorQueryMismatch,
    #[error("the cursor position is outside the result set")]
    CursorPositionOutOfRange,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SourceKind {
    M3u,
    Epg,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CatalogGeneration(NonZeroU64);

impl CatalogGeneration {
    fn for_content(
        configuration: &SourceConfigurationFingerprint,
        m3u_checksum: &[u8; 32],
        epg_checksum: Option<&[u8; 32]>,
    ) -> Self {
        let mut hasher = Hasher::new();
        hasher.update(CATALOG_GENERATION_DOMAIN);
        hash_field(&mut hasher, configuration.as_bytes());
        hasher.update(b"m3u\0");
        hash_field(&mut hasher, m3u_checksum);
        hasher.update(b"epg\0");
        match epg_checksum {
            Some(checksum) => {
                hasher.update(&[1]);
                hash_field(&mut hasher, checksum);
            }
            None => {
                hasher.update(&[0]);
            }
        }
        let digest = hasher.finalize();
        let mut encoded = [0_u8; size_of::<u64>()];
        encoded.copy_from_slice(&digest.as_bytes()[..size_of::<u64>()]);
        let value = NonZeroU64::new(u64::from_le_bytes(encoded)).unwrap_or(NonZeroU64::MIN);
        Self(value)
    }

    const fn from_cursor(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PageLimit(u16);

impl PageLimit {
    pub const MAX: u16 = 100;

    pub fn new(value: u16) -> Result<Self, CoreError> {
        if value == 0 || value > Self::MAX {
            return Err(CoreError::InvalidInput {
                field: InputField::PageLimit,
                reason: InputReason::OutOfRange,
            });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) struct CursorQueryHash([u8; 32]);

impl CursorQueryHash {
    pub(crate) const fn new(value: [u8; 32]) -> Self {
        Self(value)
    }

    fn encode(self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }

    fn parse(value: &str) -> Option<Self> {
        blake3::Hash::from_hex(value)
            .ok()
            .map(|hash| Self(*hash.as_bytes()))
    }
}

/// An opaque continuation token for one catalog generation and query shape.
///
/// The token contains no source data. Transport adapters should parse incoming
/// strings with [`PageCursor::parse`] and project outgoing values explicitly
/// with [`PageCursor::as_str`].
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct PageCursor {
    value: Arc<str>,
    generation: CatalogGeneration,
    offset: usize,
    query: CursorQueryHash,
}

impl PageCursor {
    pub(crate) fn generated(
        generation: CatalogGeneration,
        offset: usize,
        query: CursorQueryHash,
    ) -> Self {
        debug_assert!(offset > 0);
        let value = Self::encode(generation, offset, query);
        Self {
            value: Arc::from(value),
            generation,
            offset,
            query,
        }
    }

    /// Parses an untrusted transport value into a canonical opaque cursor.
    pub fn parse(value: impl Into<String>) -> Result<Self, CoreError> {
        let value = value.into();
        let invalid = || CoreError::InvalidInput {
            field: InputField::PageCursor,
            reason: InputReason::InvalidFormat,
        };
        let mut fields = value.split('.');
        let prefix = fields.next().ok_or_else(invalid)?;
        let generation = fields
            .next()
            .and_then(|field| field.parse::<u64>().ok())
            .and_then(CatalogGeneration::from_cursor)
            .ok_or_else(invalid)?;
        let offset = fields
            .next()
            .and_then(|field| field.parse::<u64>().ok())
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(invalid)?;
        let query = fields
            .next()
            .and_then(CursorQueryHash::parse)
            .ok_or_else(invalid)?;
        if prefix != PAGE_CURSOR_PREFIX || fields.next().is_some() {
            return Err(invalid());
        }
        let canonical = Self::encode(generation, offset, query);
        if value != canonical {
            return Err(invalid());
        }

        Ok(Self {
            value: Arc::from(value),
            generation,
            offset,
            query,
        })
    }

    /// Returns the opaque value for an explicit transport projection.
    pub fn as_str(&self) -> &str {
        &self.value
    }

    fn encode(generation: CatalogGeneration, offset: usize, query: CursorQueryHash) -> String {
        format!(
            "{PAGE_CURSOR_PREFIX}.{}.{}.{}",
            generation.get(),
            offset,
            query.encode()
        )
    }
}

impl Debug for PageCursor {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("PageCursor(<opaque>)")
    }
}

/// A bounded request for the first or a subsequent page of one query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageRequest {
    cursor: Option<PageCursor>,
    limit: PageLimit,
}

impl PageRequest {
    /// Creates a page request from an optional parsed cursor and bounded limit.
    pub const fn new(cursor: Option<PageCursor>, limit: PageLimit) -> Self {
        Self { cursor, limit }
    }

    /// Creates a request for the first page.
    pub const fn first(limit: PageLimit) -> Self {
        Self::new(None, limit)
    }

    /// Creates a request continuing after a cursor returned by an earlier page.
    pub const fn after(cursor: PageCursor, limit: PageLimit) -> Self {
        Self::new(Some(cursor), limit)
    }

    /// Returns the continuation cursor, when this is not a first-page request.
    pub const fn cursor(&self) -> Option<&PageCursor> {
        self.cursor.as_ref()
    }

    /// Returns the bounded maximum number of items requested.
    pub const fn limit(&self) -> PageLimit {
        self.limit
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ChannelId(Arc<str>);

impl ChannelId {
    pub(crate) fn generated(value: String) -> Self {
        debug_assert!(is_canonical_channel_id(&value));
        Self(Arc::from(value))
    }

    /// Parses an untrusted transport value into a canonical opaque identifier.
    pub fn parse(value: impl Into<String>) -> Result<Self, CoreError> {
        let value = value.into();
        if !is_canonical_channel_id(&value) {
            return Err(CoreError::InvalidInput {
                field: InputField::ChannelId,
                reason: InputReason::InvalidFormat,
            });
        }
        Ok(Self(Arc::from(value)))
    }

    /// Returns the opaque value for an explicit transport projection.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_canonical_channel_id(value: &str) -> bool {
    value.strip_prefix(CHANNEL_ID_PREFIX).is_some_and(|digest| {
        digest.len() == CHANNEL_ID_DIGEST_HEX_BYTES
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

impl Debug for ChannelId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("ChannelId(<redacted>)")
    }
}

/// A bounded canonical search term shared by Channel and Programme search.
///
/// Incoming text is compatibility-normalized, lowercased, and has runs of
/// Unicode whitespace collapsed. The canonical value is intentionally omitted
/// from diagnostics because callers may accidentally submit private text.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct SearchTerm(Arc<str>);

impl SearchTerm {
    /// Refines untrusted decoded text into a non-empty canonical search term.
    pub fn parse(value: impl Into<String>) -> Result<Self, CoreError> {
        let value = value.into();
        if value.len() > MAX_SEARCH_TERM_BYTES {
            return Err(CoreError::InvalidInput {
                field: InputField::SearchTerm,
                reason: InputReason::TooLong {
                    max_bytes: MAX_SEARCH_TERM_BYTES,
                },
            });
        }

        let normalized = normalize_search_text(&value);
        if normalized.is_empty() {
            return Err(CoreError::InvalidInput {
                field: InputField::SearchTerm,
                reason: InputReason::Required,
            });
        }
        if normalized.len() > MAX_SEARCH_TERM_BYTES {
            return Err(CoreError::InvalidInput {
                field: InputField::SearchTerm,
                reason: InputReason::TooLong {
                    max_bytes: MAX_SEARCH_TERM_BYTES,
                },
            });
        }

        Ok(Self(Arc::from(normalized)))
    }

    /// Returns the canonical term for an explicit transport projection.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Debug for SearchTerm {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("SearchTerm(<redacted>)")
    }
}

pub(crate) fn normalize_search_text(value: &str) -> String {
    let compatibility_normalized = value.nfkc().collect::<String>();
    let mut normalized = String::with_capacity(compatibility_normalized.len());

    for (index, word) in compatibility_normalized.split_whitespace().enumerate() {
        if index != 0 {
            normalized.push(' ');
        }
        normalized.extend(word.chars().flat_map(char::to_lowercase));
    }

    normalized
}

/// One source-derived Channel Group and the number of Channels it contains.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelGroupView {
    name: Arc<str>,
    channel_count: u32,
}

impl ChannelGroupView {
    pub(crate) fn new(name: Arc<str>, channel_count: u32) -> Self {
        Self {
            name,
            channel_count,
        }
    }

    /// Returns the normalized presentation name from the M3U Source.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the number of Channels in this group for the page generation.
    pub const fn channel_count(&self) -> u32 {
        self.channel_count
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelSummary {
    id: ChannelId,
    name: Arc<str>,
    group: Arc<str>,
}

impl ChannelSummary {
    pub(crate) fn new(id: ChannelId, name: Arc<str>, group: Arc<str>) -> Self {
        Self { id, name, group }
    }

    pub fn id(&self) -> &ChannelId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn group(&self) -> &str {
        &self.group
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelDetails {
    id: ChannelId,
    name: Arc<str>,
    group: Arc<str>,
}

impl ChannelDetails {
    pub(crate) fn new(id: ChannelId, name: Arc<str>, group: Arc<str>) -> Self {
        Self { id, name, group }
    }

    pub fn id(&self) -> &ChannelId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn group(&self) -> &str {
        &self.group
    }
}

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

/// Selects all Channels or the Channels in one exact source-derived group.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelQuery {
    group: Option<Arc<str>>,
    page: PageRequest,
}

/// Selects one Channel's deterministic, paginated Programme schedule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduleQuery {
    channel_id: ChannelId,
    page: PageRequest,
}

/// Selects independently bounded Channel and Programme search pages.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequest {
    term: SearchTerm,
    channels: PageRequest,
    programmes: PageRequest,
}

impl SearchRequest {
    pub const fn new(term: SearchTerm, channels: PageRequest, programmes: PageRequest) -> Self {
        Self {
            term,
            channels,
            programmes,
        }
    }

    pub const fn term(&self) -> &SearchTerm {
        &self.term
    }

    pub const fn channels(&self) -> &PageRequest {
        &self.channels
    }

    pub const fn programmes(&self) -> &PageRequest {
        &self.programmes
    }
}

impl ScheduleQuery {
    /// Creates a bounded schedule query for one parsed Channel Identifier.
    pub const fn new(channel_id: ChannelId, page: PageRequest) -> Self {
        Self { channel_id, page }
    }

    /// Returns the Channel whose Programme schedule is requested.
    pub const fn channel_id(&self) -> &ChannelId {
        &self.channel_id
    }

    /// Returns this query's bounded page request.
    pub const fn page(&self) -> &PageRequest {
        &self.page
    }
}

impl ChannelQuery {
    /// Creates a paginated query across every Channel.
    pub const fn all(page: PageRequest) -> Self {
        Self { group: None, page }
    }

    /// Creates a paginated query for an exact Channel Group name.
    pub fn in_group(group: impl Into<String>, page: PageRequest) -> Self {
        Self {
            group: Some(Arc::from(group.into())),
            page,
        }
    }

    /// Returns the exact Channel Group filter, when present.
    pub fn group(&self) -> Option<&str> {
        self.group.as_deref()
    }

    /// Returns this query's bounded page request.
    pub const fn page(&self) -> &PageRequest {
        &self.page
    }
}

#[derive(Clone)]
pub struct Page<T> {
    generation: CatalogGeneration,
    items: Arc<[T]>,
    range: Range<usize>,
    next: Option<PageCursor>,
}

impl<T> Page<T> {
    pub(crate) fn from_request(
        generation: CatalogGeneration,
        items: Arc<[T]>,
        collection: Range<usize>,
        request: &PageRequest,
        query: CursorQueryHash,
    ) -> Result<Self, CoreError> {
        debug_assert!(collection.start <= collection.end);
        debug_assert!(collection.end <= items.len());

        let window = page_window(generation, collection.len(), request, query)?;
        let range = (collection.start + window.range.start)..(collection.start + window.range.end);

        Ok(Self {
            generation,
            items,
            range,
            next: window.next,
        })
    }

    /// Builds a page by shallow-cloning only the selected bounded window.
    pub(crate) fn from_selection(
        generation: CatalogGeneration,
        source: &[T],
        selection: &[usize],
        request: &PageRequest,
        query: CursorQueryHash,
    ) -> Result<Self, CoreError>
    where
        T: Clone,
    {
        debug_assert!(selection.iter().all(|index| *index < source.len()));
        let window = page_window(generation, selection.len(), request, query)?;
        let items: Arc<[T]> = Arc::from(
            selection[window.range]
                .iter()
                .map(|index| source[*index].clone())
                .collect::<Vec<_>>(),
        );
        let range = 0..items.len();

        Ok(Self {
            generation,
            items,
            range,
            next: window.next,
        })
    }

    /// Returns the immutable catalog generation backing this page.
    pub fn generation(&self) -> CatalogGeneration {
        self.generation
    }

    /// Returns the items in this bounded page without cloning the catalog.
    pub fn items(&self) -> &[T] {
        &self.items[self.range.clone()]
    }

    /// Returns the opaque continuation cursor when more items remain.
    pub const fn next(&self) -> Option<&PageCursor> {
        self.next.as_ref()
    }
}

struct PageWindow {
    range: Range<usize>,
    next: Option<PageCursor>,
}

fn page_window(
    generation: CatalogGeneration,
    collection_len: usize,
    request: &PageRequest,
    query: CursorQueryHash,
) -> Result<PageWindow, CoreError> {
    let offset = match request.cursor() {
        None => 0,
        Some(cursor) if cursor.generation != generation => {
            return Err(CoreError::StaleCursor {
                current: generation,
            });
        }
        Some(cursor) if cursor.query != query => {
            return Err(CoreError::InvalidInput {
                field: InputField::PageCursor,
                reason: InputReason::CursorQueryMismatch,
            });
        }
        Some(cursor) => cursor.offset,
    };
    if offset > 0 && offset >= collection_len {
        return Err(CoreError::InvalidInput {
            field: InputField::PageCursor,
            reason: InputReason::CursorPositionOutOfRange,
        });
    }
    let end = offset
        .saturating_add(usize::from(request.limit().get()))
        .min(collection_len);
    let next = (end < collection_len).then(|| PageCursor::generated(generation, end, query));

    Ok(PageWindow {
        range: offset..end,
        next,
    })
}

impl<T: Debug> Debug for Page<T> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Page")
            .field("generation", &self.generation)
            .field("items", &self.items())
            .field("next", &self.next)
            .finish()
    }
}

/// Independently paginated Channel and Programme matches from one generation.
#[derive(Clone, Debug)]
pub struct SearchResults {
    channels: Page<ChannelSummary>,
    programmes: Page<ProgrammeSummary>,
}

impl SearchResults {
    pub(crate) fn new(channels: Page<ChannelSummary>, programmes: Page<ProgrammeSummary>) -> Self {
        debug_assert_eq!(channels.generation(), programmes.generation());
        Self {
            channels,
            programmes,
        }
    }

    /// Returns the immutable catalog generation shared by both result pages.
    pub fn generation(&self) -> CatalogGeneration {
        self.channels.generation()
    }

    pub const fn channels(&self) -> &Page<ChannelSummary> {
        &self.channels
    }

    pub const fn programmes(&self) -> &Page<ProgrammeSummary> {
        &self.programmes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RedactedSourceConfiguration {
    configured: bool,
    epg_configured: bool,
}

impl RedactedSourceConfiguration {
    pub(crate) const fn not_configured() -> Self {
        Self {
            configured: false,
            epg_configured: false,
        }
    }

    pub const fn is_configured(self) -> bool {
        self.configured
    }

    pub const fn has_epg(self) -> bool {
        self.epg_configured
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceState {
    Fresh {
        validated_at: DateTime<Utc>,
    },
    Stale {
        validated_at: DateTime<Utc>,
        next_attempt_at: Option<DateTime<Utc>>,
    },
    Unavailable {
        failure: Option<SafeFailure>,
    },
    Refreshing {
        validated_at: Option<DateTime<Utc>>,
        started_at: DateTime<Utc>,
    },
    Deferred {
        validated_at: Option<DateTime<Utc>>,
        deferred_at: DateTime<Utc>,
    },
    Failed {
        validated_at: Option<DateTime<Utc>>,
        failure: SafeFailure,
        next_attempt_at: DateTime<Utc>,
    },
}

/// Bounded safe evidence produced while recovering one Source Snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotRecoveryDiagnostic {
    rejected: Arc<[SafeFailure]>,
    fallback_adopted: bool,
}

impl SnapshotRecoveryDiagnostic {
    pub(crate) const MAX_FAILURES: usize = 8;

    pub(crate) fn new(mut rejected: Vec<SafeFailure>, fallback_adopted: bool) -> Option<Self> {
        rejected.truncate(Self::MAX_FAILURES);
        (!rejected.is_empty() || fallback_adopted).then(|| Self {
            rejected: Arc::from(rejected),
            fallback_adopted,
        })
    }

    pub fn rejected(&self) -> &[SafeFailure] {
        &self.rejected
    }

    pub fn fallback_adopted(&self) -> bool {
        self.fallback_adopted
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogStatus {
    generation: Option<CatalogGeneration>,
    m3u: SourceState,
    epg: Option<SourceState>,
    m3u_recovery: Option<SnapshotRecoveryDiagnostic>,
    epg_recovery: Option<SnapshotRecoveryDiagnostic>,
    configuration: RedactedSourceConfiguration,
}

impl CatalogStatus {
    pub(crate) fn not_configured() -> Self {
        Self {
            generation: None,
            m3u: SourceState::Unavailable { failure: None },
            epg: None,
            m3u_recovery: None,
            epg_recovery: None,
            configuration: RedactedSourceConfiguration::not_configured(),
        }
    }

    pub(crate) fn unavailable(
        configuration: RedactedSourceConfiguration,
        failure: Option<SafeFailure>,
    ) -> Self {
        Self {
            generation: None,
            m3u: SourceState::Unavailable { failure },
            epg: configuration
                .has_epg()
                .then_some(SourceState::Unavailable { failure: None }),
            m3u_recovery: None,
            epg_recovery: None,
            configuration,
        }
    }

    pub(crate) fn published(
        generation: CatalogGeneration,
        configuration: RedactedSourceConfiguration,
        m3u: SourceState,
        epg: Option<SourceState>,
        m3u_recovery: Option<SnapshotRecoveryDiagnostic>,
        epg_recovery: Option<SnapshotRecoveryDiagnostic>,
    ) -> Self {
        Self {
            generation: Some(generation),
            m3u,
            epg,
            m3u_recovery,
            epg_recovery,
            configuration,
        }
    }

    pub fn generation(&self) -> Option<CatalogGeneration> {
        self.generation
    }

    pub fn m3u(&self) -> &SourceState {
        &self.m3u
    }

    /// Returns independent EPG Source status, or `None` when no EPG Source is configured.
    pub fn epg(&self) -> Option<&SourceState> {
        self.epg.as_ref()
    }

    pub fn configuration(&self) -> RedactedSourceConfiguration {
        self.configuration
    }

    /// Returns bounded safe Snapshot recovery evidence for one Source.
    pub fn recovery(&self, kind: SourceKind) -> Option<&SnapshotRecoveryDiagnostic> {
        match kind {
            SourceKind::M3u => self.m3u_recovery.as_ref(),
            SourceKind::Epg => self.epg_recovery.as_ref(),
        }
    }

    pub(crate) fn set_recovery(
        &mut self,
        kind: SourceKind,
        diagnostic: Option<SnapshotRecoveryDiagnostic>,
    ) {
        match kind {
            SourceKind::M3u => self.m3u_recovery = diagnostic,
            SourceKind::Epg => self.epg_recovery = diagnostic,
        }
    }

    pub(crate) fn set_source_state(&mut self, kind: SourceKind, state: SourceState) {
        match kind {
            SourceKind::M3u => self.m3u = state,
            SourceKind::Epg => {
                if self.configuration.has_epg() {
                    self.epg = Some(state);
                }
            }
        }
    }

    pub(crate) fn set_generation(&mut self, generation: Option<CatalogGeneration>) {
        self.generation = generation;
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SourceAccessError {
    #[error("the source is unavailable")]
    Unavailable,
    #[error("source access was rejected")]
    Rejected,
    #[error("source access timed out")]
    TimedOut,
    #[error("the source returned an invalid response")]
    InvalidResponse,
}

/// A safe source-access failure plus an optional parsed retry delay.
///
/// Raw headers never cross the source-access seam.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("{reason}")]
pub struct SourceAccessFailure {
    reason: SourceAccessError,
    retry_after: Option<Duration>,
}

impl SourceAccessFailure {
    pub const fn new(reason: SourceAccessError) -> Self {
        Self {
            reason,
            retry_after: None,
        }
    }

    pub const fn with_retry_after(reason: SourceAccessError, delay: Duration) -> Self {
        Self {
            reason,
            retry_after: Some(delay),
        }
    }

    pub const fn reason(self) -> SourceAccessError {
        self.reason
    }

    pub const fn retry_after(self) -> Option<Duration> {
        self.retry_after
    }
}

impl From<SourceAccessError> for SourceAccessFailure {
    fn from(reason: SourceAccessError) -> Self {
        Self::new(reason)
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SourceReadError {
    #[error("the source body was interrupted")]
    Interrupted,
    #[error("the source body is invalid")]
    InvalidBody,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum StoreError {
    #[error("snapshot storage is unavailable")]
    Unavailable,
    #[error("snapshot storage has insufficient capacity")]
    Capacity,
    #[error("snapshot storage rejected corrupt data")]
    Corrupt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotOperation {
    ScanCandidates,
    OpenCandidate,
    AdoptCandidate,
    RevalidateCandidate,
    BeginStage,
    WriteStage,
    ReadStage,
    PrepareActivation,
    Activate,
    Discard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotRecoveryReason {
    MissingActivePointer,
    CorruptActivePointer,
    MissingManifest,
    CorruptManifest,
    MissingPayload,
    SourceMismatch,
    LengthMismatch,
    ChecksumMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum M3uFailureKind {
    MissingHeader,
    MalformedMetadata,
    UnterminatedQuote,
    IncompleteEntry,
    EmptyName,
    UnexpectedLocation,
    UnsupportedPlaybackSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EpgFailureKind {
    MalformedXml,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SafeFailure {
    #[error("source access failed")]
    SourceAccess {
        kind: SourceKind,
        reason: SourceAccessError,
        retry_after: Option<Duration>,
    },
    #[error("source reading failed")]
    SourceRead {
        kind: SourceKind,
        reason: SourceReadError,
    },
    #[error("snapshot operation failed")]
    Snapshot {
        kind: SourceKind,
        operation: SnapshotOperation,
        reason: StoreError,
    },
    #[error("snapshot recovery rejected a candidate")]
    SnapshotRecovery {
        kind: SourceKind,
        reason: SnapshotRecoveryReason,
    },
    #[error("decoded source input exceeds the {limit_bytes}-byte limit")]
    DecodedLimitExceeded { kind: SourceKind, limit_bytes: u64 },
    #[error("source input is not valid UTF-8")]
    InvalidEncoding { kind: SourceKind },
    #[error("M3U input has an invalid format")]
    InvalidFormat {
        entry: Option<u32>,
        reason: M3uFailureKind,
    },
    #[error("M3U input contains no playable Channels")]
    NoPlayableChannels,
    #[error("EPG input has an invalid format")]
    InvalidEpgFormat { reason: EpgFailureKind },
    #[error("EPG input contains no Channel records")]
    NoEpgChannels,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CoreError {
    #[error("invalid {field}: {reason}")]
    InvalidInput {
        field: InputField,
        reason: InputReason,
    },
    #[error("a Source Configuration is required")]
    NotConfigured,
    #[error("the Channel Catalog is unavailable")]
    CatalogUnavailable { status: Box<CatalogStatus> },
    #[error("the Channel was not found")]
    ChannelNotFound { id: ChannelId },
    #[error("the page cursor is stale")]
    StaleCursor { current: CatalogGeneration },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RefreshTrigger {
    Startup,
    Resume,
    FreshnessDeadline,
    Manual,
}

impl RefreshTrigger {
    pub(crate) const fn is_manual(self) -> bool {
        matches!(self, Self::Manual)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleSignal {
    Started,
    Resumed,
    Suspended,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RefreshSkipReason {
    Fresh,
    Backoff,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefreshOutcome {
    NotConfigured,
    Updated {
        validated_at: DateTime<Utc>,
    },
    NotModified {
        validated_at: DateTime<Utc>,
    },
    Skipped {
        reason: RefreshSkipReason,
        next_attempt_at: DateTime<Utc>,
    },
    Failed {
        failure: SafeFailure,
        next_attempt_at: DateTime<Utc>,
    },
}

impl RefreshOutcome {
    pub fn next_attempt_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Skipped {
                next_attempt_at, ..
            }
            | Self::Failed {
                next_attempt_at, ..
            } => Some(*next_attempt_at),
            Self::NotConfigured | Self::Updated { .. } | Self::NotModified { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshReport {
    trigger: RefreshTrigger,
    m3u: RefreshOutcome,
    epg: Option<RefreshOutcome>,
    status: CatalogStatus,
}

impl RefreshReport {
    pub(crate) fn new(
        trigger: RefreshTrigger,
        m3u: RefreshOutcome,
        epg: Option<RefreshOutcome>,
        status: CatalogStatus,
    ) -> Self {
        Self {
            trigger,
            m3u,
            epg,
            status,
        }
    }

    pub const fn trigger(&self) -> RefreshTrigger {
        self.trigger
    }

    pub const fn m3u(&self) -> &RefreshOutcome {
        &self.m3u
    }

    pub const fn epg(&self) -> Option<&RefreshOutcome> {
        self.epg.as_ref()
    }

    pub const fn status(&self) -> &CatalogStatus {
        &self.status
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CoreEvent {
    CatalogStatusChanged {
        status: CatalogStatus,
    },
    CatalogPublished {
        generation: CatalogGeneration,
    },
    RefreshCompleted {
        kind: SourceKind,
        outcome: RefreshOutcome,
    },
}
