use std::{env, path::PathBuf};

use sparrow_core::{SourceConfiguration, SourceConfigurationInput, SparrowCore};

use crate::{
    StartupError,
    auth::{DEFAULT_USERNAME, DeploymentCredential},
};

pub(crate) struct HostedConfig {
    pub(crate) authentication: DeploymentCredential,
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
) -> Result<DeploymentCredential, StartupError> {
    match environment("SPARROW_AUTH_MODE")?.as_deref() {
        None | Some("basic") => {
            let username = environment("SPARROW_AUTH_USERNAME")?
                .unwrap_or_else(|| DEFAULT_USERNAME.to_owned());
            let password = environment("PASSWORD")?.ok_or(StartupError::Configuration)?;
            DeploymentCredential::with_username(&username, password.as_bytes())
                .map_err(|_| StartupError::Configuration)
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
    ) -> Result<DeploymentCredential, StartupError> {
        deployment_auth(|name| {
            Ok(match name {
                "SPARROW_AUTH_MODE" => mode.map(str::to_owned),
                "PASSWORD" => password.map(str::to_owned),
                "SPARROW_AUTH_USERNAME" => None,
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
            assert!(authentication(mode, Some("synthetic-password")).is_ok());
        }
    }

    #[tokio::test]
    async fn configured_username_replaces_the_legacy_basic_username() {
        use axum::{
            Router,
            body::Body,
            http::{Request, StatusCode, header},
            middleware,
            routing::get,
        };
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        use tower::ServiceExt as _;

        let credential = deployment_auth(|name| {
            Ok(match name {
                "SPARROW_AUTH_MODE" => Some("basic".into()),
                "SPARROW_AUTH_USERNAME" => Some("ponbac".into()),
                "PASSWORD" => Some("synthetic-custom-password".into()),
                _ => panic!("unexpected environment lookup"),
            })
        })
        .expect("custom credentials are valid");
        let router = Router::new().route("/probe", get(|| async { "ok" })).layer(
            middleware::from_fn_with_state(credential, crate::auth::require_authentication),
        );
        for (username, password, expected) in [
            ("ponbac", "synthetic-custom-password", StatusCode::OK),
            (
                "sparrow",
                "synthetic-custom-password",
                StatusCode::UNAUTHORIZED,
            ),
            ("ponbac", "wrong-password", StatusCode::UNAUTHORIZED),
        ] {
            let request = Request::builder()
                .uri("/probe")
                .header(
                    header::AUTHORIZATION,
                    format!(
                        "Basic {}",
                        STANDARD.encode(format!("{username}:{password}"))
                    ),
                )
                .body(Body::empty())
                .unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected);
        }
    }

    #[test]
    fn malformed_usernames_fail_closed_without_echoing_credentials() {
        for username in [
            "",
            "user:canary",
            "user\ncanary",
            "user\0canary",
            &"u".repeat(129),
        ] {
            let error = deployment_auth(|name| {
                Ok(match name {
                    "SPARROW_AUTH_MODE" => None,
                    "SPARROW_AUTH_USERNAME" => Some(username.into()),
                    "PASSWORD" => Some("synthetic-password-canary".into()),
                    _ => panic!("unexpected environment lookup"),
                })
            })
            .expect_err("invalid usernames must not start serving");
            assert_eq!(error, StartupError::Configuration);
            let diagnostic = format!("{error:?} {error}");
            assert!(!diagnostic.contains("canary"));
        }
    }

    #[test]
    fn public_mode_is_rejected_even_with_a_retained_password() {
        for password in [
            None,
            Some(""),
            Some("rollback-password-canary"),
            Some(&"x".repeat(1025)),
        ] {
            assert!(matches!(
                authentication(Some("public"), password),
                Err(StartupError::Configuration)
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
                .expect_err("invalid modes must not start serving");
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
