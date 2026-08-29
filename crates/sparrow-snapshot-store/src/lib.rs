//! Crash-safe filesystem persistence for Sparrow Source Snapshots.
//!
//! The core owns snapshot policy and validation. This crate owns only the
//! bounded two-slot filesystem protocol behind that seam.

mod disk;
mod layout;
mod manifest;

use std::{fmt, path::Path, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;
use disk::{DiskCandidate, DiskError, DiskStore};
use layout::{DiskKind, Slot};
use manifest::{DiskMetadata, DiskSource, DiskValidators};
use sparrow_core::{
    PrivateSourceValidators, SnapshotCandidate, SnapshotMetadata, SnapshotRevalidation,
    SnapshotScan, SnapshotSource, SnapshotStage, SnapshotStageRequest, SnapshotStore, SourceKind,
    StoreError, ValidatedStage,
};

/// A two-slot, crash-safe Source Snapshot store rooted in private app data.
pub struct AtomicFileSnapshotStore {
    disk: Arc<DiskStore>,
}

impl AtomicFileSnapshotStore {
    /// Opens or creates the private snapshot directory and removes abandoned
    /// transient files from an earlier process.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, SnapshotStoreOpenError> {
        DiskStore::open(root.as_ref())
            .map(|disk| Self {
                disk: Arc::new(disk),
            })
            .map_err(SnapshotStoreOpenError::from)
    }
}

impl fmt::Debug for AtomicFileSnapshotStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = &self.disk;
        formatter.write_str("AtomicFileSnapshotStore(<private>)")
    }
}

#[async_trait]
impl SnapshotStore for AtomicFileSnapshotStore {
    async fn scan_candidates(&self, source: SnapshotSource) -> Result<SnapshotScan, StoreError> {
        let scan = self
            .disk
            .scan(source_to_disk(source))
            .map_err(store_error)?;
        let candidates = scan
            .candidates
            .into_iter()
            .map(|candidate| candidate_to_core(candidate, source))
            .collect::<Result<Vec<_>, _>>()?;
        SnapshotScan::new(candidates, scan.diagnostics)
    }

    async fn open_candidate(
        &self,
        candidate: &SnapshotCandidate,
    ) -> Result<Box<dyn std::io::BufRead + Send>, StoreError> {
        let disk = candidate_from_core(candidate)?;
        self.disk.open_candidate(&disk).map_err(store_error)
    }

    async fn adopt_candidate(
        &self,
        candidate: &SnapshotCandidate,
    ) -> Result<SnapshotCandidate, StoreError> {
        let disk = candidate_from_core(candidate)?;
        let adopted = self.disk.adopt(&disk).map_err(store_error)?;
        candidate_to_core(adopted, candidate.metadata().source())
    }

    async fn revalidate_candidate(
        &self,
        candidate: &SnapshotCandidate,
        revalidation: &SnapshotRevalidation,
    ) -> Result<SnapshotCandidate, StoreError> {
        let disk = candidate_from_core(candidate)?;
        let updated = self
            .disk
            .revalidate(
                &disk,
                revalidation.validated_at(),
                validators_to_disk(revalidation.validators()),
            )
            .map_err(store_error)?;
        candidate_to_core(updated, candidate.metadata().source())
    }

    fn begin_stage(&self, request: SnapshotStageRequest) -> Result<SnapshotStage, StoreError> {
        let source = request.source();
        let protected = request.protected().map(candidate_from_core).transpose()?;
        self.disk
            .begin_stage(source_to_disk(source), protected.as_ref())
            .map(|token| SnapshotStage::new(token, source))
            .map_err(store_error)
    }

    async fn append(&self, stage: &SnapshotStage, chunk: Bytes) -> Result<(), StoreError> {
        self.disk
            .append(
                stage.token(),
                source_to_disk(stage.source()),
                chunk.as_ref(),
            )
            .map_err(store_error)
    }

    async fn open_staged(
        &self,
        stage: &SnapshotStage,
    ) -> Result<Box<dyn std::io::BufRead + Send>, StoreError> {
        self.disk
            .open_staged(stage.token(), source_to_disk(stage.source()))
            .map_err(store_error)
    }

    async fn prepare_activation(&self, validated: &ValidatedStage) -> Result<(), StoreError> {
        let metadata = metadata_to_disk(&validated.metadata());
        self.disk
            .prepare(validated.stage().token(), &metadata)
            .map_err(store_error)
    }

    fn activate(&self, validated: &ValidatedStage) -> Result<SnapshotCandidate, StoreError> {
        let source = validated.stage().source();
        let metadata = metadata_to_disk(&validated.metadata());
        let candidate = self
            .disk
            .activate(validated.stage().token(), &metadata)
            .map_err(store_error)?;
        candidate_to_core(candidate, source)
    }

    fn discard(&self, stage: SnapshotStage) -> Result<(), StoreError> {
        self.disk
            .discard(stage.token(), source_to_disk(stage.source()))
            .map_err(store_error)
    }
}

fn source_to_disk(source: SnapshotSource) -> DiskSource {
    DiskSource {
        kind: match source.kind() {
            SourceKind::M3u => DiskKind::M3u,
            SourceKind::Epg => DiskKind::Epg,
        },
        key: *source.key().as_bytes(),
    }
}

fn metadata_to_disk(metadata: &SnapshotMetadata) -> DiskMetadata {
    DiskMetadata {
        source: source_to_disk(metadata.source()),
        decoded_bytes: metadata.decoded_bytes(),
        checksum: *metadata.checksum(),
        validated_at: metadata.validated_at(),
        validators: validators_to_disk(metadata.validators()),
    }
}

fn validators_to_disk(validators: &PrivateSourceValidators) -> DiskValidators {
    DiskValidators {
        etag: validators.expose_etag().map(str::to_owned),
        last_modified: validators.expose_last_modified().map(str::to_owned),
    }
}

fn candidate_to_core(
    candidate: DiskCandidate,
    source: SnapshotSource,
) -> Result<SnapshotCandidate, StoreError> {
    if candidate.metadata.source != source_to_disk(source) {
        return Err(StoreError::Corrupt);
    }
    let validators = PrivateSourceValidators::parse(
        candidate.metadata.validators.etag.clone(),
        candidate.metadata.validators.last_modified.clone(),
    )
    .map_err(|_| StoreError::Corrupt)?;
    let metadata = SnapshotMetadata::new(
        source,
        candidate.metadata.decoded_bytes,
        candidate.metadata.checksum,
        candidate.metadata.validated_at,
        validators,
    );
    Ok(SnapshotCandidate::new(
        candidate_token(candidate.metadata.source.kind, candidate.slot),
        metadata,
        candidate.requires_adoption,
    ))
}

fn candidate_from_core(candidate: &SnapshotCandidate) -> Result<DiskCandidate, StoreError> {
    let metadata = metadata_to_disk(candidate.metadata());
    let (kind, slot) = decode_candidate_token(candidate.token()).ok_or(StoreError::Corrupt)?;
    if kind != metadata.source.kind {
        return Err(StoreError::Corrupt);
    }
    Ok(DiskCandidate {
        slot,
        metadata,
        requires_adoption: candidate.requires_adoption(),
    })
}

fn candidate_token(kind: DiskKind, slot: Slot) -> u64 {
    match (kind, slot) {
        (DiskKind::M3u, Slot::A) => 1,
        (DiskKind::M3u, Slot::B) => 2,
        (DiskKind::Epg, Slot::A) => 3,
        (DiskKind::Epg, Slot::B) => 4,
    }
}

fn decode_candidate_token(token: u64) -> Option<(DiskKind, Slot)> {
    match token {
        1 => Some((DiskKind::M3u, Slot::A)),
        2 => Some((DiskKind::M3u, Slot::B)),
        3 => Some((DiskKind::Epg, Slot::A)),
        4 => Some((DiskKind::Epg, Slot::B)),
        _ => None,
    }
}

fn store_error(error: DiskError) -> StoreError {
    match error {
        DiskError::Capacity => StoreError::Capacity,
        DiskError::Corrupt | DiskError::UnsafeLayout => StoreError::Corrupt,
        DiskError::Unavailable | DiskError::Busy => StoreError::Unavailable,
    }
}

/// Safe construction failure that never retains a filesystem path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SnapshotStoreOpenError {
    #[error("snapshot storage is unavailable")]
    Unavailable,
    #[error("snapshot storage has insufficient capacity")]
    Capacity,
    #[error("snapshot storage is not private")]
    UnsafeLayout,
}

impl From<DiskError> for SnapshotStoreOpenError {
    fn from(error: DiskError) -> Self {
        match error {
            DiskError::Capacity => Self::Capacity,
            DiskError::UnsafeLayout => Self::UnsafeLayout,
            DiskError::Unavailable | DiskError::Corrupt | DiskError::Busy => Self::Unavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn public_debug_and_open_errors_never_expose_the_storage_root() {
        let directory = TempDir::new().expect("temporary directory");
        let private_root = directory.path().join("private-root-canary");
        let store = AtomicFileSnapshotStore::open(&private_root).expect("store opens");
        assert_eq!(format!("{store:?}"), "AtomicFileSnapshotStore(<private>)");
        assert!(!format!("{store:?}").contains("private-root-canary"));

        let invalid_root = directory.path().join("invalid-root-canary");
        fs::write(&invalid_root, b"not a directory").expect("invalid root writes");
        let error = AtomicFileSnapshotStore::open(&invalid_root).expect_err("open fails safely");
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("invalid-root-canary"));
        assert!(!rendered.contains(&invalid_root.display().to_string()));
    }
}
