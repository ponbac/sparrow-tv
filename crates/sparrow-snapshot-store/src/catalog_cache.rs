//! A single, bounded, disposable zero-copy `mmap` cache. Source Snapshots remain authoritative.
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use memmap2::{Mmap, MmapOptions};
use sparrow_core::CatalogCacheMapping;

use crate::disk::{FileTimestamp, VerifiedPayloadStamp};

const CACHE_FILE_NAME: &str = "catalog-cache-v1";
const STAMP_FILE_NAME: &str = "catalog-cache-v1.stamp";
const LEGACY_TEMP_NAME: &str = "catalog-cache-v1.tmp";
const TEMP_PREFIX: &str = "catalog-cache-v1.";
const STAMP_TEMP_PREFIX: &str = "catalog-cache-v1.stamp.";
const TEMP_SUFFIX: &str = ".tmp";
const MAX_BYTES: u64 = 256 * 1024 * 1024;
/// Store-owned envelope on `catalog-cache-v1`: `[0..32]` core cache key,
/// `[32..64]` BLAKE3 payload digest.
const HEADER_BYTES: usize = 64;
/// Store-owned atomic sidecar (`catalog-cache-v1.stamp`): `[0..32]` core cache
/// key, `[32..64]` BLAKE3 payload digest, `[64..96]` verified raw payload stat
/// stamp, `[96..112]` pre-verification filesystem timestamp `(secs, nanos)` LE.
/// Stored separately so verified stamp updates never rewrite or disturb
/// immutable mapped inodes of `catalog-cache-v1`.
const STAMP_BYTES: usize = 112;
const HASH_READ_BUFFER_BYTES: usize = 64 * 1024;
const WRITE_CHUNK_BYTES: usize = 256 * 1024;

static NEXT_TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

pub(crate) struct CatalogCache {
    root: PathBuf,
    write: Mutex<()>,
}

struct MappedCatalogSlice {
    mmap: Mmap,
}

impl AsRef<[u8]> for MappedCatalogSlice {
    fn as_ref(&self) -> &[u8] {
        &self.mmap[HEADER_BYTES..]
    }
}

impl CatalogCache {
    pub(crate) fn new(root: &Path) -> Self {
        remove_abandoned_temps(root);
        Self {
            root: root.to_path_buf(),
            write: Mutex::new(()),
        }
    }

    pub(crate) fn open(
        &self,
        key: &[u8; 32],
        live_payload_stamp: &[u8; 32],
        max_payload_freshness: Option<FileTimestamp>,
        reverify_raw: impl FnOnce() -> Option<VerifiedPayloadStamp>,
        validate: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Option<CatalogCacheMapping> {
        let path = self.root.join(CACHE_FILE_NAME);
        let mut file = open_private_cache_file(&path)?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.len() > MAX_BYTES || metadata.len() < HEADER_BYTES as u64
        {
            return None;
        }
        let total_len = usize::try_from(metadata.len()).ok()?;
        let expected_payload_len = total_len.checked_sub(HEADER_BYTES)?;

        let mut header = [0_u8; HEADER_BYTES];
        file.read_exact(&mut header).ok()?;
        if &header[..32] != key {
            return None;
        }
        let payload_digest: [u8; 32] = header[32..64].try_into().ok()?;

        // Fast path: when `catalog-cache-v1.stamp` matches `(key, payload_digest,
        // live_payload_stamp)` AND every participating raw payload's `mtime` and
        // `ctime` strictly preceded the pre-hashing filesystem boundary stored
        // in the sidecar, skip raw hashing. Otherwise, verify the participating
        // raw payloads directly and atomically persist the refreshed sidecar
        // once the cache itself passes verification.
        let fast_path_ok = read_stamp_file(&self.root).is_some_and(|stamp_bytes| {
            let verified_before = (
                i64::from_le_bytes(stamp_bytes[96..104].try_into().expect("8-byte secs")),
                i64::from_le_bytes(stamp_bytes[104..112].try_into().expect("8-byte nanos")),
            );
            &stamp_bytes[..32] == key
                && stamp_bytes[32..64] == payload_digest
                && &stamp_bytes[64..96] == live_payload_stamp
                && max_payload_freshness.is_some_and(|freshness| freshness < verified_before)
        });
        let refreshed_stamp = if fast_path_ok {
            None
        } else {
            Some(reverify_raw()?)
        };

        // Stream-hash payload via `read()` before mapping so flash I/O errors
        // surface cleanly as `EIO` (fallback to Source Snapshots) instead of
        // `SIGBUS`, and hashing installs zero PTEs in our process's page table.
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0_u8; HASH_READ_BUFFER_BYTES];
        let mut total_read = 0_usize;
        loop {
            let read = file.read(&mut buffer).ok()?;
            if read == 0 {
                break;
            }
            total_read = total_read.checked_add(read)?;
            if total_read > expected_payload_len {
                return None;
            }
            hasher.update(&buffer[..read]);
        }
        if total_read != expected_payload_len || hasher.finalize().as_bytes() != &payload_digest {
            return None;
        }

        // Re-check file size on the same fd before mapping.
        if file.metadata().ok()?.len() != metadata.len() {
            return None;
        }

        // Validate on a throwaway mapping, then drop it (`munmap` clears all
        // validation PTEs cleanly without unsafe madvise) before creating a
        // fresh serving map on the exact same verified file descriptor.
        let validation_mmap = map_readonly_file(&file, total_len)?;
        let valid = validate(&validation_mmap[HEADER_BYTES..]);
        drop(validation_mmap);
        if !valid || file.metadata().ok()?.len() != total_len as u64 {
            return None;
        }
        let serving = map_readonly_file(&file, total_len)?;
        drop(file);
        if let Some(verified) = refreshed_stamp {
            self.persist_verified_stamp(key, &payload_digest, verified);
        }
        Some(CatalogCacheMapping::new(MappedCatalogSlice {
            mmap: serving,
        }))
    }

    fn persist_verified_stamp(
        &self,
        key: &[u8; 32],
        payload_digest: &[u8; 32],
        verified: VerifiedPayloadStamp,
    ) {
        let Ok(_guard) = self.write.lock() else {
            return;
        };
        // Only update the sidecar if the current on-disk `catalog-cache-v1`
        // still carries the exact `(key, payload_digest)` we just verified.
        let path = self.root.join(CACHE_FILE_NAME);
        let Some(mut current) = open_private_cache_file(&path) else {
            return;
        };
        let mut current_header = [0_u8; HEADER_BYTES];
        if current.read_exact(&mut current_header).is_err()
            || &current_header[..32] != key
            || &current_header[32..64] != payload_digest
        {
            return;
        }
        drop(current);
        let _ = write_stamp_file(&self.root, key, payload_digest, verified);
    }

    pub(crate) const fn accepts_payload_len(bytes_len: usize) -> bool {
        match (bytes_len as u64).checked_add(HEADER_BYTES as u64) {
            Some(total) => total <= MAX_BYTES,
            None => false,
        }
    }

    pub(crate) fn write(
        &self,
        key: &[u8; 32],
        verified_stamp: VerifiedPayloadStamp,
        bytes: &[u8],
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Option<CatalogCacheMapping> {
        if !Self::accepts_payload_len(bytes.len()) || is_cancelled() {
            return None;
        }
        let total_len = bytes.len().checked_add(HEADER_BYTES)?;
        let Ok(_guard) = self.write.lock() else {
            return None;
        };
        if is_cancelled() {
            return None;
        }

        // Compute payload digest in chunks so superseded builds abort early.
        let mut hasher = blake3::Hasher::new();
        for chunk in bytes.chunks(WRITE_CHUNK_BYTES) {
            if is_cancelled() {
                return None;
            }
            hasher.update(chunk);
        }
        let digest = hasher.finalize();

        let (temporary, mut file) = create_unique_temp(&self.root, TEMP_PREFIX)?;
        let result = (|| -> std::io::Result<Option<Mmap>> {
            file.write_all(key)?;
            file.write_all(digest.as_bytes())?;
            for chunk in bytes.chunks(WRITE_CHUNK_BYTES) {
                if is_cancelled() {
                    return Ok(None);
                }
                file.write_all(chunk)?;
            }
            if is_cancelled() {
                return Ok(None);
            }
            file.sync_all()?;
            if is_cancelled() {
                return Ok(None);
            }
            fs::rename(&temporary, self.root.join(CACHE_FILE_NAME))?;
            sync_parent_best_effort(&self.root);
            // Map the exact written descriptor (never reopen by path) and then
            // drop the writable file handle so zero writable fds remain open.
            let mmap = map_readonly_file(&file, total_len)
                .ok_or_else(|| std::io::Error::other("failed to map persisted catalog cache"))?;
            Ok(Some(mmap))
        })();

        drop(file);
        match result {
            Ok(Some(mmap)) => {
                // Best-effort atomic sidecar write while still holding `self.write`;
                // if this fails, the next launch self-heals via `reverify_raw`.
                let _ = write_stamp_file(&self.root, key, digest.as_bytes(), verified_stamp);
                Some(CatalogCacheMapping::new(MappedCatalogSlice { mmap }))
            }
            Ok(None) | Err(_) => {
                let _ = fs::remove_file(&temporary);
                None
            }
        }
    }
}

/// Maps an immutable, private, regular cache file descriptor read-only.
///
/// Safety boundary:
/// - `catalog-cache-v1` resides inside app-private `0700` storage owned by one
///   installed Sparrow instance via `InstanceLock`.
/// - Sparrow never opens `catalog-cache-v1` for writing or truncates it in
///   place; replacements always write to a unique exclusively created temporary
///   file, `sync_all()`, and atomically `rename()` over the destination, keeping
///   any previously mapped inode immutable until every `Mmap` is dropped.
/// - Cooperative single-writer immutability is therefore upheld by construction
///   (hostile same-UID external truncation of an active mapped inode is outside
///   POSIX `mmap`'s recoverability boundary).
fn map_readonly_file(file: &File, len: usize) -> Option<Mmap> {
    if len < HEADER_BYTES {
        return None;
    }
    // SAFETY: `file` is a verified regular file inside private storage that
    // Sparrow never modifies or truncates in place after atomic publication,
    // and `len` matches its verified byte length.
    let mmap = unsafe { MmapOptions::new().len(len).map(file).ok()? };
    (mmap.len() == len).then_some(mmap)
}

fn open_private_cache_file(path: &Path) -> Option<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    options.open(path).ok()
}

fn read_stamp_file(root: &Path) -> Option<[u8; STAMP_BYTES]> {
    let mut file = open_private_cache_file(&root.join(STAMP_FILE_NAME))?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() != STAMP_BYTES as u64 {
        return None;
    }
    let mut bytes = [0_u8; STAMP_BYTES];
    file.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

fn write_stamp_file(
    root: &Path,
    key: &[u8; 32],
    payload_digest: &[u8; 32],
    verified: VerifiedPayloadStamp,
) -> std::io::Result<()> {
    let (temporary, mut file) = create_unique_temp(root, STAMP_TEMP_PREFIX)
        .ok_or_else(|| std::io::Error::other("failed to create catalog cache stamp temp"))?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(key)?;
        file.write_all(payload_digest)?;
        file.write_all(&verified.stamp)?;
        file.write_all(&verified.verified_before.0.to_le_bytes())?;
        file.write_all(&verified.verified_before.1.to_le_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, root.join(STAMP_FILE_NAME))?;
        sync_parent_best_effort(root);
        Ok(())
    })();
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_unique_temp(root: &Path, prefix: &str) -> Option<(PathBuf, File)> {
    let pid = std::process::id();
    for _ in 0..64 {
        let seq = NEXT_TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let temporary = root.join(format!("{prefix}{pid}.{seq}{TEMP_SUFFIX}"));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
            options.custom_flags(libc::O_NOFOLLOW);
        }
        match options.open(&temporary) {
            Ok(file) => return Some((temporary, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

fn remove_abandoned_temps(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == LEGACY_TEMP_NAME
            || (name.starts_with(TEMP_PREFIX) && name.ends_with(TEMP_SUFFIX))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn sync_parent_best_effort(root: &Path) {
    #[cfg(unix)]
    {
        if let Ok(dir) = File::open(root) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = root;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn never_cancelled() -> bool {
        false
    }

    fn verified(stamp: [u8; 32], verified_before: FileTimestamp) -> VerifiedPayloadStamp {
        VerifiedPayloadStamp {
            stamp,
            verified_before,
        }
    }

    #[test]
    fn private_atomic_cache_rejects_links_fifos_and_oversize_files() {
        let root = tempfile::tempdir().unwrap();
        let cache = CatalogCache::new(root.path());
        let key = [7; 32];
        let stamp = [3; 32];
        let boundary = (100, 0);
        let written = cache
            .write(
                &key,
                verified(stamp, boundary),
                b"prepared catalog",
                &never_cancelled,
            )
            .expect("write returns serving mapping");
        assert_eq!(written.as_bytes(), b"prepared catalog");
        let path = root.path().join(CACHE_FILE_NAME);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let serving = cache
            .open(
                &key,
                &stamp,
                Some((99, 999_999_999)),
                || None,
                &mut |bytes| bytes == b"prepared catalog",
            )
            .expect("open verifies written cache without raw rehash");
        assert_eq!(serving.as_bytes(), b"prepared catalog");

        // Validator rejection returns None cleanly.
        assert!(
            cache
                .open(
                    &key,
                    &stamp,
                    Some((99, 0)),
                    || Some(verified(stamp, boundary)),
                    &mut |_| false,
                )
                .is_none()
        );

        // Atomic replacement keeps earlier serving mappings readable without interference.
        let replacement = cache
            .write(
                &key,
                verified(stamp, boundary),
                b"updated catalog payload",
                &never_cancelled,
            )
            .expect("atomic replacement succeeds");
        assert_eq!(serving.as_bytes(), b"prepared catalog");
        assert_eq!(replacement.as_bytes(), b"updated catalog payload");
        drop(written);
        drop(serving);
        drop(replacement);

        // Reject symlink at final path even when its target is an otherwise
        // valid, directly-openable cache file.
        let valid_target = root.path().join("valid-cache-target");
        fs::rename(&path, &valid_target).unwrap();
        // Sanity check: moving the exact valid file back would open cleanly,
        // whereas pointing a symlink at it must be rejected by O_NOFOLLOW.
        let direct_bytes = fs::read(&valid_target).unwrap();
        assert!(direct_bytes.len() >= HEADER_BYTES);
        fs::write(&path, &direct_bytes).unwrap();
        assert!(
            cache
                .open(&key, &stamp, Some((99, 0)), || None, &mut |bytes| bytes
                    == b"updated catalog payload",)
                .is_some(),
            "direct regular file with identical contents must open cleanly"
        );
        fs::remove_file(&path).unwrap();
        symlink(&valid_target, &path).unwrap();
        assert!(
            cache
                .open(
                    &key,
                    &stamp,
                    Some((99, 0)),
                    || Some(verified(stamp, boundary)),
                    &mut |bytes| bytes == b"updated catalog payload",
                )
                .is_none(),
            "symlink to an otherwise valid cache file must be rejected"
        );

        // Legacy temp symlink is never followed; new unique temp replaces destination cleanly.
        let other = root.path().join("unrelated");
        fs::write(&other, b"keep me").unwrap();
        symlink(&other, root.path().join(LEGACY_TEMP_NAME)).unwrap();
        let written = cache
            .write(
                &key,
                verified(stamp, boundary),
                b"new catalog",
                &never_cancelled,
            )
            .expect("write succeeds safely");
        assert_eq!(fs::read(&other).unwrap(), b"keep me");
        assert_eq!(written.as_bytes(), b"new catalog");
        drop(written);

        // Reject FIFO at final path without blocking (created via safe subprocess).
        fs::remove_file(&path).unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg("-m")
            .arg("600")
            .arg(&path)
            .status()
            .expect("mkfifo runs");
        assert!(status.success(), "mkfifo exited with {status}");
        assert!(
            cache
                .open(
                    &key,
                    &stamp,
                    None,
                    || Some(verified(stamp, boundary)),
                    &mut |_| true,
                )
                .is_none()
        );
        fs::remove_file(&path).unwrap();

        // Reject oversize file cleanly.
        let oversized = root.path().join("oversize.tmp");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&oversized)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        fs::rename(&oversized, &path).unwrap();
        assert!(
            cache
                .open(
                    &key,
                    &stamp,
                    None,
                    || Some(verified(stamp, boundary)),
                    &mut |_| true,
                )
                .is_none()
        );
    }

    #[test]
    fn cancelled_write_aborts_without_replacing_existing_cache() {
        let root = tempfile::tempdir().unwrap();
        let cache = CatalogCache::new(root.path());
        let key = [9; 32];
        let stamp = [5; 32];
        let boundary = (100, 0);
        let initial = cache
            .write(
                &key,
                verified(stamp, boundary),
                b"initial bytes",
                &never_cancelled,
            )
            .expect("initial write succeeds");
        assert!(
            cache
                .write(&key, verified(stamp, boundary), b"cancelled bytes", &|| {
                    true
                },)
                .is_none()
        );
        let reopened = cache
            .open(
                &key,
                &stamp,
                None,
                || Some(verified(stamp, boundary)),
                &mut |_| true,
            )
            .expect("existing cache remains intact");
        assert_eq!(reopened.as_bytes(), b"initial bytes");
        drop(reopened);
        drop(initial);
    }

    #[test]
    fn refreshed_stamp_persists_atomically_and_enforces_pre_hashing_boundary() {
        use std::os::unix::fs::MetadataExt;
        use std::sync::atomic::AtomicUsize;

        let root = tempfile::tempdir().unwrap();
        let cache = CatalogCache::new(root.path());
        let key = [11; 32];
        let initial_stamp = [1; 32];
        let restored_stamp = [2; 32];
        // Even though the sidecar file on disk gets the current wall-clock
        // mtime, fast-path reuse MUST compare against the recorded pre-hashing
        // boundary `(100, 0)`.
        let initial_mapping = cache
            .write(
                &key,
                verified(initial_stamp, (100, 0)),
                b"immutable catalog",
                &never_cancelled,
            )
            .expect("initial write succeeds");
        let cache_inode = fs::metadata(root.path().join(CACHE_FILE_NAME))
            .unwrap()
            .ino();

        // A payload timestamp equal to the pre-hashing boundary `(100, 0)` MUST
        // trigger re-verification even when the stamp bytes match and the
        // sidecar file's own mtime is far newer.
        let boundary_checks = AtomicUsize::new(0);
        let boundary_reopen = cache
            .open(
                &key,
                &initial_stamp,
                Some((100, 0)),
                || {
                    boundary_checks.fetch_add(1, Ordering::Relaxed);
                    Some(verified(initial_stamp, (101, 0)))
                },
                &mut |bytes| bytes == b"immutable catalog",
            )
            .expect("boundary reopen succeeds after reverify");
        assert_eq!(boundary_checks.load(Ordering::Relaxed), 1);
        drop(boundary_reopen);

        // First open with a changed stamp (same-key byte-identical restore)
        // runs `reverify_raw` once and persists the verified sidecar stamp
        // without changing `catalog-cache-v1`'s inode.
        let reverify_calls = AtomicUsize::new(0);
        let first_reopen = cache
            .open(
                &key,
                &restored_stamp,
                Some((100, 0)),
                || {
                    reverify_calls.fetch_add(1, Ordering::Relaxed);
                    Some(verified(restored_stamp, (200, 0)))
                },
                &mut |bytes| bytes == b"immutable catalog",
            )
            .expect("first reopen succeeds after reverify");
        assert_eq!(reverify_calls.load(Ordering::Relaxed), 1);
        assert_eq!(first_reopen.as_bytes(), b"immutable catalog");
        assert_eq!(initial_mapping.as_bytes(), b"immutable catalog");
        assert_eq!(
            fs::metadata(root.path().join(CACHE_FILE_NAME))
                .unwrap()
                .ino(),
            cache_inode
        );

        // Second open with payload freshness strictly preceding `(200, 0)` hits
        // the fast path with zero raw verification calls.
        let second_reopen = cache
            .open(
                &key,
                &restored_stamp,
                Some((100, 0)),
                || {
                    reverify_calls.fetch_add(1, Ordering::Relaxed);
                    Some(verified(restored_stamp, (201, 0)))
                },
                &mut |bytes| bytes == b"immutable catalog",
            )
            .expect("second reopen skips raw rehash");
        assert_eq!(reverify_calls.load(Ordering::Relaxed), 1);
        assert_eq!(second_reopen.as_bytes(), b"immutable catalog");

        // Envelope size gate rejects payloads that would exceed MAX_BYTES.
        assert!(CatalogCache::accepts_payload_len(
            MAX_BYTES as usize - HEADER_BYTES
        ));
        assert!(!CatalogCache::accepts_payload_len(
            MAX_BYTES as usize - HEADER_BYTES + 1
        ));
    }
}
