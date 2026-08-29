#![allow(dead_code)]

use std::{
    collections::HashMap,
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
    Clock, CoreAdapters, SnapshotSource, SnapshotStage, SnapshotStore, SourceAccess,
    SourceAccessError, SourceByteStream, SourceKind, SourceReadError, SourceRequest,
    SourceResponse, StoreError, ValidatedStage,
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
                },
            );
        self
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
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessError> {
        let kind = request.kind();
        let mut state = self.state.lock().expect("source state poisoned");
        *state.opens.entry(kind).or_default() += 1;
        state.request_debug.insert(kind, format!("{request:?}"));
        let script = state
            .scripts
            .get(&kind)
            .ok_or(SourceAccessError::Unavailable)?;
        let chunks = script.chunks.clone();
        let declared_length = script.declared_length;
        let body: SourceByteStream = Box::pin(stream::iter(chunks));
        Ok(SourceResponse::new(declared_length, body))
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
    fn begin_stage(&self, source: SnapshotSource) -> Result<SnapshotStage, StoreError> {
        self.inner.begin_stage(source)
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

    fn activate(&self, validated: &ValidatedStage) -> Result<(), StoreError> {
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
    fn begin_stage(&self, source: SnapshotSource) -> Result<SnapshotStage, StoreError> {
        self.state
            .lock()
            .expect("pending snapshot state poisoned")
            .stage_active = true;
        Ok(SnapshotStage::new(0, source))
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

    fn activate(&self, _validated: &ValidatedStage) -> Result<(), StoreError> {
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
    fn begin_stage(&self, source: SnapshotSource) -> Result<SnapshotStage, StoreError> {
        self.state
            .lock()
            .expect("counting snapshot state poisoned")
            .stage_active = true;
        Ok(SnapshotStage::new(0, source))
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

    fn activate(&self, _validated: &ValidatedStage) -> Result<(), StoreError> {
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
    activations: usize,
    discards: usize,
    activation_failure: Option<StoreError>,
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
}

#[async_trait]
impl SnapshotStore for MemorySnapshotStore {
    fn begin_stage(&self, source: SnapshotSource) -> Result<SnapshotStage, StoreError> {
        let mut state = self.state.lock().expect("snapshot state poisoned");
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

    fn activate(&self, validated: &ValidatedStage) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("snapshot state poisoned");
        if let Some(reason) = state.activation_failure {
            return Err(reason);
        }
        state
            .staged
            .remove(&validated.stage().token())
            .ok_or(StoreError::Unavailable)?;
        state.activations += 1;
        Ok(())
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

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

pub fn adapters(source: ScriptedSource, snapshots: MemorySnapshotStore) -> CoreAdapters {
    CoreAdapters::new(
        Arc::new(source),
        Arc::new(snapshots),
        Arc::new(FixedClock::default()),
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
