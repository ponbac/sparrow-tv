use std::{env, path::PathBuf};

use sparrow_core::{SourceConfiguration, SourceConfigurationInput, SparrowCore};

use crate::StartupError;

pub(crate) struct HostedConfig {
    pub(crate) password: SecretPassword,
    pub(crate) source: SourceConfiguration,
    pub(crate) app_root: PathBuf,
}

impl HostedConfig {
    pub(crate) fn load() -> Result<Self, StartupError> {
        load_local_environment()?;
        let password = required_environment("PASSWORD")?;
        let m3u = required_environment("M3U_PATH")?;
        let epg = optional_environment("EPG_PATH")?;
        let source =
            SparrowCore::parse_source_configuration(SourceConfigurationInput::new(m3u, epg))
                .map_err(|_| StartupError::Configuration)?;

        Ok(Self {
            password: SecretPassword(password.into_bytes()),
            source,
            app_root: PathBuf::from("app/dist"),
        })
    }
}

pub(crate) struct SecretPassword(Vec<u8>);

impl SecretPassword {
    pub(crate) fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for SecretPassword {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretPassword(<redacted>)")
    }
}

fn load_local_environment() -> Result<(), StartupError> {
    let path = std::path::Path::new(".env.local");
    if path.exists() {
        dotenvy::from_path(path).map_err(|_| StartupError::Configuration)?;
    }
    Ok(())
}

fn required_environment(name: &str) -> Result<String, StartupError> {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or(StartupError::Configuration)
}

fn optional_environment(name: &str) -> Result<Option<String>, StartupError> {
    match env::var(name) {
        Ok(value) if value.is_empty() => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(StartupError::Configuration),
    }
}
