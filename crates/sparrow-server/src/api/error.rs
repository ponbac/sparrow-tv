use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use sparrow_core::{CoreError, InputField, InputReason};

use super::CatalogStatusDto;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ErrorEnvelope {
    error: ClientErrorDto,
}

impl ErrorEnvelope {
    pub(crate) const fn authentication_required() -> Self {
        Self {
            error: ClientErrorDto::AuthenticationRequired,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "_tag", rename_all = "kebab-case")]
enum ClientErrorDto {
    AuthenticationRequired,
    ServiceUnavailable,
    InvalidInput {
        field: &'static str,
        reason: &'static str,
    },
    NotConfigured,
    CatalogUnavailable {
        status: CatalogStatusDto,
    },
    NotFound {
        resource: &'static str,
    },
    StaleCursor {
        current: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ApiError {
    status: StatusCode,
    body: Box<ErrorEnvelope>,
}

impl ApiError {
    pub(crate) fn invalid(field: &'static str, reason: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: Box::new(ErrorEnvelope {
                error: ClientErrorDto::InvalidInput { field, reason },
            }),
        }
    }

    pub(crate) fn service_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: Box::new(ErrorEnvelope {
                error: ClientErrorDto::ServiceUnavailable,
            }),
        }
    }
}

impl From<CoreError> for ApiError {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::InvalidInput { field, reason } => {
                Self::invalid(input_field(field), input_reason(reason))
            }
            CoreError::NotConfigured => Self {
                status: StatusCode::CONFLICT,
                body: Box::new(ErrorEnvelope {
                    error: ClientErrorDto::NotConfigured,
                }),
            },
            CoreError::CatalogUnavailable { status } => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                body: Box::new(ErrorEnvelope {
                    error: ClientErrorDto::CatalogUnavailable {
                        status: CatalogStatusDto::from(*status),
                    },
                }),
            },
            CoreError::ChannelNotFound { .. } => Self {
                status: StatusCode::NOT_FOUND,
                body: Box::new(ErrorEnvelope {
                    error: ClientErrorDto::NotFound {
                        resource: "channel",
                    },
                }),
            },
            CoreError::StaleCursor { current } => Self {
                status: StatusCode::CONFLICT,
                body: Box::new(ErrorEnvelope {
                    error: ClientErrorDto::StaleCursor {
                        current: current.get(),
                    },
                }),
            },
            CoreError::Cancelled => Self::service_unavailable(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(*self.body)).into_response()
    }
}

const fn input_field(field: InputField) -> &'static str {
    match field {
        InputField::M3u => "m3u",
        InputField::Epg => "epg",
        InputField::ChannelId => "channel-id",
        InputField::ChannelGroup => "channel-group",
        InputField::SearchTerm => "search-term",
        InputField::PageLimit => "page-limit",
        InputField::PageCursor => "page-cursor",
    }
}

const fn input_reason(reason: InputReason) -> &'static str {
    match reason {
        InputReason::Required => "required",
        InputReason::TooLong { .. } => "too-long",
        InputReason::ContainsControlCharacter => "contains-control-character",
        InputReason::UnsupportedLocation => "unsupported-location",
        InputReason::OutOfRange => "out-of-range",
        InputReason::InvalidFormat => "invalid-format",
        InputReason::CursorQueryMismatch => "cursor-query-mismatch",
        InputReason::CursorPositionOutOfRange => "cursor-position-out-of-range",
    }
}
