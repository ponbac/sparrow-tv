use serde::Serialize;
use sparrow_client_contract::browser_instant as instant;
use sparrow_core::{
    CatalogStatus, CoreEvent, RefreshOutcome, RefreshReport, RefreshSkipReason, SafeFailure,
    SourceKind, SourceState,
};

pub(crate) use sparrow_client_contract::{
    ChannelDetailsDto, ChannelGroupDto, ChannelSummaryDto, GuideWindowChannelDto, PageDto,
    ProgrammeDto, SearchResultsDto,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapabilitiesDto {
    source_configuration: &'static str,
    playback_transport: &'static str,
    audio_track_selection: bool,
    mpv_failover: bool,
}

impl CapabilitiesDto {
    pub(crate) const fn hosted() -> Self {
        Self {
            source_configuration: "deployment-readonly",
            playback_transport: "same-origin-http",
            audio_track_selection: false,
            mpv_failover: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogStatusDto {
    generation: Option<u64>,
    configuration: SourceConfigurationStatusDto,
    m3u: SourceStateDto,
    epg: Option<SourceStateDto>,
}

impl From<CatalogStatus> for CatalogStatusDto {
    fn from(status: CatalogStatus) -> Self {
        Self::from(&status)
    }
}

impl From<&CatalogStatus> for CatalogStatusDto {
    fn from(status: &CatalogStatus) -> Self {
        let configuration = status.configuration();
        Self {
            generation: status.generation().map(|generation| generation.get()),
            configuration: SourceConfigurationStatusDto {
                configured: configuration.is_configured(),
                epg_configured: configuration.has_epg(),
            },
            m3u: SourceStateDto::from(status.m3u()),
            epg: status.epg().map(SourceStateDto::from),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceConfigurationStatusDto {
    configured: bool,
    epg_configured: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "_tag", rename_all = "kebab-case")]
enum SourceStateDto {
    Fresh {
        #[serde(rename = "validatedAt")]
        validated_at: String,
    },
    Stale {
        #[serde(rename = "validatedAt")]
        validated_at: String,
        #[serde(rename = "nextAttemptAt")]
        next_attempt_at: Option<String>,
    },
    Unavailable {
        failure: Option<SafeFailureDto>,
    },
    Refreshing {
        #[serde(rename = "validatedAt")]
        validated_at: Option<String>,
        #[serde(rename = "startedAt")]
        started_at: String,
    },
    Deferred {
        #[serde(rename = "validatedAt")]
        validated_at: Option<String>,
        #[serde(rename = "deferredAt")]
        deferred_at: String,
    },
    Failed {
        #[serde(rename = "validatedAt")]
        validated_at: Option<String>,
        failure: SafeFailureDto,
        #[serde(rename = "nextAttemptAt")]
        next_attempt_at: String,
    },
}

impl From<&SourceState> for SourceStateDto {
    fn from(state: &SourceState) -> Self {
        match state {
            SourceState::Fresh { validated_at } => Self::Fresh {
                validated_at: instant(*validated_at),
            },
            SourceState::Stale {
                validated_at,
                next_attempt_at,
            } => Self::Stale {
                validated_at: instant(*validated_at),
                next_attempt_at: next_attempt_at.map(instant),
            },
            SourceState::Unavailable { failure } => Self::Unavailable {
                failure: failure.as_ref().map(SafeFailureDto::from),
            },
            SourceState::Refreshing {
                validated_at,
                started_at,
            } => Self::Refreshing {
                validated_at: validated_at.map(instant),
                started_at: instant(*started_at),
            },
            SourceState::Deferred {
                validated_at,
                deferred_at,
            } => Self::Deferred {
                validated_at: validated_at.map(instant),
                deferred_at: instant(*deferred_at),
            },
            SourceState::Failed {
                validated_at,
                failure,
                next_attempt_at,
            } => Self::Failed {
                validated_at: validated_at.map(instant),
                failure: SafeFailureDto::from(failure),
                next_attempt_at: instant(*next_attempt_at),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "_tag", rename_all = "kebab-case")]
pub(crate) enum SafeFailureDto {
    SourceAccess {
        source: &'static str,
        reason: &'static str,
        #[serde(rename = "retryAfterSeconds")]
        retry_after_seconds: Option<u64>,
    },
    SourceRead {
        source: &'static str,
        reason: &'static str,
    },
    Snapshot {
        source: &'static str,
        operation: &'static str,
        reason: &'static str,
    },
    SnapshotRecovery {
        source: &'static str,
        reason: &'static str,
    },
    DecodedLimitExceeded {
        source: &'static str,
        #[serde(rename = "limitBytes")]
        limit_bytes: u64,
    },
    InvalidEncoding {
        source: &'static str,
    },
    InvalidFormat {
        source: &'static str,
        entry: Option<u32>,
        reason: &'static str,
    },
    NoPlayableChannels {
        source: &'static str,
    },
    InvalidEpgFormat {
        source: &'static str,
        reason: &'static str,
    },
    NoEpgChannels {
        source: &'static str,
    },
}

impl From<&SafeFailure> for SafeFailureDto {
    fn from(failure: &SafeFailure) -> Self {
        match failure {
            SafeFailure::SourceAccess {
                kind,
                reason,
                retry_after,
            } => Self::SourceAccess {
                source: source_kind(*kind),
                reason: source_access_reason(*reason),
                retry_after_seconds: retry_after
                    .map(|delay| delay.as_secs().min(serde_json_safe_integer_max())),
            },
            SafeFailure::SourceRead { kind, reason } => Self::SourceRead {
                source: source_kind(*kind),
                reason: match reason {
                    sparrow_core::SourceReadError::Interrupted => "interrupted",
                    sparrow_core::SourceReadError::InvalidBody => "invalid-body",
                },
            },
            SafeFailure::Snapshot {
                kind,
                operation,
                reason,
            } => Self::Snapshot {
                source: source_kind(*kind),
                operation: snapshot_operation(*operation),
                reason: match reason {
                    sparrow_core::StoreError::Unavailable => "unavailable",
                    sparrow_core::StoreError::Capacity => "capacity",
                    sparrow_core::StoreError::Corrupt => "corrupt",
                },
            },
            SafeFailure::SnapshotRecovery { kind, reason } => Self::SnapshotRecovery {
                source: source_kind(*kind),
                reason: snapshot_recovery_reason(*reason),
            },
            SafeFailure::DecodedLimitExceeded { kind, limit_bytes } => Self::DecodedLimitExceeded {
                source: source_kind(*kind),
                limit_bytes: *limit_bytes,
            },
            SafeFailure::InvalidEncoding { kind } => Self::InvalidEncoding {
                source: source_kind(*kind),
            },
            SafeFailure::InvalidFormat { entry, reason } => Self::InvalidFormat {
                source: "m3u",
                entry: *entry,
                reason: match reason {
                    sparrow_core::M3uFailureKind::MissingHeader => "missing-header",
                    sparrow_core::M3uFailureKind::MalformedMetadata => "malformed-metadata",
                    sparrow_core::M3uFailureKind::UnterminatedQuote => "unterminated-quote",
                    sparrow_core::M3uFailureKind::IncompleteEntry => "incomplete-entry",
                    sparrow_core::M3uFailureKind::EmptyName => "empty-name",
                    sparrow_core::M3uFailureKind::UnexpectedLocation => "unexpected-location",
                    sparrow_core::M3uFailureKind::UnsupportedPlaybackSource => {
                        "unsupported-playback-source"
                    }
                },
            },
            SafeFailure::NoPlayableChannels => Self::NoPlayableChannels { source: "m3u" },
            SafeFailure::InvalidEpgFormat { reason } => Self::InvalidEpgFormat {
                source: "epg",
                reason: match reason {
                    sparrow_core::EpgFailureKind::MalformedXml => "malformed-xml",
                },
            },
            SafeFailure::NoEpgChannels => Self::NoEpgChannels { source: "epg" },
        }
    }
}

const fn source_kind(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::M3u => "m3u",
        SourceKind::Epg => "epg",
    }
}

const fn source_access_reason(reason: sparrow_core::SourceAccessError) -> &'static str {
    match reason {
        sparrow_core::SourceAccessError::Unavailable => "unavailable",
        sparrow_core::SourceAccessError::Rejected => "rejected",
        sparrow_core::SourceAccessError::TimedOut => "timed-out",
        sparrow_core::SourceAccessError::InvalidResponse => "invalid-response",
    }
}

const fn snapshot_operation(operation: sparrow_core::SnapshotOperation) -> &'static str {
    match operation {
        sparrow_core::SnapshotOperation::ScanCandidates => "scan-candidates",
        sparrow_core::SnapshotOperation::OpenCandidate => "open-candidate",
        sparrow_core::SnapshotOperation::AdoptCandidate => "adopt-candidate",
        sparrow_core::SnapshotOperation::RevalidateCandidate => "revalidate-candidate",
        sparrow_core::SnapshotOperation::BeginStage => "begin-stage",
        sparrow_core::SnapshotOperation::WriteStage => "write-stage",
        sparrow_core::SnapshotOperation::ReadStage => "read-stage",
        sparrow_core::SnapshotOperation::PrepareActivation => "prepare-activation",
        sparrow_core::SnapshotOperation::Activate => "activate",
        sparrow_core::SnapshotOperation::Discard => "discard",
    }
}

const fn snapshot_recovery_reason(reason: sparrow_core::SnapshotRecoveryReason) -> &'static str {
    match reason {
        sparrow_core::SnapshotRecoveryReason::MissingActivePointer => "missing-active-pointer",
        sparrow_core::SnapshotRecoveryReason::CorruptActivePointer => "corrupt-active-pointer",
        sparrow_core::SnapshotRecoveryReason::MissingManifest => "missing-manifest",
        sparrow_core::SnapshotRecoveryReason::CorruptManifest => "corrupt-manifest",
        sparrow_core::SnapshotRecoveryReason::MissingPayload => "missing-payload",
        sparrow_core::SnapshotRecoveryReason::SourceMismatch => "source-mismatch",
        sparrow_core::SnapshotRecoveryReason::LengthMismatch => "length-mismatch",
        sparrow_core::SnapshotRecoveryReason::ChecksumMismatch => "checksum-mismatch",
    }
}

const fn serde_json_safe_integer_max() -> u64 {
    9_007_199_254_740_991
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RefreshReportDto {
    trigger: &'static str,
    m3u: RefreshOutcomeDto,
    epg: Option<RefreshOutcomeDto>,
    status: CatalogStatusDto,
}

impl From<RefreshReport> for RefreshReportDto {
    fn from(report: RefreshReport) -> Self {
        Self {
            trigger: match report.trigger() {
                sparrow_core::RefreshTrigger::Manual => "manual",
                sparrow_core::RefreshTrigger::Startup => "startup",
                sparrow_core::RefreshTrigger::Resume => "resume",
                sparrow_core::RefreshTrigger::FreshnessDeadline => "freshness-deadline",
            },
            m3u: RefreshOutcomeDto::from(report.m3u()),
            epg: report.epg().map(RefreshOutcomeDto::from),
            status: CatalogStatusDto::from(report.status()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "_tag", rename_all = "kebab-case")]
pub(crate) enum RefreshOutcomeDto {
    NotConfigured,
    Updated {
        #[serde(rename = "validatedAt")]
        validated_at: String,
    },
    NotModified {
        #[serde(rename = "validatedAt")]
        validated_at: String,
    },
    Skipped {
        reason: &'static str,
        #[serde(rename = "nextAttemptAt")]
        next_attempt_at: String,
    },
    Failed {
        failure: SafeFailureDto,
        #[serde(rename = "nextAttemptAt")]
        next_attempt_at: String,
    },
}

impl From<&RefreshOutcome> for RefreshOutcomeDto {
    fn from(outcome: &RefreshOutcome) -> Self {
        match outcome {
            RefreshOutcome::NotConfigured => Self::NotConfigured,
            RefreshOutcome::Updated { validated_at } => Self::Updated {
                validated_at: instant(*validated_at),
            },
            RefreshOutcome::NotModified { validated_at } => Self::NotModified {
                validated_at: instant(*validated_at),
            },
            RefreshOutcome::Skipped {
                reason,
                next_attempt_at,
            } => Self::Skipped {
                reason: match reason {
                    RefreshSkipReason::Fresh => "fresh",
                    RefreshSkipReason::Backoff => "backoff",
                },
                next_attempt_at: instant(*next_attempt_at),
            },
            RefreshOutcome::Failed {
                failure,
                next_attempt_at,
            } => Self::Failed {
                failure: SafeFailureDto::from(failure),
                next_attempt_at: instant(*next_attempt_at),
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "_tag", rename_all = "kebab-case")]
pub(crate) enum CoreEventDto {
    CatalogStatusChanged {
        #[serde(rename = "occurredAt")]
        occurred_at: String,
        status: CatalogStatusDto,
    },
    CatalogPublished {
        #[serde(rename = "occurredAt")]
        occurred_at: String,
        generation: u64,
    },
    RefreshCompleted {
        #[serde(rename = "occurredAt")]
        occurred_at: String,
        source: &'static str,
        outcome: RefreshOutcomeDto,
    },
}

impl From<CoreEvent> for CoreEventDto {
    fn from(event: CoreEvent) -> Self {
        match event {
            CoreEvent::CatalogStatusChanged {
                occurred_at,
                status,
            } => Self::CatalogStatusChanged {
                occurred_at: instant(occurred_at),
                status: CatalogStatusDto::from(status),
            },
            CoreEvent::CatalogPublished {
                occurred_at,
                generation,
            } => Self::CatalogPublished {
                occurred_at: instant(occurred_at),
                generation: generation.get(),
            },
            CoreEvent::RefreshCompleted {
                occurred_at,
                kind,
                outcome,
            } => Self::RefreshCompleted {
                occurred_at: instant(occurred_at),
                source: match kind {
                    SourceKind::M3u => "m3u",
                    SourceKind::Epg => "epg",
                },
                outcome: RefreshOutcomeDto::from(&outcome),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};
    use sparrow_core::{
        EpgFailureKind, M3uFailureKind, SafeFailure, SnapshotOperation, SnapshotRecoveryReason,
        SourceAccessError, SourceKind, SourceReadError, StoreError,
    };

    use super::SafeFailureDto;

    #[test]
    fn safe_failure_projection_is_closed_useful_and_javascript_safe() {
        let cases: Vec<(SafeFailure, Value)> = vec![
            (
                SafeFailure::SourceAccess {
                    kind: SourceKind::M3u,
                    reason: SourceAccessError::TimedOut,
                    retry_after: Some(Duration::from_secs(u64::MAX)),
                },
                json!({
                    "_tag": "source-access",
                    "source": "m3u",
                    "reason": "timed-out",
                    "retryAfterSeconds": 9_007_199_254_740_991_u64,
                }),
            ),
            (
                SafeFailure::SourceRead {
                    kind: SourceKind::Epg,
                    reason: SourceReadError::InvalidBody,
                },
                json!({
                    "_tag": "source-read",
                    "source": "epg",
                    "reason": "invalid-body",
                }),
            ),
            (
                SafeFailure::Snapshot {
                    kind: SourceKind::M3u,
                    operation: SnapshotOperation::PrepareActivation,
                    reason: StoreError::Capacity,
                },
                json!({
                    "_tag": "snapshot",
                    "source": "m3u",
                    "operation": "prepare-activation",
                    "reason": "capacity",
                }),
            ),
            (
                SafeFailure::SnapshotRecovery {
                    kind: SourceKind::Epg,
                    reason: SnapshotRecoveryReason::ChecksumMismatch,
                },
                json!({
                    "_tag": "snapshot-recovery",
                    "source": "epg",
                    "reason": "checksum-mismatch",
                }),
            ),
            (
                SafeFailure::DecodedLimitExceeded {
                    kind: SourceKind::M3u,
                    limit_bytes: 128 * 1024 * 1024,
                },
                json!({
                    "_tag": "decoded-limit-exceeded",
                    "source": "m3u",
                    "limitBytes": 134_217_728,
                }),
            ),
            (
                SafeFailure::InvalidEncoding {
                    kind: SourceKind::Epg,
                },
                json!({ "_tag": "invalid-encoding", "source": "epg" }),
            ),
            (
                SafeFailure::InvalidFormat {
                    entry: Some(7),
                    reason: M3uFailureKind::UnsupportedPlaybackSource,
                },
                json!({
                    "_tag": "invalid-format",
                    "source": "m3u",
                    "entry": 7,
                    "reason": "unsupported-playback-source",
                }),
            ),
            (
                SafeFailure::NoPlayableChannels,
                json!({ "_tag": "no-playable-channels", "source": "m3u" }),
            ),
            (
                SafeFailure::InvalidEpgFormat {
                    reason: EpgFailureKind::MalformedXml,
                },
                json!({
                    "_tag": "invalid-epg-format",
                    "source": "epg",
                    "reason": "malformed-xml",
                }),
            ),
            (
                SafeFailure::NoEpgChannels,
                json!({ "_tag": "no-epg-channels", "source": "epg" }),
            ),
        ];

        for (failure, expected) in cases {
            let projected =
                serde_json::to_value(SafeFailureDto::from(&failure)).expect("DTO serializes");
            assert_eq!(projected, expected);
            let diagnostic = projected.to_string();
            for forbidden in ["http://", "https://", "credential", "provider-body"] {
                assert!(!diagnostic.contains(forbidden));
            }
        }
    }
}
