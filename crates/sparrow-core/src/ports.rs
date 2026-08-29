use std::{
    fmt::{self, Debug, Formatter},
    io::BufRead,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_core::Stream;
use thiserror::Error;

use crate::domain::{
    SecretSourceLocation, SnapshotRecoveryReason, SourceAccessFailure, SourceConfiguration,
    SourceFingerprint, SourceKind, SourceReadError, StoreError,
};

pub type SourceByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, SourceReadError>> + Send + 'static>>;

#[async_trait]
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;

    /// Waits until this clock reaches `deadline`.
    async fn wait_until(&self, deadline: DateTime<Utc>);
}

pub struct SystemClock;

const MAX_SYSTEM_CLOCK_WAIT: Duration = Duration::from_secs(60 * 60);

#[async_trait]
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    async fn wait_until(&self, deadline: DateTime<Utc>) {
        loop {
            let delay = deadline
                .signed_duration_since(Utc::now())
                .to_std()
                .unwrap_or(Duration::ZERO);
            if delay.is_zero() {
                return;
            }
            tokio::time::sleep(delay.min(MAX_SYSTEM_CLOCK_WAIT)).await;
        }
    }
}

#[derive(Clone)]
pub struct CoreAdapters {
    source_access: Arc<dyn SourceAccess>,
    snapshot_store: Arc<dyn SnapshotStore>,
    clock: Arc<dyn Clock>,
}

impl CoreAdapters {
    pub fn new(
        source_access: Arc<dyn SourceAccess>,
        snapshot_store: Arc<dyn SnapshotStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            source_access,
            snapshot_store,
            clock,
        }
    }

    pub(crate) fn source_access(&self) -> &dyn SourceAccess {
        self.source_access.as_ref()
    }

    pub(crate) fn snapshot_store(&self) -> &dyn SnapshotStore {
        self.snapshot_store.as_ref()
    }

    pub(crate) fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    pub(crate) fn clock_arc(&self) -> Arc<dyn Clock> {
        Arc::clone(&self.clock)
    }
}

pub struct SourceRequest {
    kind: SourceKind,
    location: SecretSourceLocation,
    validators: PrivateSourceValidators,
}

impl SourceRequest {
    pub(crate) fn m3u(
        configuration: &SourceConfiguration,
        validators: PrivateSourceValidators,
    ) -> Self {
        Self {
            kind: SourceKind::M3u,
            location: configuration.m3u.location().clone(),
            validators,
        }
    }

    pub(crate) fn epg(
        configuration: &SourceConfiguration,
        validators: PrivateSourceValidators,
    ) -> Option<Self> {
        configuration.epg.as_ref().map(|source| Self {
            kind: SourceKind::Epg,
            location: source.location().clone(),
            validators,
        })
    }

    pub fn kind(&self) -> SourceKind {
        self.kind
    }

    /// Exposes a source location only to the privileged source-access adapter.
    /// Callers must not log, serialize, or retain the returned value.
    pub fn expose_location_for_access(&self) -> &str {
        self.location.expose_for_access()
    }

    /// Exposes conditional validators only to the privileged source adapter.
    pub fn validators(&self) -> &PrivateSourceValidators {
        &self.validators
    }
}

impl Debug for SourceRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceRequest")
            .field("kind", &self.kind)
            .field("location", &"<redacted>")
            .field("validators", &self.validators)
            .finish()
    }
}

pub struct SourceResponse {
    inner: SourceResponseInner,
}

impl SourceResponse {
    pub fn new(declared_decoded_length: Option<u64>, decoded_body: SourceByteStream) -> Self {
        Self::modified(
            declared_decoded_length,
            decoded_body,
            PrivateSourceValidators::default(),
        )
    }

    /// Compatibility constructor for a modified response with validators.
    pub fn with_validators(
        declared_decoded_length: Option<u64>,
        decoded_body: SourceByteStream,
        validators: PrivateSourceValidators,
    ) -> Self {
        Self::modified(declared_decoded_length, decoded_body, validators)
    }

    pub fn modified(
        declared_decoded_length: Option<u64>,
        decoded_body: SourceByteStream,
        validators: PrivateSourceValidators,
    ) -> Self {
        Self {
            inner: SourceResponseInner::Modified {
                declared_decoded_length,
                decoded_body,
                validators,
            },
        }
    }

    /// Constructs a not-modified response. A body is impossible by construction.
    pub fn not_modified(validators: PrivateSourceValidators) -> Self {
        Self {
            inner: SourceResponseInner::NotModified { validators },
        }
    }

    pub(crate) fn into_inner(self) -> SourceResponseInner {
        self.inner
    }
}

pub(crate) enum SourceResponseInner {
    Modified {
        declared_decoded_length: Option<u64>,
        decoded_body: SourceByteStream,
        validators: PrivateSourceValidators,
    },
    NotModified {
        validators: PrivateSourceValidators,
    },
}

#[async_trait]
pub trait SourceAccess: Send + Sync {
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessFailure>;
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct SourceKey([u8; 32]);

impl SourceKey {
    fn from_fingerprint(fingerprint: SourceFingerprint) -> Self {
        Self(*fingerprint.as_bytes())
    }

    /// Returns the opaque key bytes required by a snapshot adapter.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Debug for SourceKey {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("SourceKey(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct SnapshotSource {
    kind: SourceKind,
    key: SourceKey,
}

impl Debug for SnapshotSource {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotSource")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

const MAX_PRIVATE_VALIDATOR_BYTES: usize = 8 * 1024;

/// Bounded HTTP validators retained privately alongside one Source Snapshot.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct PrivateSourceValidators {
    etag: Option<Arc<str>>,
    last_modified: Option<Arc<str>>,
}

impl PrivateSourceValidators {
    pub fn parse(
        etag: Option<String>,
        last_modified: Option<String>,
    ) -> Result<Self, PrivateValidatorError> {
        Ok(Self {
            etag: refine_validator(etag)?,
            last_modified: refine_validator(last_modified)?,
        })
    }

    /// Exposes the ETag only to privileged provider and snapshot adapters.
    pub fn expose_etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    /// Exposes Last-Modified only to privileged provider and snapshot adapters.
    pub fn expose_last_modified(&self) -> Option<&str> {
        self.last_modified.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none()
    }

    /// Applies validators from a not-modified response without clearing values
    /// merely because a provider omitted their headers on that response.
    pub(crate) fn merged_with(&self, update: &Self) -> Self {
        Self {
            etag: update.etag.clone().or_else(|| self.etag.clone()),
            last_modified: update
                .last_modified
                .clone()
                .or_else(|| self.last_modified.clone()),
        }
    }
}

impl Debug for PrivateSourceValidators {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("PrivateSourceValidators(<redacted>)")
    }
}

fn refine_validator(value: Option<String>) -> Result<Option<Arc<str>>, PrivateValidatorError> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > MAX_PRIVATE_VALIDATOR_BYTES {
        return Err(PrivateValidatorError::TooLong {
            max_bytes: MAX_PRIVATE_VALIDATOR_BYTES,
        });
    }
    if value.chars().any(char::is_control) {
        return Err(PrivateValidatorError::ContainsControlCharacter);
    }
    Ok(Some(Arc::from(value)))
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum PrivateValidatorError {
    #[error("the validator exceeds the {max_bytes}-byte limit")]
    TooLong { max_bytes: usize },
    #[error("control characters are not allowed in validators")]
    ContainsControlCharacter,
}

/// Safe manifest metadata for one independently recoverable Source Snapshot.
#[derive(Clone, Eq, PartialEq)]
pub struct SnapshotMetadata {
    source: SnapshotSource,
    decoded_bytes: u64,
    checksum: [u8; 32],
    validated_at: DateTime<Utc>,
    validators: PrivateSourceValidators,
}

impl SnapshotMetadata {
    pub fn new(
        source: SnapshotSource,
        decoded_bytes: u64,
        checksum: [u8; 32],
        validated_at: DateTime<Utc>,
        validators: PrivateSourceValidators,
    ) -> Self {
        Self {
            source,
            decoded_bytes,
            checksum,
            validated_at,
            validators,
        }
    }

    pub fn source(&self) -> SnapshotSource {
        self.source
    }

    pub fn decoded_bytes(&self) -> u64 {
        self.decoded_bytes
    }

    pub fn checksum(&self) -> &[u8; 32] {
        &self.checksum
    }

    pub fn validated_at(&self) -> DateTime<Utc> {
        self.validated_at
    }

    pub fn validators(&self) -> &PrivateSourceValidators {
        &self.validators
    }
}

impl Debug for SnapshotMetadata {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotMetadata")
            .field("kind", &self.source.kind())
            .field("decoded_bytes", &self.decoded_bytes)
            .field("checksum", &"<redacted>")
            .field("validated_at", &self.validated_at)
            .field("validators", &self.validators)
            .finish()
    }
}

/// An opaque adapter-owned candidate and its bounded manifest metadata.
#[derive(Clone, Eq, PartialEq)]
pub struct SnapshotCandidate {
    token: u64,
    metadata: SnapshotMetadata,
    requires_adoption: bool,
}

impl SnapshotCandidate {
    pub fn new(token: u64, metadata: SnapshotMetadata, requires_adoption: bool) -> Self {
        Self {
            token,
            metadata,
            requires_adoption,
        }
    }

    /// Returns the opaque token only for use by the owning snapshot adapter.
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn metadata(&self) -> &SnapshotMetadata {
        &self.metadata
    }

    pub fn requires_adoption(&self) -> bool {
        self.requires_adoption
    }
}

impl Debug for SnapshotCandidate {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotCandidate")
            .field("metadata", &self.metadata)
            .field("requires_adoption", &self.requires_adoption)
            .finish()
    }
}

/// At most the active and alternate candidates, in adapter preference order.
#[derive(Debug, Default)]
pub struct SnapshotCandidates(Box<[SnapshotCandidate]>);

impl SnapshotCandidates {
    pub const MAX: usize = 2;

    pub fn new(candidates: Vec<SnapshotCandidate>) -> Result<Self, StoreError> {
        if candidates.len() > Self::MAX {
            return Err(StoreError::Corrupt);
        }
        Ok(Self(candidates.into_boxed_slice()))
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SnapshotCandidate> {
        self.0.iter()
    }
}

/// One bounded adapter scan: ordered candidates plus safe slot diagnostics.
#[derive(Debug, Default)]
pub struct SnapshotScan {
    candidates: SnapshotCandidates,
    diagnostics: Box<[SnapshotRecoveryReason]>,
}

impl SnapshotScan {
    pub const MAX_DIAGNOSTICS: usize = 4;

    pub fn new(
        candidates: Vec<SnapshotCandidate>,
        diagnostics: Vec<SnapshotRecoveryReason>,
    ) -> Result<Self, StoreError> {
        if diagnostics.len() > Self::MAX_DIAGNOSTICS {
            return Err(StoreError::Corrupt);
        }
        Ok(Self {
            candidates: SnapshotCandidates::new(candidates)?,
            diagnostics: diagnostics.into_boxed_slice(),
        })
    }

    pub fn candidates(&self) -> impl Iterator<Item = &SnapshotCandidate> {
        self.candidates.iter()
    }

    pub fn diagnostics(&self) -> &[SnapshotRecoveryReason] {
        &self.diagnostics
    }

    pub fn into_candidates(self) -> impl Iterator<Item = SnapshotCandidate> {
        self.candidates.into_iter()
    }
}

impl IntoIterator for SnapshotCandidates {
    type Item = SnapshotCandidate;
    type IntoIter = std::vec::IntoIter<SnapshotCandidate>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_vec().into_iter()
    }
}

/// A manifest-only freshness update that never changes snapshot payload bytes.
#[derive(Clone, Eq, PartialEq)]
pub struct SnapshotRevalidation {
    validated_at: DateTime<Utc>,
    validators: PrivateSourceValidators,
}

impl SnapshotRevalidation {
    pub fn new(validated_at: DateTime<Utc>, validators: PrivateSourceValidators) -> Self {
        Self {
            validated_at,
            validators,
        }
    }

    pub fn validated_at(&self) -> DateTime<Utc> {
        self.validated_at
    }

    pub fn validators(&self) -> &PrivateSourceValidators {
        &self.validators
    }
}

impl Debug for SnapshotRevalidation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotRevalidation")
            .field("validated_at", &self.validated_at)
            .field("validators", &self.validators)
            .finish()
    }
}

impl SnapshotSource {
    pub(crate) fn m3u(configuration: &SourceConfiguration) -> Self {
        Self {
            kind: SourceKind::M3u,
            key: SourceKey::from_fingerprint(configuration.m3u.fingerprint()),
        }
    }

    pub(crate) fn epg(configuration: &SourceConfiguration) -> Option<Self> {
        configuration.epg.as_ref().map(|source| Self {
            kind: SourceKind::Epg,
            key: SourceKey::from_fingerprint(source.fingerprint()),
        })
    }

    pub fn kind(&self) -> SourceKind {
        self.kind
    }

    pub fn key(&self) -> SourceKey {
        self.key
    }
}

#[derive(Eq, PartialEq)]
pub struct SnapshotStage {
    token: u64,
    source: SnapshotSource,
}

/// Names the exact last-known-good candidate that an inactive stage must not
/// replace while a refresh is being validated.
#[derive(Clone, Eq, PartialEq)]
pub struct SnapshotStageRequest {
    source: SnapshotSource,
    protected: Option<SnapshotCandidate>,
}

impl SnapshotStageRequest {
    pub fn new(source: SnapshotSource, protected: Option<SnapshotCandidate>) -> Self {
        Self { source, protected }
    }

    pub fn source(&self) -> SnapshotSource {
        self.source
    }

    pub fn protected(&self) -> Option<&SnapshotCandidate> {
        self.protected.as_ref()
    }
}

impl Debug for SnapshotStageRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotStageRequest")
            .field("kind", &self.source.kind())
            .field("protected", &self.protected)
            .finish()
    }
}

impl SnapshotStage {
    pub fn new(token: u64, source: SnapshotSource) -> Self {
        Self { token, source }
    }

    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn source(&self) -> SnapshotSource {
        self.source
    }
}

impl Debug for SnapshotStage {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotStage")
            .field("kind", &self.source.kind())
            .field("token", &"<opaque>")
            .finish()
    }
}

pub struct ValidatedStage {
    stage: SnapshotStage,
    decoded_bytes: u64,
    checksum: [u8; 32],
    validated_at: DateTime<Utc>,
    validators: PrivateSourceValidators,
}

impl ValidatedStage {
    pub(crate) fn new(
        stage: SnapshotStage,
        decoded_bytes: u64,
        checksum: [u8; 32],
        validated_at: DateTime<Utc>,
        validators: PrivateSourceValidators,
    ) -> Self {
        Self {
            stage,
            decoded_bytes,
            checksum,
            validated_at,
            validators,
        }
    }

    pub fn stage(&self) -> &SnapshotStage {
        &self.stage
    }

    pub fn decoded_bytes(&self) -> u64 {
        self.decoded_bytes
    }

    pub fn checksum(&self) -> &[u8; 32] {
        &self.checksum
    }

    pub fn validated_at(&self) -> DateTime<Utc> {
        self.validated_at
    }

    pub fn validators(&self) -> &PrivateSourceValidators {
        &self.validators
    }

    pub fn metadata(&self) -> SnapshotMetadata {
        SnapshotMetadata::new(
            self.stage.source(),
            self.decoded_bytes,
            self.checksum,
            self.validated_at,
            self.validators.clone(),
        )
    }

    pub fn into_stage(self) -> SnapshotStage {
        self.stage
    }
}

impl Debug for ValidatedStage {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedStage")
            .field("stage", &self.stage)
            .field("decoded_bytes", &self.decoded_bytes)
            .field("checksum", &"<redacted>")
            .field("validated_at", &self.validated_at)
            .field("validators", &self.validators)
            .finish()
    }
}

#[async_trait]
pub trait SnapshotStore: Send + Sync {
    /// Returns at most two candidates in adapter preference order.
    async fn scan_candidates(&self, _source: SnapshotSource) -> Result<SnapshotScan, StoreError> {
        Ok(SnapshotScan::default())
    }

    async fn open_candidate(
        &self,
        _candidate: &SnapshotCandidate,
    ) -> Result<Box<dyn BufRead + Send>, StoreError> {
        Err(StoreError::Unavailable)
    }

    /// Repairs the active pointer after the core accepts a fallback candidate.
    /// The returned handle must preserve the candidate token and metadata and
    /// differ only by no longer requiring adoption.
    async fn adopt_candidate(
        &self,
        _candidate: &SnapshotCandidate,
    ) -> Result<SnapshotCandidate, StoreError> {
        Err(StoreError::Unavailable)
    }

    /// Atomically touches manifest freshness without replacing payload bytes.
    /// The returned handle must identify the same candidate and preserve its
    /// source, length, checksum, and adoption state while applying exactly the
    /// supplied validation time and validators.
    async fn revalidate_candidate(
        &self,
        _candidate: &SnapshotCandidate,
        _revalidation: &SnapshotRevalidation,
    ) -> Result<SnapshotCandidate, StoreError> {
        Err(StoreError::Unavailable)
    }

    /// Reserves an inactive stage before the core crosses another await point.
    fn begin_stage(&self, request: SnapshotStageRequest) -> Result<SnapshotStage, StoreError>;
    async fn append(&self, stage: &SnapshotStage, chunk: Bytes) -> Result<(), StoreError>;
    async fn open_staged(
        &self,
        stage: &SnapshotStage,
    ) -> Result<Box<dyn BufRead + Send>, StoreError>;
    /// Flushes and prepares a validated stage without changing the active slot.
    async fn prepare_activation(&self, validated: &ValidatedStage) -> Result<(), StoreError>;
    /// Atomically makes a prepared stage active without crossing an await point.
    /// The returned active candidate must contain exactly the validated stage
    /// metadata and must not require adoption.
    fn activate(&self, validated: &ValidatedStage) -> Result<SnapshotCandidate, StoreError>;
    /// Immediately makes an inactive stage ineligible for activation.
    ///
    /// Implementations must be idempotent and keep this operation bounded so
    /// the core can call it from a cancellation guard. Slow physical cleanup
    /// may be deferred to adapter recovery.
    fn discard(&self, stage: SnapshotStage) -> Result<(), StoreError>;
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use crate::domain::{
        SnapshotRecoveryReason, SourceConfiguration, SourceConfigurationInput, StoreError,
    };

    use super::{
        PrivateSourceValidators, SnapshotCandidate, SnapshotMetadata, SnapshotScan, SnapshotSource,
    };

    #[test]
    fn m3u_snapshot_identity_is_independent_of_the_epg_source() {
        let first = SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://provider.fixture.invalid/channels.m3u",
            Some("https://provider.fixture.invalid/first.xml"),
        ))
        .expect("first Source Configuration is valid");
        let changed_epg = SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://provider.fixture.invalid/channels.m3u",
            Some("https://provider.fixture.invalid/second.xml"),
        ))
        .expect("changed Source Configuration is valid");
        let changed_m3u = SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://provider.fixture.invalid/other.m3u",
            Some("https://provider.fixture.invalid/first.xml"),
        ))
        .expect("changed M3U source is valid");

        assert_eq!(
            SnapshotSource::m3u(&first).key(),
            SnapshotSource::m3u(&changed_epg).key()
        );
        assert_ne!(
            SnapshotSource::m3u(&first).key(),
            SnapshotSource::m3u(&changed_m3u).key()
        );
    }

    #[test]
    fn epg_snapshot_identity_is_independent_of_the_m3u_source() {
        let first = SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://provider.fixture.invalid/channels.m3u",
            Some("https://provider.fixture.invalid/guide.xml"),
        ))
        .expect("first Source Configuration is valid");
        let changed_m3u = SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://provider.fixture.invalid/other.m3u",
            Some("https://provider.fixture.invalid/guide.xml"),
        ))
        .expect("changed M3U Source Configuration is valid");
        let changed_epg = SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://provider.fixture.invalid/channels.m3u",
            Some("https://provider.fixture.invalid/other.xml"),
        ))
        .expect("changed EPG Source Configuration is valid");

        assert_eq!(
            SnapshotSource::epg(&first)
                .expect("the first EPG Source is configured")
                .key(),
            SnapshotSource::epg(&changed_m3u)
                .expect("the unchanged EPG Source is configured")
                .key()
        );
        assert_ne!(
            SnapshotSource::epg(&first)
                .expect("the first EPG Source is configured")
                .key(),
            SnapshotSource::epg(&changed_epg)
                .expect("the changed EPG Source is configured")
                .key()
        );
    }

    #[test]
    fn snapshot_handles_metadata_and_validators_have_safe_diagnostics() {
        let configuration = SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://private-user:private-secret@provider.invalid/channels.m3u",
            None::<String>,
        ))
        .expect("fixture Source Configuration is valid");
        let source = SnapshotSource::m3u(&configuration);
        let validators = PrivateSourceValidators::parse(
            Some("private-etag-canary".to_owned()),
            Some("private-last-modified-canary".to_owned()),
        )
        .expect("fixture validators are valid");
        let metadata = SnapshotMetadata::new(
            source,
            42,
            [0xab; 32],
            Utc.with_ymd_and_hms(2026, 8, 29, 12, 0, 0)
                .single()
                .expect("fixture timestamp is valid"),
            validators.clone(),
        );
        let candidate = SnapshotCandidate::new(9_876_543_210, metadata.clone(), true);
        let debug = format!("{source:?} {validators:?} {metadata:?} {candidate:?}");

        for private in [
            "private-user",
            "private-secret",
            "private-etag-canary",
            "private-last-modified-canary",
            "9876543210",
            &"ab".repeat(32),
        ] {
            assert!(!debug.contains(private), "private marker leaked: {private}");
        }
        assert_eq!(validators.expose_etag(), Some("private-etag-canary"));
        assert_eq!(
            validators.expose_last_modified(),
            Some("private-last-modified-canary")
        );
    }

    #[test]
    fn validator_and_scan_bounds_are_closed_and_typed() {
        assert!(PrivateSourceValidators::parse(Some("x".repeat(8 * 1024)), None).is_ok());
        assert!(PrivateSourceValidators::parse(Some("x".repeat(8 * 1024 + 1)), None).is_err());
        assert!(PrivateSourceValidators::parse(Some("bad\nvalue".to_owned()), None).is_err());

        let diagnostics = vec![SnapshotRecoveryReason::CorruptManifest; 5];
        assert!(matches!(
            SnapshotScan::new(Vec::new(), diagnostics),
            Err(StoreError::Corrupt)
        ));
    }
}
