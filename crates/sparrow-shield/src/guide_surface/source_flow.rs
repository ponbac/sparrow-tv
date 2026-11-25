use chrono::{DateTime, SecondsFormat, Utc};
use gpui::{Context, Div, Window};
use sparrow_core::{
    CatalogGeneration, CatalogStatus, EpgFailureKind, M3uFailureKind, SafeFailure,
    SnapshotOperation, SnapshotRecoveryReason, SourceAccessError, SourceKind, SourceReadError,
    SourceState, StoreError,
};

use sparrow_tv_guide::{
    guide_screen::ScreenKey,
    now_focus::Direction,
    setup::{CatalogProgress, DraftView, SetupField, SetupMessage},
};

use super::GuideSurface;
use sparrow_installed_catalog::SavedSourceLocations;

use crate::{
    activity::{self, SourceEvent},
    catalog::{self, SourceSaveFailure},
    guide_view::{self, SourceFocus, SourcePanel},
};

const SOURCE_OPEN_FAILED: &str = "Couldn’t open Source Configuration.";
const EDITOR_COMMITTED: i32 = 1;
const EDITOR_REJECTED: i32 = 3;
/// `SourceDraft.CLEARED`: the Activity emptied a field on request.
const FIELD_CLEARED: i32 = 4;

/// Where the draft is shown. Both use one Activity draft session.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Presentation {
    Sheet,
    Setup,
}

/// Setup prefills its draft from the saved Source Configuration.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Seed {
    Off,
    /// The saved locations are being read for this session.
    Reading,
    /// The Activity was handed the saved locations; presence applies on Begun.
    Sent {
        has_m3u: bool,
        has_epg: bool,
    },
}

pub(super) struct SourceFlow {
    session: u64,
    presentation: Presentation,
    seed: Seed,
    focus: SourceFocus,
    draft_ready: bool,
    has_m3u: bool,
    has_epg: bool,
    /// An edit or a clear the Activity has yet to report on.
    request: Option<FieldRequest>,
    next_attempt: u64,
    pending: Option<SourceSave>,
    refreshing: bool,
    diagnostics: Option<String>,
    copy_attempt: u64,
    copy: DiagnosticCopy,
    feedback: Option<&'static str>,
}

/// A field operation waiting for the Activity's field event.
#[derive(Clone, Copy, Eq, PartialEq)]
enum FieldRequest {
    Edit(u64, SetupField),
    Clear(u64, SetupField),
}

impl FieldRequest {
    fn matches(self, generation: u64, field: SetupField) -> bool {
        match self {
            Self::Edit(pending, requested) | Self::Clear(pending, requested) => {
                pending == generation && requested == field
            }
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SourceSave {
    Request { attempt: u64, include_epg: bool },
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

const fn field_index(field: SetupField) -> i32 {
    match field {
        SetupField::M3u => 0,
        SetupField::Epg => 1,
    }
}

const fn setup_field(index: i32) -> Option<SetupField> {
    match index {
        0 => Some(SetupField::M3u),
        1 => Some(SetupField::Epg),
        _ => None,
    }
}

impl GuideSurface {
    pub(super) fn source_editor_open(&self) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| matches!(source.request, Some(FieldRequest::Edit(..))))
    }

    /// An edit or clear is in flight. Back may reach the host before the
    /// Activity mounts its editor, and must not close the draft then.
    pub(super) fn source_request_pending(&self) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| source.request.is_some())
    }

    /// Setup's draft is missing or the Activity refused to open it.
    pub(super) fn setup_draft_missing(&self) -> bool {
        self.source.as_ref().is_none_or(|source| {
            source.presentation != Presentation::Setup
                || (!source.draft_ready
                    && source.seed != Seed::Reading
                    && source.feedback == Some(SOURCE_OPEN_FAILED))
        })
    }

    pub(super) fn sources_sheet_open(&self) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| source.presentation == Presentation::Sheet)
    }

    pub(super) fn backdrop_preview_focused(&self) -> bool {
        self.source.as_ref().is_some_and(|source| {
            source.presentation == Presentation::Sheet && source.focus == SourceFocus::Backdrop
        })
    }

    /// The Setup draft as the Setup state machine sees it.
    pub(super) fn setup_draft(&self) -> DraftView {
        self.source
            .as_ref()
            .filter(|source| source.presentation == Presentation::Setup)
            .map_or(
                DraftView {
                    busy: true,
                    ..DraftView::default()
                },
                |source| DraftView {
                    has_m3u: source.has_m3u,
                    has_epg: source.has_epg,
                    busy: !source.draft_ready
                        || source.request.is_some()
                        || source.pending.is_some(),
                },
            )
    }

    pub(super) fn setup_saving(&self) -> bool {
        self.source.as_ref().is_some_and(|source| {
            source.presentation == Presentation::Setup
                && matches!(source.pending, Some(SourceSave::Request { .. }))
        })
    }

    pub(super) fn source_visibility_changed(&mut self, cx: &mut Context<Self>) {
        let renew = self.source.as_ref().and_then(|source| {
            (self.activation.is_some() && !source.draft_ready && source.pending.is_none())
                .then_some((source.feedback, source.presentation))
        });
        if let Some((feedback, presentation)) = renew {
            self.begin_source_draft(feedback, presentation, cx);
        }
    }

    pub(super) fn open_sources(&mut self, cx: &mut Context<Self>) {
        self.begin_source_draft(None, Presentation::Sheet, cx);
        cx.notify();
    }

    /// Reserves a new draft session. Setup reads the saved locations first and
    /// opens the Activity draft with them, so a correction starts prefilled.
    pub(super) fn begin_source_draft(
        &mut self,
        feedback: Option<&'static str>,
        presentation: Presentation,
        cx: &mut Context<Self>,
    ) {
        if let Some(FieldRequest::Edit(generation, _)) =
            self.source.take().and_then(|previous| previous.request)
        {
            activity::close_text_editor(self.host, generation);
        }
        self.source_sequence += 1;
        let session = self.source_sequence;
        let (requested, seed) = match presentation {
            Presentation::Sheet => (
                activity::begin_source_draft(self.host, session, None),
                Seed::Off,
            ),
            Presentation::Setup => (self.read_source_seed(session, cx), Seed::Reading),
        };
        self.source = Some(SourceFlow {
            session,
            presentation,
            seed,
            focus: SourceFocus::Close,
            draft_ready: false,
            has_m3u: false,
            has_epg: false,
            request: None,
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
        if !requested && let Some(setup) = self.setup.as_mut() {
            setup.show_message(SetupMessage::DraftUnavailable);
        }
    }

    fn read_source_seed(&mut self, session: u64, cx: &mut Context<Self>) -> bool {
        let Some(read) = catalog::saved_source_locations() else {
            return false;
        };
        cx.spawn(async move |view, cx| {
            let saved = read.await.unwrap_or(Err(()));
            let _ = view.update(cx, |view, cx| view.source_seed_read(session, saved, cx));
        })
        .detach();
        true
    }

    fn source_seed_read(
        &mut self,
        session: u64,
        saved: Result<Option<SavedSourceLocations>, ()>,
        cx: &mut Context<Self>,
    ) {
        // A renewal or a newer draft supersedes this read.
        let Some(source) = self
            .source
            .as_mut()
            .filter(|source| source.session == session && source.seed == Seed::Reading)
        else {
            return;
        };
        let seed = saved.unwrap_or_else(|()| {
            if let Some(setup) = self.setup.as_mut() {
                setup.show_message(SetupMessage::SavedAddressesUnavailable);
            }
            None
        });
        source.seed = Seed::Sent {
            has_m3u: seed.is_some(),
            has_epg: seed.as_ref().is_some_and(SavedSourceLocations::has_epg),
        };
        if !activity::begin_source_draft(self.host, session, seed) {
            source.feedback = Some(SOURCE_OPEN_FAILED);
            if let Some(setup) = self.setup.as_mut() {
                setup.show_message(SetupMessage::DraftUnavailable);
            }
        }
        cx.notify();
    }

    pub(super) fn close_sources(&mut self, cx: &mut Context<Self>) {
        if let Some(source) = self.source.take() {
            if let Some(FieldRequest::Edit(generation, _)) = source.request {
                activity::close_text_editor(self.host, generation);
            }
            activity::close_source_draft(self.host, source.session);
            self.sync_view(cx);
        }
    }

    pub(super) fn sources_sheet(&self, window: &Window) -> Option<Div> {
        let source = self
            .source
            .as_ref()
            .filter(|source| source.presentation == Presentation::Sheet)?;
        let status = catalog::status();
        Some(guide_view::sources_sheet(
            window.viewport_size(),
            SourcePanel {
                backdrop: self.backdrop(),
                status: status.as_deref(),
                ambient: catalog::ambient_light(),
                focus: source.focus,
                editing: matches!(source.request, Some(FieldRequest::Edit(..))),
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

    /// Opens the Activity editor on one field. `false` means it could not open.
    pub(super) fn edit_source_field(&mut self, field: SetupField, rect: Option<[i32; 4]>) -> bool {
        let Some(source) = self
            .source
            .as_mut()
            .filter(|source| source.draft_ready && source.pending.is_none())
        else {
            return false;
        };
        self.editor_generation += 1;
        let generation = self.editor_generation;
        source.request = Some(FieldRequest::Edit(generation, field));
        let opened = rect.is_some_and(|rect| {
            activity::edit_source(
                self.host,
                source.session,
                generation,
                field_index(field),
                rect,
            )
        });
        if !opened {
            source.request = None;
        }
        opened
    }

    /// Asks the Activity to empty one field. Its field event confirms it.
    pub(super) fn clear_source_field(&mut self, field: SetupField) -> bool {
        let Some(source) = self
            .source
            .as_mut()
            .filter(|source| source.draft_ready && source.pending.is_none())
        else {
            return false;
        };
        self.editor_generation += 1;
        let generation = self.editor_generation;
        source.request = Some(FieldRequest::Clear(generation, field));
        let requested =
            activity::clear_source_field(self.host, source.session, generation, field_index(field));
        if !requested {
            source.request = None;
        }
        requested
    }

    /// Asks the Activity to hand over the draft for saving.
    pub(super) fn submit_source_draft(&mut self, include_epg: bool) -> bool {
        let Some(source) = self
            .source
            .as_mut()
            .filter(|source| source.draft_ready && source.pending.is_none())
        else {
            return false;
        };
        source.next_attempt += 1;
        let attempt = source.next_attempt;
        if activity::submit_sources(self.host, source.session, attempt) {
            source.pending = Some(SourceSave::Request {
                attempt,
                include_epg,
            });
            source.feedback = None;
            true
        } else {
            false
        }
    }

    fn step_backdrop(&mut self, previous: bool, cx: &Context<Self>) {
        use sparrow_installed_catalog::GuideBackdrop::*;
        let choice = match (self.backdrop(), previous) {
            (Flat, false) | (ChannelTint, true) => NordicGradient,
            (NordicGradient, false) | (AuroraDrift, true) => ChannelTint,
            (ChannelTint, false) | (Flat, true) => AuroraDrift,
            (AuroraDrift, false) | (NordicGradient, true) => Flat,
        };
        let Some(source) = self.source.as_mut() else {
            return;
        };
        source.feedback = None;
        let session = source.session;
        let Some(saved) = catalog::save_backdrop(choice) else {
            source.feedback = Some("Couldn’t save backdrop.");
            return;
        };
        let task = cx.spawn(async move |view, cx| {
            let success = saved.await.unwrap_or(false);
            let _ = view.update(cx, |view, cx| {
                view.pending_backdrop = None;
                if !success
                    && let Some(source) = view.source.as_mut().filter(|s| s.session == session)
                {
                    source.feedback = Some("Couldn’t save backdrop.");
                }
                cx.notify();
            });
        });
        self.pending_backdrop = Some((choice, task));
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
            ScreenKey::Arrow(direction @ (Direction::Left | Direction::Right))
                if source.focus == SourceFocus::Backdrop =>
            {
                self.step_backdrop(direction == Direction::Left, cx)
            }
            ScreenKey::Arrow(direction) => {
                source.focus = match (source.focus, direction) {
                    (SourceFocus::Close, Direction::Down) => SourceFocus::Backdrop,
                    (SourceFocus::Backdrop, Direction::Up) => SourceFocus::Close,
                    (SourceFocus::Backdrop, Direction::Down) if configured => SourceFocus::Refresh,
                    (SourceFocus::Backdrop | SourceFocus::Refresh, Direction::Down)
                        if status.is_some() =>
                    {
                        SourceFocus::Diagnostics
                    }
                    (SourceFocus::Backdrop | SourceFocus::Refresh, Direction::Down) => {
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
                    (SourceFocus::Save, Direction::Down) => SourceFocus::Ambient,
                    (SourceFocus::Ambient, Direction::Up) => SourceFocus::Save,
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
                    ) => SourceFocus::Backdrop,
                    (focus, _) => focus,
                };
            }
            ScreenKey::Activate => match source.focus {
                SourceFocus::Backdrop => self.step_backdrop(false, cx),
                SourceFocus::Close => self.close_sources(cx),
                SourceFocus::Ambient => {
                    let saved = catalog::toggle_ambient();
                    cx.spawn(async move |view, cx| {
                        let success = if let Some(saved) = saved {
                            saved.await.unwrap_or(false)
                        } else {
                            false
                        };
                        let _ = view.update(cx, |view, cx| {
                            if !success {
                                view.show_feedback(Some("Couldn’t save ambient light."), cx);
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                }
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
                focus @ (SourceFocus::M3u | SourceFocus::Epg)
                    if source.draft_ready && source.pending.is_none() =>
                {
                    let field = if focus == SourceFocus::Epg {
                        SetupField::Epg
                    } else {
                        SetupField::M3u
                    };
                    let rect = guide_view::source_editor_rect(
                        window.viewport_size(),
                        window.scale_factor(),
                    );
                    let opened = self.edit_source_field(field, rect);
                    if let Some(source) = self.source.as_mut() {
                        source.feedback = (!opened).then_some("Couldn’t open the keyboard.");
                    }
                }
                SourceFocus::Save if source.draft_ready && source.pending.is_none() => {
                    if source.has_m3u {
                        if !self.submit_source_draft(true)
                            && let Some(source) = self.source.as_mut()
                        {
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
        let Some(refresh) = catalog::refresh_now() else {
            source.feedback = Some("Couldn’t refresh sources.");
            return;
        };
        source.refreshing = true;
        source.feedback = None;
        let session = source.session;
        cx.spawn(async move |view, cx| {
            let feedback = refresh.await.map_or("Couldn’t refresh sources.", |report| {
                catalog::refresh_summary(&report)
            });
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
        let setup = self
            .setup
            .as_mut()
            .filter(|_| source.presentation == Presentation::Setup);
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
                if let Seed::Sent { has_m3u, has_epg } = source.seed {
                    source.has_m3u = has_m3u;
                    source.has_epg = has_epg;
                }
                if source.feedback == Some(SOURCE_OPEN_FAILED) {
                    source.feedback = None;
                }
                let draft = DraftView {
                    has_m3u: source.has_m3u,
                    has_epg: source.has_epg,
                    busy: false,
                };
                if let Some(setup) = setup {
                    setup.draft_opened(draft);
                }
            }
            SourceEvent::DraftStatus {
                attempt: 0,
                status: 1,
            } => {
                source.feedback = Some(SOURCE_OPEN_FAILED);
                if let Some(setup) = setup {
                    setup.show_message(SetupMessage::DraftUnavailable);
                }
            }
            SourceEvent::DraftStatus { attempt, status: 2 }
                if matches!(
                    source.pending,
                    Some(SourceSave::Request { attempt: pending, .. }) if pending == attempt
                ) =>
            {
                source.pending = None;
                source.feedback = Some("Couldn’t save sources.");
                if let Some(setup) = setup {
                    setup.show_message(SetupMessage::SaveFailed);
                }
            }
            SourceEvent::Field {
                generation,
                field,
                event,
                has_text,
            } => {
                let Some((request, field)) = source.request.zip(setup_field(field)) else {
                    return;
                };
                if !request.matches(generation, field) {
                    return;
                }
                source.request = None;
                if matches!(event, EDITOR_COMMITTED | FIELD_CLEARED) {
                    match field {
                        SetupField::M3u => source.has_m3u = has_text,
                        SetupField::Epg => source.has_epg = has_text,
                    }
                    source.feedback = None;
                } else if event == EDITOR_REJECTED {
                    source.feedback = Some("Couldn’t open the keyboard.");
                }
                let draft = DraftView {
                    has_m3u: source.has_m3u,
                    has_epg: source.has_epg,
                    busy: false,
                };
                if let Some(setup) = setup {
                    match (event, request) {
                        (EDITOR_COMMITTED, _) => setup.field_committed(field, draft),
                        (FIELD_CLEARED, _) => setup.field_cleared(field),
                        (EDITOR_REJECTED, FieldRequest::Edit(..)) => {
                            setup.show_message(SetupMessage::KeyboardUnavailable);
                        }
                        (EDITOR_REJECTED, FieldRequest::Clear(..)) => {
                            setup.show_message(SetupMessage::DraftUnavailable);
                        }
                        _ => setup.reconcile_focus(draft),
                    }
                }
            }
            SourceEvent::Submit { attempt, m3u, epg } => {
                let Some(SourceSave::Request {
                    attempt: pending,
                    include_epg,
                }) = source.pending
                else {
                    return;
                };
                if pending != attempt {
                    return;
                }
                source.pending = Some(SourceSave::Admitted(attempt));
                if let Some(setup) = setup {
                    setup.save_started();
                }
                let epg = epg.filter(|_| include_epg);
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
        result: Result<CatalogStatus, SourceSaveFailure>,
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
        if source.presentation == Presentation::Setup {
            if result.is_ok() {
                // The Activity closed the draft. Setup continues without one.
                self.source = None;
            }
            if let Some(setup) = self.setup.as_mut() {
                match &result {
                    Ok(status) => setup
                        .save_succeeded(CatalogProgress::from_status(status), status.generation()),
                    Err(failure) => setup.save_failed(match failure {
                        SourceSaveFailure::M3u => SetupMessage::InvalidM3u,
                        SourceSaveFailure::Epg => SetupMessage::InvalidEpg,
                        SourceSaveFailure::Unavailable => SetupMessage::SaveFailed,
                    }),
                }
            }
            self.sync_view(cx);
            return;
        }
        match result {
            Ok(_) if self.activation.is_some() => {
                self.begin_source_draft(Some("Sources saved."), Presentation::Sheet, cx);
            }
            Ok(_) => {
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
