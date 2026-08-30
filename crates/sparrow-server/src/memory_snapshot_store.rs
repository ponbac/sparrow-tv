use std::{
    collections::HashMap,
    io::{BufRead, Cursor},
    sync::{Mutex, MutexGuard},
};

use async_trait::async_trait;
use bytes::Bytes;
use sparrow_core::{
    SnapshotCandidate, SnapshotMetadata, SnapshotRevalidation, SnapshotScan, SnapshotSource,
    SnapshotStage, SnapshotStageRequest, SnapshotStore, StoreError, ValidatedStage,
};

/// Process-lifetime implementation of the core's atomic snapshot seam.
///
/// Payloads never leave memory, but candidate identity, protected-generation,
/// preparation, activation, fallback retention, and revalidation rules match
/// the durable adapter's observable contract.
#[derive(Default)]
pub(crate) struct MemorySnapshotStore {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    next_token: u64,
    staged: HashMap<u64, StagedSnapshot>,
    retained: HashMap<SnapshotSource, Vec<StoredSnapshot>>,
    #[cfg(test)]
    fail_next_adoption: bool,
}

struct StagedSnapshot {
    source: SnapshotSource,
    protected: Option<SnapshotCandidate>,
    bytes: Vec<u8>,
    prepared: Option<SnapshotMetadata>,
}

struct StoredSnapshot {
    token: u64,
    metadata: SnapshotMetadata,
    bytes: Vec<u8>,
    active: bool,
}

#[async_trait]
impl SnapshotStore for MemorySnapshotStore {
    async fn scan_candidates(&self, source: SnapshotSource) -> Result<SnapshotScan, StoreError> {
        let state = self.lock()?;
        let candidates = state
            .retained
            .get(&source)
            .into_iter()
            .flatten()
            .map(StoredSnapshot::candidate)
            .collect();
        SnapshotScan::new(candidates, Vec::new())
    }

    async fn open_candidate(
        &self,
        candidate: &SnapshotCandidate,
    ) -> Result<Box<dyn BufRead + Send>, StoreError> {
        let state = self.lock()?;
        let stored = exact_candidate(&state, candidate)?;
        Ok(Box::new(Cursor::new(copy_bytes(&stored.bytes)?)))
    }

    async fn adopt_candidate(
        &self,
        candidate: &SnapshotCandidate,
    ) -> Result<SnapshotCandidate, StoreError> {
        let mut state = self.lock()?;
        #[cfg(test)]
        if std::mem::take(&mut state.fail_next_adoption) {
            return Err(StoreError::Unavailable);
        }
        exact_candidate(&state, candidate)?;
        let snapshots = state
            .retained
            .get_mut(&candidate.metadata().source())
            .ok_or(StoreError::Unavailable)?;
        for snapshot in snapshots.iter_mut() {
            snapshot.active = snapshot.token == candidate.token();
        }
        snapshots.sort_by_key(|snapshot| !snapshot.active);
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
        let mut state = self.lock()?;
        exact_candidate(&state, candidate)?;
        let snapshot = state
            .retained
            .get_mut(&candidate.metadata().source())
            .and_then(|snapshots| {
                snapshots
                    .iter_mut()
                    .find(|snapshot| snapshot.token == candidate.token())
            })
            .ok_or(StoreError::Unavailable)?;
        snapshot.metadata = SnapshotMetadata::new(
            snapshot.metadata.source(),
            snapshot.metadata.decoded_bytes(),
            *snapshot.metadata.checksum(),
            revalidation.validated_at(),
            revalidation.validators().clone(),
        );
        Ok(snapshot.candidate())
    }

    fn begin_stage(&self, request: SnapshotStageRequest) -> Result<SnapshotStage, StoreError> {
        let mut state = self.lock()?;
        if let Some(protected) = request.protected() {
            exact_candidate(&state, protected).map_err(|_| StoreError::Corrupt)?;
            if protected.metadata().source() != request.source() {
                return Err(StoreError::Corrupt);
            }
        }

        let token = state.next_token;
        state.next_token = state
            .next_token
            .checked_add(1)
            .ok_or(StoreError::Capacity)?;
        if state.staged.contains_key(&token)
            || state
                .retained
                .values()
                .flatten()
                .any(|snapshot| snapshot.token == token)
        {
            return Err(StoreError::Corrupt);
        }
        state.staged.insert(
            token,
            StagedSnapshot {
                source: request.source(),
                protected: request.protected().cloned(),
                bytes: Vec::new(),
                prepared: None,
            },
        );
        Ok(SnapshotStage::new(token, request.source()))
    }

    async fn append(&self, stage: &SnapshotStage, chunk: Bytes) -> Result<(), StoreError> {
        let mut state = self.lock()?;
        let staged = exact_stage_mut(&mut state, stage)?;
        if staged.prepared.is_some() {
            return Err(StoreError::Corrupt);
        }
        staged
            .bytes
            .try_reserve(chunk.len())
            .map_err(|_| StoreError::Capacity)?;
        staged.bytes.extend_from_slice(&chunk);
        Ok(())
    }

    async fn open_staged(
        &self,
        stage: &SnapshotStage,
    ) -> Result<Box<dyn BufRead + Send>, StoreError> {
        let state = self.lock()?;
        let staged = exact_stage(&state, stage)?;
        Ok(Box::new(Cursor::new(copy_bytes(&staged.bytes)?)))
    }

    async fn prepare_activation(&self, validated: &ValidatedStage) -> Result<(), StoreError> {
        let mut state = self.lock()?;
        validate_protected(
            &state,
            exact_stage(&state, validated.stage())?.protected.as_ref(),
        )?;
        let staged = exact_stage_mut(&mut state, validated.stage())?;
        let decoded_bytes = u64::try_from(staged.bytes.len()).map_err(|_| StoreError::Capacity)?;
        if decoded_bytes != validated.decoded_bytes()
            || blake3::hash(&staged.bytes).as_bytes() != validated.checksum()
        {
            return Err(StoreError::Corrupt);
        }
        staged.prepared = Some(validated.metadata());
        Ok(())
    }

    fn activate(&self, validated: &ValidatedStage) -> Result<SnapshotCandidate, StoreError> {
        let mut state = self.lock()?;
        let staged = exact_stage(&state, validated.stage())?;
        validate_protected(&state, staged.protected.as_ref())?;
        if staged.prepared.as_ref() != Some(&validated.metadata()) {
            return Err(StoreError::Corrupt);
        }

        let staged = state
            .staged
            .remove(&validated.stage().token())
            .ok_or(StoreError::Unavailable)?;
        let source = validated.stage().source();
        let snapshots = state.retained.entry(source).or_default();
        for snapshot in snapshots.iter_mut() {
            snapshot.active = false;
        }
        if let Some(protected) = staged.protected.as_ref() {
            snapshots.retain(|snapshot| snapshot.token == protected.token());
        }
        snapshots.insert(
            0,
            StoredSnapshot {
                token: validated.stage().token(),
                metadata: validated.metadata(),
                bytes: staged.bytes,
                active: true,
            },
        );
        snapshots.truncate(2);
        Ok(snapshots[0].candidate())
    }

    fn discard(&self, stage: SnapshotStage) -> Result<(), StoreError> {
        let mut state = self.lock()?;
        if state
            .staged
            .get(&stage.token())
            .is_some_and(|staged| staged.source != stage.source())
        {
            return Err(StoreError::Corrupt);
        }
        state.staged.remove(&stage.token());
        Ok(())
    }
}

impl MemorySnapshotStore {
    fn lock(&self) -> Result<MutexGuard<'_, State>, StoreError> {
        self.state.lock().map_err(|_| StoreError::Unavailable)
    }

    #[cfg(test)]
    pub(crate) fn corrupt_active_payload(&self) -> Result<(), StoreError> {
        let mut state = self.lock()?;
        let active = state
            .retained
            .values_mut()
            .flatten()
            .find(|snapshot| snapshot.active)
            .ok_or(StoreError::Unavailable)?;
        active.bytes.push(0);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_adoption(&self) -> Result<(), StoreError> {
        self.lock()?.fail_next_adoption = true;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn retained_snapshot_count(&self) -> Result<usize, StoreError> {
        Ok(self.lock()?.retained.values().flatten().count())
    }
}

impl StoredSnapshot {
    fn candidate(&self) -> SnapshotCandidate {
        SnapshotCandidate::new(self.token, self.metadata.clone(), !self.active)
    }
}

fn exact_candidate<'a>(
    state: &'a State,
    candidate: &SnapshotCandidate,
) -> Result<&'a StoredSnapshot, StoreError> {
    let snapshot = state
        .retained
        .get(&candidate.metadata().source())
        .and_then(|snapshots| {
            snapshots
                .iter()
                .find(|snapshot| snapshot.token == candidate.token())
        })
        .ok_or(StoreError::Unavailable)?;
    if snapshot.candidate() != *candidate {
        return Err(StoreError::Corrupt);
    }
    Ok(snapshot)
}

fn exact_stage<'a>(
    state: &'a State,
    stage: &SnapshotStage,
) -> Result<&'a StagedSnapshot, StoreError> {
    let staged = state
        .staged
        .get(&stage.token())
        .ok_or(StoreError::Unavailable)?;
    if staged.source != stage.source() {
        return Err(StoreError::Corrupt);
    }
    Ok(staged)
}

fn exact_stage_mut<'a>(
    state: &'a mut State,
    stage: &SnapshotStage,
) -> Result<&'a mut StagedSnapshot, StoreError> {
    let staged = state
        .staged
        .get_mut(&stage.token())
        .ok_or(StoreError::Unavailable)?;
    if staged.source != stage.source() {
        return Err(StoreError::Corrupt);
    }
    Ok(staged)
}

fn validate_protected(
    state: &State,
    protected: Option<&SnapshotCandidate>,
) -> Result<(), StoreError> {
    let Some(protected) = protected else {
        return Ok(());
    };
    exact_candidate(state, protected).map_err(|_| StoreError::Corrupt)?;
    Ok(())
}

fn copy_bytes(source: &[u8]) -> Result<Vec<u8>, StoreError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(source.len())
        .map_err(|_| StoreError::Capacity)?;
    bytes.extend_from_slice(source);
    Ok(bytes)
}
