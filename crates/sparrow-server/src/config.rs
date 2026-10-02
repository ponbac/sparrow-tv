use std::{env, path::PathBuf};

use sparrow_core::{SourceConfiguration, SourceConfigurationInput, SparrowCore};

use crate::{
    StartupError,
    auth::{DeploymentAuth, DeploymentCredential},
};

pub(crate) struct HostedConfig {
    pub(crate) authentication: DeploymentAuth,
    pub(crate) source: SourceConfiguration,
    pub(crate) app_root: PathBuf,
}

impl HostedConfig {
    pub(crate) fn load() -> Result<Self, StartupError> {
        load_local_environment()?;
        let authentication = deployment_auth(read_environment)?;
        let m3u = required_environment("M3U_PATH")?;
        let epg = optional_environment("EPG_PATH")?;
        let source =
            SparrowCore::parse_source_configuration(SourceConfigurationInput::new(m3u, epg))
                .map_err(|_| StartupError::Configuration)?;

        Ok(Self {
            authentication,
            source,
            app_root: PathBuf::from("app/dist"),
        })
    }
}

fn deployment_auth(
    environment: impl Fn(&str) -> Result<Option<String>, StartupError>,
) -> Result<DeploymentAuth, StartupError> {
    match environment("SPARROW_AUTH_MODE")?.as_deref() {
        Some("public") => Ok(DeploymentAuth::Public),
        None | Some("basic") => {
            let password = environment("PASSWORD")?.ok_or(StartupError::Configuration)?;
            let credential = DeploymentCredential::new(password.as_bytes())
                .map_err(|_| StartupError::Configuration)?;
            Ok(DeploymentAuth::Basic(credential))
        }
        Some(_) => Err(StartupError::Configuration),
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
    Ok(read_environment(name)?.filter(|value| !value.is_empty()))
}

fn read_environment(name: &str) -> Result<Option<String>, StartupError> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(StartupError::Configuration),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authentication(
        mode: Option<&str>,
        password: Option<&str>,
    ) -> Result<DeploymentAuth, StartupError> {
        deployment_auth(|name| {
            Ok(match name {
                "SPARROW_AUTH_MODE" => mode.map(str::to_owned),
                "PASSWORD" => password.map(str::to_owned),
                _ => panic!("unexpected environment lookup"),
            })
        })
    }

    #[test]
    fn absent_or_basic_mode_requires_a_valid_password() {
        for mode in [None, Some("basic")] {
            for password in [None, Some(""), Some(&"x".repeat(1025))] {
                assert!(matches!(
                    authentication(mode, password),
                    Err(StartupError::Configuration)
                ));
            }
            assert!(matches!(
                authentication(mode, Some("synthetic-password")),
                Ok(DeploymentAuth::Basic(_))
            ));
        }
    }

    #[test]
    fn public_mode_never_reads_even_an_invalid_rollback_password() {
        let auth = deployment_auth(|name| match name {
            "SPARROW_AUTH_MODE" => Ok(Some("public".into())),
            "PASSWORD" => panic!("public mode must not read retained passwords"),
            _ => panic!("unexpected environment lookup"),
        })
        .expect("public needs no password");
        assert!(matches!(auth, DeploymentAuth::Public));
        for password in [
            None,
            Some(""),
            Some("rollback-password-canary"),
            Some(&"x".repeat(1025)),
        ] {
            assert!(matches!(
                authentication(Some("public"), password),
                Ok(DeploymentAuth::Public)
            ));
        }
    }

    #[test]
    fn invalid_auth_modes_fail_closed_with_safe_errors() {
        for mode in [
            "",
            "PUBLIC",
            "Basic",
            " public",
            "public ",
            "none",
            "mode-secret-canary",
        ] {
            let error = authentication(Some(mode), Some("password-secret-canary"))
                .err()
                .expect("invalid modes must not start serving");
            assert_eq!(error, StartupError::Configuration);
            let diagnostic = format!("{error:?} {error}");
            assert!(!diagnostic.contains("mode-secret-canary"));
            assert!(!diagnostic.contains("password-secret-canary"));
        }
        assert!(matches!(
            deployment_auth(|_| Err(StartupError::Configuration)),
            Err(StartupError::Configuration)
        ));
    }
}
