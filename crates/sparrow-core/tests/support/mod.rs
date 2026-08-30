#![allow(dead_code)]

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, Cursor},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::stream;
use sparrow_core::{
    Clock, CoreAdapters, PrivateSourceValidators, SnapshotCandidate, SnapshotMetadata,
    SnapshotRecoveryReason, SnapshotRevalidation, SnapshotScan, SnapshotSource, SnapshotStage,
    SnapshotStageRequest, SnapshotStore, SourceAccess, SourceAccessError, SourceAccessFailure,
    SourceByteStream, SourceKind, SourceReadError, SourceRequest, SourceResponse, StoreError,
    ValidatedStage,
};

#[derive(Clone)]
pub struct ScriptedSource {
    state: Arc<Mutex<SourceState>>,
}

struct SourceState {
    scripts: HashMap<SourceKind, SourceScript>,
    opens: HashMap<SourceKind, usize>,
    request_debug: HashMap<SourceKind, String>,
}

struct SourceScript {
    chunks: Vec<Result<Bytes, SourceReadError>>,
    declared_length: Option<u64>,
    validators: PrivateSourceValidators,
}

impl ScriptedSource {
    pub fn from_bytes(bytes: impl Into<Bytes>) -> Self {
        let bytes = bytes.into();
        Self::from_chunks(vec![Ok(bytes.clone())], Some(bytes.len() as u64))
    }

    pub fn from_chunks(
        chunks: Vec<Result<Bytes, SourceReadError>>,
        declared_length: Option<u64>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(SourceState {
                scripts: HashMap::from([(
                    SourceKind::M3u,
                    SourceScript {
                        chunks,
                        declared_length,
                        validators: PrivateSourceValidators::default(),
                    },
                )]),
                opens: HashMap::new(),
                request_debug: HashMap::new(),
            })),
        }
    }

    pub fn with_epg_bytes(self, bytes: impl Into<Bytes>) -> Self {
        let bytes = bytes.into();
        self.with_epg_chunks(vec![Ok(bytes.clone())], Some(bytes.len() as u64))
    }

    pub fn with_epg_chunks(
        self,
        chunks: Vec<Result<Bytes, SourceReadError>>,
        declared_length: Option<u64>,
    ) -> Self {
        self.state
            .lock()
            .expect("source state poisoned")
            .scripts
            .insert(
                SourceKind::Epg,
                SourceScript {
                    chunks,
                    declared_length,
                    validators: PrivateSourceValidators::default(),
                },
            );
        self
    }

    pub fn with_m3u_validators(self, validators: PrivateSourceValidators) -> Self {
        self.state
            .lock()
            .expect("source state poisoned")
            .scripts
            .get_mut(&SourceKind::M3u)
            .expect("M3U source script exists")
            .validators = validators;
        self
    }

    pub fn replace_bytes(&self, kind: SourceKind, bytes: impl Into<Bytes>) {
        let bytes = bytes.into();
        let mut state = self.state.lock().expect("source state poisoned");
        let script = state
            .scripts
            .get_mut(&kind)
            .expect("the source script exists");
        script.chunks = vec![Ok(bytes.clone())];
        script.declared_length = Some(bytes.len() as u64);
    }

    pub fn unavailable() -> Self {
        Self {
            state: Arc::new(Mutex::new(SourceState {
                scripts: HashMap::new(),
                opens: HashMap::new(),
                request_debug: HashMap::new(),
            })),
        }
    }

    pub fn open_count(&self) -> usize {
        self.state
            .lock()
            .expect("source state poisoned")
            .opens
            .values()
            .sum()
    }

    pub fn open_count_for(&self, kind: SourceKind) -> usize {
        self.state
            .lock()
            .expect("source state poisoned")
            .opens
            .get(&kind)
            .copied()
            .unwrap_or_default()
    }

    pub fn request_debug(&self) -> Option<String> {
        self.request_debug_for(SourceKind::M3u)
    }

    pub fn request_debug_for(&self, kind: SourceKind) -> Option<String> {
        self.state
            .lock()
            .expect("source state poisoned")
            .request_debug
            .get(&kind)
            .cloned()
    }
}

#[async_trait]
impl SourceAccess for ScriptedSource {
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessFailure> {
        let kind = request.kind();
        let mut state = self.state.lock().expect("source state poisoned");
        *state.opens.entry(kind).or_default() += 1;
        state.request_debug.insert(kind, format!("{request:?}"));
        let script = state
            .scripts
            .get(&kind)
            .ok_or_else(|| SourceAccessFailure::new(SourceAccessError::Unavailable))?;
        let chunks = script.chunks.clone();
        let declared_length = script.declared_length;
        let body: SourceByteStream = Box::pin(stream::iter(chunks));
        Ok(SourceResponse::with_validators(
            declared_length,
            body,
            script.validators.clone(),
        ))
    }
}

#[derive(Clone, Default)]
pub struct MemorySnapshotStore {
    state: Arc<Mutex<SnapshotState>>,
}

#[derive(Clone, Default)]
pub struct CountingSnapshotStore {
    state: Arc<Mutex<CountingSnapshotState>>,
}

#[derive(Clone, Default)]
pub struct PendingAppendSnapshotStore {
    state: Arc<Mutex<PendingAppendSnapshotState>>,
}

#[derive(Clone, Default)]
pub struct PendingActivationSnapshotStore {
    inner: MemorySnapshotStore,
    preparation_started: Arc<AtomicBool>,
}

impl PendingActivationSnapshotStore {
    pub fn preparation_started(&self) -> bool {
        self.preparation_started.load(Ordering::SeqCst)
    }

    pub fn activation_count(&self) -> usize {
        self.inner.activation_count()
    }

    pub fn discard_count(&self) -> usize {
        self.inner.discard_count()
    }
}

#[async_trait]
impl SnapshotStore for PendingActivationSnapshotStore {
    fn begin_stage(&self, request: SnapshotStageRequest) -> Result<SnapshotStage, StoreError> {
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

    async fn prepare_activation(&self, _validated: &ValidatedStage) -> Result<(), StoreError> {
        self.preparation_started.store(true, Ordering::SeqCst);
        std::future::pending().await
    }

    fn activate(&self, validated: &ValidatedStage) -> Result<SnapshotCandidate, StoreError> {
        self.inner.activate(validated)
    }

    fn discard(&self, stage: SnapshotStage) -> Result<(), StoreError> {
        self.inner.discard(stage)
    }
}

#[derive(Default)]
struct PendingAppendSnapshotState {
    stage_active: bool,
    discards: usize,
}

impl PendingAppendSnapshotStore {
    pub fn stage_is_active(&self) -> bool {
        self.state
            .lock()
            .expect("pending snapshot state poisoned")
            .stage_active
    }

    pub fn discard_count(&self) -> usize {
        self.state
            .lock()
            .expect("pending snapshot state poisoned")
            .discards
    }
}

#[async_trait]
impl SnapshotStore for PendingAppendSnapshotStore {
    fn begin_stage(&self, request: SnapshotStageRequest) -> Result<SnapshotStage, StoreError> {
        self.state
            .lock()
            .expect("pending snapshot state poisoned")
            .stage_active = true;
        Ok(SnapshotStage::new(0, request.source()))
    }

    async fn append(&self, _stage: &SnapshotStage, _chunk: Bytes) -> Result<(), StoreError> {
        std::future::pending().await
    }

    async fn open_staged(
        &self,
        _stage: &SnapshotStage,
    ) -> Result<Box<dyn BufRead + Send>, StoreError> {
        Err(StoreError::Unavailable)
    }

    async fn prepare_activation(&self, _validated: &ValidatedStage) -> Result<(), StoreError> {
        Err(StoreError::Unavailable)
    }

    fn activate(&self, _validated: &ValidatedStage) -> Result<SnapshotCandidate, StoreError> {
        Err(StoreError::Unavailable)
    }

    fn discard(&self, _stage: SnapshotStage) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("pending snapshot state poisoned");
        if state.stage_active {
            state.stage_active = false;
            state.discards += 1;
        }
        Ok(())
    }
}

#[derive(Default)]
struct CountingSnapshotState {
    appends: usize,
    discards: usize,
    stage_active: bool,
}

impl CountingSnapshotStore {
    pub fn append_count(&self) -> usize {
        self.state
            .lock()
            .expect("counting snapshot state poisoned")
            .appends
    }

    pub fn discard_count(&self) -> usize {
        self.state
            .lock()
            .expect("counting snapshot state poisoned")
            .discards
    }
}

#[async_trait]
impl SnapshotStore for CountingSnapshotStore {
    fn begin_stage(&self, request: SnapshotStageRequest) -> Result<SnapshotStage, StoreError> {
        self.state
            .lock()
            .expect("counting snapshot state poisoned")
            .stage_active = true;
        Ok(SnapshotStage::new(0, request.source()))
    }

    async fn append(&self, _stage: &SnapshotStage, _chunk: Bytes) -> Result<(), StoreError> {
        self.state
            .lock()
            .expect("counting snapshot state poisoned")
            .appends += 1;
        Ok(())
    }

    async fn open_staged(
        &self,
        _stage: &SnapshotStage,
    ) -> Result<Box<dyn BufRead + Send>, StoreError> {
        Err(StoreError::Unavailable)
    }

    async fn prepare_activation(&self, _validated: &ValidatedStage) -> Result<(), StoreError> {
        Err(StoreError::Unavailable)
    }

    fn activate(&self, _validated: &ValidatedStage) -> Result<SnapshotCandidate, StoreError> {
        Err(StoreError::Unavailable)
    }

    fn discard(&self, _stage: SnapshotStage) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("counting snapshot state poisoned");
        if state.stage_active {
            state.stage_active = false;
            state.discards += 1;
        }
        Ok(())
    }
}

#[derive(Default)]
struct SnapshotState {
    next_token: u64,
    staged: HashMap<u64, Vec<u8>>,
    retained: HashMap<SnapshotSource, Vec<StoredSnapshot>>,
    scan_diagnostics: HashMap<SourceKind, Vec<SnapshotRecoveryReason>>,
    open_failures: HashSet<u64>,
    activations: usize,
    adoptions: usize,
    discards: usize,
    activation_failure: Option<StoreError>,
    adoption_failure: Option<StoreError>,
}

#[derive(Clone)]
struct StoredSnapshot {
    token: u64,
    metadata: SnapshotMetadata,
    bytes: Vec<u8>,
    active: bool,
}

impl MemorySnapshotStore {
    pub fn failing_activation(reason: StoreError) -> Self {
        Self {
            state: Arc::new(Mutex::new(SnapshotState {
                activation_failure: Some(reason),
                ..SnapshotState::default()
            })),
        }
    }

    pub fn activation_count(&self) -> usize {
        self.state
            .lock()
            .expect("snapshot state poisoned")
            .activations
    }

    pub fn discard_count(&self) -> usize {
        self.state.lock().expect("snapshot state poisoned").discards
    }

    pub fn adoption_count(&self) -> usize {
        self.state
            .lock()
            .expect("snapshot state poisoned")
            .adoptions
    }

    pub fn duplicate_active_as_fallback(&self, kind: SourceKind) {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        let token = state.next_token;
        state.next_token += 1;
        let snapshots = state
            .retained
            .values_mut()
            .find(|snapshots| {
                snapshots
                    .first()
                    .is_some_and(|snapshot| snapshot.metadata.source().kind() == kind)
            })
            .expect("active fixture snapshot exists");
        let mut fallback = snapshots
            .first()
            .expect("active fixture snapshot exists")
            .clone();
        fallback.token = token;
        fallback.active = false;
        snapshots.push(fallback);
        snapshots.truncate(2);
    }

    pub fn corrupt_active_payload(&self, kind: SourceKind) {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        let snapshot = active_snapshot_mut(&mut state, kind);
        snapshot.bytes.push(b'!');
    }

    pub fn replace_active_payload(&self, kind: SourceKind, bytes: Vec<u8>) {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        let snapshot = active_snapshot_mut(&mut state, kind);
        let checksum = *blake3::hash(&bytes).as_bytes();
        snapshot.metadata = SnapshotMetadata::new(
            snapshot.metadata.source(),
            bytes.len() as u64,
            checksum,
            snapshot.metadata.validated_at(),
            snapshot.metadata.validators().clone(),
        );
        snapshot.bytes = bytes;
    }

    pub fn set_active_length(&self, kind: SourceKind, decoded_bytes: u64) {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        let snapshot = active_snapshot_mut(&mut state, kind);
        snapshot.metadata = SnapshotMetadata::new(
            snapshot.metadata.source(),
            decoded_bytes,
            *snapshot.metadata.checksum(),
            snapshot.metadata.validated_at(),
            snapshot.metadata.validators().clone(),
        );
    }

    pub fn set_active_checksum(&self, kind: SourceKind, checksum: [u8; 32]) {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        let snapshot = active_snapshot_mut(&mut state, kind);
        snapshot.metadata = SnapshotMetadata::new(
            snapshot.metadata.source(),
            snapshot.metadata.decoded_bytes(),
            checksum,
            snapshot.metadata.validated_at(),
            snapshot.metadata.validators().clone(),
        );
    }

    pub fn set_active_validated_at(&self, kind: SourceKind, validated_at: DateTime<Utc>) {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        let snapshot = active_snapshot_mut(&mut state, kind);
        snapshot.metadata = SnapshotMetadata::new(
            snapshot.metadata.source(),
            snapshot.metadata.decoded_bytes(),
            *snapshot.metadata.checksum(),
            validated_at,
            snapshot.metadata.validators().clone(),
        );
    }

    pub fn with_scan_diagnostic(self, kind: SourceKind, reason: SnapshotRecoveryReason) -> Self {
        self.state
            .lock()
            .expect("snapshot state poisoned")
            .scan_diagnostics
            .entry(kind)
            .or_default()
            .push(reason);
        self
    }

    pub fn fail_active_open(&self, kind: SourceKind) {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        let token = active_snapshot_mut(&mut state, kind).token;
        state.open_failures.insert(token);
    }

    pub fn fail_adoption(&self, reason: StoreError) {
        self.state
            .lock()
            .expect("snapshot state poisoned")
            .adoption_failure = Some(reason);
    }

    pub fn active_validators(&self, kind: SourceKind) -> PrivateSourceValidators {
        let state = self.state.lock().expect("snapshot state poisoned");
        state
            .retained
            .values()
            .flat_map(|snapshots| snapshots.iter())
            .find(|snapshot| snapshot.active && snapshot.metadata.source().kind() == kind)
            .expect("active fixture snapshot exists")
            .metadata
            .validators()
            .clone()
    }
}

fn active_snapshot_mut(state: &mut SnapshotState, kind: SourceKind) -> &mut StoredSnapshot {
    state
        .retained
        .values_mut()
        .flat_map(|snapshots| snapshots.iter_mut())
        .find(|snapshot| snapshot.active && snapshot.metadata.source().kind() == kind)
        .expect("active fixture snapshot exists")
}

#[async_trait]
impl SnapshotStore for MemorySnapshotStore {
    async fn scan_candidates(&self, source: SnapshotSource) -> Result<SnapshotScan, StoreError> {
        let state = self.state.lock().expect("snapshot state poisoned");
        let candidates = state
            .retained
            .get(&source)
            .into_iter()
            .flatten()
            .map(|snapshot| {
                SnapshotCandidate::new(snapshot.token, snapshot.metadata.clone(), !snapshot.active)
            })
            .collect();
        let diagnostics = state
            .scan_diagnostics
            .get(&source.kind())
            .cloned()
            .unwrap_or_default();
        SnapshotScan::new(candidates, diagnostics)
    }

    async fn open_candidate(
        &self,
        candidate: &SnapshotCandidate,
    ) -> Result<Box<dyn BufRead + Send>, StoreError> {
        let state = self.state.lock().expect("snapshot state poisoned");
        if state.open_failures.contains(&candidate.token()) {
            return Err(StoreError::Unavailable);
        }
        let bytes = state
            .retained
            .values()
            .flat_map(|snapshots| snapshots.iter())
            .find(|snapshot| snapshot.token == candidate.token())
            .ok_or(StoreError::Unavailable)?
            .bytes
            .clone();
        Ok(Box::new(Cursor::new(bytes)))
    }

    async fn adopt_candidate(
        &self,
        candidate: &SnapshotCandidate,
    ) -> Result<SnapshotCandidate, StoreError> {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        if let Some(reason) = state.adoption_failure {
            return Err(reason);
        }
        let snapshots = state
            .retained
            .get_mut(&candidate.metadata().source())
            .ok_or(StoreError::Unavailable)?;
        if !snapshots
            .iter()
            .any(|snapshot| snapshot.token == candidate.token())
        {
            return Err(StoreError::Unavailable);
        }
        for snapshot in snapshots.iter_mut() {
            snapshot.active = snapshot.token == candidate.token();
        }
        snapshots.sort_by_key(|snapshot| !snapshot.active);
        state.adoptions += 1;
        Ok(SnapshotCandidate::new(
            candidate.token(),
            candidate.metadata().clone(),
            false,
        ))
    }

    async fn revalidate_candidate(
        &self,
        candidate: &SnapshotCandidate,
        revalidation: &SnapshotRevalidation,
    ) -> Result<SnapshotCandidate, StoreError> {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        let snapshot = state
            .retained
            .values_mut()
            .flat_map(|snapshots| snapshots.iter_mut())
            .find(|snapshot| snapshot.token == candidate.token())
            .ok_or(StoreError::Unavailable)?;
        snapshot.metadata = SnapshotMetadata::new(
            snapshot.metadata.source(),
            snapshot.metadata.decoded_bytes(),
            *snapshot.metadata.checksum(),
            revalidation.validated_at(),
            revalidation.validators().clone(),
        );
        Ok(SnapshotCandidate::new(
            snapshot.token,
            snapshot.metadata.clone(),
            !snapshot.active,
        ))
    }

    fn begin_stage(&self, request: SnapshotStageRequest) -> Result<SnapshotStage, StoreError> {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        let source = request.source();
        if let Some(protected) = request.protected()
            && (protected.metadata().source() != source
                || !state
                    .retained
                    .get(&source)
                    .into_iter()
                    .flatten()
                    .any(|snapshot| snapshot.token == protected.token()))
        {
            return Err(StoreError::Corrupt);
        }
        let token = state.next_token;
        state.next_token += 1;
        state.staged.insert(token, Vec::new());
        Ok(SnapshotStage::new(token, source))
    }

    async fn append(&self, stage: &SnapshotStage, chunk: Bytes) -> Result<(), StoreError> {
        self.state
            .lock()
            .expect("snapshot state poisoned")
            .staged
            .get_mut(&stage.token())
            .ok_or(StoreError::Unavailable)?
            .extend_from_slice(&chunk);
        Ok(())
    }

    async fn open_staged(
        &self,
        stage: &SnapshotStage,
    ) -> Result<Box<dyn BufRead + Send>, StoreError> {
        let bytes = self
            .state
            .lock()
            .expect("snapshot state poisoned")
            .staged
            .get(&stage.token())
            .ok_or(StoreError::Unavailable)?
            .clone();
        Ok(Box::new(Cursor::new(bytes)))
    }

    async fn prepare_activation(&self, validated: &ValidatedStage) -> Result<(), StoreError> {
        if self
            .state
            .lock()
            .expect("snapshot state poisoned")
            .staged
            .contains_key(&validated.stage().token())
        {
            Ok(())
        } else {
            Err(StoreError::Unavailable)
        }
    }

    fn activate(&self, validated: &ValidatedStage) -> Result<SnapshotCandidate, StoreError> {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        if let Some(reason) = state.activation_failure {
            return Err(reason);
        }
        let bytes = state
            .staged
            .remove(&validated.stage().token())
            .ok_or(StoreError::Unavailable)?;
        let source = validated.stage().source();
        let snapshots = state.retained.entry(source).or_default();
        for snapshot in snapshots.iter_mut() {
            snapshot.active = false;
        }
        snapshots.insert(
            0,
            StoredSnapshot {
                token: validated.stage().token(),
                metadata: validated.metadata(),
                bytes,
                active: true,
            },
        );
        snapshots.truncate(2);
        state.activations += 1;
        Ok(SnapshotCandidate::new(
            validated.stage().token(),
            validated.metadata(),
            false,
        ))
    }

    fn discard(&self, stage: SnapshotStage) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        if state.staged.remove(&stage.token()).is_some() {
            state.discards += 1;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct FixedClock(DateTime<Utc>);

impl Default for FixedClock {
    fn default() -> Self {
        Self(
            DateTime::parse_from_rfc3339("2026-08-29T12:00:00Z")
                .expect("valid fixed timestamp")
                .with_timezone(&Utc),
        )
    }
}

impl FixedClock {
    pub fn at(value: &str) -> Self {
        Self(
            DateTime::parse_from_rfc3339(value)
                .expect("valid fixed timestamp")
                .with_timezone(&Utc),
        )
    }
}

#[async_trait]
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }

    async fn wait_until(&self, _deadline: DateTime<Utc>) {
        std::future::pending().await
    }
}

pub fn adapters(source: ScriptedSource, snapshots: MemorySnapshotStore) -> CoreAdapters {
    CoreAdapters::new(
        Arc::new(source),
        Arc::new(snapshots),
        Arc::new(FixedClock::default()),
    )
}

pub fn adapters_at(
    source: ScriptedSource,
    snapshots: MemorySnapshotStore,
    now: &str,
) -> CoreAdapters {
    CoreAdapters::new(
        Arc::new(source),
        Arc::new(snapshots),
        Arc::new(FixedClock::at(now)),
    )
}

pub fn counting_adapters(source: ScriptedSource, snapshots: CountingSnapshotStore) -> CoreAdapters {
    CoreAdapters::new(
        Arc::new(source),
        Arc::new(snapshots),
        Arc::new(FixedClock::default()),
    )
}

pub fn pending_append_adapters(
    source: ScriptedSource,
    snapshots: PendingAppendSnapshotStore,
) -> CoreAdapters {
    CoreAdapters::new(
        Arc::new(source),
        Arc::new(snapshots),
        Arc::new(FixedClock::default()),
    )
}

pub fn pending_activation_adapters(
    source: ScriptedSource,
    snapshots: PendingActivationSnapshotStore,
) -> CoreAdapters {
    CoreAdapters::new(
        Arc::new(source),
        Arc::new(snapshots),
        Arc::new(FixedClock::default()),
    )
}
