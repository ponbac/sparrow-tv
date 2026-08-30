use axum::{
    Json,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use subtle::ConstantTimeEq;

use crate::{RouterBuildError, api::ErrorEnvelope};

const USERNAME_PREFIX: &[u8] = b"sparrow:";
const MAX_PASSWORD_BYTES: usize = 1024;
const MAX_AUTHORIZATION_BYTES: usize = 2048;
const CHALLENGE: &str = "Basic realm=\"sparrow\", charset=\"UTF-8\"";

#[derive(Clone)]
pub(crate) struct DeploymentCredential {
    digest: [u8; 32],
}

impl DeploymentCredential {
    pub(crate) fn new(password: &[u8]) -> Result<Self, RouterBuildError> {
        if password.is_empty() {
            return Err(RouterBuildError::MissingPassword);
        }
        if password.len() > MAX_PASSWORD_BYTES {
            return Err(RouterBuildError::PasswordTooLong);
        }

        let mut hasher = blake3::Hasher::new();
        hasher.update(USERNAME_PREFIX);
        hasher.update(password);
        Ok(Self {
            digest: *hasher.finalize().as_bytes(),
        })
    }

    fn authenticates(&self, headers: &HeaderMap) -> bool {
        let Some(value) = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= MAX_AUTHORIZATION_BYTES)
        else {
            return false;
        };
        let Some((scheme, encoded)) = value.split_once(' ') else {
            return false;
        };
        if !scheme.eq_ignore_ascii_case("basic") || encoded.is_empty() {
            return false;
        }
        let Ok(presented) = STANDARD.decode(encoded) else {
            return false;
        };

        let digest = blake3::hash(&presented);
        bool::from(self.digest.ct_eq(digest.as_bytes()))
    }
}

impl std::fmt::Debug for DeploymentCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeploymentCredential")
            .finish_non_exhaustive()
    }
}

pub(crate) async fn require_authentication(
    State(credential): State<DeploymentCredential>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if credential.authenticates(request.headers()) {
        return next.run(request).await;
    }

    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(ErrorEnvelope::authentication_required()),
    )
        .into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static(CHALLENGE),
    );
    response
}
