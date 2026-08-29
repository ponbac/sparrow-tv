mod catalog;
mod core;
mod domain;
mod identity;
mod m3u;
mod ports;

pub use core::SparrowCore;
pub use domain::{
    CatalogGeneration, CatalogStatus, ChannelDetails, ChannelGroupView, ChannelId, ChannelQuery,
    ChannelSummary, CoreError, InputField, InputReason, M3uFailureKind, Page, PageCursor,
    PageLimit, PageRequest, RedactedSourceConfiguration, SafeFailure, SnapshotOperation,
    SourceAccessError, SourceConfiguration, SourceConfigurationInput, SourceKind, SourceReadError,
    SourceState, StoreError,
};
pub use ports::{
    Clock, CoreAdapters, SnapshotSource, SnapshotStage, SnapshotStore, SourceAccess,
    SourceByteStream, SourceKey, SourceRequest, SourceResponse, SystemClock, ValidatedStage,
};
