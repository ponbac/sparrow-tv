use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use sparrow_core::{CoreAdapters, SparrowCore, SystemClock};
use sparrow_snapshot_store::AtomicFileSnapshotStore;
use sparrow_source_http::HttpSourceAccess;
use tokio::sync::Mutex;

use crate::{
    config_store::{
        ConfigurationStoreError, SourceConfigurationStore, StoredSourceConfiguration,
        ensure_private_directory,
    },
    instance_lock::{InstanceLock, InstanceLockError},
    ipc::{
        dto::{CatalogStatusDto, ClientErrorDto, CoreEventDto},
        input::SourceConfigurationInputDto,
        subscriptions::SubscriptionRegistry,
    },
};

const PRIVATE_DIRECTORY: &str = "private-v1";
const SNAPSHOT_DIRECTORY: &str = "snapshots-v1";

/// The complete on-device catalog composition managed by Tauri.
pub(crate) struct InstalledRuntime {
    core: Arc<SparrowCore>,
    configuration_store: SourceConfigurationStore,
    configuration_mutation: Mutex<()>,
    subscriptions: SubscriptionRegistry,
    _instance_lock: InstanceLock,
}

impl InstalledRuntime {
    pub(crate) async fn open(app_data: PathBuf) -> Result<Self, InstalledStartupError> {
        prepare_app_data(&app_data)?;
        let private_root = app_data.join(PRIVATE_DIRECTORY);
        ensure_private_directory(&private_root).map_err(InstalledStartupError::from)?;
        let instance_lock =
            InstanceLock::acquire(&private_root).map_err(InstalledStartupError::from)?;
        let configuration_store =
            SourceConfigurationStore::open(&private_root).map_err(InstalledStartupError::from)?;
        let configuration = load_persisted_configuration(&configuration_store)?;

        let source =
            Arc::new(HttpSourceAccess::new().map_err(|_| InstalledStartupError::SourceAdapter)?);
        let snapshots = Arc::new(
            AtomicFileSnapshotStore::open(private_root.join(SNAPSHOT_DIRECTORY))
                .map_err(|_| InstalledStartupError::SnapshotAdapter)?,
        );
        let core = Arc::new(
            SparrowCore::bootstrap_from_snapshots(
                configuration,
                CoreAdapters::new(source, snapshots, Arc::new(SystemClock)),
            )
            .await
            .map_err(|_| InstalledStartupError::Core)?,
        );

        Ok(Self {
            core,
            configuration_store,
            configuration_mutation: Mutex::new(()),
            subscriptions: SubscriptionRegistry::default(),
            _instance_lock: instance_lock,
        })
    }

    pub(crate) fn core(&self) -> &SparrowCore {
        &self.core
    }

    pub(crate) async fn replace_configuration(
        &self,
        input: SourceConfigurationInputDto,
    ) -> Result<CatalogStatusDto, ClientErrorDto> {
        let (stored, configuration) = input.validate()?;
        let _mutation = self.configuration_mutation.lock().await;
        self.persist_configuration(stored).await?;
        let status = self
            .core
            .replace_source_configuration(Some(configuration))
            .await;
        Ok(CatalogStatusDto::from(status))
    }

    async fn persist_configuration(
        &self,
        configuration: StoredSourceConfiguration,
    ) -> Result<(), ClientErrorDto> {
        let store = self.configuration_store.clone();
        tokio::task::spawn_blocking(move || store.save(&configuration))
            .await
            .map_err(|_| ClientErrorDto::service_unavailable())?
            .map_err(|_| ClientErrorDto::service_unavailable())
    }

    pub(crate) fn subscribe(
        &self,
        events: tauri::ipc::Channel<CoreEventDto>,
    ) -> Result<String, ClientErrorDto> {
        self.subscriptions.subscribe(Arc::clone(&self.core), events)
    }

    pub(crate) fn unsubscribe(&self, subscription_id: &str) {
        self.subscriptions.unsubscribe(subscription_id);
    }
}

fn load_persisted_configuration(
    store: &SourceConfigurationStore,
) -> Result<Option<sparrow_core::SourceConfiguration>, InstalledStartupError> {
    let stored = match store.load() {
        Ok(stored) => stored,
        Err(ConfigurationStoreError::Corrupt) => return Ok(None),
        Err(error) => return Err(InstalledStartupError::from(error)),
    };
    Ok(stored
        .and_then(|stored| SparrowCore::parse_source_configuration(stored.source_input()).ok()))
}

fn prepare_app_data(path: &Path) -> Result<(), InstalledStartupError> {
    fs::create_dir_all(path).map_err(|_| InstalledStartupError::AppData)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| InstalledStartupError::AppData)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(InstalledStartupError::AppData);
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| InstalledStartupError::AppData)?;
    Ok(())
}

/// Safe startup failures deliberately discard filesystem and provider context.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum InstalledStartupError {
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

impl From<ConfigurationStoreError> for InstalledStartupError {
    fn from(_error: ConfigurationStoreError) -> Self {
        Self::Configuration
    }
}

impl From<InstanceLockError> for InstalledStartupError {
    fn from(error: InstanceLockError) -> Self {
        match error {
            InstanceLockError::AlreadyRunning => Self::AlreadyRunning,
            InstanceLockError::Unavailable => Self::AppData,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read as _, Write as _},
        net::TcpListener,
        os::unix::fs::PermissionsExt,
        thread,
    };

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn opens_an_unconfigured_local_composition_and_holds_the_instance_lock() {
        let directory = TempDir::new().expect("temporary directory");
        let app_data = directory.path().join("app-data");
        let runtime = InstalledRuntime::open(app_data.clone())
            .await
            .expect("runtime opens");
        assert!(!runtime.core().status().configuration().is_configured());
        assert_eq!(
            fs::metadata(&app_data)
                .expect("app-data metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            InstalledRuntime::open(app_data)
                .await
                .err()
                .expect("second instance is excluded"),
            InstalledStartupError::AlreadyRunning
        );
        drop(runtime);
    }

    #[tokio::test]
    async fn replaces_persists_browses_and_reopens_from_an_offline_snapshot() {
        let directory = TempDir::new().expect("temporary directory");
        let app_data = directory.path().join("app-data");
        let runtime = InstalledRuntime::open(app_data.clone())
            .await
            .expect("unconfigured runtime opens");
        let (source_location, source_server) = one_shot_m3u_server();
        let input = serde_json::from_value(json!({
            "m3uLocation": source_location,
            "epgLocation": null
        }))
        .expect("source input parses");

        let status = runtime
            .replace_configuration(input)
            .await
            .expect("configuration replacement completes");
        source_server.join().expect("source server exits");
        let status_json = serde_json::to_value(status).expect("status serializes");
        assert_eq!(status_json["configuration"]["configured"], true);
        assert_eq!(status_json["configuration"]["epgConfigured"], false);
        assert!(status_json["generation"].is_number());
        assert_safe_routine_json(&status_json);

        let groups = crate::ipc::list_groups(
            &runtime,
            serde_json::from_value(json!({ "limit": 20 })).expect("group input parses"),
        )
        .expect("groups browse locally");
        let channels = crate::ipc::list_channels(
            &runtime,
            serde_json::from_value(json!({ "limit": 20, "group": "News" }))
                .expect("channel input parses"),
        )
        .expect("channels browse locally");
        let groups_json = serde_json::to_value(groups).expect("groups serialize");
        let channels_json = serde_json::to_value(channels).expect("channels serialize");
        assert_eq!(groups_json["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(channels_json["items"].as_array().map(Vec::len), Some(1));
        assert_safe_routine_json(&groups_json);
        assert_safe_routine_json(&channels_json);

        let channel_id = channels_json["items"][0]["id"]
            .as_str()
            .expect("channel id is present")
            .to_owned();
        let channel = crate::ipc::channel(
            &runtime,
            serde_json::from_value(json!({ "id": channel_id })).expect("channel input parses"),
        )
        .expect("channel resolves locally");
        assert_safe_routine_json(&serde_json::to_value(channel).expect("channel serializes"));

        let configuration_path = app_data
            .join(PRIVATE_DIRECTORY)
            .join("source-configuration.json");
        assert_eq!(
            fs::metadata(configuration_path)
                .expect("configuration is persisted")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(runtime);

        let offline = InstalledRuntime::open(app_data)
            .await
            .expect("offline snapshot reopens without the source server");
        let offline_channels = crate::ipc::list_channels(
            &offline,
            serde_json::from_value(json!({ "limit": 20 })).expect("channel input parses"),
        )
        .expect("offline catalog browses");
        let offline_json =
            serde_json::to_value(offline_channels).expect("offline channels serialize");
        assert_eq!(offline_json["items"].as_array().map(Vec::len), Some(1));
        assert_safe_routine_json(&offline_json);
    }

    #[tokio::test]
    async fn corrupt_or_invalid_persisted_configuration_boots_usable_and_unconfigured() {
        for bytes in [
            b"not-json".as_slice(),
            br#"{"version":1,"m3uLocation":"file:///private/source.m3u","epgLocation":null}"#,
        ] {
            let directory = TempDir::new().expect("temporary directory");
            let app_data = directory.path().join("app-data");
            let private_root = app_data.join(PRIVATE_DIRECTORY);
            prepare_app_data(&app_data).expect("app-data directory opens");
            ensure_private_directory(&private_root).expect("private directory opens");
            let configuration_path = private_root.join("source-configuration.json");
            fs::write(&configuration_path, bytes).expect("configuration fixture writes");
            fs::set_permissions(&configuration_path, fs::Permissions::from_mode(0o600))
                .expect("fixture permissions");

            let runtime = InstalledRuntime::open(app_data)
                .await
                .expect("runtime degrades safely");
            assert!(!runtime.core().status().configuration().is_configured());
        }
    }

    #[tokio::test]
    async fn persisted_configuration_never_blocks_startup_on_provider_io() {
        let directory = TempDir::new().expect("temporary directory");
        let app_data = directory.path().join("app-data");
        let private_root = app_data.join(PRIVATE_DIRECTORY);
        prepare_app_data(&app_data).expect("app-data directory opens");
        let store = SourceConfigurationStore::open(&private_root).expect("store opens");
        let provider = TcpListener::bind("127.0.0.1:0").expect("black-hole provider binds");
        let location = format!(
            "http://{}/channels.m3u",
            provider.local_addr().expect("provider address exists")
        );
        store
            .save(&StoredSourceConfiguration::normalized(location, None))
            .expect("configuration persists");

        let runtime = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            InstalledRuntime::open(app_data),
        )
        .await
        .expect("startup never awaits the provider")
        .expect("runtime opens");
        assert!(runtime.core().status().configuration().is_configured());
    }

    #[test]
    fn shell_manifest_and_capability_expose_no_hosted_or_frontend_io_adapter() {
        let manifest = include_str!("../Cargo.toml");
        let capability = include_str!("../capabilities/installed.json");
        assert!(!manifest.contains("sparrow-server"));
        assert!(!manifest.contains("axum"));
        for forbidden in ["fs:", "http:", "shell:"] {
            assert!(!capability.contains(forbidden));
        }
    }

    #[test]
    fn startup_diagnostics_never_expose_private_context() {
        let private_canary = "https://user:secret@provider.invalid/list.m3u";
        for error in [
            InstalledStartupError::AppData,
            InstalledStartupError::AlreadyRunning,
            InstalledStartupError::Configuration,
            InstalledStartupError::SourceAdapter,
            InstalledStartupError::SnapshotAdapter,
            InstalledStartupError::Core,
        ] {
            assert!(!format!("{error:?} {error}").contains(private_canary));
        }
    }

    fn one_shot_m3u_server() -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener binds");
        let address = listener.local_addr().expect("fixture address exists");
        let body = b"#EXTM3U\n#EXTINF:-1 tvg-id=\"fixture-one\" group-title=\"News\",World News\nhttp://127.0.0.1:9/live\n";
        let task = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("fixture request arrives");
            let mut request = [0_u8; 2048];
            let bytes = stream.read(&mut request).expect("fixture request reads");
            assert!(request[..bytes].starts_with(b"GET /channels.m3u HTTP/1.1\r\n"));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("fixture response header writes");
            stream.write_all(body).expect("fixture body writes");
            stream.flush().expect("fixture response flushes");
        });
        (format!("http://{address}/channels.m3u"), task)
    }

    fn assert_safe_routine_json(value: &serde_json::Value) {
        let serialized = value.to_string();
        for forbidden in ["http://", "https://", "m3uLocation", "epgLocation"] {
            assert!(!serialized.contains(forbidden));
        }
    }
}
