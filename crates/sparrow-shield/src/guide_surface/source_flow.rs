use chrono::{DateTime, SecondsFormat, Utc};
use gpui::{Context, Div, Window};
use sparrow_core::{
    CatalogGeneration, CatalogStatus, EpgFailureKind, M3uFailureKind, SafeFailure,
    SnapshotOperation, SnapshotRecoveryReason, SourceAccessError, SourceKind, SourceReadError,
    SourceState, StoreError,
};

use sparrow_tv_guide::{guide_screen::ScreenKey, now_focus::Direction};

use super::GuideSurface;
use crate::{
    activity::{self, SourceEvent},
    catalog::{self, SourceSaveFailure},
    guide_view::{self, SourceFocus, SourcePanel},
};

const SOURCE_OPEN_FAILED: &str = "Couldn’t open Source Configuration.";

pub(super) struct SourceFlow {
    session: u64,
    focus: SourceFocus,
    draft_ready: bool,
    has_m3u: bool,
    has_epg: bool,
    editor: Option<(u64, i32)>,
    next_attempt: u64,
    pending: Option<SourceSave>,
    refreshing: bool,
    diagnostics: Option<String>,
    copy_attempt: u64,
    copy: DiagnosticCopy,
    feedback: Option<&'static str>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SourceSave {
    Request(u64),
    Admitted(u64),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DiagnosticCopy {
    Ready,
    Pending(u64),
    Copied,
    Failed,
}

impl DiagnosticCopy {
    const fn label(self) -> &'static str {
        match self {
            Self::Ready => "Copy diagnostics",
            Self::Pending(_) => "Copying…",
            Self::Copied => "Copied",
            Self::Failed => "Clipboard unavailable",
        }
    }
}

impl GuideSurface {
    pub(super) fn source_editor_open(&self) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| source.editor.is_some())
    }

    pub(super) fn source_visibility_changed(&mut self) {
        let renew = self.source.as_ref().and_then(|source| {
            (self.activation.is_some() && !source.draft_ready && source.pending.is_none())
                .then_some(source.feedback)
        });
        if let Some(feedback) = renew {
            self.begin_source_draft(feedback);
        }
    }

    pub(super) fn open_sources(&mut self, cx: &mut Context<Self>) {
        self.begin_source_draft(None);
        cx.notify();
    }

    fn begin_source_draft(&mut self, feedback: Option<&'static str>) {
        self.source_sequence += 1;
        let session = self.source_sequence;
        let requested = activity::source_session(self.host, session, true);
        self.source = Some(SourceFlow {
            session,
            focus: SourceFocus::Close,
            draft_ready: false,
            has_m3u: false,
            has_epg: false,
            editor: None,
            next_attempt: 0,
            pending: None,
            refreshing: false,
            diagnostics: None,
            copy_attempt: 0,
            copy: DiagnosticCopy::Ready,
            feedback: if requested {
                feedback
            } else {
                Some(SOURCE_OPEN_FAILED)
            },
        });
    }

    pub(super) fn close_sources(&mut self, cx: &mut Context<Self>) {
        if let Some(source) = self.source.take() {
            if let Some((generation, _)) = source.editor {
                activity::close_text_editor(self.host, generation);
            }
            activity::source_session(self.host, source.session, false);
            self.sync_view(cx);
        }
    }

    pub(super) fn sources_sheet(&self, window: &Window) -> Option<Div> {
        let source = self.source.as_ref()?;
        let status = catalog::status();
        Some(guide_view::sources_sheet(
            window.viewport_size(),
            SourcePanel {
                status: status.as_deref(),
                focus: source.focus,
                editing: source.editor.is_some(),
                has_m3u: source.has_m3u,
                has_epg: source.has_epg,
                saving: source.pending.is_some(),
                refreshing: source.refreshing,
                diagnostics: source.diagnostics.as_deref(),
                copy_label: source.copy.label(),
                copied: source.copy == DiagnosticCopy::Copied,
                feedback: source.feedback,
            },
        ))
    }

    pub(super) fn source_key(&mut self, key: ScreenKey, window: &Window, cx: &mut Context<Self>) {
        let Some(source) = self.source.as_mut() else {
            return;
        };
        let status = catalog::status();
        let configured = status
            .as_ref()
            .is_some_and(|s| s.configuration().is_configured());
        match key {
            ScreenKey::Arrow(direction) => {
                source.focus = match (source.focus, direction) {
                    (SourceFocus::Close, Direction::Down) if configured => SourceFocus::Refresh,
                    (SourceFocus::Close | SourceFocus::Refresh, Direction::Down)
                        if status.is_some() =>
                    {
                        SourceFocus::Diagnostics
                    }
                    (SourceFocus::Close | SourceFocus::Refresh, Direction::Down) => {
                        SourceFocus::M3u
                    }
                    (SourceFocus::Diagnostics, Direction::Down) if source.diagnostics.is_some() => {
                        SourceFocus::DiagnosticText
                    }
                    (SourceFocus::DiagnosticText, Direction::Down) => SourceFocus::CopyDiagnostics,
                    (SourceFocus::Diagnostics | SourceFocus::CopyDiagnostics, Direction::Down) => {
                        SourceFocus::M3u
                    }
                    (SourceFocus::M3u, Direction::Down) => SourceFocus::Epg,
                    (SourceFocus::Epg, Direction::Down) => SourceFocus::Save,
                    (SourceFocus::Save, Direction::Up) => SourceFocus::Epg,
                    (SourceFocus::Epg, Direction::Up) => SourceFocus::M3u,
                    (SourceFocus::M3u, Direction::Up) if source.diagnostics.is_some() => {
                        SourceFocus::CopyDiagnostics
                    }
                    (SourceFocus::M3u, Direction::Up) if status.is_some() => {
                        SourceFocus::Diagnostics
                    }
                    (SourceFocus::CopyDiagnostics, Direction::Up) => SourceFocus::DiagnosticText,
                    (SourceFocus::DiagnosticText, Direction::Up) => SourceFocus::Diagnostics,
                    (SourceFocus::Diagnostics | SourceFocus::M3u, Direction::Up) if configured => {
                        SourceFocus::Refresh
                    }
                    (
                        SourceFocus::M3u | SourceFocus::Refresh | SourceFocus::Diagnostics,
                        Direction::Up,
                    ) => SourceFocus::Close,
                    (focus, _) => focus,
                };
            }
            ScreenKey::Activate => match source.focus {
                SourceFocus::Close => self.close_sources(cx),
                SourceFocus::Refresh if configured => self.request_source_refresh(cx),

                SourceFocus::Diagnostics => {
                    source.diagnostics = if source.diagnostics.is_some() {
                        None
                    } else {
                        status.as_deref().map(|status| {
                            safe_source_diagnostics(
                                status,
                                self.screen.families().len(),
                                self.screen.group_roster().len(),
                            )
                        })
                    };
                    source.copy = DiagnosticCopy::Ready;
                }
                SourceFocus::CopyDiagnostics
                    if !matches!(source.copy, DiagnosticCopy::Pending(_)) =>
                {
                    if let Some(text) = source.diagnostics.as_deref() {
                        source.copy_attempt += 1;
                        let attempt = source.copy_attempt;
                        source.copy = if activity::copy_source_diagnostics(
                            self.host,
                            source.session,
                            attempt,
                            text,
                        ) {
                            DiagnosticCopy::Pending(attempt)
                        } else {
                            DiagnosticCopy::Failed
                        };
                    }
                }
                SourceFocus::M3u | SourceFocus::Epg
                    if source.draft_ready && source.pending.is_none() =>
                {
                    self.editor_generation += 1;
                    let generation = self.editor_generation;
                    let field = i32::from(source.focus == SourceFocus::Epg);
                    source.editor = Some((generation, field));
                    let rect = guide_view::source_editor_rect(
                        window.viewport_size(),
                        window.scale_factor(),
                    );
                    source.feedback = if rect.is_some_and(|rect| {
                        activity::edit_source(self.host, source.session, generation, field, rect)
                    }) {
                        None
                    } else {
                        source.editor = None;
                        Some("Couldn’t open the keyboard.")
                    };
                }
                SourceFocus::Save if source.draft_ready && source.pending.is_none() => {
                    if source.has_m3u {
                        source.next_attempt += 1;
                        let attempt = source.next_attempt;
                        if activity::submit_sources(self.host, source.session, attempt) {
                            source.pending = Some(SourceSave::Request(attempt));
                            source.feedback = None;
                        } else {
                            source.feedback = Some("Couldn’t save sources.");
                        }
                    } else {
                        source.focus = SourceFocus::M3u;
                        source.feedback = Some("Enter the channel source.");
                    }
                }
                _ => {}
            },
        }
        cx.notify();
    }

    fn request_source_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.as_mut().filter(|source| !source.refreshing) else {
            return;
        };
        let Some(refresh) = catalog::refresh_sources() else {
            source.feedback = Some("Couldn’t refresh sources.");
            return;
        };
        source.refreshing = true;
        source.feedback = None;
        let session = source.session;
        cx.spawn(async move |view, cx| {
            let feedback = refresh.await.unwrap_or("Couldn’t refresh sources.");
            let _ = view.update(cx, |view, cx| {
                if let Some(source) = view
                    .source
                    .as_mut()
                    .filter(|source| source.session == session)
                {
                    source.refreshing = false;
                    source.feedback = Some(feedback);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn source_event(
        &mut self,
        session: u64,
        update: SourceEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self
            .source
            .as_mut()
            .filter(|source| source.session == session)
        else {
            return;
        };
        match update {
            SourceEvent::Copied { attempt, copied }
                if source.copy == DiagnosticCopy::Pending(attempt) =>
            {
                source.copy = if copied {
                    DiagnosticCopy::Copied
                } else {
                    DiagnosticCopy::Failed
                };
            }
            SourceEvent::DraftStatus {
                attempt: 0,
                status: 0,
            } => {
                source.draft_ready = true;
                if source.feedback == Some(SOURCE_OPEN_FAILED) {
                    source.feedback = None;
                }
            }
            SourceEvent::DraftStatus {
                attempt: 0,
                status: 1,
            } => {
                source.feedback = Some(SOURCE_OPEN_FAILED);
            }
            SourceEvent::DraftStatus { attempt, status: 2 }
                if source.pending == Some(SourceSave::Request(attempt)) =>
            {
                source.pending = None;
                source.feedback = Some("Couldn’t save sources.");
            }
            SourceEvent::Field {
                generation,
                field,
                event,
                has_text,
            } if source.editor == Some((generation, field)) => {
                source.editor = None;
                if event == 1 {
                    if field == 0 {
                        source.has_m3u = has_text;
                    } else {
                        source.has_epg = has_text;
                    }
                    source.feedback = None;
                } else if event == 3 {
                    source.feedback = Some("Couldn’t open the keyboard.");
                }
            }
            SourceEvent::Submit { attempt, m3u, epg }
                if source.pending == Some(SourceSave::Request(attempt)) =>
            {
                source.pending = Some(SourceSave::Admitted(attempt));
                if let Some(save) = catalog::save_sources(m3u, epg) {
                    cx.spawn(async move |view, cx| {
                        let result = save.await.unwrap_or(Err(SourceSaveFailure::Unavailable));
                        let _ = view.update(cx, |view, cx| {
                            view.source_saved(session, attempt, result, cx)
                        });
                    })
                    .detach();
                } else {
                    self.source_saved(session, attempt, Err(SourceSaveFailure::Unavailable), cx);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    fn source_saved(
        &mut self,
        session: u64,
        attempt: u64,
        result: Result<(), SourceSaveFailure>,
        cx: &mut Context<Self>,
    ) {
        if result.is_ok() {
            self.screen.clear_group_selection();
        }
        let Some(source) = self.source.as_mut().filter(|source| {
            source.session == session && source.pending == Some(SourceSave::Admitted(attempt))
        }) else {
            self.sync_view(cx);
            return;
        };
        activity::finish_source_save(self.host, session, attempt, result.is_ok());
        source.pending = None;
        match result {
            Ok(()) if self.activation.is_some() => self.begin_source_draft(Some("Sources saved.")),
            Ok(()) => {
                source.draft_ready = false;
                source.feedback = Some("Sources saved.");
            }
            Err(SourceSaveFailure::M3u) => {
                source.focus = SourceFocus::M3u;
                source.feedback = Some("Enter a valid channel source URL.");
            }
            Err(SourceSaveFailure::Epg) => {
                source.focus = SourceFocus::Epg;
                source.feedback = Some("Enter a valid Programme Guide URL.");
            }
            Err(SourceSaveFailure::Unavailable) => source.feedback = Some("Couldn’t save sources."),
        }
        self.sync_view(cx);
    }
}

// The Tauri v1 status projection, with counts of the rows loaded by this Guide.
// The sheet displays and copies one snapshot captured when Diagnostics opens.
fn safe_source_diagnostics(status: &CatalogStatus, families: usize, groups: usize) -> String {
    let configuration = status.configuration();
    let configured = |present| if present { "configured" } else { "absent" };
    let mut lines = vec![
        "sparrow-safe-diagnostics/v1".to_owned(),
        format!(
            "catalog.generation={}",
            status.generation().map_or_else(
                || "unavailable".to_owned(),
                |generation| generation.get().to_string()
            )
        ),
        format!(
            "configuration.m3u={}",
            configured(configuration.is_configured())
        ),
        format!("configuration.epg={}", configured(configuration.has_epg())),
    ];
    source_diagnostic_lines(&mut lines, "m3u", status.m3u());
    if let Some(epg) = status.epg() {
        source_diagnostic_lines(&mut lines, "epg", epg);
    } else {
        lines.push("epg.state=not-configured".into());
    }
    lines.push(format!("guide.loaded-families={families}"));
    lines.push(format!("guide.loaded-groups={groups}"));
    lines.join("\n")
}

fn source_diagnostic_lines(lines: &mut Vec<String>, prefix: &str, state: &SourceState) {
    let field = |name, value| format!("{prefix}.{name}={value}");
    match state {
        SourceState::Fresh { validated_at } => {
            lines.push(field("state", "fresh".into()));
            lines.push(field(
                "validated-at",
                diagnostic_time(Some(*validated_at), "none"),
            ));
        }
        SourceState::Stale {
            validated_at,
            next_attempt_at,
        } => {
            lines.push(field("state", "stale".into()));
            lines.push(field(
                "validated-at",
                diagnostic_time(Some(*validated_at), "none"),
            ));
            lines.push(field(
                "next-attempt-at",
                diagnostic_time(*next_attempt_at, "unscheduled"),
            ));
        }
        SourceState::Unavailable { failure } => {
            lines.push(field("state", "unavailable".into()));
            if let Some(failure) = failure {
                failure_diagnostic_lines(lines, prefix, failure);
            } else {
                lines.push(field("failure", "none".into()));
            }
        }
        SourceState::Refreshing {
            validated_at,
            started_at,
        } => {
            lines.push(field("state", "refreshing".into()));
            lines.push(field(
                "validated-at",
                diagnostic_time(*validated_at, "none"),
            ));
            lines.push(field(
                "started-at",
                diagnostic_time(Some(*started_at), "none"),
            ));
        }
        SourceState::Deferred {
            validated_at,
            deferred_at,
        } => {
            lines.push(field("state", "deferred".into()));
            lines.push(field(
                "validated-at",
                diagnostic_time(*validated_at, "none"),
            ));
            lines.push(field(
                "deferred-at",
                diagnostic_time(Some(*deferred_at), "none"),
            ));
        }
        SourceState::Failed {
            validated_at,
            next_attempt_at,
            failure,
        } => {
            lines.push(field("state", "failed".into()));
            lines.push(field(
                "validated-at",
                diagnostic_time(*validated_at, "none"),
            ));
            failure_diagnostic_lines(lines, prefix, failure);
            lines.push(field(
                "next-attempt-at",
                diagnostic_time(Some(*next_attempt_at), "none"),
            ));
        }
    }
}

fn diagnostic_time(at: Option<DateTime<Utc>>, absent: &str) -> String {
    at.map_or_else(
        || absent.to_owned(),
        |at| at.to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

fn failure_diagnostic_lines(lines: &mut Vec<String>, prefix: &str, failure: &SafeFailure) {
    let (tag, kind) = match failure {
        SafeFailure::SourceAccess { kind, .. } => ("source-access", *kind),
        SafeFailure::SourceRead { kind, .. } => ("source-read", *kind),
        SafeFailure::Snapshot { kind, .. } => ("snapshot", *kind),
        SafeFailure::SnapshotRecovery { kind, .. } => ("snapshot-recovery", *kind),
        SafeFailure::DecodedLimitExceeded { kind, .. } => ("decoded-limit-exceeded", *kind),
        SafeFailure::InvalidEncoding { kind } => ("invalid-encoding", *kind),
        SafeFailure::InvalidFormat { .. } => ("invalid-format", SourceKind::M3u),
        SafeFailure::InvalidEpgFormat { .. } => ("invalid-epg-format", SourceKind::Epg),
        SafeFailure::NoPlayableChannels => ("no-playable-channels", SourceKind::M3u),
        SafeFailure::NoEpgChannels => ("no-epg-channels", SourceKind::Epg),
    };
    let field = |name, value| format!("{prefix}.{name}={value}");
    lines.push(field("failure", tag.to_owned()));
    lines.push(field(
        "failure-source",
        match kind {
            SourceKind::M3u => "m3u",
            SourceKind::Epg => "epg",
        }
        .to_owned(),
    ));
    match failure {
        SafeFailure::SourceAccess {
            reason,
            retry_after,
            ..
        } => {
            let reason = match reason {
                SourceAccessError::Unavailable => "unavailable",
                SourceAccessError::Rejected => "rejected",
                SourceAccessError::TimedOut => "timed-out",
                SourceAccessError::InvalidResponse => "invalid-response",
            };
            lines.push(field("failure-reason", reason.into()));
            lines.push(field(
                "retry-after-seconds",
                retry_after.map_or_else(
                    || "none".into(),
                    |delay| {
                        delay
                            .as_secs()
                            .min(CatalogGeneration::MAX_SAFE_INTEGER)
                            .to_string()
                    },
                ),
            ));
        }
        SafeFailure::SourceRead { reason, .. } => {
            let reason = match reason {
                SourceReadError::Interrupted => "interrupted",
                SourceReadError::InvalidBody => "invalid-body",
            };
            lines.push(field("failure-reason", reason.into()));
        }
        SafeFailure::Snapshot {
            operation, reason, ..
        } => {
            let operation = match operation {
                SnapshotOperation::ScanCandidates => "scan-candidates",
                SnapshotOperation::OpenCandidate => "open-candidate",
                SnapshotOperation::AdoptCandidate => "adopt-candidate",
                SnapshotOperation::RevalidateCandidate => "revalidate-candidate",
                SnapshotOperation::BeginStage => "begin-stage",
                SnapshotOperation::WriteStage => "write-stage",
                SnapshotOperation::ReadStage => "read-stage",
                SnapshotOperation::PrepareActivation => "prepare-activation",
                SnapshotOperation::Activate => "activate",
                SnapshotOperation::Discard => "discard",
            };
            let reason = match reason {
                StoreError::Unavailable => "unavailable",
                StoreError::Capacity => "capacity",
                StoreError::Corrupt => "corrupt",
            };
            lines.push(field("failure-operation", operation.into()));
            lines.push(field("failure-reason", reason.into()));
        }
        SafeFailure::SnapshotRecovery { reason, .. } => {
            let reason = match reason {
                SnapshotRecoveryReason::MissingActivePointer => "missing-active-pointer",
                SnapshotRecoveryReason::CorruptActivePointer => "corrupt-active-pointer",
                SnapshotRecoveryReason::MissingManifest => "missing-manifest",
                SnapshotRecoveryReason::CorruptManifest => "corrupt-manifest",
                SnapshotRecoveryReason::MissingPayload => "missing-payload",
                SnapshotRecoveryReason::SourceMismatch => "source-mismatch",
                SnapshotRecoveryReason::LengthMismatch => "length-mismatch",
                SnapshotRecoveryReason::ChecksumMismatch => "checksum-mismatch",
            };
            lines.push(field("failure-reason", reason.into()));
        }
        SafeFailure::DecodedLimitExceeded { limit_bytes, .. } => {
            lines.push(field("limit-bytes", limit_bytes.to_string()));
        }
        SafeFailure::InvalidFormat { entry, reason } => {
            let reason = match reason {
                M3uFailureKind::MissingHeader => "missing-header",
                M3uFailureKind::MalformedMetadata => "malformed-metadata",
                M3uFailureKind::UnterminatedQuote => "unterminated-quote",
                M3uFailureKind::IncompleteEntry => "incomplete-entry",
                M3uFailureKind::EmptyName => "empty-name",
                M3uFailureKind::UnexpectedLocation => "unexpected-location",
                M3uFailureKind::UnsupportedPlaybackSource => "unsupported-playback-source",
            };
            lines.push(field(
                "entry",
                entry.map_or_else(|| "none".into(), |entry| entry.to_string()),
            ));
            lines.push(field("failure-reason", reason.into()));
        }
        SafeFailure::InvalidEpgFormat { reason } => {
            let reason = match reason {
                EpgFailureKind::MalformedXml => "malformed-xml",
            };
            lines.push(field("failure-reason", reason.into()));
        }
        SafeFailure::InvalidEncoding { .. }
        | SafeFailure::NoPlayableChannels
        | SafeFailure::NoEpgChannels => {}
    }
}
