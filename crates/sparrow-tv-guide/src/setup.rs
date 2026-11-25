//! First-launch Setup: the steps shown until the first Channel Catalog loads.
//!
//! The host owns the Source Configuration draft and reports only whether each
//! field holds an address. Setup never sees a location.

use sparrow_core::{
    CatalogGeneration, CatalogStatus, ChannelGroupView, SafeFailure, SourceAccessError,
    SourceReadError, SourceState,
};

use crate::{PageStatus, now_focus::Direction};

/// How far the catalog is from a usable Channel Catalog, as Setup sees it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogProgress {
    NotConfigured,
    Loading,
    Failed(FailureReason),
    Ready,
}

impl CatalogProgress {
    pub fn from_status(status: &CatalogStatus) -> Self {
        if status.generation().is_some() {
            return Self::Ready;
        }
        if !status.configuration().is_configured() {
            return Self::NotConfigured;
        }
        match status.m3u() {
            SourceState::Failed { failure, .. }
            | SourceState::Unavailable {
                failure: Some(failure),
            } => Self::Failed(FailureReason::from(failure)),
            _ => Self::Loading,
        }
    }
}

/// Why the first load of the M3U Source failed, in terms a viewer can act on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureReason {
    Unreachable,
    /// The download stopped part way, so the address is probably fine.
    Interrupted,
    Refused,
    TimedOut,
    NotChannelList,
    NoChannels,
    TooLarge,
    Storage,
}

impl From<&SafeFailure> for FailureReason {
    fn from(failure: &SafeFailure) -> Self {
        match failure {
            SafeFailure::SourceAccess { reason, .. } => match reason {
                SourceAccessError::Unavailable => Self::Unreachable,
                SourceAccessError::Rejected => Self::Refused,
                SourceAccessError::TimedOut => Self::TimedOut,
                SourceAccessError::InvalidResponse => Self::NotChannelList,
            },
            SafeFailure::SourceRead {
                reason: SourceReadError::Interrupted,
                ..
            } => Self::Interrupted,
            SafeFailure::SourceRead { .. }
            | SafeFailure::InvalidEncoding { .. }
            | SafeFailure::InvalidFormat { .. }
            | SafeFailure::InvalidEpgFormat { .. } => Self::NotChannelList,
            SafeFailure::NoPlayableChannels | SafeFailure::NoEpgChannels => Self::NoChannels,
            SafeFailure::DecodedLimitExceeded { .. } => Self::TooLarge,
            SafeFailure::Snapshot { .. } | SafeFailure::SnapshotRecovery { .. } => Self::Storage,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupStep {
    Channels,
    Guide,
    Loading,
    Failed(FailureReason),
    Found,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupField {
    M3u,
    Epg,
}

/// A focusable control on the current step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupControl {
    Field,
    Clear,
    Next,
    Skip,
    Load,
    TryAgain,
    ChangeSources,
    OpenGuide,
}

/// A one-line problem shown under the field or the actions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupMessage {
    InvalidM3u,
    InvalidEpg,
    SaveFailed,
    KeyboardUnavailable,
    SavedAddressesUnavailable,
    RetryUnavailable,
    DraftUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupKey {
    Arrow(Direction),
    Activate,
}

/// What the host must do after a key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupAction {
    None,
    Edit(SetupField),
    Clear(SetupField),
    Save { include_epg: bool },
    Retry,
    ChangeSources,
    OpenGuide,
    Exit,
}

/// The host's draft as Setup needs to see it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DraftView {
    pub has_m3u: bool,
    pub has_epg: bool,
    /// The draft cannot take edits or a save right now: it is opening, an
    /// editor is open, or a save is in flight.
    pub busy: bool,
}

impl DraftView {
    pub const fn has(self, field: SetupField) -> bool {
        match field {
            SetupField::M3u => self.has_m3u,
            SetupField::Epg => self.has_epg,
        }
    }
}

/// The catalog that greets the viewer once Setup finishes, frozen when Found opens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FoundSummary {
    Counted {
        channels: u64,
        groups: usize,
        /// The first groups in catalog order, with their Channel counts. An
        /// empty name is the group of Channels without a group title.
        first: Vec<(String, u32)>,
    },
    /// The group roster could not be read; the catalog is still usable.
    Uncounted,
}

pub const FOUND_GROUPS: usize = 4;

impl FoundSummary {
    pub fn from_roster(roster: &[ChannelGroupView]) -> Self {
        Self::Counted {
            channels: roster
                .iter()
                .map(|group| u64::from(group.channel_count()))
                .sum(),
            groups: roster.len(),
            first: roster
                .iter()
                .take(FOUND_GROUPS)
                .map(|group| (group.name().to_owned(), group.channel_count()))
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    /// No operation the catalog has yet to acknowledge.
    None,
    /// A save or retry has started, and the catalog has not yet shown it.
    /// A failure published before then belongs to the previous attempt.
    Started,
    /// The catalog has shown the operation, but the save has not reported back.
    /// The Guide stays closed until it does, because playback waits for it.
    Acknowledging,
}

/// Decides once per window whether Setup greets the viewer. A window that
/// has shown the Guide never falls back to Setup, even while a replacement
/// from Sources has no Channel Catalog yet.
#[derive(Default)]
pub struct SetupGate {
    decided: bool,
}

impl SetupGate {
    pub fn observe(&mut self, progress: CatalogProgress) -> Option<Setup> {
        if std::mem::replace(&mut self.decided, true) {
            return None;
        }
        Setup::start(progress)
    }
}

pub struct Setup {
    step: SetupStep,
    focus: SetupControl,
    message: Option<SetupMessage>,
    /// The Found summary and the catalog generation it counts.
    summary: Option<(Option<CatalogGeneration>, FoundSummary)>,
    /// The generation a successful save produced. Until the surface reports
    /// it, rosters of any other generation are older and never count.
    expected: Option<Option<CatalogGeneration>>,
    pending: Pending,
}

impl Setup {
    /// Setup for a window whose first known catalog has no Channel Catalog, or
    /// `None` when the Guide can open directly.
    pub fn start(progress: CatalogProgress) -> Option<Self> {
        let step = match progress {
            CatalogProgress::Ready => return None,
            CatalogProgress::NotConfigured => SetupStep::Channels,
            CatalogProgress::Loading => SetupStep::Loading,
            CatalogProgress::Failed(reason) => SetupStep::Failed(reason),
        };
        let mut setup = Self {
            step,
            focus: SetupControl::Field,
            message: None,
            summary: None,
            expected: None,
            pending: Pending::None,
        };
        setup.enter(step, DraftView::default());
        Some(setup)
    }

    pub const fn step(&self) -> SetupStep {
        self.step
    }

    pub const fn focus(&self) -> SetupControl {
        self.focus
    }

    pub const fn message(&self) -> Option<SetupMessage> {
        self.message
    }

    /// The summary on Found, or `None` while the current roster is loading.
    pub fn summary(&self) -> Option<&FoundSummary> {
        self.summary.as_ref().map(|(_, summary)| summary)
    }

    /// The catalog generation the Found summary counts.
    pub fn summary_generation(&self) -> Option<CatalogGeneration> {
        self.summary
            .as_ref()
            .and_then(|(generation, _)| *generation)
    }

    /// Whether Found can open the Guide. A save that is still finishing keeps
    /// playback fenced, so the Guide would not yet tune.
    pub fn guide_ready(&self) -> bool {
        self.step == SetupStep::Found && self.summary.is_some() && self.pending == Pending::None
    }

    /// The field on the current step, if it has one.
    pub const fn field(&self) -> Option<SetupField> {
        match self.step {
            SetupStep::Channels => Some(SetupField::M3u),
            SetupStep::Guide => Some(SetupField::Epg),
            _ => None,
        }
    }

    /// The controls on the current step, top row first, each row left to right.
    pub fn rows(&self, draft: DraftView) -> [&'static [SetupControl]; 2] {
        use SetupControl::*;
        let field: &'static [SetupControl] = match self.field() {
            Some(field) if draft.has(field) => &[Field, Clear],
            Some(_) => &[Field],
            None => &[],
        };
        let actions: &'static [SetupControl] = match self.step {
            SetupStep::Channels => &[Next],
            SetupStep::Guide => &[Skip, Load],
            SetupStep::Loading => &[],
            SetupStep::Failed(_) => &[TryAgain, ChangeSources],
            SetupStep::Found => &[OpenGuide],
        };
        [field, actions]
    }

    /// Moves to a step with its default focus and no message. Every step
    /// change goes through here, so a message never outlives its step.
    fn enter(&mut self, step: SetupStep, draft: DraftView) {
        self.step = step;
        self.message = None;
        if step != SetupStep::Found {
            self.summary = None;
        }
        self.focus = self.default_focus(draft);
    }

    fn default_focus(&self, draft: DraftView) -> SetupControl {
        match self.step {
            SetupStep::Channels if draft.has_m3u => SetupControl::Next,
            SetupStep::Channels | SetupStep::Loading => SetupControl::Field,
            SetupStep::Guide if draft.has_epg => SetupControl::Load,
            SetupStep::Guide => SetupControl::Skip,
            SetupStep::Failed(_) => SetupControl::TryAgain,
            SetupStep::Found => SetupControl::OpenGuide,
        }
    }

    /// Keeps focus on a control the step still shows after the draft changed.
    pub fn reconcile_focus(&mut self, draft: DraftView) {
        if !self.rows(draft).iter().any(|row| row.contains(&self.focus)) {
            self.focus = match self.focus {
                SetupControl::Clear => SetupControl::Field,
                _ => self.default_focus(draft),
            };
        }
    }

    pub fn press(&mut self, key: SetupKey, draft: DraftView) -> SetupAction {
        self.reconcile_focus(draft);
        match key {
            SetupKey::Arrow(direction) => {
                self.arrow(direction, draft);
                SetupAction::None
            }
            SetupKey::Activate => self.activate(draft),
        }
    }

    fn arrow(&mut self, direction: Direction, draft: DraftView) {
        let rows = self.rows(draft);
        let Some((row, column)) = rows.iter().enumerate().find_map(|(row, controls)| {
            controls
                .iter()
                .position(|control| *control == self.focus)
                .map(|column| (row, column))
        }) else {
            return;
        };
        let target = match direction {
            Direction::Left => column.checked_sub(1).map(|column| rows[row][column]),
            Direction::Right => rows[row].get(column + 1).copied(),
            Direction::Up if row == 1 && !rows[0].is_empty() => Some(SetupControl::Field),
            Direction::Down if row == 0 && !rows[1].is_empty() => {
                let preferred = self.default_focus(draft);
                Some(if rows[1].contains(&preferred) {
                    preferred
                } else {
                    rows[1][0]
                })
            }
            Direction::Up | Direction::Down => None,
        };
        if let Some(target) = target {
            self.focus = target;
        }
    }

    fn activate(&mut self, draft: DraftView) -> SetupAction {
        use SetupControl::*;
        let editable = !draft.busy;
        match (self.focus, self.field()) {
            (Field, Some(field)) if editable => SetupAction::Edit(field),
            (Clear, Some(field)) if editable => SetupAction::Clear(field),
            (Next, _) if draft.has_m3u => {
                self.enter(SetupStep::Guide, draft);
                SetupAction::None
            }
            (Next, _) if editable => SetupAction::Edit(SetupField::M3u),
            (Skip | Load, _) if editable => {
                self.message = None;
                SetupAction::Save {
                    include_epg: self.focus == Load,
                }
            }
            (TryAgain, _) => SetupAction::Retry,
            (ChangeSources, _) => SetupAction::ChangeSources,
            (OpenGuide, _) if self.guide_ready() => SetupAction::OpenGuide,
            _ => SetupAction::None,
        }
    }

    pub fn back(&mut self, draft: DraftView) -> SetupAction {
        match self.step {
            SetupStep::Guide => {
                self.enter(SetupStep::Channels, draft);
                SetupAction::None
            }
            SetupStep::Found if self.guide_ready() => SetupAction::OpenGuide,
            SetupStep::Found => SetupAction::None,
            _ => SetupAction::Exit,
        }
    }

    /// An editor closed. A committed address moves focus on to the next action.
    pub fn field_committed(&mut self, field: SetupField, draft: DraftView) {
        if self.field() != Some(field) {
            return;
        }
        self.message = None;
        self.focus = if draft.has(field) {
            self.default_focus(draft)
        } else {
            SetupControl::Field
        };
    }

    /// The host's draft opened, possibly prefilled. Focus moves to the step's
    /// next action when the field already holds an address.
    pub fn draft_opened(&mut self, draft: DraftView) {
        if self.field().is_some() {
            self.focus = self.default_focus(draft);
        }
        if self.message == Some(SetupMessage::DraftUnavailable) {
            self.message = None;
        }
    }

    /// A field was emptied, which also settles any problem with its address.
    pub fn field_cleared(&mut self, field: SetupField) {
        if self.field() == Some(field) {
            self.message = None;
            self.focus = SetupControl::Field;
        }
    }

    pub fn show_message(&mut self, message: SetupMessage) {
        self.message = Some(message);
    }

    /// The host handed the draft to the catalog. Loading starts immediately,
    /// because the save only reports back after the first refresh.
    pub fn save_started(&mut self) {
        self.enter(SetupStep::Loading, DraftView::default());
        self.pending = Pending::Started;
    }

    /// The save finished and its first refresh produced `generation`. A count
    /// taken from any other generation is retracted, and other rosters are
    /// ignored until the surface reports this one, because it may still report
    /// an older catalog before the replacement's publication arrives.
    pub fn save_succeeded(
        &mut self,
        progress: CatalogProgress,
        generation: Option<CatalogGeneration>,
    ) {
        if self.pending == Pending::None {
            return;
        }
        self.pending = Pending::None;
        self.summary.take_if(|(counted, _)| *counted != generation);
        if progress == CatalogProgress::Ready {
            self.expected = Some(generation);
        }
        self.catalog(progress);
    }

    /// The save was refused. The viewer returns to the step that can fix it.
    pub fn save_failed(&mut self, message: SetupMessage) {
        if self.pending == Pending::None {
            return;
        }
        self.pending = Pending::None;
        let step = if message == SetupMessage::InvalidM3u {
            SetupStep::Channels
        } else {
            SetupStep::Guide
        };
        self.enter(step, DraftView::default());
        self.focus = SetupControl::Field;
        self.message = Some(message);
    }

    /// A Try again refresh has started.
    pub fn retry_started(&mut self) {
        self.enter(SetupStep::Loading, DraftView::default());
        self.pending = Pending::Started;
    }

    /// The Try again refresh finished, so the current status is its result.
    pub fn retry_finished(&mut self, progress: CatalogProgress) {
        self.pending = Pending::None;
        self.catalog(progress);
    }

    /// Starts again from step 1 with the draft the host opened.
    pub fn change_sources(&mut self, draft: DraftView) {
        self.pending = Pending::None;
        self.enter(SetupStep::Channels, draft);
    }

    /// Applies a catalog publication. Steps 1 and 2 belong to the viewer, so
    /// only Loading, Failed and the start of Found follow the catalog.
    pub fn catalog(&mut self, progress: CatalogProgress) {
        if self.pending == Pending::Started {
            match progress {
                // Publications can coalesce, so a ready catalog also shows
                // that the operation has started.
                CatalogProgress::Loading | CatalogProgress::Ready => {
                    self.pending = Pending::Acknowledging;
                }
                // A result from before the operation started.
                CatalogProgress::Failed(_) | CatalogProgress::NotConfigured => return,
            }
        }
        let next = match (self.step, progress) {
            (SetupStep::Loading | SetupStep::Failed(_), CatalogProgress::Ready) => SetupStep::Found,
            // Found is only final once its catalog stays. A catalog that was
            // replaced, or an old one that recovered just before a save,
            // gives way to the replacement's own result.
            (
                SetupStep::Loading | SetupStep::Failed(_) | SetupStep::Found,
                CatalogProgress::Failed(reason),
            ) if self.pending == Pending::None => SetupStep::Failed(reason),
            (SetupStep::Failed(_) | SetupStep::Found, CatalogProgress::Loading) => {
                SetupStep::Loading
            }
            _ => return,
        };
        if next != self.step {
            self.enter(next, DraftView::default());
        }
    }

    /// Reports the unfiltered group roster of the current Channel Catalog.
    /// Found counts each generation once. A newer generation, such as the
    /// replacement after an old source recovered, drops the old count until
    /// its own roster has loaded, so the Guide never opens behind stale numbers.
    pub fn roster(
        &mut self,
        generation: Option<CatalogGeneration>,
        status: Option<PageStatus>,
        roster: &[ChannelGroupView],
    ) {
        if self.step != SetupStep::Found {
            return;
        }
        match self.expected {
            Some(expected) if expected != generation => return,
            Some(_) => self.expected = None,
            None => {}
        }
        if self
            .summary
            .as_ref()
            .is_some_and(|(counted, _)| *counted == generation)
        {
            return;
        }
        self.summary = match status {
            Some(PageStatus::Complete) => Some((generation, FoundSummary::from_roster(roster))),
            Some(PageStatus::Failed) => Some((generation, FoundSummary::Uncounted)),
            _ => None,
        };
    }
}
