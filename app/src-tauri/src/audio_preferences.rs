use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};
use sparrow_core::ChannelId;

use crate::{config_store::ensure_private_directory, selected_transport_stream::AudioTrackId};

const STORE_VERSION: u8 = 1;
const STORE_FILE: &str = "audio-track-preferences.json";
const PENDING_FILE: &str = ".audio-track-preferences.pending";
const MAX_PREFERENCES: usize = 4096;
const MAX_STORE_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct AudioPreferenceStore {
    root: PathBuf,
    preferences: Arc<RwLock<BTreeMap<String, AudioTrackId>>>,
    writes: Arc<tokio::sync::Mutex<()>>,
    writable: bool,
}

impl AudioPreferenceStore {
    /// Opens a best-effort preference store. Corruption or an unsafe preference
    /// file disables writes but can never prevent catalog or playback startup.
    pub(crate) fn open(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let loaded = load(&root);
        let (preferences, writable) = match loaded {
            Ok(preferences) => (preferences, true),
            Err(_) => (BTreeMap::new(), false),
        };
        Self {
            root,
            preferences: Arc::new(RwLock::new(preferences)),
            writes: Arc::new(tokio::sync::Mutex::new(())),
            writable,
        }
    }

    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self {
            root: PathBuf::new(),
            preferences: Arc::new(RwLock::new(BTreeMap::new())),
            writes: Arc::new(tokio::sync::Mutex::new(())),
            writable: false,
        }
    }

    pub(crate) fn preference(&self, channel_id: &ChannelId) -> Option<AudioTrackId> {
        self.preferences
            .read()
            .expect("audio preference state poisoned")
            .get(channel_id.as_str())
            .cloned()
    }

    pub(crate) async fn remember(
        &self,
        channel_id: ChannelId,
        track_id: AudioTrackId,
    ) -> PreferenceWrite {
        if !self.writable {
            return PreferenceWrite::NotSaved;
        }
        let write_guard = Arc::clone(&self.writes).lock_owned().await;
        let root = self.root.clone();
        let preferences = Arc::clone(&self.preferences);
        tokio::task::spawn_blocking(move || {
            // Keep the owned guard inside the blocking task: once a write has
            // started, cancelling its caller cannot race a later replacement.
            let _write_guard = write_guard;
            let replacement = {
                let current = preferences.read().expect("audio preference state poisoned");
                if current.get(channel_id.as_str()) == Some(&track_id) {
                    return PreferenceWrite::Unchanged;
                }
                if current.len() >= MAX_PREFERENCES && !current.contains_key(channel_id.as_str()) {
                    return PreferenceWrite::NotSaved;
                }
                let mut replacement = current.clone();
                replacement.insert(channel_id.as_str().to_owned(), track_id);
                replacement
            };
            if save(&root, &replacement).is_err() {
                return PreferenceWrite::NotSaved;
            }
            *preferences
                .write()
                .expect("audio preference state poisoned") = replacement;
            PreferenceWrite::Saved
        })
        .await
        .unwrap_or(PreferenceWrite::NotSaved)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreferenceWrite {
    Saved,
    NotSaved,
    Unchanged,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreferenceRecord {
    version: u8,
    entries: BTreeMap<String, String>,
}

fn load(root: &Path) -> Result<BTreeMap<String, AudioTrackId>, PreferenceStoreError> {
    ensure_private_directory(root).map_err(|_| PreferenceStoreError)?;
    let path = root.join(STORE_FILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(_) => return Err(PreferenceStoreError),
    };
    validate_private_regular_file(&metadata)?;
    if metadata.len() > MAX_STORE_BYTES {
        return Err(PreferenceStoreError);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| PreferenceStoreError)?;
    let opened = file.metadata().map_err(|_| PreferenceStoreError)?;
    validate_private_regular_file(&opened)?;
    if opened.len() > MAX_STORE_BYTES {
        return Err(PreferenceStoreError);
    }
    let mut bytes =
        Vec::with_capacity(usize::try_from(opened.len()).map_err(|_| PreferenceStoreError)?);
    Read::by_ref(&mut file)
        .take(MAX_STORE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| PreferenceStoreError)?;
    if bytes.len() as u64 > MAX_STORE_BYTES {
        return Err(PreferenceStoreError);
    }
    let record: PreferenceRecord =
        serde_json::from_slice(&bytes).map_err(|_| PreferenceStoreError)?;
    if record.version != STORE_VERSION || record.entries.len() > MAX_PREFERENCES {
        return Err(PreferenceStoreError);
    }
    record
        .entries
        .into_iter()
        .map(|(channel, track)| {
            let channel = ChannelId::parse(channel).map_err(|_| PreferenceStoreError)?;
            let track = AudioTrackId::parse(track).map_err(|_| PreferenceStoreError)?;
            Ok((channel.as_str().to_owned(), track))
        })
        .collect()
}

fn save(
    root: &Path,
    preferences: &BTreeMap<String, AudioTrackId>,
) -> Result<(), PreferenceStoreError> {
    if preferences.len() > MAX_PREFERENCES {
        return Err(PreferenceStoreError);
    }
    ensure_private_directory(root).map_err(|_| PreferenceStoreError)?;
    let entries = preferences
        .iter()
        .map(|(channel, track)| (channel.clone(), track.as_str().to_owned()))
        .collect();
    let bytes = serde_json::to_vec(&PreferenceRecord {
        version: STORE_VERSION,
        entries,
    })
    .map_err(|_| PreferenceStoreError)?;
    if bytes.len() as u64 > MAX_STORE_BYTES {
        return Err(PreferenceStoreError);
    }

    let pending = root.join(PENDING_FILE);
    remove_abandoned_pending(&pending)?;
    let result = write_and_activate(root, &pending, &bytes);
    if result.is_err() {
        let _ = fs::remove_file(&pending);
    }
    result
}

fn write_and_activate(
    root: &Path,
    pending: &Path,
    bytes: &[u8],
) -> Result<(), PreferenceStoreError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(pending)
        .map_err(|_| PreferenceStoreError)?;
    file.write_all(bytes).map_err(|_| PreferenceStoreError)?;
    file.sync_all().map_err(|_| PreferenceStoreError)?;
    drop(file);
    fs::rename(pending, root.join(STORE_FILE)).map_err(|_| PreferenceStoreError)?;
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| PreferenceStoreError)
}

fn remove_abandoned_pending(path: &Path) -> Result<(), PreferenceStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_private_regular_file(&metadata)?;
            fs::remove_file(path).map_err(|_| PreferenceStoreError)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PreferenceStoreError),
    }
}

fn validate_private_regular_file(metadata: &fs::Metadata) -> Result<(), PreferenceStoreError> {
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(PreferenceStoreError);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PreferenceStoreError;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preferences_round_trip_and_report_unchanged() {
        let root = tempfile::tempdir().expect("temporary directory");
        let channel = channel(1);
        let track = track(1);
        let store = AudioPreferenceStore::open(root.path());
        assert_eq!(
            store.remember(channel.clone(), track.clone()).await,
            PreferenceWrite::Saved
        );
        assert_eq!(
            store.remember(channel.clone(), track.clone()).await,
            PreferenceWrite::Unchanged
        );
        let reopened = AudioPreferenceStore::open(root.path());
        assert_eq!(reopened.preference(&channel), Some(track));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unsafe_or_corrupt_records_degrade_without_overwriting() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temporary directory");
        let target = root.path().join("outside");
        fs::write(&target, b"canary").expect("writes canary");
        symlink(&target, root.path().join(STORE_FILE)).expect("creates unsafe link");
        let store = AudioPreferenceStore::open(root.path());
        assert_eq!(
            store.remember(channel(2), track(2)).await,
            PreferenceWrite::NotSaved
        );
        assert_eq!(fs::read(target).expect("reads canary"), b"canary");
    }

    fn channel(sequence: u8) -> ChannelId {
        ChannelId::parse(format!("ch1_{sequence:064x}")).expect("fixture channel ID")
    }

    fn track(sequence: u8) -> AudioTrackId {
        AudioTrackId::parse(format!("atrk1_{sequence:032x}")).expect("fixture track ID")
    }
}
