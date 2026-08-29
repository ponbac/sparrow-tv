use arc_swap::ArcSwap;
use futures_util::StreamExt;
use std::{
    io::{self, BufRead, Read},
    sync::Arc,
};

use crate::{
    catalog::ChannelCatalog,
    domain::{
        CatalogStatus, ChannelDetails, ChannelGroupView, ChannelId, ChannelQuery, ChannelSummary,
        CoreError, Page, PageRequest, ProgrammeSummary, SafeFailure, ScheduleQuery, SearchRequest,
        SearchResults, SnapshotOperation, SnapshotRecoveryDiagnostic, SnapshotRecoveryReason,
        SourceConfiguration, SourceConfigurationInput, SourceKind, SourceState,
    },
    m3u,
    ports::{
        CoreAdapters, SnapshotCandidate, SnapshotSource, SnapshotStage, SourceRequest,
        ValidatedStage,
    },
    xmltv,
};

const M3U_DECODED_LIMIT: u64 = 128 * 1024 * 1024;
const EPG_DECODED_LIMIT: u64 = 64 * 1024 * 1024;

/// The transport-neutral entry point for Sparrow catalog behavior.
pub struct SparrowCore {
    state: ArcSwap<CoreState>,
}

impl SparrowCore {
    /// Validates and refines user-supplied source locations without exposing them.
    pub fn parse_source_configuration(
        input: SourceConfigurationInput,
    ) -> Result<SourceConfiguration, CoreError> {
        SourceConfiguration::parse(input)
    }

    /// Builds a usable core after independently validating the required M3U and
    /// optional EPG Source Snapshots. EPG failure yields a Channel-only catalog.
    pub async fn bootstrap(
        configuration: Option<SourceConfiguration>,
        adapters: CoreAdapters,
    ) -> Result<Self, CoreError> {
        let Some(configuration) = configuration else {
            return Ok(Self {
                state: ArcSwap::from_pointee(CoreState::NotConfigured(Box::new(
                    CatalogStatus::not_configured(),
                ))),
            });
        };

        let redacted = configuration.redacted();
        let core = Self {
            state: ArcSwap::from_pointee(CoreState::Unavailable(Box::new(
                CatalogStatus::unavailable(redacted, None),
            ))),
        };

        match load_catalog(&configuration, &adapters).await {
            Ok(loaded) => {
                let status = CatalogStatus::published(
                    loaded.catalog.generation(),
                    redacted,
                    loaded.m3u,
                    loaded.epg,
                    loaded.m3u_recovery,
                    loaded.epg_recovery,
                );
                core.state.store(Arc::new(CoreState::Published {
                    status: Box::new(status),
                    catalog: Arc::new(loaded.catalog),
                    _snapshots: Box::new(loaded.snapshots),
                }));
            }
            Err(failure) => {
                let mut status = CatalogStatus::unavailable(redacted, Some(failure.failure));
                status.set_recovery(SourceKind::M3u, failure.m3u_recovery);
                status.set_recovery(SourceKind::Epg, failure.epg_recovery);
                core.state
                    .store(Arc::new(CoreState::Unavailable(Box::new(status))));
            }
        }

        Ok(core)
    }

    pub fn status(&self) -> CatalogStatus {
        match self.state.load().as_ref() {
            CoreState::NotConfigured(status)
            | CoreState::Unavailable(status)
            | CoreState::Published { status, .. } => status.as_ref().clone(),
        }
    }

    /// Returns a deterministic bounded page of source-derived Channel Groups.
    pub fn list_groups(&self, request: PageRequest) -> Result<Page<ChannelGroupView>, CoreError> {
        self.query_catalog(|catalog| catalog.groups_page(&request))
    }

    /// Returns a deterministic bounded page of all Channels or one exact group.
    pub fn list_channels(&self, query: ChannelQuery) -> Result<Page<ChannelSummary>, CoreError> {
        self.query_catalog(|catalog| catalog.channels_page(&query))
    }

    pub fn channel(&self, id: &ChannelId) -> Result<ChannelDetails, CoreError> {
        self.query_catalog(|catalog| catalog.channel(id))
    }

    /// Returns a deterministic bounded Programme page for one Channel.
    pub fn schedule(&self, query: ScheduleQuery) -> Result<Page<ProgrammeSummary>, CoreError> {
        self.query_catalog(|catalog| catalog.schedule(&query))
    }

    /// Searches Channels and Programmes without reparsing the active Sources.
    pub fn search(&self, request: SearchRequest) -> Result<SearchResults, CoreError> {
        self.query_catalog(|catalog| catalog.search(&request))
    }

    fn query_catalog<T>(
        &self,
        query: impl FnOnce(&ChannelCatalog) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let state = self.state.load_full();
        match state.as_ref() {
            CoreState::NotConfigured(_) => Err(CoreError::NotConfigured),
            CoreState::Unavailable(status) => Err(CoreError::CatalogUnavailable {
                status: Box::new(status.as_ref().clone()),
            }),
            CoreState::Published { catalog, .. } => query(catalog),
        }
    }
}

enum CoreState {
    NotConfigured(Box<CatalogStatus>),
    Unavailable(Box<CatalogStatus>),
    Published {
        status: Box<CatalogStatus>,
        catalog: Arc<ChannelCatalog>,
        _snapshots: Box<PublishedSnapshots>,
    },
}

async fn load_catalog(
    configuration: &SourceConfiguration,
    adapters: &CoreAdapters,
) -> Result<LoadedCatalog, CatalogLoadFailure> {
    let m3u_snapshot = SnapshotSource::m3u(configuration);
    let m3u_recovery = recover_source(adapters, m3u_snapshot, M3U_DECODED_LIMIT, m3u::parse).await;
    let recovered_m3u = m3u_recovery.loaded.is_some();
    let m3u_diagnostic = m3u_recovery.diagnostic;
    let m3u = match m3u_recovery.loaded {
        Some(loaded) => loaded,
        None => load_source(
            adapters,
            SourceRequest::m3u(configuration),
            m3u_snapshot,
            M3U_DECODED_LIMIT,
            m3u::parse,
        )
        .await
        .map_err(|failure| CatalogLoadFailure {
            failure,
            m3u_recovery: m3u_diagnostic.clone(),
            epg_recovery: None,
        })?,
    };

    let (guide, epg_checksum, epg, epg_recovery, epg_candidate) = match (
        SourceRequest::epg(configuration),
        SnapshotSource::epg(configuration),
    ) {
        (Some(request), Some(snapshot)) => {
            let recovery =
                recover_source(adapters, snapshot, EPG_DECODED_LIMIT, xmltv::parse).await;
            let recovery_diagnostic = recovery.diagnostic;
            match recovery.loaded {
                Some(loaded) => {
                    let state = source_state(loaded.validated_at, adapters.clock().now());
                    (
                        Some(loaded.value),
                        Some(loaded.checksum),
                        Some(state),
                        recovery_diagnostic,
                        Some(loaded.candidate),
                    )
                }
                None if recovered_m3u => (
                    None,
                    None,
                    Some(SourceState::Unavailable {
                        failure: recovery.terminal_failure,
                    }),
                    recovery_diagnostic,
                    None,
                ),
                None => {
                    match load_source(adapters, request, snapshot, EPG_DECODED_LIMIT, xmltv::parse)
                        .await
                    {
                        Ok(loaded) => (
                            Some(loaded.value),
                            Some(loaded.checksum),
                            Some(source_state(loaded.validated_at, adapters.clock().now())),
                            recovery_diagnostic,
                            Some(loaded.candidate),
                        ),
                        Err(failure) => (
                            None,
                            None,
                            Some(SourceState::Unavailable {
                                failure: Some(failure),
                            }),
                            recovery_diagnostic,
                            None,
                        ),
                    }
                }
            }
        }
        (None, None) => (None, None, None, None, None),
        _ => unreachable!("EPG request and snapshot identity are derived together"),
    };

    let generation = configuration.catalog_generation(&m3u.checksum, epg_checksum.as_ref());
    let catalog = ChannelCatalog::from_parsed(configuration, m3u.value, guide, generation);
    let snapshots = PublishedSnapshots {
        m3u: m3u.candidate,
        epg: epg_candidate,
    };
    Ok(LoadedCatalog {
        catalog,
        m3u: source_state(m3u.validated_at, adapters.clock().now()),
        epg,
        m3u_recovery: m3u_diagnostic,
        epg_recovery,
        snapshots,
    })
}

struct LoadedCatalog {
    catalog: ChannelCatalog,
    m3u: SourceState,
    epg: Option<SourceState>,
    m3u_recovery: Option<SnapshotRecoveryDiagnostic>,
    epg_recovery: Option<SnapshotRecoveryDiagnostic>,
    snapshots: PublishedSnapshots,
}

struct PublishedSnapshots {
    #[allow(dead_code)]
    m3u: SnapshotCandidate,
    #[allow(dead_code)]
    epg: Option<SnapshotCandidate>,
}

struct CatalogLoadFailure {
    failure: SafeFailure,
    m3u_recovery: Option<SnapshotRecoveryDiagnostic>,
    epg_recovery: Option<SnapshotRecoveryDiagnostic>,
}

struct LoadedSource<T> {
    value: T,
    checksum: [u8; 32],
    validated_at: chrono::DateTime<chrono::Utc>,
    candidate: SnapshotCandidate,
}

type SourceParser<T> = fn(&mut dyn BufRead) -> Result<T, SafeFailure>;

struct RecoveryAttempt<T> {
    loaded: Option<LoadedSource<T>>,
    diagnostic: Option<SnapshotRecoveryDiagnostic>,
    terminal_failure: Option<SafeFailure>,
}

async fn recover_source<T>(
    adapters: &CoreAdapters,
    source: SnapshotSource,
    decoded_limit: u64,
    parse: SourceParser<T>,
) -> RecoveryAttempt<T> {
    let store = adapters.snapshot_store();
    let scan = match store.scan_candidates(source).await {
        Ok(scan) => scan,
        Err(reason) => {
            let failure = SafeFailure::Snapshot {
                kind: source.kind(),
                operation: SnapshotOperation::ScanCandidates,
                reason,
            };
            return RecoveryAttempt {
                loaded: None,
                diagnostic: SnapshotRecoveryDiagnostic::new(vec![failure.clone()], false),
                terminal_failure: Some(failure),
            };
        }
    };
    let mut failures = scan
        .diagnostics()
        .iter()
        .copied()
        .map(|reason| SafeFailure::SnapshotRecovery {
            kind: source.kind(),
            reason,
        })
        .collect::<Vec<_>>();

    for candidate in scan.into_candidates() {
        match recover_candidate(store, source, &candidate, decoded_limit, parse).await {
            Ok(mut loaded) => {
                let fallback_adopted = if candidate.requires_adoption() {
                    match store.adopt_candidate(&candidate).await {
                        Ok(adopted) => {
                            loaded.candidate = adopted;
                            true
                        }
                        Err(reason) => {
                            failures.push(SafeFailure::Snapshot {
                                kind: source.kind(),
                                operation: SnapshotOperation::AdoptCandidate,
                                reason,
                            });
                            false
                        }
                    }
                } else {
                    false
                };
                return RecoveryAttempt {
                    loaded: Some(loaded),
                    diagnostic: SnapshotRecoveryDiagnostic::new(failures, fallback_adopted),
                    terminal_failure: None,
                };
            }
            Err(failure) => failures.push(failure),
        }
    }

    let terminal_failure = failures.last().cloned();
    RecoveryAttempt {
        loaded: None,
        diagnostic: SnapshotRecoveryDiagnostic::new(failures, false),
        terminal_failure,
    }
}

async fn recover_candidate<T>(
    store: &dyn crate::ports::SnapshotStore,
    expected: SnapshotSource,
    candidate: &SnapshotCandidate,
    decoded_limit: u64,
    parse: SourceParser<T>,
) -> Result<LoadedSource<T>, SafeFailure> {
    let metadata = candidate.metadata();
    if metadata.source() != expected {
        return Err(SafeFailure::SnapshotRecovery {
            kind: expected.kind(),
            reason: SnapshotRecoveryReason::SourceMismatch,
        });
    }
    if metadata.decoded_bytes() > decoded_limit {
        return Err(SafeFailure::DecodedLimitExceeded {
            kind: expected.kind(),
            limit_bytes: decoded_limit,
        });
    }

    let reader = store
        .open_candidate(candidate)
        .await
        .map_err(|reason| SafeFailure::Snapshot {
            kind: expected.kind(),
            operation: SnapshotOperation::OpenCandidate,
            reason,
        })?;
    let mut reader = CandidateReader::new(reader, decoded_limit);
    let parsed = parse(&mut reader);
    let drain = io::copy(&mut reader, &mut io::sink());
    if reader.exceeded_limit {
        return Err(SafeFailure::DecodedLimitExceeded {
            kind: expected.kind(),
            limit_bytes: decoded_limit,
        });
    }
    if reader.read_failed || drain.is_err() {
        return Err(SafeFailure::Snapshot {
            kind: expected.kind(),
            operation: SnapshotOperation::OpenCandidate,
            reason: crate::domain::StoreError::Unavailable,
        });
    }
    if reader.decoded_bytes != metadata.decoded_bytes() {
        return Err(SafeFailure::SnapshotRecovery {
            kind: expected.kind(),
            reason: SnapshotRecoveryReason::LengthMismatch,
        });
    }
    if reader.checksum.finalize().as_bytes() != metadata.checksum() {
        return Err(SafeFailure::SnapshotRecovery {
            kind: expected.kind(),
            reason: SnapshotRecoveryReason::ChecksumMismatch,
        });
    }
    let value = parsed?;

    Ok(LoadedSource {
        value,
        checksum: *metadata.checksum(),
        validated_at: metadata.validated_at(),
        candidate: candidate.clone(),
    })
}

struct CandidateReader<R> {
    inner: R,
    checksum: blake3::Hasher,
    decoded_bytes: u64,
    decoded_limit: u64,
    exceeded_limit: bool,
    read_failed: bool,
}

impl<R> CandidateReader<R> {
    fn new(inner: R, decoded_limit: u64) -> Self {
        Self {
            inner,
            checksum: blake3::Hasher::new(),
            decoded_bytes: 0,
            decoded_limit,
            exceeded_limit: false,
            read_failed: false,
        }
    }

    fn record(&mut self, bytes: &[u8]) {
        self.decoded_bytes = self.decoded_bytes.saturating_add(bytes.len() as u64);
        self.exceeded_limit |= self.decoded_bytes > self.decoded_limit;
        self.checksum.update(bytes);
    }

    fn limit_error() -> io::Error {
        io::Error::other("snapshot candidate exceeds its decoded limit")
    }
}

impl<R: BufRead> Read for CandidateReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.exceeded_limit {
            return Err(Self::limit_error());
        }
        match self.inner.read(buffer) {
            Ok(read) => {
                self.record(&buffer[..read]);
                Ok(read)
            }
            Err(error) => {
                self.read_failed = true;
                Err(error)
            }
        }
    }
}

impl<R: BufRead> BufRead for CandidateReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.exceeded_limit {
            return Err(Self::limit_error());
        }
        match self.inner.fill_buf() {
            Ok(buffer) => Ok(buffer),
            Err(error) => {
                self.read_failed = true;
                Err(error)
            }
        }
    }

    fn consume(&mut self, amount: usize) {
        match self.inner.fill_buf() {
            Ok(buffer) => {
                let amount = amount.min(buffer.len());
                self.decoded_bytes = self.decoded_bytes.saturating_add(amount as u64);
                self.exceeded_limit |= self.decoded_bytes > self.decoded_limit;
                self.checksum.update(&buffer[..amount]);
                self.inner.consume(amount);
            }
            Err(_) => self.read_failed = true,
        }
    }
}

fn source_state(
    validated_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> SourceState {
    let fresh = validated_at <= now
        && validated_at
            .checked_add_signed(chrono::Duration::hours(6))
            .is_some_and(|deadline| now < deadline);
    if fresh {
        SourceState::Fresh { validated_at }
    } else {
        SourceState::Stale {
            validated_at,
            next_attempt_at: None,
        }
    }
}

async fn load_source<T>(
    adapters: &CoreAdapters,
    request: SourceRequest,
    snapshot: SnapshotSource,
    decoded_limit: u64,
    parse: SourceParser<T>,
) -> Result<LoadedSource<T>, SafeFailure> {
    let source = request.kind();
    debug_assert_eq!(source, snapshot.kind());
    let response = adapters
        .source_access()
        .open(request)
        .await
        .map_err(|reason| SafeFailure::SourceAccess {
            kind: source,
            reason,
        })?;
    let (declared_length, mut body, validators) = response.into_parts();

    if declared_length.is_some_and(|length| length > decoded_limit) {
        return Err(SafeFailure::DecodedLimitExceeded {
            kind: source,
            limit_bytes: decoded_limit,
        });
    }

    let store = adapters.snapshot_store();
    let stage = store
        .begin_stage(snapshot)
        .map_err(|reason| SafeFailure::Snapshot {
            kind: source,
            operation: SnapshotOperation::BeginStage,
            reason,
        })?;
    let staged = StagedCandidate::new(store, stage);
    let mut decoded_bytes = 0_u64;
    let mut checksum = blake3::Hasher::new();

    while let Some(next) = body.next().await {
        let chunk = match next {
            Ok(chunk) => chunk,
            Err(reason) => {
                return Err(staged.reject(SafeFailure::SourceRead {
                    kind: source,
                    reason,
                }));
            }
        };
        decoded_bytes = match decoded_bytes.checked_add(chunk.len() as u64) {
            Some(length) if length <= decoded_limit => length,
            _ => {
                return Err(staged.reject(SafeFailure::DecodedLimitExceeded {
                    kind: source,
                    limit_bytes: decoded_limit,
                }));
            }
        };
        checksum.update(&chunk);
        if let Err(reason) = store.append(staged.stage(), chunk).await {
            return Err(staged.reject(SafeFailure::Snapshot {
                kind: source,
                operation: SnapshotOperation::WriteStage,
                reason,
            }));
        }
    }

    let mut reader = match store.open_staged(staged.stage()).await {
        Ok(reader) => reader,
        Err(reason) => {
            return Err(staged.reject(SafeFailure::Snapshot {
                kind: source,
                operation: SnapshotOperation::ReadStage,
                reason,
            }));
        }
    };
    let value = match parse(reader.as_mut()) {
        Ok(value) => value,
        Err(failure) => {
            drop(reader);
            return Err(staged.reject(failure));
        }
    };
    drop(reader);

    let checksum = *checksum.finalize().as_bytes();
    let validated_at = adapters.clock().now();
    let validated = staged.validate(decoded_bytes, checksum, validated_at, validators);
    if let Err(reason) = store.prepare_activation(validated.value()).await {
        return Err(validated.reject(SafeFailure::Snapshot {
            kind: source,
            operation: SnapshotOperation::PrepareActivation,
            reason,
        }));
    }
    let candidate = match store.activate(validated.value()) {
        Ok(candidate) => candidate,
        Err(reason) => {
            return Err(validated.reject(SafeFailure::Snapshot {
                kind: source,
                operation: SnapshotOperation::Activate,
                reason,
            }));
        }
    };
    validated.commit();

    Ok(LoadedSource {
        value,
        checksum,
        validated_at,
        candidate,
    })
}

fn discard_after(
    store: &dyn crate::ports::SnapshotStore,
    stage: SnapshotStage,
    original: SafeFailure,
) -> SafeFailure {
    let source = stage.source().kind();
    match store.discard(stage) {
        Ok(()) => original,
        Err(reason) => SafeFailure::Snapshot {
            kind: source,
            operation: SnapshotOperation::Discard,
            reason,
        },
    }
}

struct StagedCandidate<'a> {
    store: &'a dyn crate::ports::SnapshotStore,
    stage: Option<SnapshotStage>,
}

impl<'a> StagedCandidate<'a> {
    fn new(store: &'a dyn crate::ports::SnapshotStore, stage: SnapshotStage) -> Self {
        Self {
            store,
            stage: Some(stage),
        }
    }

    fn stage(&self) -> &SnapshotStage {
        self.stage.as_ref().expect("staged candidate is armed")
    }

    fn validate(
        mut self,
        decoded_bytes: u64,
        checksum: [u8; 32],
        validated_at: chrono::DateTime<chrono::Utc>,
        validators: crate::ports::PrivateSourceValidators,
    ) -> ValidatedCandidate<'a> {
        let stage = self.stage.take().expect("staged candidate is armed");
        ValidatedCandidate {
            store: self.store,
            validated: Some(ValidatedStage::new(
                stage,
                decoded_bytes,
                checksum,
                validated_at,
                validators,
            )),
        }
    }

    fn reject(mut self, original: SafeFailure) -> SafeFailure {
        let stage = self.stage.take().expect("staged candidate is armed");
        discard_after(self.store, stage, original)
    }
}

impl Drop for StagedCandidate<'_> {
    fn drop(&mut self) {
        if let Some(stage) = self.stage.take() {
            let _ = self.store.discard(stage);
        }
    }
}

struct ValidatedCandidate<'a> {
    store: &'a dyn crate::ports::SnapshotStore,
    validated: Option<ValidatedStage>,
}

impl ValidatedCandidate<'_> {
    fn value(&self) -> &ValidatedStage {
        self.validated
            .as_ref()
            .expect("validated candidate is armed")
    }

    fn reject(mut self, original: SafeFailure) -> SafeFailure {
        let stage = self
            .validated
            .take()
            .expect("validated candidate is armed")
            .into_stage();
        discard_after(self.store, stage, original)
    }

    fn commit(mut self) {
        self.validated.take();
    }
}

impl Drop for ValidatedCandidate<'_> {
    fn drop(&mut self) {
        if let Some(validated) = self.validated.take() {
            let _ = self.store.discard(validated.into_stage());
        }
    }
}
