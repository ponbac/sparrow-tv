//! Shared composition for Sparrow's on-device catalog and private storage.

mod config_store;

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use sparrow_core::{
    CatalogAccess, CatalogStatus, CoreAdapters, CoreError, SourceConfiguration, SparrowCore,
    SystemClock,
};
use sparrow_snapshot_store::{AtomicFileSnapshotStore, InstanceLock, InstanceLockError};
use sparrow_source_http::HttpSourceAccess;
use tokio::{runtime::Handle, sync::Mutex};

pub use config_store::{ConfigurationStoreError, ensure_private_directory};

use config_store::{SourceConfigurationStore, StoredSourceConfiguration};

const PRIVATE_DIRECTORY: &str = "private-v1";
const SNAPSHOT_DIRECTORY: &str = "snapshots-v1";

pub struct InstalledCatalog {
    core: SparrowCore,
    runtime: Handle,
    private_root: PathBuf,
    configuration_store: SourceConfigurationStore,
    configuration_mutation: Arc<Mutex<()>>,
}

/// Holds the instance lock across synchronous preparation and async catalog bootstrap.
pub struct PreparedInstalledCatalog {
    private_root: PathBuf,
    configuration_store: SourceConfigurationStore,
    configuration: Option<SourceConfiguration>,
    instance_lock: InstanceLock,
}

impl InstalledCatalog {
    pub fn prepare(app_data: &Path) -> Result<PreparedInstalledCatalog, CatalogStartupError> {
        prepare_app_data(app_data)?;
        let private_root = app_data.join(PRIVATE_DIRECTORY);
        ensure_private_directory(&private_root).map_err(|_| CatalogStartupError::Configuration)?;
        let instance_lock = InstanceLock::acquire(&private_root).map_err(|error| match error {
            InstanceLockError::AlreadyRunning => CatalogStartupError::AlreadyRunning,
            InstanceLockError::Unavailable => CatalogStartupError::AppData,
        })?;
        let configuration_store = SourceConfigurationStore::open(&private_root)
            .map_err(|_| CatalogStartupError::Configuration)?;
        let configuration = match configuration_store.load() {
            Ok(stored) => stored.and_then(|stored| {
                SparrowCore::parse_source_configuration(stored.source_input()).ok()
            }),
            Err(ConfigurationStoreError::Corrupt) => None,
            Err(_) => return Err(CatalogStartupError::Configuration),
        };
        Ok(PreparedInstalledCatalog {
            private_root,
            configuration_store,
            configuration,
            instance_lock,
        })
    }

    pub fn core(&self) -> &CatalogAccess {
        self.core.catalog()
    }

    /// Reads the current view and activates background source automation on the
    /// catalog runtime, including when the host calls from its UI thread.
    pub fn status(&self) -> CatalogStatus {
        let status = self.core().status();
        let _runtime = self.runtime.enter();
        self.core().activate_automation();
        status
    }

    pub fn private_root(&self) -> &Path {
        &self.private_root
    }

    /// Runs on Tokio. Once admitted, the save and core replacement finish even if the caller stops waiting.
    pub async fn replace_configuration(
        self: &Arc<Self>,
        update: SourceConfigurationUpdate,
    ) -> Result<CatalogStatus, ConfigurationStoreError> {
        let mutation = Arc::clone(&self.configuration_mutation).lock_owned().await;
        let catalog = Arc::clone(self);
        tokio::spawn(async move {
            let _mutation = mutation;
            // The blocking save must hold the instance lock even if its Tokio task is dropped.
            let persistence = Arc::clone(&catalog);
            tokio::task::spawn_blocking(move || {
                persistence.configuration_store.save(&update.stored)
            })
            .await
            .map_err(|_| ConfigurationStoreError::Unavailable)??;
            Ok(catalog
                .core
                .replace_source_configuration(Some(update.configuration))
                .await)
        })
        .await
        .map_err(|_| ConfigurationStoreError::Unavailable)?
    }
}

impl PreparedInstalledCatalog {
    pub async fn finish_open(self) -> Result<Arc<InstalledCatalog>, CatalogStartupError> {
        let runtime = Handle::current();
        let source =
            Arc::new(HttpSourceAccess::new().map_err(|_| CatalogStartupError::SourceAdapter)?);
        let snapshot_dir = self.private_root.join(SNAPSHOT_DIRECTORY);
        let instance_lock = self.instance_lock;
        let snapshots = tokio::task::spawn_blocking(move || {
            AtomicFileSnapshotStore::open_for_instance(snapshot_dir, instance_lock)
        })
        .await
        .map_err(|_| CatalogStartupError::SnapshotAdapter)?
        .map_err(|_| CatalogStartupError::SnapshotAdapter)?;
        let core = SparrowCore::bootstrap_installed(
            self.configuration,
            CoreAdapters::new(source, Arc::new(snapshots), Arc::new(SystemClock)),
        )
        .await
        .map_err(|_| CatalogStartupError::Core)?;
        Ok(Arc::new(InstalledCatalog {
            core,
            runtime,
            private_root: self.private_root,
            configuration_store: self.configuration_store,
            configuration_mutation: Arc::new(Mutex::new(())),
        }))
    }
}

/// A validated change bound to its private on-disk representation.
/// Neither location can be read or formatted through this type.
pub struct SourceConfigurationUpdate {
    stored: StoredSourceConfiguration,
    configuration: SourceConfiguration,
}

impl SourceConfigurationUpdate {
    pub fn parse(m3u_location: String, epg_location: Option<String>) -> Result<Self, CoreError> {
        let stored = StoredSourceConfiguration::normalized(m3u_location, epg_location);
        let configuration = SparrowCore::parse_source_configuration(stored.source_input())?;
        Ok(Self {
            stored,
            configuration,
        })
    }
}

fn prepare_app_data(path: &Path) -> Result<(), CatalogStartupError> {
    fs::create_dir_all(path).map_err(|_| CatalogStartupError::AppData)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| CatalogStartupError::AppData)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CatalogStartupError::AppData);
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| CatalogStartupError::AppData)?;
    Ok(())
}

/// Safe startup failures deliberately discard filesystem and provider context.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CatalogStartupError {
    #[error("the private app-data directory is unavailable")]
    AppData,
    #[error("another Sparrow instance is already running")]
    AlreadyRunning,
    #[error("the source configuration is unavailable")]
    Configuration,
    #[error("the source access adapter could not be initialized")]
    SourceAdapter,
    #[error("the snapshot adapter could not be initialized")]
    SnapshotAdapter,
    #[error("the catalog core could not be initialized")]
    Core,
}
