use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const CONFIGURATION_VERSION: u8 = 1;
const MAX_LOCATION_BYTES: usize = 16 * 1024;
// Each allowed byte can require one additional JSON escape byte (`"` or `\\`).
const MAX_CONFIGURATION_BYTES: u64 = (4 * MAX_LOCATION_BYTES + 512) as u64;
const CONFIGURATION_FILE: &str = "source-configuration.json";
const PENDING_FILE: &str = ".source-configuration.pending";

/// Private source locations loaded from or ready to be written to device storage.
///
/// This value intentionally implements neither `Debug` nor `Display`.
pub(crate) struct StoredSourceConfiguration {
    m3u_location: String,
    epg_location: Option<String>,
}

impl StoredSourceConfiguration {
    pub(crate) fn normalized(m3u_location: String, epg_location: Option<String>) -> Self {
        let m3u_location = m3u_location.trim().to_owned();
        let epg_location = epg_location.and_then(|location| {
            let location = location.trim().to_owned();
            (!location.is_empty()).then_some(location)
        });
        Self {
            m3u_location,
            epg_location,
        }
    }

    pub(crate) fn source_input(&self) -> sparrow_core::SourceConfigurationInput {
        sparrow_core::SourceConfigurationInput::new(
            self.m3u_location.clone(),
            self.epg_location.clone(),
        )
    }
}

#[derive(Clone)]
pub(crate) struct SourceConfigurationStore {
    root: PathBuf,
}

impl SourceConfigurationStore {
    pub(crate) fn open(root: impl AsRef<Path>) -> Result<Self, ConfigurationStoreError> {
        ensure_private_directory(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
        })
    }

    pub(crate) fn load(
        &self,
    ) -> Result<Option<StoredSourceConfiguration>, ConfigurationStoreError> {
        let path = self.root.join(CONFIGURATION_FILE);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ConfigurationStoreError::Unavailable),
        };
        validate_private_regular_file(&metadata)?;
        if metadata.len() > MAX_CONFIGURATION_BYTES {
            return Err(ConfigurationStoreError::Corrupt);
        }

        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|_| ConfigurationStoreError::Unavailable)?;
        let opened_metadata = file
            .metadata()
            .map_err(|_| ConfigurationStoreError::Unavailable)?;
        validate_private_regular_file(&opened_metadata)?;
        if opened_metadata.len() > MAX_CONFIGURATION_BYTES {
            return Err(ConfigurationStoreError::Corrupt);
        }
        let mut bytes = Vec::with_capacity(
            usize::try_from(opened_metadata.len()).map_err(|_| ConfigurationStoreError::Corrupt)?,
        );
        Read::by_ref(&mut file)
            .take(MAX_CONFIGURATION_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ConfigurationStoreError::Unavailable)?;
        if bytes.len() as u64 > MAX_CONFIGURATION_BYTES {
            return Err(ConfigurationStoreError::Corrupt);
        }

        let record: ConfigurationRecord =
            serde_json::from_slice(&bytes).map_err(|_| ConfigurationStoreError::Corrupt)?;
        if record.version != CONFIGURATION_VERSION
            || !valid_location_record(&record.m3u_location)
            || record
                .epg_location
                .as_deref()
                .is_some_and(|location| !valid_location_record(location))
        {
            return Err(ConfigurationStoreError::Corrupt);
        }

        Ok(Some(StoredSourceConfiguration {
            m3u_location: record.m3u_location,
            epg_location: record.epg_location,
        }))
    }

    pub(crate) fn save(
        &self,
        configuration: &StoredSourceConfiguration,
    ) -> Result<(), ConfigurationStoreError> {
        if !valid_location_record(&configuration.m3u_location)
            || configuration
                .epg_location
                .as_deref()
                .is_some_and(|location| !valid_location_record(location))
        {
            return Err(ConfigurationStoreError::Corrupt);
        }
        ensure_private_directory(&self.root)?;

        let bytes = serde_json::to_vec(&ConfigurationRecord {
            version: CONFIGURATION_VERSION,
            m3u_location: configuration.m3u_location.clone(),
            epg_location: configuration.epg_location.clone(),
        })
        .map_err(|_| ConfigurationStoreError::Corrupt)?;
        if bytes.len() as u64 > MAX_CONFIGURATION_BYTES {
            return Err(ConfigurationStoreError::Corrupt);
        }

        let pending = self.root.join(PENDING_FILE);
        remove_abandoned_pending(&pending)?;
        let result = write_and_activate(&self.root, &pending, &bytes);
        if result.is_err() {
            let _ = fs::remove_file(&pending);
        }
        result
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigurationRecord {
    version: u8,
    m3u_location: String,
    epg_location: Option<String>,
}

fn write_and_activate(
    root: &Path,
    pending: &Path,
    bytes: &[u8],
) -> Result<(), ConfigurationStoreError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(pending)
        .map_err(map_write_error)?;
    file.write_all(bytes).map_err(map_write_error)?;
    file.sync_all().map_err(map_write_error)?;
    drop(file);

    fs::rename(pending, root.join(CONFIGURATION_FILE)).map_err(map_write_error)?;
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(map_write_error)
}

fn remove_abandoned_pending(path: &Path) -> Result<(), ConfigurationStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_private_regular_file(&metadata)?;
            fs::remove_file(path).map_err(|_| ConfigurationStoreError::Unavailable)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ConfigurationStoreError::Unavailable),
    }
}

pub(crate) fn ensure_private_directory(path: &Path) -> Result<(), ConfigurationStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ConfigurationStoreError::UnsafeLayout);
            }
            if metadata.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .map_err(|_| ConfigurationStoreError::UnsafeLayout)?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder
                .create(path)
                .map_err(|_| ConfigurationStoreError::Unavailable)?;
        }
        Err(_) => return Err(ConfigurationStoreError::Unavailable),
    }

    let metadata = fs::symlink_metadata(path).map_err(|_| ConfigurationStoreError::Unavailable)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(ConfigurationStoreError::UnsafeLayout);
    }
    Ok(())
}

fn validate_private_regular_file(metadata: &fs::Metadata) -> Result<(), ConfigurationStoreError> {
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(ConfigurationStoreError::UnsafeLayout);
    }
    Ok(())
}

fn valid_location_record(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_LOCATION_BYTES
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn map_write_error(error: io::Error) -> ConfigurationStoreError {
    match error.kind() {
        io::ErrorKind::StorageFull | io::ErrorKind::WriteZero => ConfigurationStoreError::Capacity,
        _ => ConfigurationStoreError::Unavailable,
    }
}

/// A safe storage failure that never retains a path or source location.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum ConfigurationStoreError {
    #[error("source configuration storage is unavailable")]
    Unavailable,
    #[error("source configuration storage has insufficient capacity")]
    Capacity,
    #[error("source configuration storage is corrupt")]
    Corrupt,
    #[error("source configuration storage is not private")]
    UnsafeLayout,
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    use tempfile::TempDir;

    use super::*;

    const PRIVATE_M3U: &str = "https://user:secret@provider.invalid/channels.m3u";
    const PRIVATE_EPG: &str = "https://user:secret@provider.invalid/guide.xml";

    #[test]
    fn round_trips_a_versioned_private_record_atomically() {
        let directory = TempDir::new().expect("temporary directory");
        let root = directory.path().join("private");
        let store = SourceConfigurationStore::open(&root).expect("store opens");
        assert!(store.load().expect("empty store loads").is_none());

        let first = StoredSourceConfiguration::normalized(PRIVATE_M3U.to_owned(), None);
        store.save(&first).expect("first record saves");
        let second = StoredSourceConfiguration::normalized(
            "https://other.invalid/list.m3u".to_owned(),
            Some(PRIVATE_EPG.to_owned()),
        );
        store.save(&second).expect("replacement record saves");

        let loaded = store.load().expect("record loads").expect("record exists");
        let serialized = serde_json::to_value(ConfigurationRecord {
            version: CONFIGURATION_VERSION,
            m3u_location: loaded.m3u_location,
            epg_location: loaded.epg_location,
        })
        .expect("fixture serializes");
        assert_eq!(serialized["version"], 1);
        assert_eq!(serialized["m3uLocation"], "https://other.invalid/list.m3u");
        assert_eq!(serialized["epgLocation"], PRIVATE_EPG);
        assert!(!root.join(PENDING_FILE).exists());
        assert_eq!(
            fs::metadata(root.join(CONFIGURATION_FILE))
                .expect("configuration metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&root)
                .expect("root metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn rejects_oversized_unknown_versioned_and_non_private_records() {
        for bytes in [
            vec![b'x'; usize::try_from(MAX_CONFIGURATION_BYTES + 1).expect("bounded fixture")],
            br#"{"version":2,"m3uLocation":"https://example.invalid/a","epgLocation":null}"#
                .to_vec(),
            br#"{"version":1,"m3uLocation":"https://example.invalid/a","epgLocation":null,"extra":true}"#
                .to_vec(),
        ] {
            let directory = TempDir::new().expect("temporary directory");
            let root = directory.path().join("private");
            let store = SourceConfigurationStore::open(&root).expect("store opens");
            let path = root.join(CONFIGURATION_FILE);
            fs::write(&path, bytes).expect("fixture writes");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .expect("fixture permissions");
            assert_eq!(
                store.load().err().expect("record is rejected"),
                ConfigurationStoreError::Corrupt
            );
        }

        let directory = TempDir::new().expect("temporary directory");
        let root = directory.path().join("private");
        let store = SourceConfigurationStore::open(&root).expect("store opens");
        let path = root.join(CONFIGURATION_FILE);
        fs::write(
            &path,
            br#"{"version":1,"m3uLocation":"https://example.invalid/a","epgLocation":null}"#,
        )
        .expect("fixture writes");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("fixture permissions");
        assert_eq!(
            store.load().err().expect("public record is rejected"),
            ConfigurationStoreError::UnsafeLayout
        );
    }

    #[test]
    fn round_trips_valid_boundary_locations_with_worst_case_json_escaping() {
        fn escaped_location(prefix: &str) -> String {
            format!("{prefix}{}", "\"".repeat(MAX_LOCATION_BYTES - prefix.len()))
        }

        let m3u = escaped_location("https://m3u.invalid/");
        let epg = escaped_location("https://epg.invalid/");
        let configuration = StoredSourceConfiguration::normalized(m3u.clone(), Some(epg.clone()));
        assert!(
            sparrow_core::SparrowCore::parse_source_configuration(configuration.source_input())
                .is_ok()
        );

        let directory = TempDir::new().expect("temporary directory");
        let store = SourceConfigurationStore::open(directory.path()).expect("store opens");
        store.save(&configuration).expect("boundary record saves");
        let loaded = store.load().expect("record loads").expect("record exists");
        assert_eq!(loaded.m3u_location, m3u);
        assert_eq!(loaded.epg_location.as_deref(), Some(epg.as_str()));
    }

    #[test]
    fn refuses_symlinked_active_and_pending_records() {
        let directory = TempDir::new().expect("temporary directory");
        let root = directory.path().join("private");
        let store = SourceConfigurationStore::open(&root).expect("store opens");
        let outside = directory.path().join("outside");
        fs::write(&outside, b"do not follow").expect("outside fixture writes");

        symlink(&outside, root.join(CONFIGURATION_FILE)).expect("active symlink creates");
        assert_eq!(
            store.load().err().expect("active symlink is rejected"),
            ConfigurationStoreError::UnsafeLayout
        );
        fs::remove_file(root.join(CONFIGURATION_FILE)).expect("active symlink removes");

        symlink(&outside, root.join(PENDING_FILE)).expect("pending symlink creates");
        let configuration = StoredSourceConfiguration::normalized(PRIVATE_M3U.to_owned(), None);
        assert_eq!(
            store
                .save(&configuration)
                .expect_err("pending symlink is rejected"),
            ConfigurationStoreError::UnsafeLayout
        );
        assert_eq!(
            fs::read(outside).expect("outside fixture reads"),
            b"do not follow"
        );
    }

    #[test]
    fn errors_and_store_diagnostics_never_expose_paths_or_locations() {
        let private_canary = "private-path-canary";
        let source_canary = PRIVATE_M3U;
        for error in [
            ConfigurationStoreError::Unavailable,
            ConfigurationStoreError::Capacity,
            ConfigurationStoreError::Corrupt,
            ConfigurationStoreError::UnsafeLayout,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains(private_canary));
            assert!(!rendered.contains(source_canary));
        }

        let directory = TempDir::new().expect("temporary directory");
        let root = directory.path().join(private_canary);
        fs::write(&root, source_canary).expect("unsafe root fixture writes");
        let error = SourceConfigurationStore::open(&root)
            .err()
            .expect("unsafe root is rejected");
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains(private_canary));
        assert!(!rendered.contains(source_canary));
    }
}
