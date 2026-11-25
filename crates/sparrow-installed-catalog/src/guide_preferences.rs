//! Durable preferences shared by the installed Guide's Activities.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs::File,
    io::{self, Write},
    path::Path,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use sparrow_core::ChannelQuality;
use tokio::sync::{Mutex, watch};

use crate::config_store::{
    ConfigurationStoreError, activate_private_file, ensure_private_directory,
    open_private_directory, read_private_file, sync_private_directory,
};

const PREFERENCES_FILE: &str = "guide-preferences.json";
const PENDING_FILE: &str = ".guide-preferences.pending";
const VERSION: u8 = 1;
const MAX_GROUP_BYTES: usize = 1024;
const MAX_FAMILY_UTF16: usize = 4096;
const MAX_RECORD_ITEMS: usize = 10_000;
const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;

/// A Guide preference snapshot. Names and family keys stay out of diagnostics.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct GuidePreferences {
    excluded_groups: HashSet<String>,
    variant_preferences: HashMap<String, ChannelQuality>,
}

impl GuidePreferences {
    pub fn excluded_groups(&self) -> &HashSet<String> {
        &self.excluded_groups
    }

    /// Keys are the v1 WebView strings produced by JSON.stringify([group, foldedTitle]).
    pub fn variant_preferences(&self) -> &HashMap<String, ChannelQuality> {
        &self.variant_preferences
    }

    /// Projects canonical v1 keys as exact group names and already folded titles.
    pub fn variant_choices(&self) -> impl Iterator<Item = (String, String, ChannelQuality)> + '_ {
        self.variant_preferences
            .iter()
            .filter_map(|(family, quality)| {
                let [group, title] = serde_json::from_str::<[String; 2]>(family).ok()?;
                let canonical = serde_json::to_string(&[&group, &title]).ok()?;
                (canonical == *family).then_some((group, title, *quality))
            })
    }

    fn apply(&mut self, change: GuidePreferenceChange) -> Result<(), GuidePreferenceError> {
        match change {
            GuidePreferenceChange::SetGroupVisibility { name, hidden } => {
                if !valid_group(&name) {
                    return Err(GuidePreferenceError::Invalid);
                }
                if hidden {
                    self.excluded_groups.insert(name);
                    if self.excluded_groups.len() > MAX_RECORD_ITEMS {
                        return Err(GuidePreferenceError::Invalid);
                    }
                } else {
                    self.excluded_groups.remove(&name);
                }
            }
            GuidePreferenceChange::ShowAllGroups => self.excluded_groups.clear(),
            GuidePreferenceChange::PreferVariant {
                group,
                normalized_title,
                quality,
            } => {
                if !valid_group(&group)
                    || normalized_title
                        .encode_utf16()
                        .take(MAX_FAMILY_UTF16 + 1)
                        .count()
                        > MAX_FAMILY_UTF16
                {
                    return Err(GuidePreferenceError::Invalid);
                }
                let family = serde_json::to_string(&[group, normalized_title])
                    .map_err(|_| GuidePreferenceError::Unavailable)?;
                if !valid_family(&family)
                    || (self.variant_preferences.len() >= MAX_RECORD_ITEMS
                        && !self.variant_preferences.contains_key(&family))
                {
                    return Err(GuidePreferenceError::Invalid);
                }
                self.variant_preferences.insert(family, quality);
            }
        }
        Ok(())
    }
}

/// Changes are applied in order to the latest active snapshot in one write.
pub enum GuidePreferenceChange {
    SetGroupVisibility {
        name: String,
        hidden: bool,
    },
    ShowAllGroups,
    PreferVariant {
        group: String,
        /// The title already folded according to the Guide's v1 key rules.
        normalized_title: String,
        quality: ChannelQuality,
    },
}

/// A safe failure that contains neither a file path nor preference data.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GuidePreferenceError {
    #[error("Guide preferences could not be accessed")]
    Unavailable,
    #[error("Guide preference storage has insufficient capacity")]
    Capacity,
    #[error("Guide preference storage is not private")]
    UnsafeLayout,
    #[error("Guide preferences exceed the supported limits")]
    Invalid,
}

impl From<ConfigurationStoreError> for GuidePreferenceError {
    fn from(error: ConfigurationStoreError) -> Self {
        match error {
            ConfigurationStoreError::Unavailable => Self::Unavailable,
            ConfigurationStoreError::Capacity => Self::Capacity,
            ConfigurationStoreError::UnsafeLayout => Self::UnsafeLayout,
            ConfigurationStoreError::Corrupt => Self::Invalid,
        }
    }
}

pub(crate) struct GuidePreferenceStore {
    active: watch::Sender<Arc<GuidePreferences>>,
    loaded: OnceLock<()>,
    directory_sync_pending: AtomicBool,
    pub(crate) mutation: Arc<Mutex<()>>,
}

impl GuidePreferenceStore {
    pub(crate) fn new() -> Self {
        Self {
            active: watch::channel(Arc::new(GuidePreferences::default())).0,
            loaded: OnceLock::new(),
            directory_sync_pending: AtomicBool::new(false),
            mutation: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) fn snapshot(&self) -> Arc<GuidePreferences> {
        self.active.borrow().clone()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Arc<GuidePreferences>> {
        self.active.subscribe()
    }

    pub(crate) fn loaded_snapshot(&self) -> Option<Arc<GuidePreferences>> {
        self.loaded.get().map(|()| self.snapshot())
    }

    /// Initial reads and mutations hold the same owned mutation guard.
    pub(crate) fn load(&self, root: &Path) -> Result<Arc<GuidePreferences>, GuidePreferenceError> {
        if let Some(loaded) = self.loaded_snapshot() {
            return Ok(loaded);
        }
        ensure_private_directory(root)?;
        let record = match read_private_file(&root.join(PREFERENCES_FILE), MAX_RECORD_BYTES as u64)
        {
            Ok(record) => record,
            Err(ConfigurationStoreError::Corrupt) => None,
            Err(error) => return Err(error.into()),
        };
        let recovered = record
            .as_deref()
            .and_then(|bytes| serde_json::from_slice::<PreferenceRecord<'_>>(bytes).ok())
            .and_then(PreferenceRecord::into_preferences);
        if let Some(recovered) =
            recovered.filter(|preferences| *preferences != GuidePreferences::default())
        {
            self.active.send_replace(Arc::new(recovered));
        }
        let _ = self.loaded.set(());
        Ok(self.snapshot())
    }

    pub(crate) fn update(
        &self,
        root: &Path,
        changes: Vec<GuidePreferenceChange>,
    ) -> Result<Arc<GuidePreferences>, GuidePreferenceError> {
        self.update_with_directory_sync(root, changes, sync_private_directory)
    }

    fn update_with_directory_sync(
        &self,
        root: &Path,
        changes: Vec<GuidePreferenceChange>,
        sync_directory: impl FnOnce(&File) -> Result<(), ConfigurationStoreError>,
    ) -> Result<Arc<GuidePreferences>, GuidePreferenceError> {
        let previous = self.load(root)?;
        let mut next = (*previous).clone();
        for change in changes {
            next.apply(change)?;
        }
        let (next, synced) = if next == *previous {
            if !self.directory_sync_pending.load(Ordering::Relaxed) {
                return Ok(previous);
            }
            ensure_private_directory(root)?;
            let directory = open_private_directory(root)?;
            (previous, sync_directory(&directory))
        } else {
            let bytes = encode_record(&next, MAX_RECORD_BYTES)?;
            let directory = activate_private_file(root, PREFERENCES_FILE, PENDING_FILE, &bytes)?;
            let synced = sync_directory(&directory);
            let next = Arc::new(next);
            // The new record is already active even if its durability is uncertain.
            self.active.send_replace(Arc::clone(&next));
            (next, synced)
        };
        self.directory_sync_pending
            .store(synced.is_err(), Ordering::Relaxed);
        synced?;
        Ok(next)
    }
}

fn encode_record(
    preferences: &GuidePreferences,
    maximum_bytes: usize,
) -> Result<Vec<u8>, GuidePreferenceError> {
    let mut output = RecordBuffer {
        bytes: Vec::new(),
        maximum_bytes,
    };
    serde_json::to_writer(&mut output, &PreferenceRecord::from(preferences)).map_err(|error| {
        if error.io_error_kind() == Some(io::ErrorKind::InvalidData) {
            GuidePreferenceError::Invalid
        } else {
            GuidePreferenceError::Unavailable
        }
    })?;
    Ok(output.bytes)
}

fn valid_group(name: &str) -> bool {
    name.len() <= MAX_GROUP_BYTES && !name.chars().any(char::is_control)
}

fn valid_family(family: &str) -> bool {
    family.encode_utf16().count() <= MAX_FAMILY_UTF16
}

// The v1 WebView keys and each value's field names are kept for a future importer.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreferenceRecord<'a> {
    version: u8,
    #[serde(rename = "sparrow.board-group-exclusions:v1")]
    groups: GroupRecord<'a>,
    #[serde(rename = "sparrow.variant-preferences:v1")]
    variants: VariantRecord<'a>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GroupRecord<'a> {
    excluded: Vec<Cow<'a, str>>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VariantRecord<'a> {
    preferences: Vec<VariantRecordEntry<'a>>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VariantRecordEntry<'a> {
    family: Cow<'a, str>,
    #[serde(with = "StoredQuality")]
    quality: ChannelQuality,
}

#[derive(Deserialize, Serialize)]
#[serde(remote = "ChannelQuality", rename_all = "lowercase")]
enum StoredQuality {
    Sd,
    Hd,
    Fhd,
    Uhd,
}

impl PreferenceRecord<'_> {
    fn into_preferences(self) -> Option<GuidePreferences> {
        if self.version != VERSION
            || self.groups.excluded.len() > MAX_RECORD_ITEMS
            || !self.groups.excluded.iter().all(|name| valid_group(name))
            || self.variants.preferences.len() > MAX_RECORD_ITEMS
            || !self
                .variants
                .preferences
                .iter()
                .all(|entry| valid_family(&entry.family))
        {
            return None;
        }
        Some(GuidePreferences {
            excluded_groups: self
                .groups
                .excluded
                .into_iter()
                .map(Cow::into_owned)
                .collect(),
            variant_preferences: self
                .variants
                .preferences
                .into_iter()
                .map(|entry| (entry.family.into_owned(), entry.quality))
                .collect(),
        })
    }
}

impl<'a> From<&'a GuidePreferences> for PreferenceRecord<'a> {
    fn from(preferences: &'a GuidePreferences) -> Self {
        let mut excluded: Vec<_> = preferences
            .excluded_groups
            .iter()
            .map(|name| Cow::Borrowed(name.as_str()))
            .collect();
        excluded.sort_unstable();
        let mut variants: Vec<_> = preferences
            .variant_preferences
            .iter()
            .map(|(family, quality)| VariantRecordEntry {
                family: Cow::Borrowed(family.as_str()),
                quality: *quality,
            })
            .collect();
        variants.sort_unstable_by(|left, right| left.family.cmp(&right.family));
        Self {
            version: VERSION,
            groups: GroupRecord { excluded },
            variants: VariantRecord {
                preferences: variants,
            },
        }
    }
}

// Bound encoded bytes as well as individual entries, including JSON escaping.
struct RecordBuffer {
    bytes: Vec<u8>,
    maximum_bytes: usize,
}

impl Write for RecordBuffer {
    fn write(&mut self, chunk: &[u8]) -> io::Result<usize> {
        if chunk.len() > self.maximum_bytes - self.bytes.len() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.bytes.extend_from_slice(chunk);
        Ok(chunk.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_post_rename_sync_failure_publishes_the_active_record_and_can_retry() {
        let root = tempfile::tempdir().unwrap();
        let store = GuidePreferenceStore::new();
        let mut changes = store.subscribe();
        let hide = || GuidePreferenceChange::SetGroupVisibility {
            name: "News".to_owned(),
            hidden: true,
        };

        let failed = store.update_with_directory_sync(root.path(), vec![hide()], |_| {
            Err(ConfigurationStoreError::Capacity)
        });
        assert_eq!(failed.err(), Some(GuidePreferenceError::Capacity));
        assert!(changes.has_changed().unwrap());
        let active = changes.borrow_and_update().clone();
        assert!(active.excluded_groups().contains("News"));
        let reopened = GuidePreferenceStore::new().load(root.path()).unwrap();
        assert!(active.as_ref() == reopened.as_ref());

        let mut retried = false;
        let saved = store
            .update_with_directory_sync(root.path(), vec![hide()], |directory| {
                retried = true;
                sync_private_directory(directory)
            })
            .unwrap();
        assert!(retried);
        assert!(Arc::ptr_eq(&active, &saved));
        assert!(!changes.has_changed().unwrap());
    }

    #[test]
    fn an_encoding_limit_reports_invalid_input() {
        let preferences = GuidePreferences::default();
        let bytes = encode_record(&preferences, MAX_RECORD_BYTES).unwrap();
        assert_eq!(
            encode_record(&preferences, bytes.len() - 1),
            Err(GuidePreferenceError::Invalid)
        );
        assert_eq!(encode_record(&preferences, bytes.len()).unwrap(), bytes);
    }
}
