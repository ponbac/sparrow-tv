use std::{collections::HashSet, sync::Mutex, time::Duration};

use chrono::{DateTime, FixedOffset, Local, Timelike, Utc};
use gpui::{
    AnyWindowHandle, App, Context, Div, FocusHandle, IntoElement, KeyDownEvent, Render,
    SharedString, Task, Window, WindowOptions, div, prelude::*, px, rgb,
};
use sparrow_core::{CatalogStatus, ChannelGroupView, CoreError, GuideWindowChannel, Page};
use sparrow_tv_guide::{
    GroupPageRequest, GuidePageRequest, PageStatus, PageUpdate,
    group_chooser::{GroupChooserFocus, GroupChooserIntent},
    guide_screen::{
        GROUP_PREFETCH_DELAY, GroupPrefetchTicket, GroupVisibilityChange, GuideMode, GuideRead,
        GuideScreen, LaneFocus, ScreenAction, ScreenFocus, ScreenKey, ToolbarFocus,
    },
    now_focus::{Direction, NowFocus},
    preview::{DescriptionPage, PreviewProgramme},
    search::{SearchDebounce, SearchView},
    setup::{Setup, SetupControl, SetupGate, SetupStep},
    timeline_focus::TimelineFocus,
};
use tokio::task::AbortHandle;

use crate::{
    activity, catalog,
    channel_icons::ChannelIcons,
    guide_view, playback,
    search_flow::{SearchEffect, SearchFlow},
};

mod playback_ui;
mod search_ui;
mod sections;
mod setup_ui;
mod source_flow;
use playback_ui::PlaybackUi;
use source_flow::SourceFlow;

const PREVIEW_DWELL: Duration = Duration::from_millis(300);
const FEEDBACK_DURATION: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy)]
pub(super) struct GuideDiagnostics {
    pub(super) mode: GuideMode,
    pub(super) focus: &'static str,
    pub(super) channel: Option<u32>,
    pub(super) group_index: Option<usize>,
    pub(super) chip_index: Option<usize>,
    pub(super) programme_index: Option<usize>,
    pub(super) pan_minutes: Option<i64>,
    pub(super) first_row: usize,
    pub(super) scroll_offset_units: u32,
    pub(super) loaded_families: usize,
    pub(super) page_status: Option<PageStatus>,
}

#[derive(Clone, Copy)]
pub(super) struct WorkflowDiagnostics {
    pub(super) search: SearchView,
    pub(super) channel_results: usize,
    pub(super) programme_results: usize,
    pub(super) hidden_groups: usize,
    pub(super) groups_open: bool,
    pub(super) sources_open: bool,
    pub(super) editor_open: bool,
    pub(super) setup: &'static str,
    pub(super) setup_focus: &'static str,
}

pub(super) static WORKFLOW_DIAGNOSTICS: Mutex<WorkflowDiagnostics> =
    Mutex::new(WorkflowDiagnostics {
        search: SearchView::Closed,
        channel_results: 0,
        programme_results: 0,
        hidden_groups: 0,
        groups_open: false,
        sources_open: false,
        editor_open: false,
        setup: "None",
        setup_focus: "None",
    });

pub(super) static GUIDE_DIAGNOSTICS: Mutex<GuideDiagnostics> = Mutex::new(GuideDiagnostics {
    mode: GuideMode::Now,
    focus: "None",
    channel: None,
    group_index: None,
    chip_index: None,
    programme_index: None,
    pan_minutes: None,
    first_row: 0,
    scroll_offset_units: 0,
    loaded_families: 0,
    page_status: None,
});

pub(super) struct GuideSurface {
    host: u64,
    sections: sections::Sections,
    screen: GuideScreen,
    focus: FocusHandle,
    group_slide: guide_view::GroupSlide,
    arrival: guide_view::arrival::Arrival,
    now_lane: guide_view::LaneViewport,
    timeline_rail: guide_view::RailViewport,
    group_viewport: guide_view::GroupViewport,
    loaded_exclusions: bool,
    pending_exclusions: Option<u64>,
    pending_ambient: Option<(bool, Task<()>)>,
    pending_backdrop: Option<(sparrow_installed_catalog::GuideBackdrop, Task<()>)>,
    tint_dwell: Option<Task<()>>,
    backdrop_motion: guide_view::BackdropMotion,
    editor_generation: u64,
    active_editor: Option<(u64, TextTarget)>,
    search: SearchFlow,
    search_viewport: guide_view::SearchViewport,
    search_dwell: Option<Task<()>>,
    source: Option<SourceFlow>,
    source_sequence: u64,
    setup: Option<Setup>,
    setup_gate: SetupGate,
    now: DateTime<FixedOffset>,
    description: Option<SharedString>,
    description_programme: Option<PreviewProgramme>,
    desired_preview: Option<PreviewTarget>,
    preview_revision: u64,
    presented_preview: Option<guide_view::PreviewSelection>,
    preview_frame: PreviewFrame,
    group_ticket: Option<GroupPrefetchTicket>,
    activation: Option<u64>,
    status_revision: Option<u64>,
    feedback: Option<&'static str>,
    group_dwell: Option<Task<()>>,
    preview_dwell: Option<Task<()>>,
    feedback_timeout: Option<Task<()>>,
    clock: Option<Task<()>>,
    minute_tick: Option<guide_view::MinuteTick>,
    _catalog_updates: Task<()>,
    _group_writes: Task<()>,
    icons: ChannelIcons,
    _icon_updates: Task<()>,
    playback: PlaybackUi,
    _playback_updates: Option<Task<()>>,
    _variant_updates: Task<()>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreviewFrame {
    Idle,
    WaitingForPaint,
    Scheduled,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TextTarget {
    GroupFilter,
    Search,
}

#[derive(PartialEq, Eq)]
struct PreviewTarget {
    number: u32,
    programme: Option<PreviewProgramme>,
}

enum ReadResult {
    Groups(GroupPageRequest, Result<Page<ChannelGroupView>, CoreError>),
    Guide(
        GuidePageRequest,
        Result<Page<GuideWindowChannel>, CoreError>,
    ),
}

impl GuideSurface {
    pub(super) fn new(host: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // open_window lays out the root before the platform presents it. Keep
        // the Activity cover through that first presentation.
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |_, _| activity::guide_presented(host));
        });
        let now = Local::now().fixed_offset();
        let mut screen = GuideScreen::new(now.to_utc(), *now.offset(), HashSet::new());
        screen.set_surface_height(guide_view::surface_height(window.viewport_size()));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        let mut updates = catalog::subscribe();
        let catalog_updates = cx.spawn(async move |view, cx| {
            loop {
                let (activation, phase, revision, status) = {
                    let publication = updates.borrow_and_update();
                    (
                        publication.activation(host),
                        publication.phase(),
                        publication.status_revision,
                        publication.status.clone(),
                    )
                };
                if view
                    .update(cx, |view, cx| {
                        view.catalog_update(activation, phase, revision, status.as_deref(), cx);
                    })
                    .is_err()
                    || updates.changed().await.is_err()
                {
                    break;
                }
            }
        });
        let mut writes = catalog::subscribe_group_writes();
        let group_writes = cx.spawn(async move |view, cx| {
            while writes.changed().await.is_ok() {
                let revision = *writes.borrow_and_update();
                if view
                    .update(cx, |view, cx| view.reconcile_group_writes(revision, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let (icons, mut icon_updates) = ChannelIcons::new();
        let icon_updates = cx.spawn(async move |view, cx| {
            while let Some(batch) = icon_updates.next_batch(cx.background_executor()).await {
                if view
                    .update(cx, |view, cx| {
                        if view.icons.apply(batch, cx) {
                            view.sections.icons_changed();
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut variants = catalog::variant_preferences();
        let variant_updates = cx.spawn(async move |view, cx| {
            loop {
                let preferences = variants.borrow_and_update().clone();
                if view
                    .update(cx, |view, cx| {
                        view.sections.catalog_changed();
                        view.screen.set_variant_preferences(preferences);
                        view.sync_search_preferences(cx);
                        view.sync_view(cx);
                    })
                    .is_err()
                    || variants.changed().await.is_err()
                {
                    break;
                }
            }
        });
        Self {
            host,
            sections: sections::Sections::new(cx),
            screen,
            focus,
            group_slide: Default::default(),
            arrival: Default::default(),
            now_lane: guide_view::LaneViewport::default(),
            timeline_rail: guide_view::RailViewport::default(),
            group_viewport: guide_view::GroupViewport::default(),
            loaded_exclusions: false,
            pending_exclusions: None,
            pending_ambient: None,
            pending_backdrop: None,
            tint_dwell: None,
            backdrop_motion: Default::default(),
            editor_generation: 0,
            active_editor: None,
            search: SearchFlow::new(None),
            search_viewport: guide_view::SearchViewport::default(),
            search_dwell: None,
            source: None,
            source_sequence: 0,
            setup: None,
            setup_gate: SetupGate::default(),
            now,
            description: None,
            description_programme: None,
            desired_preview: None,
            preview_revision: 0,
            presented_preview: None,
            preview_frame: PreviewFrame::Idle,
            group_ticket: None,
            activation: None,
            status_revision: None,
            feedback: None,
            group_dwell: None,
            preview_dwell: None,
            feedback_timeout: None,
            clock: None,
            minute_tick: None,
            _catalog_updates: catalog_updates,
            _group_writes: group_writes,
            icons,
            _icon_updates: icon_updates,
            playback: PlaybackUi::default(),
            _playback_updates: None,
            _variant_updates: variant_updates,
        }
    }

    fn catalog_update(
        &mut self,
        activation: Option<u64>,
        phase: catalog::Phase,
        revision: u64,
        status: Option<&CatalogStatus>,
        cx: &mut Context<Self>,
    ) {
        let resumed = activation.is_some() && self.activation != activation;
        if self.activation != activation {
            self.activation = activation;
            self.clock = None;
            self.minute_tick = None;
            self.group_dwell = None;
            self.group_ticket = None;
            self.retire_preview_read();
            self.preview_frame = PreviewFrame::Idle;
            self.feedback = None;
            self.feedback_timeout = None;
            self.retire_query_editor();
            self.icons.hide();
            self.source_visibility_changed(cx);
        }

        if phase == catalog::Phase::StartupFailed {
            self.screen.catalog_open_failed();
            cx.notify();
            return;
        }
        let Some(activation) = activation else {
            return;
        };
        if resumed {
            self.observe_playback(cx);
            self.set_clock(Local::now().fixed_offset());
            let mut clock = self.now;
            self.clock = Some(cx.spawn(async move |view, cx| {
                loop {
                    let until_minute = Duration::from_secs(u64::from(60 - clock.second()))
                        .saturating_sub(Duration::from_nanos(u64::from(clock.nanosecond())));
                    cx.background_executor().timer(until_minute).await;
                    clock = Local::now().fixed_offset();
                    if view
                        .update(cx, |view, cx| {
                            if view.activation == Some(activation) {
                                view.minute_boundary(clock);
                                view.refresh_playback_subject(cx);
                                view.sync_view(cx);
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        if let Some(status) = status
            && (resumed || self.status_revision != Some(revision))
        {
            if !self.loaded_exclusions
                && let Some(exclusions) = catalog::group_exclusions()
            {
                self.loaded_exclusions = true;
                self.screen.set_group_exclusions(exclusions);
            }
            self.sections.catalog_changed();
            self.status_revision = Some(revision);
            self.screen.catalog_status(status);
            self.setup_catalog(status, cx);
            self.icons.set_generation(self.screen.generation());
            self.refresh_playback_subject(cx);
            let timer = self.search.set_generation(self.screen.generation());
            self.schedule_search(timer, cx);
            self.sync_search_preferences(cx);
            self.sync_view(cx);
        }
    }

    fn arm_badge_timer(&mut self, cx: &Context<Self>) {
        if let Some(tick) = &mut self.minute_tick
            && tick.needs_badge_timer()
        {
            let fade = tick.badge_fade_at();
            let end = tick.expires_at();
            tick.badge_expiry = Some(cx.spawn(async move |view, cx| {
                for deadline in [fade, end] {
                    while let Some(delay) =
                        deadline.checked_duration_since(std::time::Instant::now())
                    {
                        cx.background_executor().timer(delay).await;
                    }
                    let _ = view.update(cx, |view, cx| {
                        if deadline == end {
                            view.minute_tick = None;
                        }
                        cx.notify();
                    });
                }
            }));
        }
    }

    fn minute_boundary(&mut self, clock: DateTime<FixedOffset>) {
        let previous = self.now;
        let rolled = self.set_clock(clock);
        self.minute_tick = None;
        if self.setup.is_some()
            || self.source.is_some()
            || self.screen.group_chooser().is_some()
            || self.search.state().view() == SearchView::Desk
            || (self.playback.fullscreen() && !self.playback.controls_visible())
            || clock <= previous
            || clock - previous > chrono::TimeDelta::seconds(90)
            || previous.offset() != clock.offset()
            || guide_view::clock_time(previous.to_utc()) == guide_view::clock_time(clock.to_utc())
        {
            return;
        }
        let decorate = !self.playback.fullscreen() && self.screen.mode() == GuideMode::Now;
        // The pager retains these rows until the new half-hour query completes.
        let prior_rows = if rolled && decorate {
            self.screen.families()[self.screen.visible_families(0)].to_vec()
        } else {
            Vec::new()
        };
        self.minute_tick = Some(guide_view::MinuteTick::new(
            previous, rolled, decorate, prior_rows,
        ));
    }

    fn set_clock(&mut self, clock: DateTime<FixedOffset>) -> bool {
        if let Some(preview) = &self.presented_preview
            && self.description.is_some()
        {
            let selected = |clock| preview.programme(clock).map(|(_, programme)| programme);
            // The displayed Channel's live-or-next slot follows the clock even
            // if a focus change is still waiting for the next frame.
            if selected(self.now) != selected(clock) {
                self.description = None;
                self.description_programme = None;
            }
        }
        self.now = clock;
        self.screen.set_clock(clock.to_utc(), *clock.offset())
    }

    fn observe_playback(&mut self, cx: &mut Context<Self>) {
        if self._playback_updates.is_some() {
            return;
        }
        let Some(playback) = playback::get() else {
            return;
        };
        let mut updates = playback.runner.subscribe();
        self._playback_updates = Some(cx.spawn(async move |view, cx| {
            loop {
                let snapshot = updates.borrow_and_update().clone();
                if view
                    .update(cx, |view, cx| view.playback_update(snapshot, cx))
                    .is_err()
                    || updates.changed().await.is_err()
                {
                    break;
                }
            }
        }));
    }

    fn cancel_arrival(&mut self, cx: &mut Context<Self>) {
        if self.arrival.cancel() {
            cx.notify();
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_arrival(cx);
        let key = match event.keystroke.key.as_str() {
            "up" => ScreenKey::Arrow(Direction::Up),
            "down" => ScreenKey::Arrow(Direction::Down),
            "left" => ScreenKey::Arrow(Direction::Left),
            "right" => ScreenKey::Arrow(Direction::Right),
            "enter" => ScreenKey::Activate,
            _ => return,
        };
        let modifiers = event.keystroke.modifiers;
        if matches!(key, ScreenKey::Arrow(_))
            && (modifiers.control || modifiers.alt || modifiers.platform)
        {
            return;
        }
        if key != ScreenKey::Activate || !event.is_held {
            self.press(key, event.is_held, window, cx);
        }
    }

    pub(super) fn activity_event(
        &mut self,
        event: activity::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, activity::Event::Back | activity::Event::Key { .. }) {
            self.cancel_arrival(cx);
        }
        match event {
            activity::Event::PictureBottom(bottom) => {
                self.playback.picture_bottom = bottom;
                if self.playback.fullscreen() {
                    cx.notify();
                }
            }
            activity::Event::Presented => {
                self.arrival.presented = true;
                cx.notify();
            }
            activity::Event::Source { session, update } => self.source_event(session, update, cx),
            activity::Event::Back if self.activation.is_none() => activity::finish(self.host),
            _ if self.activation.is_none() => {}
            activity::Event::Back if self.setup.is_some() => {
                // Android can pass Back on before it mounts a requested editor,
                // or before it hands over a submitted draft. Acting on it then
                // would strand the draft.
                if !self.setup_settling() {
                    self.setup_back(cx);
                }
            }
            // Setup owns the remote: media, channel and held keys stop here.
            activity::Event::Key { code, repeat } if self.setup.is_some() => {
                if let Some(key) = remote_key(code, repeat) {
                    self.press(key, repeat > 0, window, cx);
                }
            }
            activity::Event::Ambient { epoch, colors } => self.ambient_sample(epoch, colors, cx),
            activity::Event::PlaybackDiagnosticsCopied { attempt, copied } => {
                self.playback_diagnostics_copied(attempt, copied, cx);
            }
            activity::Event::Back => {
                if self.source.is_some() {
                    self.close_sources(cx);
                } else if self.search.state().view() != SearchView::Closed {
                    let effect = self.search.back();
                    self.search_effect(effect, Some((window, false)), cx);
                } else if self.screen.group_chooser().is_some() {
                    self.screen.group_chooser_intent(GroupChooserIntent::Close);
                    self.retire_query_editor();
                    self.sync_view(cx);
                } else if !self.playback_back(cx) {
                    activity::finish(self.host);
                }
            }
            activity::Event::Key { code, repeat } => {
                if self.media_key(code, repeat, cx) {
                    return;
                }
                if let Some(key) = remote_key(code, repeat) {
                    self.press(key, repeat > 0, window, cx);
                } else if matches!(code, 23 | 66)
                    && self.source.is_none()
                    && self.screen.group_chooser().is_none()
                    && self.search.state().view() == SearchView::Closed
                    && !self.playback.fullscreen()
                {
                    self.guide_tune(true, cx);
                }
            }
            activity::Event::Edited { generation, edit } => {
                let Some((current, target)) = self.active_editor else {
                    return;
                };
                if current != generation {
                    return;
                }
                self.minute_tick = None;
                self.icons.note_navigation();
                if !matches!(edit, activity::Edit::Changed(_)) {
                    self.active_editor = None;
                }
                match edit {
                    activity::Edit::Changed(text) | activity::Edit::Committed(text) => match target
                    {
                        TextTarget::GroupFilter => {
                            self.screen
                                .group_chooser_intent(GroupChooserIntent::Filter(text));
                            self.sync_view(cx);
                        }
                        TextTarget::Search => {
                            let timer = self.search.edit(text);
                            self.schedule_search(timer, cx);
                        }
                    },
                    activity::Edit::Rejected => {
                        self.show_feedback(Some("Couldn’t open the keyboard."), cx);
                    }
                    activity::Edit::Cancelled => {}
                }
                cx.notify();
            }
        }
    }

    fn press(&mut self, key: ScreenKey, repeat: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.activation.is_none() || self.active_editor.is_some() || self.source_editor_open() {
            return;
        }
        self.minute_tick = None;
        self.icons.note_navigation();
        if self.setup.is_some() {
            self.setup_key(key, window, cx);
            return;
        }
        if self.source.is_some() {
            self.source_key(key, window, cx);
            return;
        }
        if self.search.state().view() != SearchView::Closed {
            let effect = self.search.key(key);
            self.search_effect(effect, Some((window, repeat)), cx);
            return;
        }
        if self.screen.group_chooser().is_none() && self.player_key(key, repeat, cx) {
            return;
        }
        if key == ScreenKey::Activate
            && self.screen.group_chooser().is_none()
            && self.screen.focus() == ScreenFocus::Toolbar(ToolbarFocus::Search)
        {
            let timer = self.search.open_inline();
            self.schedule_search(timer, cx);
            self.search_effect(SearchEffect::Edit, Some((window, repeat)), cx);
            return;
        }
        if key == ScreenKey::Activate
            && self.screen.group_chooser().is_none()
            && self.screen.focus() == ScreenFocus::Toolbar(ToolbarFocus::Sources)
        {
            self.open_sources(cx);
            return;
        }
        let now = Local::now().fixed_offset();
        if matches!(key, ScreenKey::Arrow(_)) && self.tint_dwell.is_some() {
            self.defer_tint(cx);
        }
        let previous_group = self.screen.selected_group().cloned();
        let previous = (self.screen.mode(), self.screen.selected_lane());
        let action = self.screen.press(key, now.to_utc(), *now.offset());
        let group_changed = previous_group.as_ref() != self.screen.selected_group();
        if group_changed {
            self.presented_preview = None;
            self.preview_frame = PreviewFrame::Idle;
        }
        self.group_slide.press(
            previous,
            (self.screen.mode(), self.screen.selected_lane()),
            group_changed,
            repeat,
        );
        if self.screen.mode() != previous.0 {
            self.now_lane = guide_view::LaneViewport::default();
            self.timeline_rail = guide_view::RailViewport::default();
        }
        let feedback = match action {
            ScreenAction::Unavailable
                if key == ScreenKey::Activate && self.guide_tune(false, cx) =>
            {
                self.sync_view(cx);
                return;
            }
            ScreenAction::Unavailable => Some("This action is unavailable."),
            ScreenAction::None => None,
            ScreenAction::GroupVisibilityChanged(change) => self.save_group_visibility(change, cx),
            ScreenAction::EditGroupFilter => {
                (!self.open_group_filter(window)).then_some("Couldn’t open the keyboard.")
            }
        };
        self.show_feedback(feedback, cx);
        self.sync_view(cx);
    }

    fn open_group_filter(&mut self, window: &Window) -> bool {
        let rect = guide_view::group_filter_rect(
            window.viewport_size(),
            window.scale_factor(),
            self.screen.group_chooser_can_show_all(),
        );
        self.open_query_editor(TextTarget::GroupFilter, rect)
    }

    fn open_query_editor(&mut self, target: TextTarget, rect: [i32; 4]) -> bool {
        if self.active_editor.is_some() {
            return true;
        }
        let text = match target {
            TextTarget::GroupFilter => {
                let Some(chooser) = self.screen.group_chooser() else {
                    return false;
                };
                chooser.filter()
            }
            TextTarget::Search => self.search.term(),
        };
        self.editor_generation += 1;
        let generation = self.editor_generation;
        // Keep the first generation until its terminal event, even before the
        // posted request reaches Android's main thread.
        self.active_editor = Some((generation, target));
        if activity::open_text_query(self.host, generation, rect, text) {
            true
        } else {
            self.active_editor = None;
            false
        }
    }

    fn retire_query_editor(&mut self) {
        if let Some((generation, _)) = self.active_editor.take() {
            activity::close_text_editor(self.host, generation);
        }
    }

    fn reconcile_group_writes(&mut self, revision: u64, cx: &mut Context<Self>) {
        if self
            .pending_exclusions
            .is_some_and(|pending| pending > revision)
        {
            return;
        }
        self.pending_exclusions = None;
        if let Some(exclusions) = catalog::group_exclusions() {
            self.screen.set_group_exclusions(exclusions);
            self.refresh_playback_subject(cx);
        }
        self.sync_search_preferences(cx);
        self.sync_view(cx);
    }

    fn save_group_visibility(
        &mut self,
        change: GroupVisibilityChange,
        cx: &mut Context<Self>,
    ) -> Option<&'static str> {
        use sparrow_installed_catalog::GuidePreferenceChange;
        let update = match change {
            GroupVisibilityChange::Set { name, hidden } => {
                GuidePreferenceChange::SetGroupVisibility { name, hidden }
            }
            GroupVisibilityChange::ShowAll => GuidePreferenceChange::ShowAllGroups,
        };
        if let Some((sequence, saved)) = catalog::save_group_visibility(update) {
            self.pending_exclusions = Some(sequence);
            cx.spawn(async move |view, cx| {
                if !saved.await.unwrap_or(false) {
                    let _ = view.update(cx, |view, cx| {
                        view.show_feedback(Some("Couldn’t save channel groups."), cx);
                        cx.notify();
                    });
                }
            })
            .detach();
            return None;
        }
        if let Some(exclusions) = catalog::group_exclusions() {
            self.screen.set_group_exclusions(exclusions);
            self.refresh_playback_subject(cx);
        }
        Some("Couldn’t save channel groups.")
    }

    fn show_feedback(&mut self, feedback: Option<&'static str>, cx: &mut Context<Self>) {
        self.feedback = feedback;
        self.feedback_timeout = self.feedback.map(|_| {
            cx.spawn(async |view, cx| {
                cx.background_executor().timer(FEEDBACK_DURATION).await;
                let _ = view.update(cx, |view, cx| {
                    view.feedback = None;
                    cx.notify();
                });
            })
        });
    }

    fn sync_view(&mut self, cx: &mut Context<Self>) {
        let Some(activation) = self.activation else {
            // In-flight reads complete into the reducer while hidden. Retain any
            // newly queued requests so their pager tickets remain valid on resume.
            return;
        };
        self.setup_roster();
        self.dispatch_reads(cx);
        let next_ticket = self.screen.prefetch_ticket();
        if next_ticket != self.group_ticket {
            self.group_ticket = next_ticket;
            self.group_dwell = next_ticket.map(|ticket| {
                cx.spawn(async move |view, cx| {
                    cx.background_executor().timer(GROUP_PREFETCH_DELAY).await;
                    let _ = view.update(cx, |view, cx| {
                        if view.activation == Some(activation) {
                            view.screen.prefetch(ticket);
                            view.dispatch_reads(cx);
                        }
                    });
                })
            });
        }
        self.sync_preview(cx);
        cx.notify();
    }

    fn dispatch_reads(&mut self, cx: &Context<Self>) {
        for read in self.screen.take_reads() {
            let task = match read {
                GuideRead::Status => {
                    catalog::reconcile_status();
                    continue;
                }
                GuideRead::Groups(request) => catalog::read(move |core| {
                    let result = core.list_groups(request.query().clone());
                    ReadResult::Groups(request, result)
                }),
                GuideRead::Guide(request) => catalog::read(move |core| {
                    let result = core.guide_window(request.query().clone());
                    ReadResult::Guide(request, result)
                }),
            };
            let Some(task) = task else {
                continue;
            };
            cx.spawn(async move |view, cx| {
                if let Ok(result) = task.await {
                    let _ = view.update(cx, |view, cx| view.apply_read(result, cx));
                }
            })
            .detach();
        }
    }

    fn apply_read(&mut self, result: ReadResult, cx: &mut Context<Self>) {
        let update = match result {
            ReadResult::Groups(request, result) => self.screen.groups_completed(request, result),
            ReadResult::Guide(request, result) => self.screen.guide_completed(request, result),
        };
        if update != PageUpdate::Superseded {
            self.sections.catalog_changed();
            self.sync_view(cx);
        }
    }

    fn sync_preview(&mut self, cx: &Context<Self>) {
        let selection = guide_view::PreviewSelection::from_screen(&self.screen);
        // The same Programme can move to a different cell index after an
        // updated Guide window. Keep the existing lookup if its identity stays.
        let desired = selection.as_ref().map(|selection| {
            let programme = self.screen.generation().and_then(|generation| {
                let (row, programme) = selection.programme(self.now)?;
                PreviewProgramme::for_row(generation, row, programme)
            });
            PreviewTarget {
                number: selection.family.number(),
                programme,
            }
        });
        if self.desired_preview != desired {
            self.defer_tint(cx);
            self.desired_preview = desired;
            self.retire_preview_read();
            if self.presented_preview.is_some() {
                self.preview_frame = PreviewFrame::WaitingForPaint;
            }
        }
        if self.preview_frame == PreviewFrame::Idle {
            self.publish_preview(selection, cx);
        }
    }

    fn retire_preview_read(&mut self) {
        self.preview_revision = self.preview_revision.wrapping_add(1);
        self.preview_dwell = None;
        // Keep an already displayed description paired with its old band until
        // publication. An interrupted lookup can restart if focus returns.
        if self.description.is_none() {
            self.description_programme = None;
        }
    }

    fn schedule_preview_frame(&mut self, window: &Window, cx: &Context<Self>) {
        if self.preview_frame != PreviewFrame::WaitingForPaint {
            return;
        }
        self.preview_frame = PreviewFrame::Scheduled;
        let activation = self.activation;
        let revision = self.preview_revision;
        let view = cx.weak_entity();
        // GPUI runs the callback before drawing its next frame. Registering
        // during render lets the current focus update paint first.
        window.on_next_frame(move |_, cx| {
            let _ = view.update(cx, |view, cx| {
                if view.activation == activation
                    && view.preview_revision == revision
                    && view.preview_frame == PreviewFrame::Scheduled
                {
                    view.preview_frame = PreviewFrame::Idle;
                    let selection = guide_view::PreviewSelection::from_screen(&view.screen);
                    view.publish_preview(selection, cx);
                    cx.notify();
                }
            });
        });
    }

    fn publish_preview(
        &mut self,
        selection: Option<guide_view::PreviewSelection>,
        cx: &Context<Self>,
    ) {
        self.sections.preview_changed();
        self.presented_preview = selection;
        let key = self
            .desired_preview
            .as_ref()
            .and_then(|target| target.programme.clone());
        if self.description_programme == key {
            return;
        }
        self.description_programme.clone_from(&key);
        self.description = None;
        let window_start = self.screen.displayed_window().starts_at();
        let activation = self.activation;
        let revision = self.preview_revision;
        self.preview_dwell = key.map(|key| {
            cx.spawn(async move |view, cx| {
                cx.background_executor().timer(PREVIEW_DWELL).await;
                let current = |view: &Self| {
                    view.activation == activation
                        && view.preview_revision == revision
                        && view.description_programme.as_ref() == Some(&key)
                };
                let description = read_description(&key, window_start, || {
                    view.read_with(cx, |view, _| current(view)).unwrap_or(false)
                })
                .await;
                let _ = view.update(cx, |view, cx| {
                    if current(view) {
                        view.description = description;
                        cx.notify();
                    }
                });
            })
        });
    }
}

struct CancelScheduleRead(AbortHandle);

impl Drop for CancelScheduleRead {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn read_description(
    key: &PreviewProgramme,
    window_start: DateTime<Utc>,
    mut current: impl FnMut() -> bool,
) -> Option<SharedString> {
    let mut lookup = key.description(window_start);
    while current() {
        let query = lookup.query();
        let read = catalog::read(move |core| core.schedule(query))?;
        // A queued blocking read can be aborted. If it already started, it
        // finishes one page, which cannot publish or schedule a continuation.
        let _cancel = CancelScheduleRead(read.abort_handle());
        let page = read.await.ok()?.ok()?;
        match lookup.accept(&page) {
            DescriptionPage::Finished(description) => return description.map(SharedString::from),
            DescriptionPage::Next(next) => lookup = next,
        }
    }
    None
}

/// D-pad arrows, and a fresh Center or Enter press. A held Center is not one.
const fn remote_key(code: i32, repeat: i32) -> Option<ScreenKey> {
    Some(match code {
        19 => ScreenKey::Arrow(Direction::Up),
        20 => ScreenKey::Arrow(Direction::Down),
        21 => ScreenKey::Arrow(Direction::Left),
        22 => ScreenKey::Arrow(Direction::Right),
        23 | 66 if repeat == 0 => ScreenKey::Activate,
        _ => return None,
    })
}

const fn step_name(step: SetupStep) -> &'static str {
    match step {
        SetupStep::Channels => "Channels",
        SetupStep::Guide => "Guide",
        SetupStep::Loading => "Loading",
        SetupStep::Failed(_) => "Failed",
        SetupStep::Found => "Found",
    }
}

const fn control_name(control: SetupControl) -> &'static str {
    match control {
        SetupControl::Field => "Field",
        SetupControl::Clear => "Clear",
        SetupControl::Next => "Next",
        SetupControl::Skip => "Skip",
        SetupControl::Load => "Load",
        SetupControl::TryAgain => "TryAgain",
        SetupControl::ChangeSources => "ChangeSources",
        SetupControl::OpenGuide => "OpenGuide",
    }
}

fn publish_diagnostics(screen: &GuideScreen, focused_dock: Option<u32>) {
    let chooser_focus = screen.group_chooser().map(|chooser| match chooser.focus() {
        GroupChooserFocus::Close => ("GroupClose", None),
        GroupChooserFocus::Filter => ("GroupFilter", None),
        GroupChooserFocus::ShowAll => ("GroupShowAll", None),
        GroupChooserFocus::Pick(row) => ("GroupPick", Some(row)),
        GroupChooserFocus::Toggle(row) => ("GroupToggle", Some(row)),
    });
    let (focus, family, group_index, chip_index, programme_index) = match screen.focus() {
        ScreenFocus::None => ("None", None, None, None, None),
        ScreenFocus::Toolbar(ToolbarFocus::Search) => ("Search", None, None, None, None),
        ScreenFocus::Toolbar(ToolbarFocus::Sources) => ("Sources", None, None, None, None),
        ScreenFocus::GroupLane(LaneFocus::Phase) => ("Phase", None, None, None, None),
        ScreenFocus::GroupLane(LaneFocus::All) => ("All", None, None, None, None),
        ScreenFocus::GroupLane(LaneFocus::Group(index)) => ("Group", None, Some(index), None, None),
        ScreenFocus::GroupLane(LaneFocus::ChooseGroups) => ("ChooseGroups", None, None, None, None),
        ScreenFocus::Board(NowFocus::Channel(family))
        | ScreenFocus::TimelineBoard(TimelineFocus::Channel(family)) => {
            ("Channel", Some(family), None, None, None)
        }
        ScreenFocus::Board(NowFocus::Variant { family, chip })
        | ScreenFocus::TimelineBoard(TimelineFocus::Variant { family, chip }) => {
            ("Quality", Some(family), None, Some(chip), None)
        }
        ScreenFocus::TimelineBoard(TimelineFocus::Programme { family, cell }) => {
            ("Programme", Some(family), None, None, Some(cell))
        }
        ScreenFocus::Board(NowFocus::Retry) | ScreenFocus::TimelineBoard(TimelineFocus::Retry) => {
            ("Retry", None, None, None, None)
        }
    };
    let channel = family
        .and_then(|index| screen.families().get(index))
        .map(|row| row.number());
    let focused_control = focused_dock.map_or((focus, channel, group_index), |number| {
        ("Dock", Some(number), None)
    });
    let (focus, channel, group_index) =
        chooser_focus.map_or(focused_control, |(focus, row)| (focus, None, row));
    *GUIDE_DIAGNOSTICS.lock().unwrap() = GuideDiagnostics {
        mode: screen.mode(),
        focus,
        channel,
        group_index,
        chip_index,
        programme_index,
        pan_minutes: screen.timeline().map(|timeline| {
            (timeline.starts_at() - screen.displayed_window().starts_at()).num_minutes()
        }),
        first_row: screen.first_visible_row(),
        scroll_offset_units: screen.scroll_offset().round() as u32,
        loaded_families: screen.families().len(),
        page_status: screen.page_status(),
    };
}

impl GuideSurface {
    fn ambient_light(&self) -> bool {
        self.pending_ambient
            .as_ref()
            .map_or_else(catalog::ambient_light, |(enabled, _)| *enabled)
    }

    fn backdrop(&self) -> sparrow_installed_catalog::GuideBackdrop {
        self.pending_backdrop
            .as_ref()
            .map_or_else(catalog::backdrop, |(choice, _)| *choice)
    }

    fn defer_tint(&mut self, cx: &Context<Self>) {
        if self.backdrop() == sparrow_installed_catalog::GuideBackdrop::ChannelTint
            && !self.playback.docked()
        {
            self.tint_dwell = Some(cx.spawn(async move |view, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(380))
                    .await;
                let _ = view.update(cx, |view, cx| {
                    view.tint_dwell = None;
                    cx.notify();
                });
            }));
        }
    }

    fn render_search(
        &mut self,
        window: &Window,
        choice: sparrow_installed_catalog::GuideBackdrop,
    ) -> Option<Div> {
        let (surface, search_icons) = guide_view::search_panel(
            window.viewport_size(),
            guide_view::SearchPanel {
                state: self.search.state(),
                display_term: self.search.term(),
                focus: self.search.focus(),
                editing: self
                    .active_editor
                    .is_some_and(|(_, target)| target == TextTarget::Search),
                has_exclusions: self.search.has_exclusions(),
                docked: self.playback.docked(),
                families: self.screen.families(),
                now: self.now.to_utc(),
            },
            &mut self.search_viewport,
            &self.icons,
        );
        if self.activation.is_none() {
            return surface;
        }
        let guide_visible = self.search.state().view() != SearchView::Desk;
        let preview = self
            .presented_preview
            .as_ref()
            .filter(|_| guide_visible)
            .and_then(|preview| guide_view::family_icon(&preview.family));
        let guide_families = self.screen.visible_families(0);
        let guide_icons = self.screen.families()[guide_families]
            .iter()
            .take_while(|_| guide_visible)
            .filter_map(guide_view::family_icon);
        self.icons.reconcile(
            self.screen.generation(),
            self.playback
                .channel_icon()
                .filter(|_| {
                    guide_visible && choice == sparrow_installed_catalog::GuideBackdrop::ChannelTint
                })
                .into_iter()
                .chain(preview)
                .chain(search_icons)
                .chain(guide_icons),
        );
        surface
    }
}

impl Render for GuideSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.screen
            .set_surface_height(guide_view::surface_height(window.viewport_size()));
        let (frame, picture) = self.playback.fullscreen_frame(window, cx.reduce_motion());
        self.update_playback_viewports(window, picture);
        self.sync_ambient(cx.reduce_motion());
        self.schedule_preview_frame(window, cx);
        publish_diagnostics(&self.screen, self.playback.focused_dock_channel());
        *WORKFLOW_DIAGNOSTICS.lock().unwrap() = WorkflowDiagnostics {
            search: self.search.state().view(),
            channel_results: self.search.state().channels().len(),
            programme_results: self.search.state().programmes().len(),
            hidden_groups: self.screen.exclusions().len(),
            groups_open: self.screen.group_chooser().is_some(),
            sources_open: self.sources_sheet_open(),
            editor_open: self.active_editor.is_some() || self.source_editor_open(),
            setup: self
                .setup
                .as_ref()
                .map_or("None", |setup| step_name(setup.step())),
            setup_focus: self
                .setup
                .as_ref()
                .map_or("None", |setup| control_name(setup.focus())),
        };
        let root = div()
            .id("sparrow-guide")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key));
        if let Some(setup) = self.render_setup(window) {
            self.arrival.leave();
            self.icons.hide();
            return root.child(setup);
        }
        let choice = self.backdrop();
        let search = (!self.playback.fullscreen())
            .then(|| self.render_search(window, choice))
            .flatten();
        let minute = guide_view::MinuteFrame::new(
            self.now,
            self.minute_tick.as_ref(),
            !cx.reduce_motion(),
            window,
        );
        let player = self
            .playback
            .render(window, minute, cx.reduce_motion(), frame);
        let root = root.relative().overflow_hidden().bg(rgb(guide_view::INK));
        let glow = self
            .playback
            .ambient
            .layer(window.viewport_size(), picture, cx.reduce_motion());
        if frame.fullscreen >= 1. {
            self.arrival.leave();
            self.icons.hide();
            return root
                .when(frame.band > 0., |root| root.child(glow))
                .children(player);
        }
        let viewport = window.viewport_size();
        if let Some(loading) = self
            .arrival
            .loading(&self.screen, viewport, cx.reduce_motion())
        {
            return root.child(loading);
        }
        let wave = self.arrival.frame(window, cx.reduce_motion());
        let selected = self.playback.docked() || self.playback.fullscreen();
        let icon = if selected {
            self.playback.channel_icon()
        } else {
            self.presented_preview
                .as_ref()
                .and_then(|p| guide_view::family_icon(&p.family))
        };
        let tint = icon
            .and_then(|i| self.icons.get(i))
            .and_then(|image| image.tint());
        let backdrop = self.backdrop_motion.render(
            choice,
            tint,
            self.tint_dwell.is_some() && !self.playback.docked(),
            window,
            cx.reduce_motion()
                || self.activation.is_none()
                || self.search.state().view() == SearchView::Desk
                || (self.sources_sheet_open() && !self.backdrop_preview_focused())
                || self.screen.group_chooser().is_some(),
        );
        let slide = self.group_slide.frame(
            window,
            cx.reduce_motion(),
            !self.screen.families().is_empty(),
        );
        let guide = self.sections.guide(
            &self.screen,
            self.playback.dock_focused(),
            selected,
            self.search.state().view() == SearchView::Inline,
            self.description.clone(),
            choice,
            minute,
            self.minute_tick
                .as_ref()
                .is_some_and(guide_view::MinuteTick::visible),
            (wave, slide),
            viewport,
        );
        let opacity = (1. - frame.fullscreen * 1.45).max(0.);
        let mut glow = Some(glow);
        root.child(backdrop.opacity(wave.backdrop_opacity()))
            .when(frame.fullscreen == 0., |root| root.children(glow.take()))
            .child(guide)
            .when(frame.fullscreen > 0., |root| {
                root.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(rgb(guide_view::INK))
                        .opacity(1. - opacity),
                )
                .children(glow)
            })
            .children(self.arrival.landing(viewport, wave))
            .children(player)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .opacity(opacity)
                    .children(guide_view::group_sheet(
                        window.viewport_size(),
                        &self.screen,
                        &mut self.group_viewport,
                        self.active_editor
                            .is_some_and(|(_, target)| target == TextTarget::GroupFilter),
                    ))
                    .children(self.sources_sheet(window))
                    .children(search)
                    .when_some(self.feedback, |surface, message| {
                        surface.child(
                            div()
                                .absolute()
                                .right(px(20.))
                                .top(px(78.))
                                .rounded(px(8.))
                                .bg(rgb(0x22324f))
                                .text_color(rgb(0xeef3f8))
                                .px(px(12.))
                                .py(px(8.))
                                .child(message),
                        )
                    }),
            )
    }
}

pub(super) fn open(host: u64, cx: &mut App) -> Option<AnyWindowHandle> {
    match cx.open_window(WindowOptions::default(), |window, cx| {
        let mut diagnostics = crate::DIAGNOSTICS.lock().unwrap();
        diagnostics.software = window.gpu_specs().map(|specs| specs.is_software_emulated);
        drop(diagnostics);
        cx.new(|cx| GuideSurface::new(host, window, cx))
    }) {
        Ok(window) => Some(window.into()),
        Err(_) => {
            crate::DIAGNOSTICS.lock().unwrap().graphics_failed = true;
            log::error!("Sparrow Shield could not open its window");
            None
        }
    }
}
