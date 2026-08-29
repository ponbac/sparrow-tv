use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use chrono::{DateTime, Utc};
use sparrow_core::SnapshotRecoveryReason;

use crate::{
    layout::{DiskKind, Layout, MAX_MANIFEST_BYTES, MAX_POINTER_BYTES, Slot, is_transient_name},
    manifest::{
        DiskMetadata, DiskPointer, DiskSource, DiskValidators, ManifestError, decode_manifest,
        decode_pointer, encode_manifest, encode_pointer,
    },
};

const FIRST_TRANSIENT_TOKEN: u64 = 1_024;
const HASH_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiskError {
    Unavailable,
    Capacity,
    Corrupt,
    UnsafeLayout,
    Busy,
}

#[derive(Clone)]
pub(crate) struct DiskCandidate {
    pub(crate) slot: Slot,
    pub(crate) metadata: DiskMetadata,
    pub(crate) requires_adoption: bool,
}

pub(crate) struct DiskScan {
    pub(crate) candidates: Vec<DiskCandidate>,
    pub(crate) diagnostics: Vec<SnapshotRecoveryReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileOperation {
    CreateStage,
    AppendStage,
    SyncPayload,
    WriteManifest,
    SyncManifest,
    InstallPayload,
    InstallManifest,
    SyncPreparedDirectory,
    WritePointer,
    SyncPointer,
    ActivatePointer,
    SyncActivatedDirectory,
    WriteAdoptPointer,
    SyncAdoptPointer,
    AdoptPointer,
    SyncAdoptedDirectory,
    WriteRevalidation,
    SyncRevalidation,
    InstallRevalidation,
    SyncRevalidatedDirectory,
    ReadDiscardPointer,
}

pub(crate) trait FaultInjector: Send + Sync {
    fn check(&self, operation: FileOperation) -> io::Result<()>;
}

struct NoFaults;

impl FaultInjector for NoFaults {
    fn check(&self, _operation: FileOperation) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) struct DiskStore {
    layout: Layout,
    next_token: AtomicU64,
    registry: Mutex<HashMap<u64, StageRecord>>,
    faults: Arc<dyn FaultInjector>,
}

#[derive(Clone)]
struct StageRecord {
    source: DiskSource,
    protected: Option<DiskCandidate>,
    phase: StagePhase,
}

#[derive(Clone, Copy)]
struct DiscardTarget {
    slot: Slot,
    checksum: [u8; 32],
}

#[derive(Clone, Copy)]
enum StagePhase {
    Staged,
    Preparing,
    Installing {
        slot: Slot,
        checksum: [u8; 32],
        payload_installed: bool,
    },
    Prepared {
        slot: Slot,
        checksum: [u8; 32],
    },
    Discarding {
        target: Option<DiscardTarget>,
    },
}

impl DiskStore {
    pub(crate) fn open(root: &Path) -> Result<Self, DiskError> {
        Self::open_with_faults(root, Arc::new(NoFaults))
    }

    pub(crate) fn open_with_faults(
        root: &Path,
        faults: Arc<dyn FaultInjector>,
    ) -> Result<Self, DiskError> {
        let layout = Layout::new(root.to_path_buf());
        ensure_private_directory(layout.root())?;
        for kind in [DiskKind::M3u, DiskKind::Epg] {
            let directory = layout.source_dir(kind);
            ensure_private_directory(&directory)?;
            remove_abandoned_transients(&directory)?;
        }
        Ok(Self {
            layout,
            next_token: AtomicU64::new(FIRST_TRANSIENT_TOKEN),
            registry: Mutex::new(HashMap::new()),
            faults,
        })
    }

    pub(crate) fn begin_stage(
        &self,
        source: DiskSource,
        protected: Option<&DiskCandidate>,
    ) -> Result<u64, DiskError> {
        loop {
            self.finish_pending_discard(source.kind)?;
            if let Some(candidate) = protected {
                self.target_opposite_protected(source, candidate)?;
            }
            let token = self.next_token();
            {
                let mut registry = self.registry();
                if registry
                    .values()
                    .any(|record| record.source.kind == source.kind)
                {
                    return Err(DiskError::Busy);
                }
                registry.insert(
                    token,
                    StageRecord {
                        source,
                        protected: protected.cloned(),
                        phase: StagePhase::Staged,
                    },
                );
            }
            let path = self.layout.stage(source.kind, token);
            let creation = self
                .faults
                .check(FileOperation::CreateStage)
                .map_err(map_io)
                .and_then(|()| create_private_file(&path).map(drop));
            match creation {
                Ok(()) => return Ok(token),
                Err(DiskError::Busy) => {
                    self.registry().remove(&token);
                }
                Err(error) => {
                    self.registry().remove(&token);
                    return Err(error);
                }
            }
        }
    }

    pub(crate) fn append(
        &self,
        token: u64,
        source: DiskSource,
        bytes: &[u8],
    ) -> Result<(), DiskError> {
        self.require_stage(token, source, |phase| matches!(phase, StagePhase::Staged))?;
        self.faults
            .check(FileOperation::AppendStage)
            .map_err(map_io)?;
        let path = self.layout.stage(source.kind, token);
        let mut options = OpenOptions::new();
        options.append(true);
        #[cfg(unix)]
        options.custom_flags(0);
        let mut file = options.open(path).map_err(map_io)?;
        file.write_all(bytes).map_err(map_io)
    }

    pub(crate) fn open_staged(
        &self,
        token: u64,
        source: DiskSource,
    ) -> Result<Box<dyn std::io::BufRead + Send>, DiskError> {
        self.require_stage(token, source, |phase| matches!(phase, StagePhase::Staged))?;
        let path = self.layout.stage(source.kind, token);
        open_private_regular(&path).map(|file| Box::new(BufReader::new(file)) as _)
    }

    pub(crate) fn prepare(&self, token: u64, metadata: &DiskMetadata) -> Result<(), DiskError> {
        {
            let mut registry = self.registry();
            let record = registry.get_mut(&token).ok_or(DiskError::Corrupt)?;
            if record.source != metadata.source || !matches!(record.phase, StagePhase::Staged) {
                return Err(DiskError::Corrupt);
            }
            record.phase = StagePhase::Preparing;
        }

        let result = self.prepare_inner(token, metadata);
        match result {
            Ok(slot) => {
                let mut registry = self.registry();
                let record = registry.get_mut(&token).ok_or(DiskError::Corrupt)?;
                record.phase = StagePhase::Prepared {
                    slot,
                    checksum: metadata.checksum,
                };
                Ok(())
            }
            // Keep the record until `discard` so it can make an inactive pair
            // installed before the failure immediately ineligible.
            Err(error) => Err(error),
        }
    }

    fn prepare_inner(&self, token: u64, metadata: &DiskMetadata) -> Result<Slot, DiskError> {
        let kind = metadata.source.kind;
        let stage_path = self.layout.stage(kind, token);
        validate_payload(&stage_path, metadata.decoded_bytes, &metadata.checksum)?;

        self.faults
            .check(FileOperation::SyncPayload)
            .map_err(map_io)?;
        open_private_regular(&stage_path)?
            .sync_all()
            .map_err(map_io)?;

        let target = self.target_for_prepare(token, metadata.source)?;

        let manifest_bytes = encode_manifest(metadata).map_err(map_manifest)?;
        if manifest_bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(DiskError::Corrupt);
        }
        let manifest_temp = self.layout.manifest_temp(kind, token);
        write_synced_temp(
            &manifest_temp,
            &manifest_bytes,
            self.faults.as_ref(),
            FileOperation::WriteManifest,
            FileOperation::SyncManifest,
        )?;

        {
            let mut registry = self.registry();
            let record = registry.get_mut(&token).ok_or(DiskError::Corrupt)?;
            if record.source != metadata.source || !matches!(record.phase, StagePhase::Preparing) {
                return Err(DiskError::Corrupt);
            }
            record.phase = StagePhase::Installing {
                slot: target,
                checksum: metadata.checksum,
                payload_installed: false,
            };
        }

        self.faults
            .check(FileOperation::InstallPayload)
            .map_err(map_io)?;
        {
            // Keep discard from observing the filesystem rename without the
            // matching phase transition.
            let mut registry = self.registry();
            let record = registry.get_mut(&token).ok_or(DiskError::Corrupt)?;
            if record.source != metadata.source
                || !matches!(
                    record.phase,
                    StagePhase::Installing {
                        slot,
                        checksum,
                        payload_installed: false,
                    } if slot == target && checksum == metadata.checksum
                )
            {
                return Err(DiskError::Corrupt);
            }
            fs::rename(&stage_path, self.layout.payload(kind, target)).map_err(map_io)?;
            record.phase = StagePhase::Installing {
                slot: target,
                checksum: metadata.checksum,
                payload_installed: true,
            };
        }
        self.faults
            .check(FileOperation::InstallManifest)
            .map_err(map_io)?;
        fs::rename(&manifest_temp, self.layout.manifest(kind, target)).map_err(map_io)?;
        self.faults
            .check(FileOperation::SyncPreparedDirectory)
            .map_err(map_io)?;
        sync_directory(&self.layout.source_dir(kind))?;

        let pointer_bytes = encode_pointer(target, &metadata.checksum).map_err(map_manifest)?;
        let pointer_temp = self.layout.pointer_temp(kind, token);
        write_synced_temp(
            &pointer_temp,
            &pointer_bytes,
            self.faults.as_ref(),
            FileOperation::WritePointer,
            FileOperation::SyncPointer,
        )?;
        Ok(target)
    }

    pub(crate) fn activate(
        &self,
        token: u64,
        metadata: &DiskMetadata,
    ) -> Result<DiskCandidate, DiskError> {
        let slot = {
            // Keep activation and discard serialized through the pointer
            // rename. Once discard observes a prepared record, activation
            // must no longer be able to make that slot active.
            let mut registry = self.registry();
            let record = registry.get(&token).ok_or(DiskError::Corrupt)?;
            if record.source != metadata.source {
                return Err(DiskError::Corrupt);
            }
            let slot = match record.phase {
                StagePhase::Prepared { slot, checksum } if checksum == metadata.checksum => slot,
                StagePhase::Staged
                | StagePhase::Preparing
                | StagePhase::Installing { .. }
                | StagePhase::Prepared { .. }
                | StagePhase::Discarding { .. } => return Err(DiskError::Corrupt),
            };

            self.verify_candidate(slot, metadata)?;
            let pointer_temp = self.layout.pointer_temp(metadata.source.kind, token);
            self.faults
                .check(FileOperation::ActivatePointer)
                .map_err(map_io)?;
            fs::rename(&pointer_temp, self.layout.pointer(metadata.source.kind)).map_err(map_io)?;
            registry.remove(&token);
            slot
        };

        // The pointer rename is the in-process linearization point. Returning
        // an error after it succeeds would make the core retain its old view
        // even though this process now observes the new active candidate.
        // Still attempt the directory sync for crash durability; a crash may
        // then reveal either complete pointer, while this live process must
        // publish the candidate it just activated.
        self.best_effort_sync_after_rename(
            metadata.source.kind,
            FileOperation::SyncActivatedDirectory,
        );
        Ok(DiskCandidate {
            slot,
            metadata: metadata.clone(),
            requires_adoption: false,
        })
    }

    pub(crate) fn discard(&self, token: u64, source: DiskSource) -> Result<(), DiskError> {
        let has_record = {
            let mut registry = self.registry();
            match registry.get_mut(&token) {
                Some(record) if record.source != source => return Err(DiskError::Corrupt),
                Some(record) => {
                    if !matches!(record.phase, StagePhase::Discarding { .. }) {
                        let target = match record.phase {
                            StagePhase::Installing {
                                slot,
                                checksum,
                                payload_installed: true,
                            }
                            | StagePhase::Prepared { slot, checksum } => {
                                Some(DiscardTarget { slot, checksum })
                            }
                            StagePhase::Staged
                            | StagePhase::Preparing
                            | StagePhase::Installing {
                                payload_installed: false,
                                ..
                            } => None,
                            StagePhase::Discarding { .. } => unreachable!(),
                        };
                        record.phase = StagePhase::Discarding { target };
                    }
                    true
                }
                None => false,
            }
        };

        if has_record {
            self.finish_discard(token, source)
        } else {
            self.remove_stage_transients(token, source.kind)
        }
    }

    fn finish_pending_discard(&self, kind: DiskKind) -> Result<(), DiskError> {
        loop {
            let pending = {
                let registry = self.registry();
                match registry
                    .iter()
                    .find(|(_, record)| record.source.kind == kind)
                {
                    Some((&token, record))
                        if matches!(record.phase, StagePhase::Discarding { .. }) =>
                    {
                        Some((token, record.source))
                    }
                    Some(_) => return Err(DiskError::Busy),
                    None => return Ok(()),
                }
            };
            let (token, source) = pending.expect("matching registry entry exists");
            self.finish_discard(token, source)?;
        }
    }

    fn finish_discard(&self, token: u64, source: DiskSource) -> Result<(), DiskError> {
        // Serialize cleanup with other discard retries and same-kind staging.
        // The tombstone stays visible until every logical and physical cleanup
        // step succeeds.
        let mut registry = self.registry();
        let target = match registry.get(&token).cloned() {
            Some(record) if record.source != source => return Err(DiskError::Corrupt),
            Some(StageRecord {
                phase: StagePhase::Discarding { target },
                ..
            }) => target,
            Some(_) => return Err(DiskError::Corrupt),
            None => {
                drop(registry);
                return self.remove_stage_transients(token, source.kind);
            }
        };

        if let Some(DiscardTarget { slot, checksum }) = target {
            self.faults
                .check(FileOperation::ReadDiscardPointer)
                .map_err(map_io)?;
            let is_active = match self.read_pointer(source.kind) {
                Ok(pointer) => pointer
                    .is_some_and(|pointer| pointer.slot == slot && pointer.checksum == checksum),
                Err(DiskError::Corrupt | DiskError::UnsafeLayout) => false,
                Err(error) => return Err(error),
            };
            if !is_active {
                remove_file_if_present(&self.layout.manifest(source.kind, slot))?;
                sync_directory(&self.layout.source_dir(source.kind))?;
            }
        }

        self.remove_stage_transients(token, source.kind)?;
        registry.remove(&token);
        Ok(())
    }

    fn remove_stage_transients(&self, token: u64, kind: DiskKind) -> Result<(), DiskError> {
        let paths = [
            self.layout.stage(kind, token),
            self.layout.manifest_temp(kind, token),
            self.layout.pointer_temp(kind, token),
            self.layout.adopt_temp(kind, token),
            self.layout.revalidate_temp(kind, token),
        ];
        let mut failure = None;
        for path in paths {
            if let Err(error) = remove_file_if_present(&path) {
                failure.get_or_insert(error);
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    }

    fn inactive_slot_for_prepare(&self, source: DiskSource) -> Result<Slot, DiskError> {
        // Normal recovery remains structural so the core can parse and hash in
        // one pass. Refresh is rare and must not overwrite the last usable
        // fallback, so integrity-check both bounded candidates here.
        for candidate in self.scan(source)?.candidates {
            let payload = self.layout.payload(source.kind, candidate.slot);
            match validate_payload(
                &payload,
                candidate.metadata.decoded_bytes,
                &candidate.metadata.checksum,
            ) {
                Ok(()) => return Ok(candidate.slot.other()),
                Err(DiskError::Corrupt | DiskError::UnsafeLayout) => {}
                Err(error) => return Err(error),
            }
        }

        let protected = match self.read_pointer(source.kind) {
            Ok(pointer) => pointer.map(|pointer| pointer.slot),
            Err(DiskError::Corrupt | DiskError::UnsafeLayout) => None,
            Err(error) => return Err(error),
        };
        Ok(protected.map_or(Slot::A, Slot::other))
    }

    fn target_opposite_protected(
        &self,
        source: DiskSource,
        protected: &DiskCandidate,
    ) -> Result<Slot, DiskError> {
        if protected.metadata.source != source {
            return Err(DiskError::Corrupt);
        }
        let manifest = self.layout.manifest(source.kind, protected.slot);
        let payload = self.layout.payload(source.kind, protected.slot);
        if !path_presence(&manifest)? || !path_presence(&payload)? {
            return Err(DiskError::Corrupt);
        }
        self.verify_candidate(protected.slot, &protected.metadata)?;
        validate_payload(
            &payload,
            protected.metadata.decoded_bytes,
            &protected.metadata.checksum,
        )?;
        Ok(protected.slot.other())
    }

    fn target_for_prepare(&self, token: u64, source: DiskSource) -> Result<Slot, DiskError> {
        let protected = {
            let registry = self.registry();
            let record = registry.get(&token).ok_or(DiskError::Corrupt)?;
            if record.source != source || !matches!(record.phase, StagePhase::Preparing) {
                return Err(DiskError::Corrupt);
            }
            record.protected.clone()
        };
        protected.map_or_else(
            || self.inactive_slot_for_prepare(source),
            |candidate| self.target_opposite_protected(source, &candidate),
        )
    }

    pub(crate) fn scan(&self, source: DiskSource) -> Result<DiskScan, DiskError> {
        let kind = source.kind;
        let discarding_slot = self.discarding_slot(kind);
        let (pointer, pointer_corrupt) = match self.read_pointer(kind) {
            Ok(pointer) => (pointer, false),
            Err(DiskError::Corrupt | DiskError::UnsafeLayout) => (None, true),
            Err(error) => return Err(error),
        };
        let pointer_slot = pointer.map(|pointer| pointer.slot);
        let mut diagnostics = Vec::new();
        if pointer_corrupt {
            push_diagnostic(
                &mut diagnostics,
                SnapshotRecoveryReason::CorruptActivePointer,
            );
        }

        let mut candidates = Vec::with_capacity(2);
        let mut any_slot_artifact = false;
        for slot in Slot::ALL {
            if discarding_slot == Some(slot) {
                continue;
            }
            let inspected = self.inspect_slot(source, slot)?;
            any_slot_artifact |= inspected.had_artifact;
            if let Some(reason) = inspected.diagnostic {
                push_diagnostic(&mut diagnostics, reason);
            }
            if let Some(metadata) = inspected.metadata {
                let pointer_matches = pointer.is_some_and(|pointer| {
                    pointer.slot == slot && pointer.checksum == metadata.checksum
                });
                if pointer_slot == Some(slot) && !pointer_matches {
                    push_diagnostic(
                        &mut diagnostics,
                        SnapshotRecoveryReason::CorruptActivePointer,
                    );
                }
                candidates.push(DiskCandidate {
                    slot,
                    metadata,
                    requires_adoption: !pointer_matches,
                });
            }
        }

        if pointer_slot.is_none() && (any_slot_artifact || !candidates.is_empty()) {
            push_diagnostic(
                &mut diagnostics,
                SnapshotRecoveryReason::MissingActivePointer,
            );
        }
        if let Some(slot) = pointer_slot
            && !candidates.iter().any(|candidate| candidate.slot == slot)
            && !diagnostics.contains(&SnapshotRecoveryReason::SourceMismatch)
        {
            push_diagnostic(&mut diagnostics, SnapshotRecoveryReason::MissingManifest);
        }

        candidates.sort_by(|left, right| {
            let left_active = !left.requires_adoption;
            let right_active = !right.requires_adoption;
            right_active
                .cmp(&left_active)
                .then_with(|| right.metadata.validated_at.cmp(&left.metadata.validated_at))
                .then_with(|| left.slot.cmp(&right.slot))
        });
        candidates.truncate(2);
        diagnostics.truncate(4);
        Ok(DiskScan {
            candidates,
            diagnostics,
        })
    }

    fn discarding_slot(&self, kind: DiskKind) -> Option<Slot> {
        self.registry().values().find_map(|record| {
            if record.source.kind != kind {
                return None;
            }
            match record.phase {
                StagePhase::Discarding {
                    target: Some(target),
                } => Some(target.slot),
                StagePhase::Staged
                | StagePhase::Preparing
                | StagePhase::Installing { .. }
                | StagePhase::Prepared { .. }
                | StagePhase::Discarding { target: None } => None,
            }
        })
    }

    pub(crate) fn open_candidate(
        &self,
        candidate: &DiskCandidate,
    ) -> Result<Box<dyn std::io::BufRead + Send>, DiskError> {
        self.verify_candidate(candidate.slot, &candidate.metadata)?;
        open_private_regular(
            &self
                .layout
                .payload(candidate.metadata.source.kind, candidate.slot),
        )
        .map(|file| Box::new(BufReader::new(file)) as _)
    }

    pub(crate) fn adopt(&self, candidate: &DiskCandidate) -> Result<DiskCandidate, DiskError> {
        self.verify_candidate(candidate.slot, &candidate.metadata)?;
        let kind = candidate.metadata.source.kind;
        let token = self.next_token();
        let temp = self.layout.adopt_temp(kind, token);
        let pointer =
            encode_pointer(candidate.slot, &candidate.metadata.checksum).map_err(map_manifest)?;
        let result = (|| {
            write_synced_temp(
                &temp,
                &pointer,
                self.faults.as_ref(),
                FileOperation::WriteAdoptPointer,
                FileOperation::SyncAdoptPointer,
            )?;
            self.faults
                .check(FileOperation::AdoptPointer)
                .map_err(map_io)?;
            fs::rename(&temp, self.layout.pointer(kind)).map_err(map_io)?;
            self.faults
                .check(FileOperation::SyncAdoptedDirectory)
                .map_err(map_io)?;
            sync_directory(&self.layout.source_dir(kind))
        })();
        let _ = remove_file_if_present(&temp);
        result?;
        Ok(DiskCandidate {
            slot: candidate.slot,
            metadata: candidate.metadata.clone(),
            requires_adoption: false,
        })
    }

    pub(crate) fn revalidate(
        &self,
        candidate: &DiskCandidate,
        validated_at: DateTime<Utc>,
        validators: DiskValidators,
    ) -> Result<DiskCandidate, DiskError> {
        self.verify_candidate(candidate.slot, &candidate.metadata)?;
        validate_payload(
            &self
                .layout
                .payload(candidate.metadata.source.kind, candidate.slot),
            candidate.metadata.decoded_bytes,
            &candidate.metadata.checksum,
        )?;
        let metadata = DiskMetadata {
            validated_at,
            validators,
            ..candidate.metadata.clone()
        };
        let bytes = encode_manifest(&metadata).map_err(map_manifest)?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(DiskError::Corrupt);
        }
        let kind = metadata.source.kind;
        let token = self.next_token();
        let temp = self.layout.revalidate_temp(kind, token);
        let installation = (|| {
            write_synced_temp(
                &temp,
                &bytes,
                self.faults.as_ref(),
                FileOperation::WriteRevalidation,
                FileOperation::SyncRevalidation,
            )?;
            self.faults
                .check(FileOperation::InstallRevalidation)
                .map_err(map_io)?;
            fs::rename(&temp, self.layout.manifest(kind, candidate.slot)).map_err(map_io)
        })();
        let _ = remove_file_if_present(&temp);
        installation?;

        // As with activation, the rename is the in-process linearization
        // point. Both possible post-crash manifests describe the same
        // already-validated payload.
        self.best_effort_sync_after_rename(kind, FileOperation::SyncRevalidatedDirectory);
        Ok(DiskCandidate {
            slot: candidate.slot,
            metadata,
            requires_adoption: candidate.requires_adoption,
        })
    }

    fn best_effort_sync_after_rename(&self, kind: DiskKind, operation: FileOperation) {
        let _sync_result = self
            .faults
            .check(operation)
            .map_err(map_io)
            .and_then(|()| sync_directory(&self.layout.source_dir(kind)));
    }

    fn inspect_slot(&self, source: DiskSource, slot: Slot) -> Result<SlotInspection, DiskError> {
        let manifest_path = self.layout.manifest(source.kind, slot);
        let payload_path = self.layout.payload(source.kind, slot);
        let manifest_exists = path_presence(&manifest_path)?;
        let payload_exists = path_presence(&payload_path)?;
        let had_artifact = manifest_exists || payload_exists;

        if !manifest_exists && !payload_exists {
            return Ok(SlotInspection::empty());
        }
        if !manifest_exists {
            return Ok(SlotInspection::damaged(
                had_artifact,
                SnapshotRecoveryReason::MissingManifest,
            ));
        }
        if !payload_exists {
            return Ok(SlotInspection::damaged(
                had_artifact,
                SnapshotRecoveryReason::MissingPayload,
            ));
        }

        let metadata = match read_manifest(&manifest_path) {
            Ok(metadata) => metadata,
            Err(DiskError::Corrupt | DiskError::UnsafeLayout) => {
                return Ok(SlotInspection::damaged(
                    had_artifact,
                    SnapshotRecoveryReason::CorruptManifest,
                ));
            }
            Err(error) => return Err(error),
        };
        if metadata.source != source {
            return Ok(SlotInspection::damaged(
                had_artifact,
                SnapshotRecoveryReason::SourceMismatch,
            ));
        }

        let payload_metadata = fs::metadata(&payload_path).map_err(map_io)?;
        if payload_metadata.len() != metadata.decoded_bytes {
            return Ok(SlotInspection::damaged(
                had_artifact,
                SnapshotRecoveryReason::LengthMismatch,
            ));
        }
        Ok(SlotInspection {
            metadata: Some(metadata),
            diagnostic: None,
            had_artifact,
        })
    }

    fn verify_candidate(&self, slot: Slot, expected: &DiskMetadata) -> Result<(), DiskError> {
        let manifest = read_manifest(&self.layout.manifest(expected.source.kind, slot))?;
        if &manifest != expected {
            return Err(DiskError::Corrupt);
        }
        let payload = self.layout.payload(expected.source.kind, slot);
        let metadata = fs::metadata(&payload).map_err(map_io)?;
        if !metadata.is_file() || metadata.len() != expected.decoded_bytes {
            return Err(DiskError::Corrupt);
        }
        Ok(())
    }

    fn read_pointer(&self, kind: DiskKind) -> Result<Option<DiskPointer>, DiskError> {
        let path = self.layout.pointer(kind);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                ensure_private_file_mode(&path)?;
                let bytes = read_bounded(&path, MAX_POINTER_BYTES)?;
                decode_pointer(&bytes).map(Some).map_err(map_manifest)
            }
            Ok(_) => Err(DiskError::UnsafeLayout),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(map_io(error)),
        }
    }

    fn require_stage(
        &self,
        token: u64,
        source: DiskSource,
        predicate: impl FnOnce(StagePhase) -> bool,
    ) -> Result<(), DiskError> {
        let registry = self.registry();
        let record = registry.get(&token).ok_or(DiskError::Corrupt)?;
        if record.source != source || !predicate(record.phase) {
            return Err(DiskError::Corrupt);
        }
        Ok(())
    }

    fn registry(&self) -> std::sync::MutexGuard<'_, HashMap<u64, StageRecord>> {
        self.registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn next_token(&self) -> u64 {
        self.next_token.fetch_add(1, Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn layout(&self) -> &Layout {
        &self.layout
    }
}

struct SlotInspection {
    metadata: Option<DiskMetadata>,
    diagnostic: Option<SnapshotRecoveryReason>,
    had_artifact: bool,
}

impl SlotInspection {
    fn empty() -> Self {
        Self {
            metadata: None,
            diagnostic: None,
            had_artifact: false,
        }
    }

    fn damaged(had_artifact: bool, diagnostic: SnapshotRecoveryReason) -> Self {
        Self {
            metadata: None,
            diagnostic: Some(diagnostic),
            had_artifact,
        }
    }
}

fn push_diagnostic(diagnostics: &mut Vec<SnapshotRecoveryReason>, reason: SnapshotRecoveryReason) {
    if diagnostics.len() < 4 && !diagnostics.contains(&reason) {
        diagnostics.push(reason);
    }
}

fn read_manifest(path: &Path) -> Result<DiskMetadata, DiskError> {
    let bytes = read_bounded(path, MAX_MANIFEST_BYTES)?;
    decode_manifest(&bytes).map_err(map_manifest)
}

fn validate_payload(
    path: &Path,
    expected_bytes: u64,
    expected_checksum: &[u8; 32],
) -> Result<(), DiskError> {
    let metadata = fs::metadata(path).map_err(map_io)?;
    if !metadata.is_file() || metadata.len() != expected_bytes {
        return Err(DiskError::Corrupt);
    }
    if &checksum_file(path)? != expected_checksum {
        return Err(DiskError::Corrupt);
    }
    Ok(())
}

fn checksum_file(path: &Path) -> Result<[u8; 32], DiskError> {
    let mut file = open_private_regular(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(map_io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(*hasher.finalize().as_bytes())
}

fn path_presence(path: &Path) -> Result<bool, DiskError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            ensure_private_file_mode(path)?;
            Ok(true)
        }
        Ok(_) => Err(DiskError::UnsafeLayout),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(map_io(error)),
    }
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, DiskError> {
    let mut file = open_private_regular(path)?;
    if file.metadata().map_err(map_io)?.len() > maximum {
        return Err(DiskError::Corrupt);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(map_io)?;
    if bytes.len() as u64 > maximum {
        return Err(DiskError::Corrupt);
    }
    Ok(bytes)
}

fn write_synced_temp(
    path: &Path,
    bytes: &[u8],
    faults: &dyn FaultInjector,
    write_operation: FileOperation,
    sync_operation: FileOperation,
) -> Result<(), DiskError> {
    faults.check(write_operation).map_err(map_io)?;
    let mut file = create_private_file(path)?;
    let result = (|| {
        file.write_all(bytes).map_err(map_io)?;
        faults.check(sync_operation).map_err(map_io)?;
        file.sync_all().map_err(map_io)
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

fn create_private_file(path: &Path) -> Result<File, DiskError> {
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    match options.open(path) {
        Ok(file) => {
            ensure_private_file_mode(path)?;
            Ok(file)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(DiskError::Busy),
        Err(error) => Err(map_io(error)),
    }
}

fn open_private_regular(path: &Path) -> Result<File, DiskError> {
    let metadata = fs::symlink_metadata(path).map_err(map_io)?;
    if !metadata.file_type().is_file() {
        return Err(DiskError::UnsafeLayout);
    }
    ensure_private_file_mode(path)?;
    File::open(path).map_err(map_io)
}

fn ensure_private_directory(path: &Path) -> Result<(), DiskError> {
    fs::create_dir_all(path).map_err(map_io)?;
    let metadata = fs::symlink_metadata(path).map_err(map_io)?;
    if !metadata.file_type().is_dir() {
        return Err(DiskError::UnsafeLayout);
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(map_io)?;
    Ok(())
}

fn ensure_private_file_mode(path: &Path) -> Result<(), DiskError> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(map_io)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), DiskError> {
    File::open(path).map_err(map_io)?.sync_all().map_err(map_io)
}

fn remove_abandoned_transients(directory: &Path) -> Result<(), DiskError> {
    for entry in fs::read_dir(directory).map_err(map_io)? {
        let entry = entry.map_err(map_io)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_transient_name(name) {
            let file_type = entry.file_type().map_err(map_io)?;
            if file_type.is_file() || file_type.is_symlink() {
                fs::remove_file(entry.path()).map_err(map_io)?;
            } else {
                return Err(DiskError::UnsafeLayout);
            }
        }
    }
    sync_directory(directory)
}

fn remove_file_if_present(path: &Path) -> Result<(), DiskError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(map_io(error)),
    }
}

fn map_manifest(error: ManifestError) -> DiskError {
    match error {
        ManifestError::Invalid => DiskError::Corrupt,
    }
}

fn map_io(error: io::Error) -> DiskError {
    if matches!(error.raw_os_error(), Some(28 | 122)) || error.kind() == io::ErrorKind::WriteZero {
        DiskError::Capacity
    } else {
        DiskError::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        io::Read,
        sync::{Arc, Barrier, Mutex, mpsc},
        thread,
        time::Duration as StdDuration,
    };

    use chrono::{Duration, TimeZone, Utc};
    use tempfile::TempDir;

    use super::*;

    struct PlannedFaults {
        operations: Mutex<VecDeque<(FileOperation, io::ErrorKind)>>,
    }

    impl PlannedFaults {
        fn one(operation: FileOperation, kind: io::ErrorKind) -> Arc<Self> {
            Arc::new(Self {
                operations: Mutex::new(VecDeque::from([(operation, kind)])),
            })
        }

        fn assert_consumed(&self) {
            assert!(
                self.operations
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_empty(),
                "every planned filesystem operation was reached"
            );
        }
    }

    impl FaultInjector for PlannedFaults {
        fn check(&self, operation: FileOperation) -> io::Result<()> {
            let mut operations = self
                .operations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if operations
                .front()
                .is_some_and(|planned| planned.0 == operation)
            {
                let (_, kind) = operations.pop_front().expect("front exists");
                return Err(io::Error::from(kind));
            }
            Ok(())
        }
    }

    struct PausingFaults {
        operation: FileOperation,
        reached: Arc<Barrier>,
        release: Arc<Barrier>,
    }

    impl FaultInjector for PausingFaults {
        fn check(&self, operation: FileOperation) -> io::Result<()> {
            if operation == self.operation {
                self.reached.wait();
                self.release.wait();
            }
            Ok(())
        }
    }

    #[test]
    fn restart_recovers_two_bounded_slots_and_private_permissions() {
        let directory = TempDir::new().expect("temporary directory");
        let source = source(DiskKind::M3u, 1);
        let first = activate_payload(
            &DiskStore::open(directory.path()).expect("store opens"),
            source,
            b"first",
            instant(1),
        );
        assert_eq!(first.slot, Slot::A);

        let store = DiskStore::open(directory.path()).expect("store restarts");
        let scan = store.scan(source).expect("snapshot scan succeeds");
        assert_eq!(scan.candidates.len(), 1);
        assert!(!scan.candidates[0].requires_adoption);
        assert_eq!(read_candidate(&store, &scan.candidates[0]), b"first");

        activate_payload(&store, source, b"second", instant(2));
        activate_payload(&store, source, b"third", instant(3));
        let scan = store.scan(source).expect("snapshot scan succeeds");
        assert_eq!(scan.candidates.len(), 2);
        assert_eq!(read_candidate(&store, &scan.candidates[0]), b"third");
        assert_eq!(read_candidate(&store, &scan.candidates[1]), b"second");

        #[cfg(unix)]
        {
            assert_eq!(mode(directory.path()), 0o700);
            for kind in [DiskKind::M3u, DiskKind::Epg] {
                assert_eq!(mode(&store.layout().source_dir(kind)), 0o700);
            }
            for entry in fs::read_dir(store.layout().source_dir(DiskKind::M3u))
                .expect("source directory reads")
            {
                assert_eq!(mode(&entry.expect("entry reads").path()), 0o600);
            }
        }
        assert!(persistent_file_count(&store, DiskKind::M3u) <= 5);
    }

    #[test]
    fn corrupt_active_payload_falls_back_and_adopt_repairs_the_pointer() {
        let directory = TempDir::new().expect("temporary directory");
        let store = DiskStore::open(directory.path()).expect("store opens");
        let source = source(DiskKind::M3u, 2);
        activate_payload(&store, source, b"fallback", instant(1));
        let active = activate_payload(&store, source, b"active", instant(2));
        fs::write(
            store.layout().payload(source.kind, active.slot),
            b"broken-long",
        )
        .expect("active payload corrupts");

        let scan = store.scan(source).expect("snapshot scan succeeds");
        assert_eq!(scan.candidates.len(), 1);
        assert!(scan.candidates[0].requires_adoption);
        assert!(
            scan.diagnostics
                .contains(&SnapshotRecoveryReason::LengthMismatch)
        );
        assert_eq!(read_candidate(&store, &scan.candidates[0]), b"fallback");
        store.adopt(&scan.candidates[0]).expect("fallback adopts");

        let repaired = store.scan(source).expect("snapshot scan succeeds");
        assert!(!repaired.candidates[0].requires_adoption);
        assert_eq!(read_candidate(&store, &repaired.candidates[0]), b"fallback");
    }

    #[test]
    fn corrupt_pointer_prefers_newest_structural_candidate_with_slot_tie_break() {
        let directory = TempDir::new().expect("temporary directory");
        let store = DiskStore::open(directory.path()).expect("store opens");
        let source = source(DiskKind::M3u, 3);
        activate_payload(&store, source, b"older", instant(1));
        activate_payload(&store, source, b"newer", instant(2));
        fs::write(store.layout().pointer(source.kind), b"{not-json").expect("pointer corrupts");

        let scan = store.scan(source).expect("snapshot scan succeeds");
        assert_eq!(read_candidate(&store, &scan.candidates[0]), b"newer");
        assert!(scan.candidates[0].requires_adoption);
        assert!(
            scan.diagnostics
                .contains(&SnapshotRecoveryReason::CorruptActivePointer)
        );
    }

    #[test]
    fn pointer_checksum_must_bind_the_selected_manifest() {
        let directory = TempDir::new().expect("temporary directory");
        let store = DiskStore::open(directory.path()).expect("store opens");
        let source = source(DiskKind::M3u, 30);
        let active = activate_payload(&store, source, b"payload", instant(1));
        let stale = encode_pointer(active.slot, &[0x55; 32]).expect("stale pointer encodes");
        fs::write(store.layout().pointer(source.kind), stale).expect("pointer becomes stale");

        let scan = store.scan(source).expect("snapshot scan succeeds");
        assert_eq!(scan.candidates.len(), 1);
        assert!(scan.candidates[0].requires_adoption);
        assert!(
            scan.diagnostics
                .contains(&SnapshotRecoveryReason::CorruptActivePointer)
        );
    }

    #[test]
    fn restart_removes_only_bounded_known_transients() {
        let directory = TempDir::new().expect("temporary directory");
        let initial = DiskStore::open(directory.path()).expect("store opens");
        let source_dir = initial.layout().source_dir(DiskKind::M3u);
        drop(initial);
        let transients = [
            ".stage-9000.payload",
            ".manifest-9000.tmp",
            ".pointer-9000.tmp",
            ".adopt-9000.tmp",
            ".revalidate-9000.tmp",
        ];
        for name in transients {
            fs::write(source_dir.join(name), b"abandoned").expect("transient writes");
        }
        fs::write(source_dir.join("unrelated-canary"), b"keep").expect("canary writes");

        let _restarted = DiskStore::open(directory.path()).expect("store restarts");
        for name in transients {
            assert!(!source_dir.join(name).exists());
        }
        assert!(source_dir.join("unrelated-canary").exists());
    }

    #[test]
    fn concurrent_stages_for_one_kind_are_single_flight() {
        let directory = TempDir::new().expect("temporary directory");
        let store = Arc::new(DiskStore::open(directory.path()).expect("store opens"));
        let source = source(DiskKind::M3u, 31);
        let barrier = Arc::new(Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    store.begin_stage(source, None)
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().expect("stage thread completes"))
            .collect::<Vec<_>>();

        let token = results
            .iter()
            .find_map(|result| result.as_ref().ok().copied())
            .expect("one stage reserves the source kind");
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .all(|error| *error == DiskError::Busy)
        );
        store
            .discard(token, source)
            .expect("reserved stage discards");
    }

    #[test]
    fn source_kinds_and_keys_recover_independently() {
        let directory = TempDir::new().expect("temporary directory");
        let store = DiskStore::open(directory.path()).expect("store opens");
        let m3u = source(DiskKind::M3u, 4);
        let epg = source(DiskKind::Epg, 5);
        activate_payload(&store, m3u, b"channels", instant(1));
        activate_payload(&store, epg, b"guide", instant(2));

        assert_eq!(
            read_candidate(&store, &store.scan(m3u).expect("M3U scans").candidates[0]),
            b"channels"
        );
        assert_eq!(
            read_candidate(&store, &store.scan(epg).expect("EPG scans").candidates[0]),
            b"guide"
        );
        let changed = source(DiskKind::M3u, 99);
        let changed_scan = store.scan(changed).expect("changed source scans");
        assert!(changed_scan.candidates.is_empty());
        assert!(
            changed_scan
                .diagnostics
                .contains(&SnapshotRecoveryReason::SourceMismatch)
        );
    }

    #[test]
    fn preparation_faults_never_replace_the_old_active_snapshot() {
        for operation in [
            FileOperation::SyncPayload,
            FileOperation::WriteManifest,
            FileOperation::SyncManifest,
            FileOperation::InstallPayload,
            FileOperation::InstallManifest,
            FileOperation::SyncPreparedDirectory,
            FileOperation::WritePointer,
            FileOperation::SyncPointer,
        ] {
            let directory = TempDir::new().expect("temporary directory");
            let source = source(DiskKind::M3u, 6);
            activate_payload(
                &DiskStore::open(directory.path()).expect("store opens"),
                source,
                b"retained",
                instant(1),
            );
            let faults = PlannedFaults::one(operation, io::ErrorKind::Other);
            let store =
                DiskStore::open_with_faults(directory.path(), faults).expect("faulted store opens");
            let token = stage_payload(&store, source, b"candidate");
            let metadata = metadata(source, b"candidate", instant(2));
            assert!(store.prepare(token, &metadata).is_err(), "{operation:?}");
            store.discard(token, source).expect("discard is idempotent");

            let restarted = DiskStore::open(directory.path()).expect("store restarts");
            let scan = restarted.scan(source).expect("snapshot scan succeeds");
            assert_eq!(scan.candidates.len(), 1, "{operation:?}");
            assert_eq!(read_candidate(&restarted, &scan.candidates[0]), b"retained");
        }
    }

    #[test]
    fn checksum_corrupt_active_is_not_protected_over_the_last_valid_fallback() {
        let directory = TempDir::new().expect("temporary directory");
        let source = source(DiskKind::M3u, 32);
        let initial = DiskStore::open(directory.path()).expect("store opens");
        activate_payload(&initial, source, b"old-old!", instant(1));
        activate_payload(&initial, source, b"fallback", instant(2));
        let corrupt_active = activate_payload(&initial, source, b"active-a", instant(3));
        assert_eq!(corrupt_active.slot, Slot::A);
        fs::write(
            initial.layout().payload(source.kind, corrupt_active.slot),
            b"corrupt!",
        )
        .expect("active payload is corrupted without changing its length");
        drop(initial);

        let faults = PlannedFaults::one(FileOperation::WritePointer, io::ErrorKind::Other);
        let store =
            DiskStore::open_with_faults(directory.path(), faults).expect("faulted store opens");
        let token = stage_payload(&store, source, b"candidate");
        let metadata = metadata(source, b"candidate", instant(4));
        assert!(store.prepare(token, &metadata).is_err());
        store
            .discard(token, source)
            .expect("failed preparation discards");

        let restarted = DiskStore::open(directory.path()).expect("store restarts");
        let scan = restarted.scan(source).expect("snapshot scan succeeds");
        assert_eq!(scan.candidates.len(), 1);
        assert_eq!(scan.candidates[0].slot, Slot::B);
        assert_eq!(read_candidate(&restarted, &scan.candidates[0]), b"fallback");
    }

    #[test]
    fn protected_refresh_preserves_the_core_validated_fallback_when_adoption_failed() {
        for fault in [
            FileOperation::WritePointer,
            FileOperation::ActivatePointer,
            FileOperation::SyncActivatedDirectory,
        ] {
            let directory = TempDir::new().expect("temporary directory");
            let source = source(DiskKind::M3u, 39);
            let initial = DiskStore::open(directory.path()).expect("store opens");
            let fallback = activate_payload(
                &initial,
                source,
                b"#EXTM3U\n#EXTINF:-1,Valid\nhttps://media.invalid/valid\n",
                instant(1),
            );
            let invalid_active = activate_payload(
                &initial,
                source,
                b"checksum-valid but not an m3u document",
                instant(2),
            );
            assert_eq!(fallback.slot.other(), invalid_active.slot);
            drop(initial);

            // Model the core parsing the pointer-selected candidate, rejecting
            // it, accepting the fallback, and then being unable to repair the
            // pointer. Refresh must trust that semantic decision instead of
            // protecting the merely checksum-valid pointer selection.
            let faults = Arc::new(PlannedFaults {
                operations: Mutex::new(VecDeque::from([
                    (FileOperation::AdoptPointer, io::ErrorKind::Other),
                    (fault, io::ErrorKind::Other),
                ])),
            });
            let store =
                DiskStore::open_with_faults(directory.path(), faults).expect("faulted store opens");
            let scan = store.scan(source).expect("snapshot scan succeeds");
            let retained = scan
                .candidates
                .iter()
                .find(|candidate| candidate.slot == fallback.slot)
                .expect("parse-valid fallback remains eligible")
                .clone();
            assert!(retained.requires_adoption);
            assert!(matches!(
                store.adopt(&retained),
                Err(DiskError::Unavailable)
            ));

            let token = store
                .begin_stage(source, Some(&retained))
                .expect("protected refresh begins");
            let replacement = b"#EXTM3U\n#EXTINF:-1,Replacement\nhttps://media.invalid/new\n";
            store
                .append(token, source, replacement)
                .expect("replacement appends");
            let metadata = metadata(source, replacement, instant(3));
            match fault {
                FileOperation::WritePointer => {
                    assert_eq!(store.prepare(token, &metadata), Err(DiskError::Unavailable));
                }
                FileOperation::ActivatePointer => {
                    store.prepare(token, &metadata).expect("refresh prepares");
                    assert!(matches!(
                        store.activate(token, &metadata),
                        Err(DiskError::Unavailable)
                    ));
                }
                FileOperation::SyncActivatedDirectory => {
                    store.prepare(token, &metadata).expect("refresh prepares");
                    let activated = store
                        .activate(token, &metadata)
                        .expect("pointer rename completes activation before directory sync");
                    assert!(activated.metadata == metadata);
                }
                _ => unreachable!("the fixture covers prepare and activation faults"),
            }
            store
                .discard(token, source)
                .expect("refresh cleanup is idempotent");

            let restarted = DiskStore::open(directory.path()).expect("store restarts");
            let recovered = restarted.scan(source).expect("snapshot scan succeeds");
            let retained = recovered
                .candidates
                .iter()
                .find(|candidate| candidate.slot == fallback.slot)
                .expect("core-validated fallback survives the refresh fault");
            assert_eq!(
                read_candidate(&restarted, retained),
                b"#EXTM3U\n#EXTINF:-1,Valid\nhttps://media.invalid/valid\n"
            );
        }
    }

    #[test]
    fn protected_stage_fails_closed_for_a_different_or_stale_source_candidate() {
        let directory = TempDir::new().expect("temporary directory");
        let store = DiskStore::open(directory.path()).expect("store opens");
        let expected_source = source(DiskKind::M3u, 40);
        let retained = activate_payload(&store, expected_source, b"retained", instant(1));

        let different_source = source(DiskKind::M3u, 41);
        assert_eq!(
            store.begin_stage(different_source, Some(&retained)),
            Err(DiskError::Corrupt)
        );

        let mut stale = retained.clone();
        stale.metadata.validated_at = instant(2);
        assert_eq!(
            store.begin_stage(expected_source, Some(&stale)),
            Err(DiskError::Corrupt)
        );

        let token = store
            .begin_stage(expected_source, Some(&retained))
            .expect("the exact retained candidate protects its slot");
        store
            .discard(token, expected_source)
            .expect("stage discards");

        fs::remove_file(store.layout().manifest(expected_source.kind, retained.slot))
            .expect("retained manifest removes");
        assert_eq!(
            store.begin_stage(expected_source, Some(&retained)),
            Err(DiskError::Corrupt)
        );
    }

    #[test]
    fn protected_candidate_is_reverified_immediately_before_slot_installation() {
        for change_manifest in [false, true] {
            let directory = TempDir::new().expect("temporary directory");
            let store = DiskStore::open(directory.path()).expect("store opens");
            let source = source(DiskKind::M3u, 42);
            let retained = activate_payload(&store, source, b"retained", instant(1));
            let token = store
                .begin_stage(source, Some(&retained))
                .expect("protected refresh begins");
            store
                .append(token, source, b"replacement")
                .expect("replacement appends");

            if change_manifest {
                store
                    .revalidate(
                        &retained,
                        instant(2),
                        DiskValidators::new(Some("changed".to_owned()), None)
                            .expect("validator is valid"),
                    )
                    .expect("manifest changes after reservation");
            } else {
                fs::write(
                    store.layout().payload(source.kind, retained.slot),
                    b"corrupt!",
                )
                .expect("protected payload changes after reservation");
            }

            let replacement = metadata(source, b"replacement", instant(3));
            assert_eq!(store.prepare(token, &replacement), Err(DiskError::Corrupt));
            store
                .discard(token, source)
                .expect("failed protected stage discards");
        }
    }

    #[test]
    fn preinstall_failure_preserves_both_existing_complete_slots() {
        let directory = TempDir::new().expect("temporary directory");
        let source = source(DiskKind::M3u, 34);
        let initial = DiskStore::open(directory.path()).expect("store opens");
        activate_payload(&initial, source, b"fallback", instant(1));
        activate_payload(&initial, source, b"active", instant(2));
        drop(initial);

        let faults = PlannedFaults::one(FileOperation::InstallPayload, io::ErrorKind::Other);
        let store =
            DiskStore::open_with_faults(directory.path(), faults).expect("faulted store opens");
        let token = stage_payload(&store, source, b"candidate");
        let metadata = metadata(source, b"candidate", instant(3));
        assert!(store.prepare(token, &metadata).is_err());
        store.discard(token, source).expect("failed stage discards");

        let restarted = DiskStore::open(directory.path()).expect("store restarts");
        let scan = restarted.scan(source).expect("snapshot scan succeeds");
        assert_eq!(scan.candidates.len(), 2);
        assert_eq!(read_candidate(&restarted, &scan.candidates[0]), b"active");
        assert_eq!(read_candidate(&restarted, &scan.candidates[1]), b"fallback");
    }

    #[test]
    fn discarding_a_prepared_stage_immediately_removes_its_eligibility() {
        let directory = TempDir::new().expect("temporary directory");
        let source = source(DiskKind::M3u, 33);
        let store = DiskStore::open(directory.path()).expect("store opens");
        activate_payload(&store, source, b"active", instant(1));
        let token = stage_payload(&store, source, b"prepared");
        let metadata = metadata(source, b"prepared", instant(2));
        store.prepare(token, &metadata).expect("candidate prepares");
        assert_eq!(
            store.scan(source).expect("scan succeeds").candidates.len(),
            2
        );

        store
            .discard(token, source)
            .expect("prepared stage discards");
        let scan = store.scan(source).expect("scan succeeds");
        assert_eq!(scan.candidates.len(), 1);
        assert_eq!(read_candidate(&store, &scan.candidates[0]), b"active");
    }

    #[test]
    fn discard_tombstone_hides_candidate_until_next_stage_retries_cleanup() {
        let directory = TempDir::new().expect("temporary directory");
        let source = source(DiskKind::M3u, 35);
        activate_payload(
            &DiskStore::open(directory.path()).expect("store opens"),
            source,
            b"active",
            instant(1),
        );
        let faults = PlannedFaults::one(FileOperation::ReadDiscardPointer, io::ErrorKind::Other);
        let store =
            DiskStore::open_with_faults(directory.path(), faults).expect("faulted store opens");
        let token = stage_payload(&store, source, b"prepared");
        let metadata = metadata(source, b"prepared", instant(2));
        store.prepare(token, &metadata).expect("candidate prepares");
        assert_eq!(store.discard(token, source), Err(DiskError::Unavailable));

        let tombstoned = store.scan(source).expect("snapshot scan succeeds");
        assert_eq!(tombstoned.candidates.len(), 1);
        assert_eq!(read_candidate(&store, &tombstoned.candidates[0]), b"active");

        let next = store
            .begin_stage(source, None)
            .expect("next stage retries the one-shot discard failure");
        let cleaned = store.scan(source).expect("snapshot scan succeeds");
        assert_eq!(cleaned.candidates.len(), 1);
        assert_eq!(read_candidate(&store, &cleaned.candidates[0]), b"active");
        store.discard(next, source).expect("next stage discards");
    }

    #[test]
    fn discard_waits_for_activation_pointer_rename_to_finish() {
        let directory = TempDir::new().expect("temporary directory");
        let source = source(DiskKind::M3u, 36);
        activate_payload(
            &DiskStore::open(directory.path()).expect("store opens"),
            source,
            b"old",
            instant(1),
        );

        let activation_reached = Arc::new(Barrier::new(2));
        let activation_release = Arc::new(Barrier::new(2));
        let faults = Arc::new(PausingFaults {
            operation: FileOperation::ActivatePointer,
            reached: Arc::clone(&activation_reached),
            release: Arc::clone(&activation_release),
        });
        let store = Arc::new(
            DiskStore::open_with_faults(directory.path(), faults).expect("faulted store opens"),
        );
        let token = stage_payload(&store, source, b"new");
        let metadata = metadata(source, b"new", instant(2));
        store.prepare(token, &metadata).expect("candidate prepares");

        let activation_store = Arc::clone(&store);
        let activation_metadata = metadata.clone();
        let activation =
            thread::spawn(move || activation_store.activate(token, &activation_metadata));
        activation_reached.wait();

        let (discard_started_tx, discard_started_rx) = mpsc::sync_channel(0);
        let (discard_finished_tx, discard_finished_rx) = mpsc::channel();
        let discard_store = Arc::clone(&store);
        let discard = thread::spawn(move || {
            discard_started_tx
                .send(())
                .expect("discard start is observed");
            let result = discard_store.discard(token, source);
            discard_finished_tx
                .send(())
                .expect("discard completion is observed");
            result
        });
        discard_started_rx.recv().expect("discard thread starts");
        let early_discard = discard_finished_rx.recv_timeout(StdDuration::from_millis(100));

        activation_release.wait();
        let activated = activation.join().expect("activation thread completes");
        let discarded = discard.join().expect("discard thread completes");

        assert!(
            matches!(early_discard, Err(mpsc::RecvTimeoutError::Timeout)),
            "discard must not pass the activation registry lock"
        );
        activated.expect("activation succeeds");
        discarded.expect("post-activation discard is idempotent");
        let scan = store.scan(source).expect("snapshot scan succeeds");
        assert!(!scan.candidates[0].requires_adoption);
        assert_eq!(read_candidate(&store, &scan.candidates[0]), b"new");
    }

    #[test]
    fn activation_failure_before_pointer_rename_preserves_the_old_active_snapshot() {
        let directory = TempDir::new().expect("temporary directory");
        let source = source(DiskKind::M3u, 7);
        activate_payload(
            &DiskStore::open(directory.path()).expect("store opens"),
            source,
            b"old",
            instant(1),
        );
        let faults = PlannedFaults::one(FileOperation::ActivatePointer, io::ErrorKind::Other);
        let store = DiskStore::open_with_faults(directory.path(), faults.clone())
            .expect("faulted store opens");
        let token = stage_payload(&store, source, b"new");
        let metadata = metadata(source, b"new", instant(2));
        store.prepare(token, &metadata).expect("candidate prepares");

        assert!(matches!(
            store.activate(token, &metadata),
            Err(DiskError::Unavailable)
        ));
        faults.assert_consumed();
        store
            .discard(token, source)
            .expect("failed activation discards");

        let restarted = DiskStore::open(directory.path()).expect("store restarts");
        let scan = restarted.scan(source).expect("snapshot scan succeeds");
        assert_eq!(read_candidate(&restarted, &scan.candidates[0]), b"old");
        assert!(
            scan.candidates
                .iter()
                .all(|candidate| restarted.open_candidate(candidate).is_ok())
        );
    }

    #[test]
    fn activation_reports_success_after_pointer_rename_when_directory_sync_fails() {
        let directory = TempDir::new().expect("temporary directory");
        let source = source(DiskKind::M3u, 43);
        activate_payload(
            &DiskStore::open(directory.path()).expect("store opens"),
            source,
            b"old",
            instant(1),
        );
        let faults =
            PlannedFaults::one(FileOperation::SyncActivatedDirectory, io::ErrorKind::Other);
        let store = DiskStore::open_with_faults(directory.path(), faults.clone())
            .expect("faulted store opens");
        let token = stage_payload(&store, source, b"new");
        let metadata = metadata(source, b"new", instant(2));
        store.prepare(token, &metadata).expect("candidate prepares");

        let activated = store
            .activate(token, &metadata)
            .expect("pointer rename is the activation linearization point");
        faults.assert_consumed();
        assert_eq!(activated.slot, Slot::B);
        assert!(activated.metadata == metadata);
        assert!(!activated.requires_adoption);
        let next = store
            .begin_stage(source, Some(&activated))
            .expect("returned activation handle matches the installed candidate");
        store
            .discard(next, source)
            .expect("follow-up protected stage discards");

        let scan = store.scan(source).expect("snapshot scan succeeds");
        assert_eq!(read_candidate(&store, &scan.candidates[0]), b"new");
        assert!(!scan.candidates[0].requires_adoption);
    }

    #[test]
    fn append_capacity_failure_leaves_only_the_old_complete_snapshot() {
        let directory = TempDir::new().expect("temporary directory");
        let source = source(DiskKind::M3u, 8);
        activate_payload(
            &DiskStore::open(directory.path()).expect("store opens"),
            source,
            b"old",
            instant(1),
        );
        let faults = PlannedFaults::one(FileOperation::AppendStage, io::ErrorKind::WriteZero);
        let store =
            DiskStore::open_with_faults(directory.path(), faults).expect("faulted store opens");
        let token = store.begin_stage(source, None).expect("stage begins");
        assert_eq!(
            store.append(token, source, b"new"),
            Err(DiskError::Capacity)
        );
        store.discard(token, source).expect("stage discards");
        assert_eq!(
            read_candidate(
                &store,
                &store.scan(source).expect("scan succeeds").candidates[0]
            ),
            b"old"
        );
    }

    #[test]
    fn revalidation_is_manifest_only_and_interruption_is_atomic() {
        for fault in [
            None,
            Some(FileOperation::WriteRevalidation),
            Some(FileOperation::SyncRevalidation),
            Some(FileOperation::InstallRevalidation),
            Some(FileOperation::SyncRevalidatedDirectory),
        ] {
            let directory = TempDir::new().expect("temporary directory");
            let source = source(DiskKind::M3u, 9);
            let original_store = DiskStore::open(directory.path()).expect("store opens");
            let original = activate_payload(&original_store, source, b"unchanged", instant(1));
            let payload_path = original_store.layout().payload(source.kind, original.slot);
            let before = fs::read(&payload_path).expect("payload reads");
            drop(original_store);

            let store = match fault {
                Some(operation) => DiskStore::open_with_faults(
                    directory.path(),
                    PlannedFaults::one(operation, io::ErrorKind::Other),
                )
                .expect("faulted store opens"),
                None => DiskStore::open(directory.path()).expect("store opens"),
            };
            let candidate = store.scan(source).expect("scan succeeds").candidates[0].clone();
            let updated = store.revalidate(
                &candidate,
                instant(2),
                DiskValidators::new(Some("new-validator".to_owned()), None)
                    .expect("validator is valid"),
            );
            if fault.is_none() || fault == Some(FileOperation::SyncRevalidatedDirectory) {
                let updated = updated.expect("revalidation succeeds");
                assert_eq!(updated.metadata.validated_at, instant(2));
                let token = store
                    .begin_stage(source, Some(&updated))
                    .expect("the returned exact handle protects the revalidated slot");
                store
                    .discard(token, source)
                    .expect("protected stage discards");
            } else {
                assert!(updated.is_err());
            }
            assert_eq!(fs::read(&payload_path).expect("payload reads"), before);

            let restarted = DiskStore::open(directory.path()).expect("store restarts");
            let recovered = &restarted.scan(source).expect("scan succeeds").candidates[0];
            assert!(
                recovered.metadata.validated_at == instant(1)
                    || recovered.metadata.validated_at == instant(2)
            );
            if recovered.metadata.validated_at == instant(2) {
                assert_eq!(
                    recovered.metadata.validators.etag.as_deref(),
                    Some("new-validator")
                );
            } else {
                assert!(recovered.metadata.validators.etag.is_none());
            }
            assert_eq!(read_candidate(&restarted, recovered), b"unchanged");
        }
    }

    #[test]
    fn revalidation_refuses_to_bless_same_length_corrupt_payload_bytes() {
        let directory = TempDir::new().expect("temporary directory");
        let source = source(DiskKind::M3u, 36);
        let store = DiskStore::open(directory.path()).expect("store opens");
        let active = activate_payload(&store, source, b"original", instant(1));
        fs::write(
            store.layout().payload(source.kind, active.slot),
            b"corrupt!",
        )
        .expect("payload corrupts without changing length");
        let candidate = store
            .scan(source)
            .expect("structural scan succeeds")
            .candidates[0]
            .clone();

        assert!(matches!(
            store.revalidate(
                &candidate,
                instant(2),
                DiskValidators::new(Some("must-not-stick".to_owned()), None)
                    .expect("validator is valid"),
            ),
            Err(DiskError::Corrupt)
        ));
        let manifest = read_manifest(&store.layout().manifest(source.kind, active.slot))
            .expect("original manifest remains readable");
        assert_eq!(manifest.validated_at, instant(1));
        assert!(manifest.validators.etag.is_none());
    }

    #[test]
    fn missing_active_manifest_or_payload_falls_back_without_losing_the_alternate() {
        for missing_manifest in [true, false] {
            let directory = TempDir::new().expect("temporary directory");
            let source = source(DiskKind::M3u, if missing_manifest { 37 } else { 38 });
            let store = DiskStore::open(directory.path()).expect("store opens");
            activate_payload(&store, source, b"fallback", instant(1));
            let active = activate_payload(&store, source, b"active", instant(2));
            let removed = if missing_manifest {
                store.layout().manifest(source.kind, active.slot)
            } else {
                store.layout().payload(source.kind, active.slot)
            };
            fs::remove_file(removed).expect("active artifact removes");

            let scan = store.scan(source).expect("snapshot scan succeeds");
            assert_eq!(scan.candidates.len(), 1);
            assert_eq!(read_candidate(&store, &scan.candidates[0]), b"fallback");
            let expected = if missing_manifest {
                SnapshotRecoveryReason::MissingManifest
            } else {
                SnapshotRecoveryReason::MissingPayload
            };
            assert!(scan.diagnostics.contains(&expected));
        }
    }

    #[test]
    fn oversized_and_torn_metadata_are_safely_classified() {
        let directory = TempDir::new().expect("temporary directory");
        let store = DiskStore::open(directory.path()).expect("store opens");
        let source = source(DiskKind::M3u, 10);
        let active = activate_payload(&store, source, b"payload", instant(1));
        fs::write(
            store.layout().manifest(source.kind, active.slot),
            vec![b'x'; (MAX_MANIFEST_BYTES + 1) as usize],
        )
        .expect("manifest corrupts");
        let scan = store.scan(source).expect("scan succeeds");
        assert!(scan.candidates.is_empty());
        assert!(
            scan.diagnostics
                .contains(&SnapshotRecoveryReason::CorruptManifest)
        );

        fs::write(
            store.layout().pointer(source.kind),
            vec![b'x'; (MAX_POINTER_BYTES + 1) as usize],
        )
        .expect("pointer corrupts");
        let scan = store.scan(source).expect("scan succeeds");
        assert!(
            scan.diagnostics
                .contains(&SnapshotRecoveryReason::CorruptActivePointer)
        );
    }

    fn activate_payload(
        store: &DiskStore,
        source: DiskSource,
        payload: &[u8],
        validated_at: DateTime<Utc>,
    ) -> DiskCandidate {
        let token = stage_payload(store, source, payload);
        let metadata = metadata(source, payload, validated_at);
        store.prepare(token, &metadata).expect("stage prepares");
        store.activate(token, &metadata).expect("stage activates")
    }

    fn stage_payload(store: &DiskStore, source: DiskSource, payload: &[u8]) -> u64 {
        let token = store.begin_stage(source, None).expect("stage begins");
        for chunk in payload.chunks(2) {
            store.append(token, source, chunk).expect("chunk appends");
        }
        let mut staged = store.open_staged(token, source).expect("stage reopens");
        let mut observed = Vec::new();
        staged.read_to_end(&mut observed).expect("stage reads");
        assert_eq!(observed, payload);
        token
    }

    fn metadata(source: DiskSource, payload: &[u8], validated_at: DateTime<Utc>) -> DiskMetadata {
        DiskMetadata {
            source,
            decoded_bytes: payload.len() as u64,
            checksum: *blake3::hash(payload).as_bytes(),
            validated_at,
            validators: DiskValidators::default(),
        }
    }

    fn source(kind: DiskKind, discriminator: u8) -> DiskSource {
        DiskSource {
            kind,
            key: [discriminator; 32],
        }
    }

    fn instant(hour: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 29, hour as u32, 0, 0)
            .single()
            .expect("fixture timestamp")
            + Duration::milliseconds(hour)
    }

    fn read_candidate(store: &DiskStore, candidate: &DiskCandidate) -> Vec<u8> {
        let mut reader = store.open_candidate(candidate).expect("candidate opens");
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).expect("candidate reads");
        bytes
    }

    fn persistent_file_count(store: &DiskStore, kind: DiskKind) -> usize {
        fs::read_dir(store.layout().source_dir(kind))
            .expect("source directory reads")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| !is_transient_name(name))
            })
            .count()
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        fs::metadata(path)
            .expect("metadata reads")
            .permissions()
            .mode()
            & 0o777
    }
}
