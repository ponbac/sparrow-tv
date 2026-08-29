mod catalog;
mod core;
mod domain;
mod identity;
mod m3u;
mod ports;
mod xmltv;

pub use core::{CoreEventStream, PlaybackActivityLease, SparrowCore};
pub use domain::{
    CatalogGeneration, CatalogStatus, ChannelDetails, ChannelGroupView, ChannelId, ChannelQuery,
    ChannelSummary, CoreError, CoreEvent, EpgFailureKind, InputField, InputReason, LifecycleSignal,
    M3uFailureKind, Page, PageCursor, PageLimit, PageRequest, ProgrammeSummary,
    RedactedSourceConfiguration, RefreshOutcome, RefreshReport, RefreshSkipReason, RefreshTrigger,
    SafeFailure, ScheduleQuery, SearchRequest, SearchResults, SearchTerm, SnapshotOperation,
    SnapshotRecoveryDiagnostic, SnapshotRecoveryReason, SourceAccessError, SourceAccessFailure,
    SourceConfiguration, SourceConfigurationInput, SourceKind, SourceReadError, SourceState,
    StoreError,
};
pub use ports::{
    Clock, CoreAdapters, PrivateSourceValidators, PrivateValidatorError, SnapshotCandidate,
    SnapshotCandidates, SnapshotMetadata, SnapshotRevalidation, SnapshotScan, SnapshotSource,
    SnapshotStage, SnapshotStageRequest, SnapshotStore, SourceAccess, SourceByteStream, SourceKey,
    SourceRequest, SourceResponse, SystemClock, ValidatedStage,
};
