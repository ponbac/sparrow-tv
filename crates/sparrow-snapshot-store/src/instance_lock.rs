use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

use std::fs::TryLockError;

const LOCK_FILE: &str = "instance.lock";

/// Holds exclusive access to the installed app's private data directory.
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire(private_root: &Path) -> Result<Self, InstanceLockError> {
        let path = private_root.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|_| InstanceLockError::Unavailable)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| InstanceLockError::Unavailable)?;
        let metadata = file
            .metadata()
            .map_err(|_| InstanceLockError::Unavailable)?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(InstanceLockError::Unavailable);
        }
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(InstanceLockError::AlreadyRunning),
            Err(_) => Err(InstanceLockError::Unavailable),
        }
    }
}

impl std::fmt::Debug for InstanceLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InstanceLock(<held>)")
    }
}

/// A safe lock failure that never contains the app-data path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InstanceLockError {
    #[error("another Sparrow instance is already using this app data")]
    AlreadyRunning,
    #[error("the Sparrow instance lock is unavailable")]
    Unavailable,
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{DirBuilderExt, symlink};

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn excludes_a_second_instance_and_releases_on_drop() {
        let directory = TempDir::new().expect("temporary directory");
        let root = directory.path().join("private");
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();

        let first = InstanceLock::acquire(&root).expect("first lock succeeds");
        assert_eq!(
            InstanceLock::acquire(&root).expect_err("second lock is excluded"),
            InstanceLockError::AlreadyRunning
        );
        drop(first);
        InstanceLock::acquire(&root).expect("lock is released with its owner");
    }

    #[test]
    fn diagnostics_do_not_expose_the_lock_path() {
        let private_canary = "private-instance-canary";
        for error in [
            InstanceLockError::AlreadyRunning,
            InstanceLockError::Unavailable,
        ] {
            assert!(!format!("{error:?} {error}").contains(private_canary));
        }
    }

    #[test]
    fn refuses_a_symlinked_lock_without_changing_its_target() {
        let directory = TempDir::new().expect("temporary directory");
        let root = directory.path().join("private");
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let outside = directory.path().join("outside");
        fs::write(&outside, b"outside").expect("outside fixture writes");
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o644))
            .expect("outside fixture permissions set");
        symlink(&outside, root.join(LOCK_FILE)).expect("lock symlink creates");

        assert_eq!(
            InstanceLock::acquire(&root).expect_err("lock symlink is rejected"),
            InstanceLockError::Unavailable
        );
        assert_eq!(
            fs::metadata(outside)
                .expect("outside metadata remains")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }
}
