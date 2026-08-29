use arc_swap::ArcSwap;
use futures_util::StreamExt;
use std::sync::Arc;

use crate::{
    catalog::ChannelCatalog,
    domain::{
        CatalogStatus, ChannelDetails, ChannelGroupView, ChannelId, ChannelQuery, ChannelSummary,
        CoreError, Page, PageRequest, ProgrammeSummary, SafeFailure, ScheduleQuery,
        SnapshotOperation, SourceConfiguration, SourceConfigurationInput, SourceState,
    },
    m3u,
    ports::{CoreAdapters, SnapshotSource, SnapshotStage, SourceRequest, ValidatedStage},
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
                state: ArcSwap::from_pointee(CoreState::NotConfigured(
                    CatalogStatus::not_configured(),
                )),
            });
        };

        let redacted = configuration.redacted();
        let core = Self {
            state: ArcSwap::from_pointee(CoreState::Unavailable(CatalogStatus::unavailable(
                redacted, None,
            ))),
        };

        match load_catalog(&configuration, &adapters).await {
            Ok(loaded) => {
                let status = CatalogStatus::fresh(
                    loaded.catalog.generation(),
                    redacted,
                    loaded.m3u_validated_at,
                    loaded.epg,
                );
                core.state.store(Arc::new(CoreState::Published {
                    status,
                    catalog: Arc::new(loaded.catalog),
                }));
            }
            Err(failure) => {
                core.state.store(Arc::new(CoreState::Unavailable(
                    CatalogStatus::unavailable(redacted, Some(failure)),
                )));
            }
        }

        Ok(core)
    }

    pub fn status(&self) -> CatalogStatus {
        match self.state.load().as_ref() {
            CoreState::NotConfigured(status)
            | CoreState::Unavailable(status)
            | CoreState::Published { status, .. } => status.clone(),
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

    fn query_catalog<T>(
        &self,
        query: impl FnOnce(&ChannelCatalog) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let state = self.state.load_full();
        match state.as_ref() {
            CoreState::NotConfigured(_) => Err(CoreError::NotConfigured),
            CoreState::Unavailable(status) => Err(CoreError::CatalogUnavailable {
                status: status.clone(),
            }),
            CoreState::Published { catalog, .. } => query(catalog),
        }
    }
}

enum CoreState {
    NotConfigured(CatalogStatus),
    Unavailable(CatalogStatus),
    Published {
        status: CatalogStatus,
        catalog: Arc<ChannelCatalog>,
    },
}

async fn load_catalog(
    configuration: &SourceConfiguration,
    adapters: &CoreAdapters,
) -> Result<LoadedCatalog, SafeFailure> {
    let m3u = load_source(
        adapters,
        SourceRequest::m3u(configuration),
        SnapshotSource::m3u(configuration),
        M3U_DECODED_LIMIT,
        m3u::parse,
    )
    .await?;

    let (guide, epg_checksum, epg) = match (
        SourceRequest::epg(configuration),
        SnapshotSource::epg(configuration),
    ) {
        (Some(request), Some(snapshot)) => {
            match load_source(adapters, request, snapshot, EPG_DECODED_LIMIT, xmltv::parse).await {
                Ok(loaded) => {
                    let state = SourceState::Fresh {
                        validated_at: loaded.validated_at,
                    };
                    (Some(loaded.value), Some(loaded.checksum), Some(state))
                }
                Err(failure) => (
                    None,
                    None,
                    Some(SourceState::Unavailable {
                        failure: Some(failure),
                    }),
                ),
            }
        }
        (None, None) => (None, None, None),
        _ => unreachable!("EPG request and snapshot identity are derived together"),
    };

    let generation = configuration.catalog_generation(&m3u.checksum, epg_checksum.as_ref());
    let catalog = ChannelCatalog::from_parsed(configuration, m3u.value, guide, generation);
    Ok(LoadedCatalog {
        catalog,
        m3u_validated_at: m3u.validated_at,
        epg,
    })
}

struct LoadedCatalog {
    catalog: ChannelCatalog,
    m3u_validated_at: chrono::DateTime<chrono::Utc>,
    epg: Option<SourceState>,
}

struct LoadedSource<T> {
    value: T,
    checksum: [u8; 32],
    validated_at: chrono::DateTime<chrono::Utc>,
}

async fn load_source<T>(
    adapters: &CoreAdapters,
    request: SourceRequest,
    snapshot: SnapshotSource,
    decoded_limit: u64,
    parse: impl FnOnce(&mut dyn std::io::BufRead) -> Result<T, SafeFailure>,
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
    let (declared_length, mut body) = response.into_parts();

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
    let validated = staged.validate(decoded_bytes, checksum, validated_at);
    if let Err(reason) = store.prepare_activation(validated.value()).await {
        return Err(validated.reject(SafeFailure::Snapshot {
            kind: source,
            operation: SnapshotOperation::PrepareActivation,
            reason,
        }));
    }
    if let Err(reason) = store.activate(validated.value()) {
        return Err(validated.reject(SafeFailure::Snapshot {
            kind: source,
            operation: SnapshotOperation::Activate,
            reason,
        }));
    }
    validated.commit();

    Ok(LoadedSource {
        value,
        checksum,
        validated_at,
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
    ) -> ValidatedCandidate<'a> {
        let stage = self.stage.take().expect("staged candidate is armed");
        ValidatedCandidate {
            store: self.store,
            validated: Some(ValidatedStage::new(
                stage,
                decoded_bytes,
                checksum,
                validated_at,
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
