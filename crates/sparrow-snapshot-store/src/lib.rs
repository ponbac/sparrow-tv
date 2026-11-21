//! Crash-safe filesystem persistence for Sparrow Source Snapshots.
//!
//! The core owns snapshot policy and validation. This crate owns only the
//! bounded two-slot filesystem protocol behind that seam.

mod catalog_cache;
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
    CatalogCacheMapping, PrivateSourceValidators, SnapshotCandidate, SnapshotMetadata,
    SnapshotRevalidation, SnapshotScan, SnapshotSource, SnapshotStage, SnapshotStageRequest,
    SnapshotStore, SourceKind, StoreError, ValidatedStage,
};

/// A two-slot, crash-safe Source Snapshot store rooted in private app data.
pub struct AtomicFileSnapshotStore {
    disk: Arc<DiskStore>,
    catalog_cache: catalog_cache::CatalogCache,
}

impl AtomicFileSnapshotStore {
    /// Opens or creates the private snapshot directory and removes abandoned
    /// transient files from an earlier process.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, SnapshotStoreOpenError> {
        DiskStore::open(root.as_ref())
            .map(|disk| Self {
                disk: Arc::new(disk),
                catalog_cache: catalog_cache::CatalogCache::new(root.as_ref()),
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
    fn catalog_cache_enabled(&self) -> bool {
        true
    }

    fn open_mapped_catalog_cache(
        &self,
        key: &[u8; 32],
        candidates: &[&SnapshotCandidate],
        validate: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Option<CatalogCacheMapping> {
        let disk_candidates = candidates
            .iter()
            .map(|candidate| candidate_from_core(candidate).ok())
            .collect::<Option<Vec<_>>>()?;
        let (live_stamp, max_freshness) = self.disk.candidates_payload_stamp(&disk_candidates)?;
        self.catalog_cache.open(
            key,
            &live_stamp,
            max_freshness,
            || {
                self.disk
                    .verified_candidates_payload_stamp(&disk_candidates, &|| false)
            },
            validate,
        )
    }

    fn write_mapped_catalog_cache(
        &self,
        key: &[u8; 32],
        candidates: &[&SnapshotCandidate],
        bytes: &[u8],
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Option<CatalogCacheMapping> {
        // Reject oversized or already-cancelled writes BEFORE hashing raw
        // snapshot payloads on disk.
        if !catalog_cache::CatalogCache::accepts_payload_len(bytes.len()) || is_cancelled() {
            return None;
        }
        let disk_candidates = candidates
            .iter()
            .map(|candidate| candidate_from_core(candidate).ok())
            .collect::<Option<Vec<_>>>()?;
        let verified_stamp = self
            .disk
            .verified_candidates_payload_stamp(&disk_candidates, is_cancelled)?;
        self.catalog_cache
            .write(key, verified_stamp, bytes, is_cancelled)
    }

    async fn scan_candidates(&self, source: SnapshotSource) -> Result<SnapshotScan, StoreError> {
        let disk = Arc::clone(&self.disk);
        let disk_source = source_to_disk(source);
        let scan = run_disk_blocking(move || disk.scan(disk_source))
            .await
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
        let disk = Arc::clone(&self.disk);
        let candidate = candidate_from_core(candidate)?;
        run_disk_blocking(move || disk.open_candidate(&candidate))
            .await
            .map_err(store_error)
    }

    async fn adopt_candidate(
        &self,
        candidate: &SnapshotCandidate,
    ) -> Result<SnapshotCandidate, StoreError> {
        let disk = Arc::clone(&self.disk);
        let source = candidate.metadata().source();
        let candidate = candidate_from_core(candidate)?;
        let adopted = run_disk_blocking(move || disk.adopt(&candidate))
            .await
            .map_err(store_error)?;
        candidate_to_core(adopted, source)
    }

    async fn revalidate_candidate(
        &self,
        candidate: &SnapshotCandidate,
        revalidation: &SnapshotRevalidation,
    ) -> Result<SnapshotCandidate, StoreError> {
        let disk = Arc::clone(&self.disk);
        let source = candidate.metadata().source();
        let candidate = candidate_from_core(candidate)?;
        let validated_at = revalidation.validated_at();
        let validators = validators_to_disk(revalidation.validators());
        let updated =
            run_disk_blocking(move || disk.revalidate(&candidate, validated_at, validators))
                .await
                .map_err(store_error)?;
        candidate_to_core(updated, source)
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
        let disk = Arc::clone(&self.disk);
        let token = stage.token();
        let source = source_to_disk(stage.source());
        run_disk_blocking(move || disk.append(token, source, chunk.as_ref()))
            .await
            .map_err(store_error)
    }

    async fn open_staged(
        &self,
        stage: &SnapshotStage,
    ) -> Result<Box<dyn std::io::BufRead + Send>, StoreError> {
        let disk = Arc::clone(&self.disk);
        let token = stage.token();
        let source = source_to_disk(stage.source());
        run_disk_blocking(move || disk.open_staged(token, source))
            .await
            .map_err(store_error)
    }

    async fn prepare_activation(&self, validated: &ValidatedStage) -> Result<(), StoreError> {
        let disk = Arc::clone(&self.disk);
        let token = validated.stage().token();
        let metadata = metadata_to_disk(&validated.metadata());
        run_disk_blocking(move || disk.prepare(token, &metadata))
            .await
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

async fn run_disk_blocking<F, R>(job: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle
            .spawn_blocking(job)
            .await
            .expect("snapshot store blocking task did not panic"),
        Err(_) => job(),
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
        candidate_token(
            candidate.metadata.source.kind,
            candidate.slot,
            candidate.observed_pointer_epoch,
        ),
        metadata,
        candidate.requires_adoption,
    ))
}

fn candidate_from_core(candidate: &SnapshotCandidate) -> Result<DiskCandidate, StoreError> {
    let metadata = metadata_to_disk(candidate.metadata());
    let (kind, slot, observed_pointer_epoch) =
        decode_candidate_token(candidate.token()).ok_or(StoreError::Corrupt)?;
    if kind != metadata.source.kind {
        return Err(StoreError::Corrupt);
    }
    Ok(DiskCandidate {
        slot,
        metadata,
        requires_adoption: candidate.requires_adoption(),
        observed_pointer_epoch,
    })
}

const CANDIDATE_TOKEN_TAG_SHIFT: u32 = 60;
const CANDIDATE_TOKEN_TAG: u64 = 0x5;
const CANDIDATE_TOKEN_EPOCH_MASK: u64 = (1_u64 << 58) - 1;

fn candidate_token(kind: DiskKind, slot: Slot, observed_pointer_epoch: u64) -> u64 {
    let slot_bits = match (kind, slot) {
        (DiskKind::M3u, Slot::A) => 0_u64,
        (DiskKind::M3u, Slot::B) => 1_u64,
        (DiskKind::Epg, Slot::A) => 2_u64,
        (DiskKind::Epg, Slot::B) => 3_u64,
    };
    (CANDIDATE_TOKEN_TAG << CANDIDATE_TOKEN_TAG_SHIFT)
        | ((observed_pointer_epoch & CANDIDATE_TOKEN_EPOCH_MASK) << 2)
        | slot_bits
}

fn decode_candidate_token(token: u64) -> Option<(DiskKind, Slot, u64)> {
    if (token >> CANDIDATE_TOKEN_TAG_SHIFT) != CANDIDATE_TOKEN_TAG {
        return None;
    }
    let observed_pointer_epoch = (token >> 2) & CANDIDATE_TOKEN_EPOCH_MASK;
    let (kind, slot) = match token & 0b11 {
        0 => (DiskKind::M3u, Slot::A),
        1 => (DiskKind::M3u, Slot::B),
        2 => (DiskKind::Epg, Slot::A),
        3 => (DiskKind::Epg, Slot::B),
        _ => unreachable!("2-bit mask has 4 values"),
    };
    Some((kind, slot, observed_pointer_epoch))
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

    #[tokio::test]
    async fn same_key_byte_identical_restore_rehashes_once_and_persists_verified_stamp() {
        use chrono::Utc;
        use futures_util::stream;
        use sparrow_core::{
            Clock, CoreAdapters, SourceAccess, SourceAccessError, SourceAccessFailure,
            SourceConfigurationInput, SourceRequest, SourceResponse, SparrowCore,
        };
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        struct StaticSource(Option<&'static [u8]>);
        #[async_trait]
        impl SourceAccess for StaticSource {
            async fn open(
                &self,
                _request: SourceRequest,
            ) -> Result<SourceResponse, SourceAccessFailure> {
                let bytes = self.0.ok_or(SourceAccessError::Unavailable)?;
                Ok(SourceResponse::new(
                    Some(bytes.len() as u64),
                    Box::pin(stream::iter([Ok(Bytes::from_static(bytes))])),
                ))
            }
        }
        struct NowClock;
        #[async_trait]
        impl Clock for NowClock {
            fn now(&self) -> chrono::DateTime<Utc> {
                Utc::now()
            }

            async fn wait_until(&self, _instant: chrono::DateTime<Utc>) {
                std::future::pending::<()>().await;
            }
        }

        const PAYLOAD: &[u8] = b"#EXTM3U\n#EXTINF:-1,Alpha\nhttps://media.fixture.invalid/alpha\n";
        let config = || {
            SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
                "https://private.fixture.invalid/channels.m3u",
                None::<String>,
            ))
            .expect("config parses")
        };

        let directory = TempDir::new().expect("temporary directory");
        let store = Arc::new(AtomicFileSnapshotStore::open(directory.path()).expect("store opens"));
        // Seed M3U snapshot online.
        let seeded = SparrowCore::bootstrap(
            Some(config()),
            CoreAdapters::new(
                Arc::new(StaticSource(Some(PAYLOAD))),
                Arc::clone(&store) as _,
                Arc::new(NowClock),
            ),
        )
        .await
        .expect("seed succeeds");
        drop(seeded);

        // Wait until the filesystem clock strictly advances past the payload's
        // timestamps before building and persisting the processed cache.
        let payload_path = directory.path().join("m3u/slot-a.payload");
        wait_for_fs_tick_after(directory.path(), &[&payload_path]);
        let builder = SparrowCore::bootstrap_from_snapshots(
            Some(config()),
            CoreAdapters::new(
                Arc::new(StaticSource(None)),
                Arc::clone(&store) as _,
                Arc::new(NowClock),
            ),
        )
        .await
        .expect("builder opens");
        let cache_path = directory.path().join("catalog-cache-v1");
        let stamp_path = directory.path().join("catalog-cache-v1.stamp");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while !stamp_path.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "cache and stamp sidecar were not written"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        drop(builder);
        #[cfg(unix)]
        let cache_inode = fs::metadata(&cache_path).expect("cache exists").ino();

        // Warm open before restore performs zero raw payload checksum reads.
        store.disk.reset_verify_payload_reads();
        let warm = SparrowCore::bootstrap_installed(
            Some(config()),
            CoreAdapters::new(
                Arc::new(StaticSource(None)),
                Arc::clone(&store) as _,
                Arc::new(NowClock),
            ),
        )
        .await
        .expect("warm open succeeds");
        assert!(warm.status().generation().is_some());
        assert_eq!(store.disk.verify_payload_reads(), 0);
        drop(warm);

        // Byte-identical external restore (same cache key, new inode/mtime,
        // preserving private 0o600 mode so scan does not bump ctime).
        let payload_path = directory.path().join("m3u/slot-a.payload");
        let restored_tmp = directory.path().join("m3u/restore-copy.tmp");
        fs::write(&restored_tmp, PAYLOAD).expect("restored copy writes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&restored_tmp, fs::Permissions::from_mode(0o600))
                .expect("restored mode sets");
        }
        fs::rename(&restored_tmp, &payload_path).expect("restored copy replaces payload");
        wait_for_fs_tick_after(directory.path(), &[&payload_path]);

        // Restart 1 after restore: verifies the raw payload once, keeps the
        // mapped cache inode immutable, and persists the refreshed stamp sidecar.
        store.disk.reset_verify_payload_reads();
        let restart_1 = SparrowCore::bootstrap_installed(
            Some(config()),
            CoreAdapters::new(
                Arc::new(StaticSource(None)),
                Arc::clone(&store) as _,
                Arc::new(NowClock),
            ),
        )
        .await
        .expect("restart 1 opens");
        assert!(restart_1.status().generation().is_some());
        assert_eq!(store.disk.verify_payload_reads(), 1);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&cache_path).expect("cache exists").ino(),
            cache_inode
        );
        drop(restart_1);

        // Restart 2 after restore: fast path hits with ZERO raw checksum reads.
        store.disk.reset_verify_payload_reads();
        let restart_2 = SparrowCore::bootstrap_installed(
            Some(config()),
            CoreAdapters::new(
                Arc::new(StaticSource(None)),
                Arc::clone(&store) as _,
                Arc::new(NowClock),
            ),
        )
        .await
        .expect("restart 2 opens");
        assert!(restart_2.status().generation().is_some());
        assert_eq!(store.disk.verify_payload_reads(), 0);
    }

    fn wait_for_fs_tick_after(root: &Path, paths: &[&Path]) {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        let freshness = |meta: &fs::Metadata| -> (i64, i64) {
            #[cfg(unix)]
            {
                (meta.mtime(), meta.mtime_nsec()).max((meta.ctime(), meta.ctime_nsec()))
            }
            #[cfg(not(unix))]
            {
                let d = meta
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap();
                (d.as_secs() as i64, i64::from(d.subsec_nanos()))
            }
        };
        let target = paths
            .iter()
            .map(|p| freshness(&fs::symlink_metadata(p).expect("target file exists")))
            .max()
            .expect("at least one path");
        let probe = root.join(".test-fs-tick-probe.tmp");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            fs::write(&probe, b"").expect("probe writes");
            let meta = fs::symlink_metadata(&probe).expect("probe stats");
            let _ = fs::remove_file(&probe);
            #[cfg(unix)]
            let probe_mtime = (meta.mtime(), meta.mtime_nsec());
            #[cfg(not(unix))]
            let probe_mtime = freshness(&meta);
            if probe_mtime > target {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "filesystem timestamp did not advance past {target:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
