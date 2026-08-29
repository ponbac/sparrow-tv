mod support;

use std::{
    collections::VecDeque,
    io::BufRead,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream;
use sparrow_core::{
    ChannelQuery, CoreAdapters, CoreError, PageLimit, PageRequest, PrivateSourceValidators,
    RefreshOutcome, RefreshTrigger, SafeFailure, SnapshotCandidate, SnapshotMetadata,
    SnapshotOperation, SnapshotRevalidation, SnapshotScan, SnapshotSource, SnapshotStage,
    SnapshotStageRequest, SnapshotStore, SourceAccess, SourceAccessFailure, SourceByteStream,
    SourceConfigurationInput, SourceKind, SourceReadError, SourceRequest, SourceResponse,
    SourceState, SparrowCore, StoreError, ValidatedStage,
};

use support::{FixedClock, MemorySnapshotStore};

const ORIGINAL_M3U: &[u8] = br#"#EXTM3U
#EXTINF:-1 tvg-id="alpha" group-title="News",Alpha
https://media.fixture.invalid/alpha
"#;

const UPDATED_M3U: &[u8] = br#"#EXTM3U
#EXTINF:-1 tvg-id="beta" group-title="News",Beta
https://media.fixture.invalid/beta
"#;

#[derive(Clone, Copy, Debug)]
enum ContractBoundary {
    Revalidation,
    Activation,
}

#[derive(Clone, Copy, Debug)]
enum MetadataSubstitution {
    Source,
    DecodedLength,
    Checksum,
    ValidatedAt,
}

const METADATA_SUBSTITUTIONS: [MetadataSubstitution; 4] = [
    MetadataSubstitution::Source,
    MetadataSubstitution::DecodedLength,
    MetadataSubstitution::Checksum,
    MetadataSubstitution::ValidatedAt,
];

#[derive(Clone)]
struct FaultySnapshotStore {
    inner: MemorySnapshotStore,
    state: Arc<Mutex<FaultState>>,
}

#[derive(Default)]
struct FaultState {
    observed_sources: Vec<SnapshotSource>,
    fault: Option<(ContractBoundary, MetadataSubstitution)>,
}

impl FaultySnapshotStore {
    fn new() -> Self {
        Self {
            inner: MemorySnapshotStore::default(),
            state: Arc::new(Mutex::new(FaultState::default())),
        }
    }

    fn arm(&self, boundary: ContractBoundary, substitution: MetadataSubstitution) {
        self.state.lock().expect("fault state poisoned").fault = Some((boundary, substitution));
    }

    fn record_source(&self, source: SnapshotSource) {
        let mut state = self.state.lock().expect("fault state poisoned");
        if !state.observed_sources.contains(&source) {
            state.observed_sources.push(source);
        }
    }

    fn corrupt_at(
        &self,
        boundary: ContractBoundary,
        candidate: SnapshotCandidate,
    ) -> SnapshotCandidate {
        let state = self.state.lock().expect("fault state poisoned");
        let Some((armed_boundary, substitution)) = state.fault else {
            return candidate;
        };
        if !matches_boundary(armed_boundary, boundary)
            || candidate.metadata().source().kind() != SourceKind::M3u
        {
            return candidate;
        }

        let original = candidate.metadata();
        let mut source = original.source();
        let mut decoded_bytes = original.decoded_bytes();
        let mut checksum = *original.checksum();
        let mut validated_at = original.validated_at();

        match substitution {
            MetadataSubstitution::Source => {
                source = state
                    .observed_sources
                    .iter()
                    .copied()
                    .find(|observed| *observed != source)
                    .expect("the seed bootstrap captured an alternate source identity");
            }
            MetadataSubstitution::DecodedLength => {
                decoded_bytes = decoded_bytes
                    .checked_add(1)
                    .expect("fixture decoded length can be incremented");
            }
            MetadataSubstitution::Checksum => checksum[0] ^= 0xff,
            MetadataSubstitution::ValidatedAt => {
                validated_at = validated_at
                    .checked_add_signed(chrono::Duration::seconds(1))
                    .expect("fixture validation time can be incremented");
            }
        }

        SnapshotCandidate::new(
            candidate.token(),
            SnapshotMetadata::new(
                source,
                decoded_bytes,
                checksum,
                validated_at,
                original.validators().clone(),
            ),
            candidate.requires_adoption(),
        )
    }
}

fn matches_boundary(left: ContractBoundary, right: ContractBoundary) -> bool {
    matches!(
        (left, right),
        (
            ContractBoundary::Revalidation,
            ContractBoundary::Revalidation
        ) | (ContractBoundary::Activation, ContractBoundary::Activation)
    )
}

#[async_trait]
impl SnapshotStore for FaultySnapshotStore {
    async fn scan_candidates(&self, source: SnapshotSource) -> Result<SnapshotScan, StoreError> {
        self.inner.scan_candidates(source).await
    }

    async fn open_candidate(
        &self,
        candidate: &SnapshotCandidate,
    ) -> Result<Box<dyn BufRead + Send>, StoreError> {
        self.inner.open_candidate(candidate).await
    }

    async fn adopt_candidate(
        &self,
        candidate: &SnapshotCandidate,
    ) -> Result<SnapshotCandidate, StoreError> {
        self.inner.adopt_candidate(candidate).await
    }

    async fn revalidate_candidate(
        &self,
        candidate: &SnapshotCandidate,
        revalidation: &SnapshotRevalidation,
    ) -> Result<SnapshotCandidate, StoreError> {
        let candidate = self
            .inner
            .revalidate_candidate(candidate, revalidation)
            .await?;
        Ok(self.corrupt_at(ContractBoundary::Revalidation, candidate))
    }

    fn begin_stage(&self, request: SnapshotStageRequest) -> Result<SnapshotStage, StoreError> {
        self.record_source(request.source());
        self.inner.begin_stage(request)
    }

    async fn append(&self, stage: &SnapshotStage, chunk: Bytes) -> Result<(), StoreError> {
        self.inner.append(stage, chunk).await
    }

    async fn open_staged(
        &self,
        stage: &SnapshotStage,
    ) -> Result<Box<dyn BufRead + Send>, StoreError> {
        self.inner.open_staged(stage).await
    }

    async fn prepare_activation(&self, validated: &ValidatedStage) -> Result<(), StoreError> {
        self.inner.prepare_activation(validated).await
    }

    fn activate(&self, validated: &ValidatedStage) -> Result<SnapshotCandidate, StoreError> {
        let candidate = self.inner.activate(validated)?;
        Ok(self.corrupt_at(ContractBoundary::Activation, candidate))
    }

    fn discard(&self, stage: SnapshotStage) -> Result<(), StoreError> {
        self.inner.discard(stage)
    }
}

#[derive(Clone)]
struct SequencedSource {
    actions: Arc<Mutex<VecDeque<SourceAction>>>,
}

enum SourceAction {
    Modified(&'static [u8]),
    NotModified,
}

impl SequencedSource {
    fn new(actions: impl IntoIterator<Item = SourceAction>) -> Self {
        Self {
            actions: Arc::new(Mutex::new(actions.into_iter().collect())),
        }
    }
}

#[async_trait]
impl SourceAccess for SequencedSource {
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessFailure> {
        assert_eq!(request.kind(), SourceKind::M3u);
        match self
            .actions
            .lock()
            .expect("source actions poisoned")
            .pop_front()
            .expect("the fixture has one source action per request")
        {
            SourceAction::Modified(bytes) => {
                let bytes = Bytes::from_static(bytes);
                let length = bytes.len() as u64;
                let body: SourceByteStream =
                    Box::pin(stream::iter([Ok::<Bytes, SourceReadError>(bytes)]));
                Ok(SourceResponse::modified(
                    Some(length),
                    body,
                    PrivateSourceValidators::default(),
                ))
            }
            SourceAction::NotModified => Ok(SourceResponse::not_modified(
                PrivateSourceValidators::default(),
            )),
        }
    }
}

#[tokio::test]
async fn not_modified_revalidation_rejects_all_metadata_substitutions_and_retains_catalog() {
    for substitution in METADATA_SUBSTITUTIONS {
        let source = SequencedSource::new([
            SourceAction::Modified(ORIGINAL_M3U),
            SourceAction::Modified(ORIGINAL_M3U),
            SourceAction::NotModified,
        ]);
        let store = FaultySnapshotStore::new();
        seed_alternate_source(&source, &store).await;
        let core = bootstrap_target(&source, &store).await;
        let before = core
            .list_channels(first_channels())
            .expect("the target catalog is initially available");
        let generation = before.generation();

        store.arm(ContractBoundary::Revalidation, substitution);
        let report = core.refresh(RefreshTrigger::Manual).await;

        assert_corrupt_failure(
            report.m3u(),
            SnapshotOperation::RevalidateCandidate,
            substitution,
        );
        assert_retained_catalog(&core, generation, substitution);
    }
}

#[tokio::test]
async fn activation_rejects_all_metadata_substitutions_and_retains_previous_catalog() {
    for substitution in METADATA_SUBSTITUTIONS {
        let source = SequencedSource::new([
            SourceAction::Modified(ORIGINAL_M3U),
            SourceAction::Modified(ORIGINAL_M3U),
            SourceAction::Modified(UPDATED_M3U),
        ]);
        let store = FaultySnapshotStore::new();
        seed_alternate_source(&source, &store).await;
        let core = bootstrap_target(&source, &store).await;
        let before = core
            .list_channels(first_channels())
            .expect("the target catalog is initially available");
        let generation = before.generation();

        store.arm(ContractBoundary::Activation, substitution);
        let report = core.refresh(RefreshTrigger::Manual).await;

        assert_corrupt_failure(report.m3u(), SnapshotOperation::Activate, substitution);
        assert_retained_catalog(&core, generation, substitution);
    }
}

#[tokio::test]
async fn malformed_activation_candidate_never_publishes_a_bootstrap_catalog() {
    for substitution in METADATA_SUBSTITUTIONS {
        let source = SequencedSource::new([
            SourceAction::Modified(ORIGINAL_M3U),
            SourceAction::Modified(UPDATED_M3U),
        ]);
        let store = FaultySnapshotStore::new();
        seed_alternate_source(&source, &store).await;
        store.arm(ContractBoundary::Activation, substitution);

        let core = bootstrap_target(&source, &store).await;
        let status = core.status();

        assert_eq!(status.generation(), None, "substitution: {substitution:?}");
        assert!(
            matches!(
                status.m3u(),
                SourceState::Failed {
                    validated_at: None,
                    failure: SafeFailure::Snapshot {
                        kind: SourceKind::M3u,
                        operation: SnapshotOperation::Activate,
                        reason: StoreError::Corrupt,
                    },
                    ..
                }
            ),
            "substitution: {substitution:?}, status: {status:?}"
        );
        assert!(
            matches!(
                core.list_channels(first_channels()),
                Err(CoreError::CatalogUnavailable { .. })
            ),
            "substitution: {substitution:?}"
        );
    }
}

async fn seed_alternate_source(source: &SequencedSource, store: &FaultySnapshotStore) {
    let seed = bootstrap("https://seed.fixture.invalid/channels.m3u", source, store).await;
    assert!(seed.status().generation().is_some());
    drop(seed);
}

async fn bootstrap_target(source: &SequencedSource, store: &FaultySnapshotStore) -> SparrowCore {
    bootstrap("https://target.fixture.invalid/channels.m3u", source, store).await
}

async fn bootstrap(
    location: &str,
    source: &SequencedSource,
    store: &FaultySnapshotStore,
) -> SparrowCore {
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        location,
        None::<String>,
    ))
    .expect("fixture configuration is valid");
    SparrowCore::bootstrap(
        Some(configuration),
        CoreAdapters::new(
            Arc::new(source.clone()),
            Arc::new(store.clone()),
            Arc::new(FixedClock::default()),
        ),
    )
    .await
    .expect("the core remains usable when an adapter violates its contract")
}

fn assert_corrupt_failure(
    outcome: &RefreshOutcome,
    expected_operation: SnapshotOperation,
    substitution: MetadataSubstitution,
) {
    assert!(
        matches!(
            outcome,
            RefreshOutcome::Failed {
                failure: SafeFailure::Snapshot {
                    kind: SourceKind::M3u,
                    operation,
                    reason: StoreError::Corrupt,
                },
                ..
            } if *operation == expected_operation
        ),
        "substitution: {substitution:?}, outcome: {outcome:?}"
    );
}

fn assert_retained_catalog(
    core: &SparrowCore,
    generation: sparrow_core::CatalogGeneration,
    substitution: MetadataSubstitution,
) {
    let status = core.status();
    assert_eq!(
        status.generation(),
        Some(generation),
        "substitution: {substitution:?}"
    );
    assert!(
        matches!(
            status.m3u(),
            SourceState::Failed {
                validated_at: Some(_),
                failure: SafeFailure::Snapshot {
                    kind: SourceKind::M3u,
                    reason: StoreError::Corrupt,
                    ..
                },
                ..
            }
        ),
        "substitution: {substitution:?}, status: {status:?}"
    );

    let after = core
        .list_channels(first_channels())
        .expect("the previously published catalog remains available");
    assert_eq!(after.generation(), generation);
    assert_eq!(after.items().len(), 1);
    assert_eq!(after.items()[0].name(), "Alpha");
}

fn first_channels() -> ChannelQuery {
    ChannelQuery::all(PageRequest::first(
        PageLimit::new(10).expect("fixture page limit is valid"),
    ))
}
