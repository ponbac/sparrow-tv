use arc_swap::ArcSwap;
use futures_util::StreamExt;

use crate::{
    catalog::ChannelCatalog,
    domain::{
        CatalogStatus, ChannelDetails, ChannelGroupView, ChannelId, ChannelQuery, ChannelSummary,
        CoreError, Page, PageRequest, SafeFailure, SnapshotOperation, SourceConfiguration,
        SourceConfigurationInput,
    },
    m3u,
    ports::{CoreAdapters, SnapshotSource, SnapshotStage, SourceRequest, ValidatedStage},
};

const M3U_DECODED_LIMIT: u64 = 128 * 1024 * 1024;

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

    /// Builds a usable core and publishes a catalog only after its complete M3U
    /// Source Snapshot has been validated and activated.
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
            Ok((catalog, validated_at)) => {
                let status = CatalogStatus::fresh(catalog.generation(), redacted, validated_at);
                core.state.store(std::sync::Arc::new(CoreState::Published {
                    status,
                    catalog,
                }));
            }
            Err(failure) => {
                core.state.store(std::sync::Arc::new(CoreState::Unavailable(
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
        catalog: ChannelCatalog,
    },
}

async fn load_catalog(
    configuration: &SourceConfiguration,
    adapters: &CoreAdapters,
) -> Result<(ChannelCatalog, chrono::DateTime<chrono::Utc>), SafeFailure> {
    let response = adapters
        .source_access()
        .open(SourceRequest::m3u(configuration))
        .await
        .map_err(|reason| SafeFailure::SourceAccess { reason })?;
    let (declared_length, mut body) = response.into_parts();

    if declared_length.is_some_and(|length| length > M3U_DECODED_LIMIT) {
        return Err(SafeFailure::DecodedLimitExceeded {
            limit_bytes: M3U_DECODED_LIMIT,
        });
    }

    let store = adapters.snapshot_store();
    let stage = store
        .begin_stage(SnapshotSource::m3u(configuration))
        .map_err(|reason| SafeFailure::Snapshot {
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
                return Err(staged.reject(SafeFailure::SourceRead { reason }));
            }
        };
        decoded_bytes = match decoded_bytes.checked_add(chunk.len() as u64) {
            Some(length) if length <= M3U_DECODED_LIMIT => length,
            _ => {
                return Err(staged.reject(SafeFailure::DecodedLimitExceeded {
                    limit_bytes: M3U_DECODED_LIMIT,
                }));
            }
        };
        checksum.update(&chunk);
        if let Err(reason) = store.append(staged.stage(), chunk).await {
            return Err(staged.reject(SafeFailure::Snapshot {
                operation: SnapshotOperation::WriteStage,
                reason,
            }));
        }
    }

    let mut reader = match store.open_staged(staged.stage()).await {
        Ok(reader) => reader,
        Err(reason) => {
            return Err(staged.reject(SafeFailure::Snapshot {
                operation: SnapshotOperation::ReadStage,
                reason,
            }));
        }
    };
    let parsed = match m3u::parse(reader.as_mut()) {
        Ok(parsed) => parsed,
        Err(failure) => {
            drop(reader);
            return Err(staged.reject(failure));
        }
    };
    drop(reader);

    let m3u_checksum = *checksum.finalize().as_bytes();
    let generation = configuration.catalog_generation(&m3u_checksum, None);
    let catalog = ChannelCatalog::from_parsed(configuration, parsed, generation);
    let validated_at = adapters.clock().now();
    let validated = staged.validate(decoded_bytes, m3u_checksum, validated_at);
    if let Err(reason) = store.prepare_activation(validated.value()).await {
        return Err(validated.reject(SafeFailure::Snapshot {
            operation: SnapshotOperation::PrepareActivation,
            reason,
        }));
    }
    if let Err(reason) = store.activate(validated.value()) {
        return Err(validated.reject(SafeFailure::Snapshot {
            operation: SnapshotOperation::Activate,
            reason,
        }));
    }
    validated.commit();

    Ok((catalog, validated_at))
}

fn discard_after(
    store: &dyn crate::ports::SnapshotStore,
    stage: SnapshotStage,
    original: SafeFailure,
) -> SafeFailure {
    match store.discard(stage) {
        Ok(()) => original,
        Err(reason) => SafeFailure::Snapshot {
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
