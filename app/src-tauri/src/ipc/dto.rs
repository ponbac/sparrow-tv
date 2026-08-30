use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use serde::Serialize;
use sparrow_core::{
    CatalogStatus, ChannelDetails, ChannelGroupView, ChannelSummary, CoreError, CoreEvent,
    InputField, InputReason, Page, ProgrammeSummary, RefreshOutcome, RefreshReport,
    RefreshSkipReason, SafeFailure, SearchResults, SourceKind, SourceState,
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
    pub(crate) const fn installed_catalog() -> Self {
        Self {
            source_configuration: "device-writable",
            playback_transport: "unavailable",
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
                source: source_kind(kind),
                outcome: RefreshOutcomeDto::from(&outcome),
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageDto<T> {
    generation: u64,
    items: Vec<T>,
    next: Option<String>,
}

impl PageDto<ChannelGroupDto> {
    pub(crate) fn groups(page: &Page<ChannelGroupView>) -> Self {
        Self::new(page, |group| ChannelGroupDto {
            name: group.name().to_owned(),
            channel_count: group.channel_count(),
        })
    }
}

impl PageDto<ChannelSummaryDto> {
    pub(crate) fn channels(page: &Page<ChannelSummary>) -> Self {
        Self::new(page, |channel| ChannelSummaryDto::from(channel))
    }
}

impl PageDto<ProgrammeDto> {
    pub(crate) fn programmes(page: &Page<ProgrammeSummary>) -> Self {
        Self::new(page, |programme| ProgrammeDto::from(programme))
    }
}

impl<T> PageDto<T> {
    fn new<U>(page: &Page<U>, project: impl Fn(&U) -> T) -> Self {
        Self {
            generation: page.generation().get(),
            items: page.items().iter().map(project).collect(),
            next: page.next().map(|cursor| cursor.as_str().to_owned()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChannelGroupDto {
    name: String,
    channel_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChannelSummaryDto {
    id: String,
    name: String,
    group: String,
}

impl From<&ChannelSummary> for ChannelSummaryDto {
    fn from(channel: &ChannelSummary) -> Self {
        Self {
            id: channel.id().as_str().to_owned(),
            name: channel.name().to_owned(),
            group: channel.group().to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChannelDetailsDto {
    id: String,
    name: String,
    group: String,
}

impl From<&ChannelDetails> for ChannelDetailsDto {
    fn from(channel: &ChannelDetails) -> Self {
        Self {
            id: channel.id().as_str().to_owned(),
            name: channel.name().to_owned(),
            group: channel.group().to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProgrammeDto {
    channel_id: String,
    title: String,
    description: Option<String>,
    starts_at: String,
    ends_at: String,
}

impl From<&ProgrammeSummary> for ProgrammeDto {
    fn from(programme: &ProgrammeSummary) -> Self {
        Self {
            channel_id: programme.channel_id().as_str().to_owned(),
            title: programme.title().to_owned(),
            description: programme.description().map(str::to_owned),
            starts_at: instant(programme.starts_at()),
            ends_at: instant(programme.ends_at()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchResultsDto {
    generation: u64,
    channels: PageDto<ChannelSummaryDto>,
    programmes: PageDto<ProgrammeDto>,
}

impl From<&SearchResults> for SearchResultsDto {
    fn from(results: &SearchResults) -> Self {
        Self {
            generation: results.generation().get(),
            channels: PageDto::channels(results.channels()),
            programmes: PageDto::programmes(results.programmes()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "_tag", rename_all = "kebab-case")]
pub(crate) enum ClientErrorDto {
    ServiceUnavailable,
    InvalidInput {
        field: &'static str,
        reason: &'static str,
    },
    NotConfigured,
    CatalogUnavailable {
        status: Box<CatalogStatusDto>,
    },
    NotFound {
        resource: &'static str,
    },
    StaleCursor {
        current: u64,
    },
}

impl ClientErrorDto {
    pub(crate) const fn service_unavailable() -> Self {
        Self::ServiceUnavailable
    }
}

impl From<CoreError> for ClientErrorDto {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::InvalidInput { field, reason } => Self::InvalidInput {
                field: input_field(field),
                reason: input_reason(reason),
            },
            CoreError::NotConfigured => Self::NotConfigured,
            CoreError::CatalogUnavailable { status } => Self::CatalogUnavailable {
                status: Box::new(CatalogStatusDto::from(*status)),
            },
            CoreError::ChannelNotFound { .. } => Self::NotFound {
                resource: "channel",
            },
            CoreError::StaleCursor { current } => Self::StaleCursor {
                current: current.get(),
            },
            CoreError::Cancelled => Self::ServiceUnavailable,
        }
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

fn instant(value: DateTime<Utc>) -> String {
    match value.year() {
        ..=-1 => "0000-01-01T00:00:00Z".to_owned(),
        10_000.. => "9999-12-31T23:59:59Z".to_owned(),
        _ => value.to_rfc3339_opts(SecondsFormat::AutoSi, true),
    }
}

const fn serde_json_safe_integer_max() -> u64 {
    9_007_199_254_740_991
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn installed_capabilities_are_closed_and_do_not_grant_playback_yet() {
        assert_eq!(
            serde_json::to_value(CapabilitiesDto::installed_catalog())
                .expect("capabilities serialize"),
            json!({
                "sourceConfiguration": "device-writable",
                "playbackTransport": "unavailable",
                "audioTrackSelection": false,
                "mpvFailover": false,
            })
        );
    }

    #[tokio::test]
    async fn routine_status_projection_contains_no_source_location() {
        use std::sync::Arc;

        use sparrow_core::{CoreAdapters, SparrowCore, SystemClock};
        use tempfile::TempDir;

        let directory = TempDir::new().expect("temporary directory");
        let source = sparrow_source_http::HttpSourceAccess::new().expect("source adapter opens");
        let snapshots = sparrow_snapshot_store::AtomicFileSnapshotStore::open(directory.path())
            .expect("snapshot store opens");
        let core = SparrowCore::bootstrap(
            None,
            CoreAdapters::new(Arc::new(source), Arc::new(snapshots), Arc::new(SystemClock)),
        )
        .await
        .expect("unconfigured core bootstraps");
        let json = serde_json::to_string(&CatalogStatusDto::from(core.status()))
            .expect("status serializes");
        assert!(!json.contains("http://"));
        assert!(!json.contains("https://"));
        assert!(!json.contains("location"));
    }
}
