mod catalog;
mod core;
mod domain;
mod identity;
mod m3u;
mod ports;
mod xmltv;

pub use core::SparrowCore;
pub use domain::{
    CatalogGeneration, CatalogStatus, ChannelDetails, ChannelGroupView, ChannelId, ChannelQuery,
    ChannelSummary, CoreError, EpgFailureKind, InputField, InputReason, M3uFailureKind, Page,
    PageCursor, PageLimit, PageRequest, ProgrammeSummary, RedactedSourceConfiguration, SafeFailure,
    ScheduleQuery, SnapshotOperation, SourceAccessError, SourceConfiguration,
    SourceConfigurationInput, SourceKind, SourceReadError, SourceState, StoreError,
};
pub use ports::{
    Clock, CoreAdapters, SnapshotSource, SnapshotStage, SnapshotStore, SourceAccess,
    SourceByteStream, SourceKey, SourceRequest, SourceResponse, SystemClock, ValidatedStage,
};
