use std::{
    fmt::{self, Debug, Formatter},
    io::BufRead,
    pin::Pin,
    sync::Arc,
};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_core::Stream;

use crate::domain::{
    SecretSourceLocation, SourceAccessError, SourceConfiguration, SourceFingerprint, SourceKind,
    SourceReadError, StoreError,
};

pub type SourceByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, SourceReadError>> + Send + 'static>>;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
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
}

pub struct SourceRequest {
    kind: SourceKind,
    location: SecretSourceLocation,
}

impl SourceRequest {
    pub(crate) fn m3u(configuration: &SourceConfiguration) -> Self {
        Self {
            kind: SourceKind::M3u,
            location: configuration.m3u.location().clone(),
        }
    }

    pub(crate) fn epg(configuration: &SourceConfiguration) -> Option<Self> {
        configuration.epg.as_ref().map(|source| Self {
            kind: SourceKind::Epg,
            location: source.location().clone(),
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
}

impl Debug for SourceRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceRequest")
            .field("kind", &self.kind)
            .field("location", &"<redacted>")
            .finish()
    }
}

pub struct SourceResponse {
    declared_decoded_length: Option<u64>,
    decoded_body: SourceByteStream,
}

impl SourceResponse {
    pub fn new(declared_decoded_length: Option<u64>, decoded_body: SourceByteStream) -> Self {
        Self {
            declared_decoded_length,
            decoded_body,
        }
    }

    pub(crate) fn into_parts(self) -> (Option<u64>, SourceByteStream) {
        (self.declared_decoded_length, self.decoded_body)
    }
}

#[async_trait]
pub trait SourceAccess: Send + Sync {
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessError>;
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SnapshotSource {
    kind: SourceKind,
    key: SourceKey,
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
}

impl ValidatedStage {
    pub(crate) fn new(
        stage: SnapshotStage,
        decoded_bytes: u64,
        checksum: [u8; 32],
        validated_at: DateTime<Utc>,
    ) -> Self {
        Self {
            stage,
            decoded_bytes,
            checksum,
            validated_at,
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
            .finish()
    }
}

#[async_trait]
pub trait SnapshotStore: Send + Sync {
    /// Reserves an inactive stage before the core crosses another await point.
    fn begin_stage(&self, source: SnapshotSource) -> Result<SnapshotStage, StoreError>;
    async fn append(&self, stage: &SnapshotStage, chunk: Bytes) -> Result<(), StoreError>;
    async fn open_staged(
        &self,
        stage: &SnapshotStage,
    ) -> Result<Box<dyn BufRead + Send>, StoreError>;
    /// Flushes and prepares a validated stage without changing the active slot.
    async fn prepare_activation(&self, validated: &ValidatedStage) -> Result<(), StoreError>;
    /// Atomically makes a prepared stage active without crossing an await point.
    fn activate(&self, validated: &ValidatedStage) -> Result<(), StoreError>;
    /// Immediately makes an inactive stage ineligible for activation.
    ///
    /// Implementations must be idempotent and keep this operation bounded so
    /// the core can call it from a cancellation guard. Slow physical cleanup
    /// may be deferred to adapter recovery.
    fn discard(&self, stage: SnapshotStage) -> Result<(), StoreError>;
}

#[cfg(test)]
mod tests {
    use crate::domain::{SourceConfiguration, SourceConfigurationInput};

    use super::SnapshotSource;

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
}
