use std::{collections::VecDeque, fmt::Write, time::Duration};

use chrono::{DateTime, Utc};
use gpui::{Context, Div, Window, prelude::*};
use sparrow_core::{
    ChannelDirection, ChannelFamily, ChannelId, ChannelQuality, ChannelSummary, Page, PageLimit,
    PageRequest, ProgrammeSummary, ScheduleQuery,
};
use sparrow_installed_playback::{
    AudioTrack,
    android::AndroidPlaybackViewport,
    tv::{
        TvPlaybackCommand as Command, TvPlaybackFailure, TvPlaybackPhase, TvPlaybackSnapshot,
        TvTuneTarget,
    },
};
use sparrow_tv_guide::{
    guide_screen::{ScreenFocus, ScreenKey, ToolbarFocus},
    now_focus::Direction,
    search::SearchView,
};

use super::GuideSurface;
use crate::{
    activity, catalog,
    guide_view::{
        self,
        playback::{
            PlaybackControl as Control, PlaybackLayout, PlaybackNext, PlaybackOverlay,
            PlaybackProgramme, PlaybackStage, docked_playback, fullscreen_actions,
            fullscreen_playback, picker, picture_rect,
        },
    },
    playback,
};

const CONTROLS_IDLE: Duration = Duration::from_secs(4);
mod tuning;

#[derive(Default)]
pub(super) struct PlaybackUi {
    state: TvPlaybackSnapshot,
    subject: Option<PlaybackSubject>,
    subject_task: Option<gpui::Task<()>>,
    tune_revision: u64,
    zap_keys: VecDeque<ChannelDirection>,
    zap_target: Option<TvTuneTarget>,
    zap_running: bool,
    dock_focus: bool,
    controls_visible: bool,
    control: Option<Control>,
    copy_sequence: u64,
    copy: DiagnosticCopy,
    menu: Option<PlayerMenu>,
    idle: Option<gpui::Task<()>>,
    viewports: Option<[Option<AndroidPlaybackViewport>; 2]>,
}

#[derive(Default)]
enum DiagnosticCopy {
    #[default]
    Ready,
    Pending(u64),
    Copied,
    Unavailable,
}

impl DiagnosticCopy {
    const fn label(&self) -> &'static str {
        match self {
            Self::Ready => "Copy diagnostics",
            Self::Pending(_) => "Copying…",
            Self::Copied => "Diagnostics copied",
            Self::Unavailable => "Clipboard unavailable",
        }
    }
}

struct PlaybackSubject {
    id: ChannelId,
    title: String,
    family: ChannelFamily,
    schedule: Option<Page<ProgrammeSummary>>,
}

enum PlayerMenu {
    Audio(usize),
    Quality(usize),
}

impl PlayerMenu {
    fn control(&self) -> Control {
        match self {
            Self::Audio(_) => Control::AudioTrack,
            Self::Quality(_) => Control::Quality,
        }
    }
}

impl PlaybackUi {
    pub(super) fn fullscreen(&self) -> bool {
        self.state.controls.fullscreen && self.state.selected.is_some()
    }

    pub(super) fn docked(&self) -> bool {
        self.state.selected.is_some() && !self.fullscreen()
    }

    pub(super) fn dock_focused(&self) -> bool {
        self.docked() && self.dock_focus
    }

    pub(super) fn focused_dock_channel(&self) -> Option<u32> {
        self.shown()
            .filter(|_| self.dock_focused())
            .map(|target| self.shown_number(target))
    }

    fn shown(&self) -> Option<&TvTuneTarget> {
        self.state
            .pending_zap
            .as_ref()
            .or(self.state.selected.as_ref())
    }

    fn subject_for(&self, target: &TvTuneTarget) -> Option<&PlaybackSubject> {
        self.subject
            .as_ref()
            .filter(|subject| subject.id == target.channel)
    }

    fn shown_subject(&self) -> Option<&PlaybackSubject> {
        self.subject_for(self.shown()?)
    }

    fn shown_number(&self, target: &TvTuneTarget) -> u32 {
        self.subject_for(target)
            .map_or(target.number, |subject| subject.family.number())
    }

    fn cancel_zap(&mut self) {
        self.tune_revision = self.tune_revision.wrapping_add(1);
        self.reset_zap();
    }

    fn reset_zap(&mut self) {
        self.zap_keys.clear();
        self.zap_target = None;
        self.zap_running = false;
    }

    fn stage(&self) -> PlaybackStage {
        match self.state.phase {
            TvPlaybackPhase::Idle | TvPlaybackPhase::Starting | TvPlaybackPhase::ReplacingAudio => {
                PlaybackStage::Tuning
            }
            TvPlaybackPhase::Playing => PlaybackStage::Playing,
            TvPlaybackPhase::Pausing | TvPlaybackPhase::Paused(_) => PlaybackStage::Paused,
            TvPlaybackPhase::Recovering { .. } => PlaybackStage::Reconnecting,
            TvPlaybackPhase::Stopping => PlaybackStage::Stopping,
            TvPlaybackPhase::Failed { .. } => PlaybackStage::Failed,
        }
    }

    fn selected_audio_track(&self) -> Option<&AudioTrack> {
        self.state.transport_token?;
        let audio = self.state.audio.as_ref()?;
        if audio.tracks.len() < 2 {
            return None;
        }
        audio.tracks.iter().find(|track| track.selected())
    }

    fn fullscreen_actions(&self) -> Vec<Control> {
        fullscreen_actions(
            self.stage(),
            self.state.controls.muted,
            self.selected_audio_track().is_some(),
            self.shown_subject()
                .is_some_and(|subject| subject.family.variants().len() > 1),
            matches!(
                self.state.phase,
                TvPlaybackPhase::Failed {
                    can_restart: true,
                    ..
                }
            ),
        )
    }

    /// Keeps input on a visible control when the active Playback Session changes.
    fn reconcile_fullscreen_controls(&mut self, retire_menu: bool) -> bool {
        if !self.fullscreen() || !self.controls_visible {
            return false;
        }
        let actions = self.fullscreen_actions();
        if let Some(control) = self.control
            && !actions.iter().any(|action| action.same_slot(control))
        {
            self.control = actions.first().copied();
        }
        if let Some(menu) = &self.menu
            && (retire_menu || !actions.contains(&menu.control()))
        {
            self.menu = None;
            return true;
        }
        false
    }

    fn overlay<R>(
        &self,
        now: DateTime<Utc>,
        render: impl FnOnce(&PlaybackOverlay<'_>) -> R,
    ) -> Option<R> {
        let selected = self.shown()?;
        let subject = self.subject_for(selected);
        let cursor = subject
            .and_then(|subject| subject.schedule.as_ref())
            .map(Page::items)
            .unwrap_or_default();
        let current = cursor
            .iter()
            .find(|slot| slot.starts_at() <= now && now < slot.ends_at());
        let next = cursor
            .iter()
            .find(|slot| slot.starts_at() >= current.map_or(now, ProgrammeSummary::ends_at));
        let starts = current.map(|slot| guide_view::clock_time(slot.starts_at()));
        let ends = current.map(|slot| guide_view::clock_time(slot.ends_at()));
        let next_starts = next.map(|slot| guide_view::clock_time(slot.starts_at()));
        let programme = current.map(|slot| PlaybackProgramme {
            title: slot.title(),
            starts: starts.as_deref().unwrap_or_default(),
            ends: ends.as_deref().unwrap_or_default(),
            minutes_left: ((slot.ends_at() - now).num_seconds().max(0) + 59) / 60,
            elapsed: ((now - slot.starts_at()).num_seconds() as f32
                / (slot.ends_at() - slot.starts_at()).num_seconds().max(1) as f32)
                .clamp(0., 1.),
            next: next
                .zip(next_starts.as_deref())
                .map(|(slot, starts)| PlaybackNext {
                    starts,
                    title: slot.title(),
                }),
        });
        let clock = guide_view::clock_time(now);
        let feedback = match self.state.phase {
            TvPlaybackPhase::Recovering { failure, .. }
            | TvPlaybackPhase::Failed { failure, .. } => Some(failure_label(failure)),
            _ if self.state.quality_fallback.is_some() => {
                Some("Switched to a lower Quality Variant.")
            }
            _ => None,
        };
        Some(render(&PlaybackOverlay {
            channel_number: self.shown_number(selected),
            channel_name: subject.map_or("", |subject| &subject.title),
            programme,
            clock: &clock,
            stage: self.stage(),
            audio_track: self.selected_audio_track().map(AudioTrack::display_name),
            quality: selected.quality.map(quality_name),
            copy_label: self.copy.label(),
            feedback,
            focus: if self.fullscreen() {
                self.control
            } else {
                self.dock_focus.then_some(Control::EnterFullscreen)
            },
        }))
    }

    pub(super) fn render(&self, window: &Window, now: DateTime<Utc>) -> Option<Div> {
        self.overlay(now, |overlay| {
            if self.fullscreen() {
                let actions = if self.controls_visible {
                    self.fullscreen_actions()
                } else {
                    Vec::new()
                };
                let player = fullscreen_playback(window.viewport_size(), overlay, &actions);
                player.children(self.menu_panel(window))
            } else {
                docked_playback(window.viewport_size(), overlay)
            }
        })
    }

    fn menu_panel(&self, window: &Window) -> Option<Div> {
        let (focused, options) = match self.menu.as_ref()? {
            PlayerMenu::Audio(index) => (
                *index,
                self.state
                    .audio
                    .as_ref()?
                    .tracks
                    .iter()
                    .map(|track| track.display_name())
                    .collect::<Vec<_>>(),
            ),
            PlayerMenu::Quality(index) => (
                *index,
                self.shown_subject()?
                    .family
                    .variants()
                    .iter()
                    .filter_map(|variant| {
                        variant
                            .variant()
                            .map(|variant| quality_name(variant.quality()))
                    })
                    .collect(),
            ),
        };
        Some(picker(
            window.viewport_size(),
            focused,
            options.into_iter().map(str::to_owned),
        ))
    }
}

impl GuideSurface {
    pub(super) fn playback_update(&mut self, snapshot: TvPlaybackSnapshot, cx: &mut Context<Self>) {
        let prior_subject = self.playback.shown().map(|target| target.channel.clone());
        let was_fullscreen = self.playback.fullscreen();
        let began_playing = self.playback.state.phase != TvPlaybackPhase::Playing
            && snapshot.phase == TvPlaybackPhase::Playing;
        let zap_settled =
            self.playback.state.pending_zap.is_some() && snapshot.pending_zap.is_none();
        let playback_changed = snapshot.phase != self.playback.state.phase
            || snapshot.selected != self.playback.state.selected;
        let audio_replaced = matches!(self.playback.menu, Some(PlayerMenu::Audio(_)))
            && snapshot.transport_token != self.playback.state.transport_token;
        if snapshot.zap_revision != self.playback.state.zap_revision {
            self.playback.reset_zap();
        }
        self.playback.state = snapshot;
        let fullscreen = self.playback.fullscreen();
        let entering = !was_fullscreen && fullscreen;
        let exiting = was_fullscreen && !fullscreen;
        if entering || exiting || playback_changed {
            self.playback.copy = DiagnosticCopy::Ready;
        }
        if self.playback.state.selected.is_none() {
            self.playback.cancel_zap();
            self.playback.dock_focus = false;
        }
        if entering {
            self.playback.dock_focus = false;
            self.playback.controls_visible = false;
            self.playback.control = None;
        }
        if !fullscreen {
            self.playback.menu = None;
            self.playback.idle = None;
            self.playback.control = None;
            self.playback.controls_visible = false;
            if exiting
                && self.search.state().view() == SearchView::Closed
                && self.screen.group_chooser().is_none()
                && self.source.is_none()
            {
                self.restore_guide_after_playback(true, cx);
            }
        } else if self.playback.state.phase != TvPlaybackPhase::Playing
            || self.playback.state.pending_zap.is_some()
        {
            self.playback.controls_visible = true;
            self.playback.idle = None;
        }
        let subject_changed =
            prior_subject.as_ref() != self.playback.shown().map(|target| &target.channel);
        if subject_changed {
            self.refresh_playback_subject(cx);
        }
        if fullscreen && let TvPlaybackPhase::Failed { can_restart, .. } = self.playback.state.phase
        {
            self.playback.controls_visible = true;
            if entering || playback_changed {
                self.playback.menu = None;
                self.playback.control = Some(if can_restart {
                    Control::Restart
                } else {
                    Control::ExitFullscreen
                });
            }
        }
        let closed_menu = self
            .playback
            .reconcile_fullscreen_controls(subject_changed || audio_replaced);
        if fullscreen && (began_playing || zap_settled || closed_menu) {
            self.arm_controls(cx);
        } else {
            cx.notify();
        }
    }

    fn set_playback_subject(&mut self, subject: Option<PlaybackSubject>, cx: &mut Context<Self>) {
        let replaced_menu = matches!(self.playback.menu, Some(PlayerMenu::Quality(_)))
            && self
                .playback
                .subject
                .as_ref()
                .map(|subject| &subject.family)
                != subject.as_ref().map(|subject| &subject.family);
        self.playback.subject = subject;
        if self.playback.reconcile_fullscreen_controls(replaced_menu) {
            self.arm_controls(cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn refresh_playback_subject(&mut self, cx: &mut Context<Self>) {
        self.playback.subject_task = None;
        let Some(channel_id) = self.playback.shown().map(|target| target.channel.clone()) else {
            self.playback.subject = None;
            return;
        };
        let (Some(activation), Some(generation)) = (self.activation, self.screen.generation())
        else {
            return;
        };
        let from = self.now.to_utc();
        let requested_channel = channel_id.clone();
        let Some(query) = catalog::playback_subject_query(move |core| {
            let number = core.channel_number(generation, &channel_id).ok()??;
            let family = core.channel_family(generation, number).ok()??;
            let channel = family
                .variants()
                .iter()
                .find(|channel| channel.id() == &channel_id)?;
            let title = channel_title(channel).to_owned();
            let limit = PageRequest::first(PageLimit::new(4).ok()?);
            let schedule = family_order(channel, family.variants())
                .filter_map(|candidate| {
                    core.schedule(
                        ScheduleQuery::new(candidate.id().clone(), limit.clone()).with_from(from),
                    )
                    .ok()
                })
                .find(|page| page.generation() == generation && !page.items().is_empty());
            Some(PlaybackSubject {
                id: channel_id,
                title,
                family,
                schedule,
            })
        }) else {
            return;
        };
        let cancel = super::CancelScheduleRead(query.abort_handle());
        self.playback.subject_task = Some(cx.spawn(async move |view, cx| {
            let _cancel = cancel;
            let Ok(Some(subject)) = query.await else {
                return;
            };
            let _ = view.update(cx, |view, cx| {
                if view.activation == Some(activation)
                    && view.screen.generation() == Some(generation)
                    && view.playback.shown().map(|target| &target.channel)
                        == Some(&requested_channel)
                {
                    view.set_playback_subject(subject, cx);
                }
            });
        }));
    }

    pub(super) fn update_playback_viewports(&mut self, window: &Window) {
        if self.activation.is_none() || playback::get().is_none() {
            self.playback.viewports = None;
            return;
        }
        let bounds = |layout, fullscreen| {
            let [left, top, width, height] =
                picture_rect(window.viewport_size(), window.scale_factor(), layout);
            AndroidPlaybackViewport::parse(
                left.max(0) as u32,
                top.max(0) as u32,
                width.max(0) as u32,
                height.max(0) as u32,
                fullscreen,
            )
            .ok()
        };
        let docked = if self.search.state().view() == SearchView::Desk {
            None
        } else {
            bounds(PlaybackLayout::Docked, false)
        };
        let fullscreen = bounds(
            PlaybackLayout::Fullscreen {
                controls_visible: self.playback.controls_visible,
            },
            true,
        );
        let viewports = [docked, fullscreen];
        if self.playback.viewports != Some(viewports)
            && playback::send(Command::SetViewports {
                host: self.host,
                docked,
                fullscreen,
            })
        {
            self.playback.viewports = Some(viewports);
        }
    }

    pub(super) fn playback_back(&mut self, cx: &mut Context<Self>) -> bool {
        if self.playback.menu.take().is_some() {
            self.arm_controls(cx);
            return true;
        }
        if self.playback.fullscreen() {
            playback::send(Command::ExitFullscreen);
            return true;
        }
        if !self.playback.dock_focused()
            && self.screen.focus() == ScreenFocus::Toolbar(ToolbarFocus::Search)
        {
            self.restore_guide_after_playback(false, cx);
            return true;
        }
        false
    }

    fn restore_guide_after_playback(&mut self, preserve_origin: bool, cx: &mut Context<Self>) {
        let channel = self
            .playback
            .state
            .selected
            .as_ref()
            .or(self.playback.state.last_played.as_ref())
            .map(|target| &target.channel);
        self.screen.restore_playback_focus(channel, preserve_origin);
        self.sync_view(cx);
    }

    pub(super) fn media_key(&mut self, code: i32, repeat: i32, cx: &mut Context<Self>) -> bool {
        if matches!(code, 102 | 103 | 166 | 167)
            && (self.source.is_some()
                || self.screen.group_chooser().is_some()
                || self.search.state().view() != SearchView::Closed
                || self.playback.menu.is_some())
        {
            return true;
        }
        match code {
            103 | 166 => self.zap(ChannelDirection::Next, cx),
            102 | 167 => self.zap(ChannelDirection::Previous, cx),
            85 | 86 | 126 | 127 => {
                if repeat == 0 {
                    let command = match code {
                        86 => Command::Stop,
                        127 => Command::Pause,
                        126 => Command::Resume,
                        _ if matches!(self.playback.state.phase, TvPlaybackPhase::Paused(_)) => {
                            Command::Resume
                        }
                        _ => Command::Pause,
                    };
                    if matches!(command, Command::Stop) {
                        self.playback.cancel_zap();
                    }
                    playback::send(command);
                }
            }
            _ => return false,
        }
        true
    }

    pub(super) fn player_key(&mut self, key: ScreenKey, cx: &mut Context<Self>) -> bool {
        if !self.playback.fullscreen() {
            if self.playback.dock_focus {
                match key {
                    ScreenKey::Activate => {
                        playback::send(Command::Fullscreen);
                    }
                    ScreenKey::Arrow(_) => self.playback.dock_focus = false,
                }
                cx.notify();
                return true;
            }
            if self.playback.docked()
                && key == ScreenKey::Arrow(Direction::Left)
                && matches!(self.screen.focus(), ScreenFocus::Toolbar(_))
            {
                self.playback.dock_focus = true;
                cx.notify();
                return true;
            }
            return false;
        }
        if self.playback.menu.is_some() {
            self.menu_key(key, cx);
            return true;
        }
        match key {
            ScreenKey::Arrow(Direction::Up) if self.playback.control.is_some() => {
                self.playback.control = None;
                if self.playback.state.phase == TvPlaybackPhase::Playing
                    && self.playback.state.pending_zap.is_none()
                {
                    self.playback.controls_visible = false;
                }
            }
            ScreenKey::Arrow(Direction::Down) if self.playback.control.is_some() => {}
            ScreenKey::Arrow(Direction::Up) => self.zap(ChannelDirection::Previous, cx),
            ScreenKey::Arrow(Direction::Down) => self.zap(ChannelDirection::Next, cx),
            _ if !self.playback.controls_visible => {
                self.playback.controls_visible = true;
                self.playback.control = self.playback.fullscreen_actions().first().copied();
            }
            ScreenKey::Arrow(direction) => {
                let actions = self.playback.fullscreen_actions();
                let selected = self
                    .playback
                    .control
                    .and_then(|focus| actions.iter().position(|action| action.same_slot(focus)));
                self.playback.control = step(selected, actions.len(), direction)
                    .and_then(|index| actions.get(index).copied());
            }
            ScreenKey::Activate => {
                let actions = self.playback.fullscreen_actions();
                let action = self
                    .playback
                    .control
                    .and_then(|focus| actions.iter().copied().find(|item| item.same_slot(focus)))
                    .or_else(|| actions.first().copied());
                if let Some(action) = action {
                    self.activate_player(action);
                }
            }
        }
        self.arm_controls(cx);
        true
    }

    fn activate_player(&mut self, action: Control) {
        let command = match action {
            Control::EnterFullscreen => Command::Fullscreen,
            Control::ExitFullscreen => Command::ExitFullscreen,
            Control::Pause => Command::Pause,
            Control::Resume => Command::Resume,
            Control::SetMuted(_) => Command::ToggleMute,
            Control::Restart => Command::Restart,
            Control::CopyDiagnostics => {
                self.copy_playback_diagnostics();
                return;
            }
            Control::Stop => {
                self.playback.cancel_zap();
                Command::Stop
            }
            Control::AudioTrack => {
                if let Some(audio) = &self.playback.state.audio {
                    self.playback.menu = Some(PlayerMenu::Audio(
                        audio
                            .tracks
                            .iter()
                            .position(|track| track.selected())
                            .unwrap_or(0),
                    ));
                }
                return;
            }
            Control::Quality => {
                if let Some(subject) = self.playback.shown_subject() {
                    self.playback.menu = Some(PlayerMenu::Quality(
                        subject
                            .family
                            .variants()
                            .iter()
                            .position(|channel| channel.id() == &subject.id)
                            .unwrap_or(0),
                    ));
                }
                return;
            }
        };
        playback::send(command);
    }

    fn copy_playback_diagnostics(&mut self) {
        let playback = &mut self.playback;
        if matches!(playback.copy, DiagnosticCopy::Pending(_)) {
            return;
        }
        let Some(text) = playback_diagnostics(&playback.state) else {
            return;
        };
        playback.copy_sequence += 1;
        let attempt = playback.copy_sequence;
        playback.copy = if activity::copy_playback_diagnostics(self.host, attempt, &text) {
            DiagnosticCopy::Pending(attempt)
        } else {
            DiagnosticCopy::Unavailable
        };
    }

    pub(super) fn playback_diagnostics_copied(
        &mut self,
        attempt: u64,
        copied: bool,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.playback.copy, DiagnosticCopy::Pending(current) if current == attempt) {
            self.playback.copy = if copied {
                DiagnosticCopy::Copied
            } else {
                DiagnosticCopy::Unavailable
            };
            cx.notify();
        }
    }

    fn menu_key(&mut self, key: ScreenKey, cx: &mut Context<Self>) {
        match (self.playback.menu.as_mut(), key) {
            (Some(PlayerMenu::Audio(index)), ScreenKey::Arrow(direction)) => {
                let count = self
                    .playback
                    .state
                    .audio
                    .as_ref()
                    .map_or(0, |audio| audio.tracks.len());
                if let Some(next) = step(Some(*index), count, direction) {
                    *index = next;
                }
            }
            (Some(PlayerMenu::Quality(index)), ScreenKey::Arrow(direction)) => {
                let count = self
                    .playback
                    .subject
                    .as_ref()
                    .map_or(0, |subject| subject.family.variants().len());
                if let Some(next) = step(Some(*index), count, direction) {
                    *index = next;
                }
            }
            (Some(PlayerMenu::Audio(index)), ScreenKey::Activate) => {
                if let Some(track) = self
                    .playback
                    .state
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.tracks.get(*index))
                    && let Some(token) = self.playback.state.transport_token
                {
                    playback::send(Command::SelectAudio {
                        token,
                        track: track.id().clone(),
                    });
                }
                self.playback.menu = None;
            }
            (Some(PlayerMenu::Quality(index)), ScreenKey::Activate) => {
                let choice = self.playback.subject.as_ref().and_then(|subject| {
                    let channel = subject.family.variants().get(*index)?;
                    Some((subject.family.generation(), channel.id().clone()))
                });
                if let Some((generation, channel)) = choice {
                    self.tune(generation, channel, true, false, cx);
                }
                self.playback.menu = None;
            }
            _ => {}
        }
        self.arm_controls(cx);
    }

    fn arm_controls(&mut self, cx: &mut Context<Self>) {
        self.playback.idle = None;
        if self.playback.controls_visible
            && self.playback.menu.is_none()
            && self.playback.state.phase == TvPlaybackPhase::Playing
            && self.playback.state.pending_zap.is_none()
        {
            self.playback.idle = Some(cx.spawn(async move |view, cx| {
                cx.background_executor().timer(CONTROLS_IDLE).await;
                let _ = view.update(cx, |view, cx| {
                    if view.playback.fullscreen()
                        && view.playback.state.phase == TvPlaybackPhase::Playing
                    {
                        view.playback.controls_visible = false;
                        view.playback.control = None;
                        cx.notify();
                    }
                });
            }));
        }
        cx.notify();
    }
}

fn step(current: Option<usize>, length: usize, direction: Direction) -> Option<usize> {
    if length == 0 {
        return None;
    }
    let Some(current) = current else {
        return Some(0);
    };
    Some(match direction {
        Direction::Left | Direction::Up => current.saturating_sub(1),
        Direction::Right | Direction::Down => (current + 1).min(length - 1),
    })
}

fn channel_title(channel: &ChannelSummary) -> &str {
    channel
        .variant()
        .map_or_else(|| channel.name(), |variant| variant.base_name())
}

fn family_order<'a>(
    channel: &'a ChannelSummary,
    variants: &'a [ChannelSummary],
) -> impl Iterator<Item = &'a ChannelSummary> {
    std::iter::once(channel).chain(
        variants
            .iter()
            .filter(move |candidate| candidate.id() != channel.id()),
    )
}

fn quality_name(quality: ChannelQuality) -> &'static str {
    match quality {
        ChannelQuality::Sd => "SD",
        ChannelQuality::Hd => "HD",
        ChannelQuality::Fhd => "FHD",
        ChannelQuality::Uhd => "UHD",
    }
}

fn playback_diagnostics(state: &TvPlaybackSnapshot) -> Option<String> {
    let TvPlaybackPhase::Failed {
        failure,
        can_restart,
    } = state.phase
    else {
        return None;
    };
    let failure = match failure {
        TvPlaybackFailure::ChannelUnavailable => "channel-unavailable",
        TvPlaybackFailure::SourceRejected => "source-rejected",
        TvPlaybackFailure::SourceTimeout => "source-timeout",
        TvPlaybackFailure::SourceUnavailable => "source-unavailable",
        TvPlaybackFailure::InvalidSource => "invalid-source",
        TvPlaybackFailure::UnsupportedMedia => "unsupported-media",
        TvPlaybackFailure::StreamInterrupted => "stream-interrupted",
        TvPlaybackFailure::Ended => "ended",
        TvPlaybackFailure::PresentationUnavailable => "presentation-unavailable",
        TvPlaybackFailure::PlayerUnavailable => "player-unavailable",
        TvPlaybackFailure::CleanupUnconfirmed => "cleanup-unconfirmed",
    };
    let mut text = format!(
        "sparrow-safe-playback-diagnostics/v1\nplayback.phase=failed\nplayback.failure={failure}\nplayback.can-restart={can_restart}"
    );
    if let Some(audio) = &state.audio {
        let _ = write!(text, "\naudio.track-count={}", audio.tracks.len());
    }
    if let Some(status) = state.status {
        let _ = write!(
            text,
            "\nvideo.decoded-frames={}\nvideo.dropped-frames={}",
            status.decoded_frames(),
            status.dropped_frames()
        );
    }
    Some(text)
}

fn failure_label(failure: TvPlaybackFailure) -> &'static str {
    match failure {
        TvPlaybackFailure::ChannelUnavailable => "This Channel is unavailable.",
        TvPlaybackFailure::SourceRejected => "The Playback Source rejected the connection.",
        TvPlaybackFailure::SourceTimeout => "The Playback Source took too long to respond.",
        TvPlaybackFailure::SourceUnavailable => "The Playback Source is unavailable.",
        TvPlaybackFailure::InvalidSource => "The Playback Source is invalid.",
        TvPlaybackFailure::UnsupportedMedia => "This video or audio format is unsupported.",
        TvPlaybackFailure::StreamInterrupted => "The stream was interrupted.",
        TvPlaybackFailure::Ended => "The stream ended.",
        TvPlaybackFailure::PlayerUnavailable => "The player is unavailable.",
        TvPlaybackFailure::PresentationUnavailable => "The player didn’t respond.",
        TvPlaybackFailure::CleanupUnconfirmed => {
            "The previous Playback Session could not be released."
        }
    }
}
