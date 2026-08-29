use std::{
    fmt::{self, Debug, Display, Formatter},
    ops::Range,
    sync::Arc,
};

use blake3::Hasher;
use chrono::{DateTime, Utc};
use thiserror::Error;
use url::Url;

const MAX_SOURCE_LOCATION_BYTES: usize = 16 * 1024;

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
    PageLimit,
}

impl Display for InputField {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            InputField::M3u => "m3u",
            InputField::Epg => "epg",
            InputField::PageLimit => "page limit",
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
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SourceKind {
    M3u,
    Epg,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CatalogGeneration(u64);

impl CatalogGeneration {
    pub(crate) const fn initial() -> Self {
        Self(1)
    }

    pub const fn get(self) -> u64 {
        self.0
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

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ChannelId(Arc<str>);

impl ChannelId {
    pub(crate) fn generated(value: String) -> Self {
        Self(Arc::from(value))
    }

    /// Returns the opaque value for an explicit transport projection.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Debug for ChannelId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("ChannelId(<redacted>)")
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

#[derive(Clone)]
pub struct Page<T> {
    generation: CatalogGeneration,
    items: Arc<[T]>,
    range: Range<usize>,
}

impl<T> Page<T> {
    pub(crate) fn first(generation: CatalogGeneration, items: Arc<[T]>, limit: PageLimit) -> Self {
        let range = 0..items.len().min(usize::from(limit.get()));
        Self {
            generation,
            items,
            range,
        }
    }

    pub fn generation(&self) -> CatalogGeneration {
        self.generation
    }

    pub fn items(&self) -> &[T] {
        &self.items[self.range.clone()]
    }
}

impl<T: Debug> Debug for Page<T> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Page")
            .field("generation", &self.generation)
            .field("items", &self.items())
            .finish()
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
    Fresh { validated_at: DateTime<Utc> },
    Unavailable { failure: Option<SafeFailure> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogStatus {
    generation: Option<CatalogGeneration>,
    m3u: SourceState,
    configuration: RedactedSourceConfiguration,
}

impl CatalogStatus {
    pub(crate) fn not_configured() -> Self {
        Self {
            generation: None,
            m3u: SourceState::Unavailable { failure: None },
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
            configuration,
        }
    }

    pub(crate) fn fresh(
        generation: CatalogGeneration,
        configuration: RedactedSourceConfiguration,
        validated_at: DateTime<Utc>,
    ) -> Self {
        Self {
            generation: Some(generation),
            m3u: SourceState::Fresh { validated_at },
            configuration,
        }
    }

    pub fn generation(&self) -> Option<CatalogGeneration> {
        self.generation
    }

    pub fn m3u(&self) -> &SourceState {
        &self.m3u
    }

    pub fn configuration(&self) -> RedactedSourceConfiguration {
        self.configuration
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
    BeginStage,
    WriteStage,
    ReadStage,
    PrepareActivation,
    Activate,
    Discard,
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

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SafeFailure {
    #[error("source access failed")]
    SourceAccess { reason: SourceAccessError },
    #[error("source reading failed")]
    SourceRead { reason: SourceReadError },
    #[error("snapshot operation failed")]
    Snapshot {
        operation: SnapshotOperation,
        reason: StoreError,
    },
    #[error("decoded M3U input exceeds the {limit_bytes}-byte limit")]
    DecodedLimitExceeded { limit_bytes: u64 },
    #[error("M3U input is not valid UTF-8")]
    InvalidEncoding,
    #[error("M3U input has an invalid format")]
    InvalidFormat {
        entry: Option<u32>,
        reason: M3uFailureKind,
    },
    #[error("M3U input contains no playable Channels")]
    NoPlayableChannels,
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
    CatalogUnavailable { status: CatalogStatus },
    #[error("the Channel was not found")]
    ChannelNotFound { id: ChannelId },
}
