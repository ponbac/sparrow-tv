//! A single, bounded, disposable cache. Source Snapshots remain authoritative.
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::Mutex,
};

const MAX_BYTES: u64 = 256 * 1024 * 1024;
const HEADER_BYTES: usize = 64;

pub(crate) struct CatalogCache {
    root: std::path::PathBuf,
    write: Mutex<()>,
}

impl CatalogCache {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            write: Mutex::new(()),
        }
    }

    pub(crate) fn read(&self, key: &[u8; 32]) -> Option<Vec<u8>> {
        let path = self.root.join("catalog-cache-v1");
        // Reject links and non-regular files, just like private Source Snapshots.
        let metadata = fs::symlink_metadata(&path).ok()?;
        if !metadata.is_file() || metadata.len() > MAX_BYTES || metadata.len() < HEADER_BYTES as u64
        {
            return None;
        }
        let mut file = fs::File::open(path).ok()?;
        let mut header = [0; HEADER_BYTES];
        file.read_exact(&mut header).ok()?;
        if &header[..32] != key {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() as u64 + HEADER_BYTES as u64 > MAX_BYTES
            || blake3::hash(&bytes).as_bytes() != &header[32..]
        {
            return None;
        }
        Some(bytes)
    }

    pub(crate) fn write(&self, key: &[u8; 32], bytes: &[u8]) {
        if bytes.len() as u64 + HEADER_BYTES as u64 > MAX_BYTES {
            return;
        }
        let Ok(_guard) = self.write.lock() else {
            return;
        };
        let temporary = self.root.join("catalog-cache-v1.tmp");
        let result = (|| -> std::io::Result<()> {
            // An abandoned temporary is never read. Remove it before create_new
            // so a symlink cannot redirect a write outside private app data.
            match fs::remove_file(&temporary) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&temporary)?;
            file.write_all(key)?;
            file.write_all(blake3::hash(bytes).as_bytes())?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, self.root.join("catalog-cache-v1"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn private_atomic_cache_rejects_links_and_oversize_files() {
        let root = tempfile::tempdir().unwrap();
        let cache = CatalogCache::new(root.path());
        let key = [7; 32];
        cache.write(&key, b"prepared catalog");
        let path = root.path().join("catalog-cache-v1");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(cache.read(&key).unwrap(), b"prepared catalog");
        let other = root.path().join("unrelated");
        fs::write(&other, b"keep me").unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&other, &path).unwrap();
        assert!(cache.read(&key).is_none());
        symlink(&other, root.path().join("catalog-cache-v1.tmp")).unwrap();
        cache.write(&key, b"new catalog");
        assert_eq!(fs::read(&other).unwrap(), b"keep me");
        assert_eq!(cache.read(&key).unwrap(), b"new catalog");
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(cache.read(&key).is_none());
    }
}
