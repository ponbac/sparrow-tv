use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use sparrow_core::{
    CatalogStatus, ChannelDetails, ChannelGroupView, ChannelSummary, Page, SafeFailure, SourceState,
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
enum SafeFailureDto {
    SourceAccess,
    SourceRead,
    Snapshot,
    SnapshotRecovery,
    DecodedLimitExceeded,
    InvalidEncoding,
    InvalidFormat,
    NoPlayableChannels,
    InvalidEpgFormat,
    NoEpgChannels,
}

impl From<&SafeFailure> for SafeFailureDto {
    fn from(failure: &SafeFailure) -> Self {
        match failure {
            SafeFailure::SourceAccess { .. } => Self::SourceAccess,
            SafeFailure::SourceRead { .. } => Self::SourceRead,
            SafeFailure::Snapshot { .. } => Self::Snapshot,
            SafeFailure::SnapshotRecovery { .. } => Self::SnapshotRecovery,
            SafeFailure::DecodedLimitExceeded { .. } => Self::DecodedLimitExceeded,
            SafeFailure::InvalidEncoding { .. } => Self::InvalidEncoding,
            SafeFailure::InvalidFormat { .. } => Self::InvalidFormat,
            SafeFailure::NoPlayableChannels => Self::NoPlayableChannels,
            SafeFailure::InvalidEpgFormat { .. } => Self::InvalidEpgFormat,
            SafeFailure::NoEpgChannels => Self::NoEpgChannels,
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

fn instant(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}
