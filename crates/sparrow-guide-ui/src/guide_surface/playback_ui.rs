use std::{
    collections::VecDeque,
    fmt::Write,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use gpui::{Context, Div, RenderImage, Window, prelude::*};
use sparrow_core::{
    ChannelDirection, ChannelFamily, ChannelId, ChannelQuality, ChannelSummary, Page, PageLimit,
    PageRequest, ProgrammeSummary, ScheduleQuery,
};
use sparrow_installed_playback::{
    AudioTrack,
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
    ClipboardTarget, FormFactor, HostKey, PlaybackHost,
    guide_view::{
        self,
        playback::{
            FullscreenFrame, FullscreenMotion, PlaybackControl as Control, PlaybackNext,
            PlaybackOverlay, PlaybackProgramme, PlaybackStage, PlayerClick, PlayerPointer, ZapRoll,
            docked_playback, fullscreen_actions, fullscreen_band, picker, picture_rect,
            pocket::{self, PocketDetails, Upcoming},
            tuning_field, video_picture,
        },
    },
};

const CONTROLS_IDLE: Duration = Duration::from_secs(4);
/// How long the flight still stays under the native picture after Android
/// reports it placed, so the platform has shown it before GPUI stops covering.
const FLIGHT_LANDING: Duration = Duration::from_millis(150);
/// Releases the flight still if Android never reports the landing placement.
const FLIGHT_LANDING_FALLBACK: Duration = Duration::from_secs(1);
/// The largest still copied for a flight, in physical pixels.
const FLIGHT_STILL_SIZE: [u32; 2] = [960, 540];
mod tuning;

#[derive(Default)]
pub(super) struct PlaybackUi {
    pub(super) picture_bottom: i32,
    motion: FullscreenMotion,
    pub(super) ambient: guide_view::ambient::Ambient,
    ambient_epoch: Option<u64>,
    still: Option<std::sync::Arc<gpui::RenderImage>>,
    hold_started: Option<Instant>,
    release_still: Option<gpui::Task<()>>,
    state: TvPlaybackSnapshot,
    start_origin: Option<PlaybackStage>,
    signal_landing: Option<(Instant, PlaybackStage)>,
    subject: Option<PlaybackSubject>,
    subject_task: Option<gpui::Task<()>>,
    tune_revision: u64,
    zap_keys: VecDeque<ChannelDirection>,
    zap_target: Option<TvTuneTarget>,
    zap_running: bool,
    last_zap: Option<Instant>,
    roll: Option<ZapRoll>,
    dock_focus: bool,
    controls_visible: bool,
    control: Option<Control>,
    copy_sequence: u64,
    copy: DiagnosticCopy,
    menu: Option<PlayerMenu>,
    idle: Option<gpui::Task<()>>,
    viewports: Option<[Option<[i32; 4]>; 2]>,
    picture_in_picture: bool,
    live_picture: Option<Arc<RenderImage>>,
    flight: FlightStill,
    flight_request: u64,
    flight_landing: Option<gpui::Task<()>>,
    /// The Channel the flight still was copied from.
    flight_channel: Option<TvTuneTarget>,
    /// Desktop gets a draggable volume slider in the fullscreen band.
    pub(super) volume_slider: bool,
    /// Handhelds that support it get a picture-in-picture control.
    pub(super) picture_in_picture_button: bool,
}

/// A still of the picture that GPUI moves between the dock and fullscreen
/// while the native surface waits offscreen.
#[derive(Default)]
enum FlightStill {
    #[default]
    Idle,
    Requested(u64),
    /// No still this flight; the native picture moves instead.
    Unavailable,
    /// The native picture parks once GPUI has painted the still.
    Flying {
        image: std::sync::Arc<gpui::RenderImage>,
        painted: bool,
    },
    Landing(std::sync::Arc<gpui::RenderImage>),
}

impl FlightStill {
    fn image(&self) -> Option<&std::sync::Arc<gpui::RenderImage>> {
        match self {
            Self::Flying { image, .. } | Self::Landing(image) => Some(image),
            _ => None,
        }
    }
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
    reel: [Option<(u32, String)>; 2],
}

impl PlaybackSubject {
    fn current(&self, now: DateTime<Utc>) -> Option<&ProgrammeSummary> {
        self.schedule
            .as_ref()?
            .items()
            .iter()
            .find(|slot| slot.starts_at() <= now && now < slot.ends_at())
    }

    fn start_roll(&self, now: DateTime<Utc>, direction: ChannelDirection) -> ZapRoll {
        ZapRoll::new(
            self.family.number(),
            &self.title,
            self.current(now).map(ProgrammeSummary::title),
            self.reel.clone(),
            direction,
        )
    }
}

enum PlayerMenu {
    Audio(usize),
    Quality(usize),
    /// The Pocket overflow controls.
    More(usize),
}

impl PlayerMenu {
    fn control(&self) -> Control {
        match self {
            Self::Audio(_) => Control::AudioTrack,
            Self::Quality(_) => Control::Quality,
            Self::More(_) => Control::More,
        }
    }
}

impl PlaybackUi {
    pub(super) fn set_live_picture(
        &mut self,
        picture: Option<Arc<RenderImage>>,
        window: &mut Window,
        cx: &mut Context<GuideSurface>,
    ) {
        if self
            .live_picture
            .as_ref()
            .zip(picture.as_ref())
            .is_some_and(|(old, new)| Arc::ptr_eq(old, new))
            || self.live_picture.is_none() && picture.is_none()
        {
            return;
        }
        if let Some(old) = std::mem::replace(&mut self.live_picture, picture) {
            let _ = window.drop_image(old);
        }
        cx.notify();
    }

    pub(super) fn drop_still(&mut self, cx: &mut gpui::App) {
        self.hold_started = None;
        if let Some(image) = self.still.take() {
            cx.drop_image(image, None);
        }
    }

    pub(super) fn controls_visible(&self) -> bool {
        self.controls_visible
    }

    pub(super) fn fullscreen(&self) -> bool {
        self.state.controls.fullscreen && self.state.selected.is_some()
    }

    pub(super) fn docked(&self) -> bool {
        self.state.selected.is_some() && !self.fullscreen()
    }

    pub(super) fn selected(&self) -> bool {
        self.state.selected.is_some()
    }

    pub(super) fn dock_focused(&self) -> bool {
        self.docked() && self.dock_focus
    }

    pub(super) fn channel_icon(&self) -> Option<&sparrow_core::ChannelIcon> {
        self.subject_for(self.state.selected.as_ref()?)?
            .family
            .variants()
            .first()?
            .icon()
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
        self.roll = None;
        self.last_zap = None;
        self.zap_keys.clear();
        self.zap_target = None;
        self.zap_running = false;
    }

    pub(super) fn playing_marker(
        &self,
    ) -> Option<crate::guide_view::playing_marker::PlayingMarker> {
        let shown = self.shown()?;
        crate::guide_view::playing_marker::PlayingMarker::new(
            self.stage(),
            &shown.channel,
            self.subject_for(shown).map(|subject| &subject.family),
        )
    }

    pub(super) fn stage(&self) -> PlaybackStage {
        if self.state.pending_zap.is_some()
            || (self.state.phase == TvPlaybackPhase::Stopping && self.state.selected.is_some())
        {
            return PlaybackStage::Tuning;
        }
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
            self.volume_slider,
            self.picture_in_picture_button,
        )
    }

    fn can_restart(&self) -> bool {
        match self.state.phase {
            TvPlaybackPhase::Failed { can_restart, .. } => can_restart,
            _ => matches!(self.stage(), PlaybackStage::Playing | PlaybackStage::Paused),
        }
    }

    /// The Pocket control bar outside fullscreen, left to right.
    fn watch_actions(&self) -> Vec<Control> {
        let stage = self.stage();
        let live = matches!(stage, PlaybackStage::Playing | PlaybackStage::Paused);
        let mut actions = Vec::new();
        if live {
            actions.push(if stage == PlaybackStage::Paused {
                Control::Resume
            } else {
                Control::Pause
            });
            actions.push(Control::SetMuted(!self.state.controls.muted));
            if self.selected_audio_track().is_some() {
                actions.push(Control::AudioTrack);
            }
        } else if stage == PlaybackStage::Failed && self.can_restart() {
            actions.push(Control::Restart);
        }
        if live && self.picture_in_picture_button {
            actions.push(Control::PictureInPicture);
        }
        actions.push(Control::EnterFullscreen);
        if !self.more_actions().is_empty() {
            actions.push(Control::More);
        }
        if stage != PlaybackStage::Stopping {
            actions.push(Control::Stop);
        }
        actions
    }

    /// The overflow menu behind More.
    fn more_actions(&self) -> Vec<Control> {
        let mut actions = Vec::new();
        if self.stage() != PlaybackStage::Failed && self.can_restart() {
            actions.push(Control::Restart);
        }
        if self
            .shown_subject()
            .is_some_and(|subject| subject.family.variants().len() > 1)
        {
            actions.push(Control::Quality);
        }
        if self.stage() == PlaybackStage::Failed {
            actions.push(Control::CopyDiagnostics);
        }
        actions
    }

    /// Description, Up Next and Quality Variants for the Pocket chrome.
    fn pocket_details(&self, now: DateTime<Utc>) -> PocketDetails<'_> {
        let Some(subject) = self.shown().and_then(|shown| self.subject_for(shown)) else {
            return PocketDetails {
                actions: self.watch_actions(),
                ..Default::default()
            };
        };
        let schedule = subject
            .schedule
            .as_ref()
            .map(Page::items)
            .unwrap_or_default();
        let current = subject.current(now);
        let selected = self.state.selected.as_ref().map(|target| &target.channel);
        PocketDetails {
            group: subject
                .family
                .variants()
                .first()
                .map_or("", ChannelSummary::group),
            description: current.and_then(ProgrammeSummary::description),
            upcoming: schedule
                .iter()
                .filter(|slot| slot.starts_at() >= current.map_or(now, ProgrammeSummary::ends_at))
                .take(4)
                .map(|slot| Upcoming {
                    starts: guide_view::clock_time(slot.starts_at()),
                    title: slot.title(),
                    minutes: (slot.ends_at() - slot.starts_at()).num_minutes(),
                })
                .collect(),
            variants: subject
                .family
                .variants()
                .iter()
                .enumerate()
                .filter_map(|(index, channel)| {
                    Some((
                        index,
                        channel.variant()?.quality(),
                        Some(channel.id()) == selected,
                    ))
                })
                .collect(),
            actions: self.watch_actions(),
        }
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
        minute: guide_view::MinuteFrame,
        reduce_motion: bool,
        picture_reveal: Duration,
        focus_visible: bool,
        render: impl FnOnce(&PlaybackOverlay<'_>) -> R,
    ) -> Option<R> {
        let now = minute.now.to_utc();
        let selected = self.shown()?;
        // Keep the last resolved identity on screen while the zap's Guide query runs.
        let subject =
            if self.state.pending_zap.is_some() || self.zap_target.as_ref() == Some(selected) {
                self.subject.as_ref()
            } else {
                self.subject_for(selected)
            };
        let cursor = subject
            .and_then(|subject| subject.schedule.as_ref())
            .map(Page::items)
            .unwrap_or_default();
        let current = subject.and_then(|s| s.current(now));
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
            elapsed: ((minute.meter_time() - slot.starts_at()).as_seconds_f32()
                / (slot.ends_at() - slot.starts_at()).num_seconds().max(1) as f32)
                .clamp(0., 1.),
            next: next
                .zip(next_starts.as_deref())
                .map(|(slot, starts)| PlaybackNext {
                    starts,
                    title: slot.title(),
                }),
        });
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
            reel: subject.map(|s| &s.reel),
            roll: self
                .roll
                .as_ref()
                .filter(|r| !reduce_motion && r.active(self.shown_number(selected))),
            channel_name: subject.map_or("", |subject| &subject.title),
            programme,
            stage: self.stage(),
            signal_landing: self.signal_landing,
            // Paint beneath the outgoing surface while a retune or queued zap awaits selection.
            first_picture: self
                .state
                .first_picture
                .filter(|_| self.state.pending_zap.is_none()),
            reduce_motion,
            picture_reveal,
            still: self
                .still
                .as_ref()
                .filter(|_| self.state.pending_zap.is_none()),
            hold_started: self
                .hold_started
                .filter(|_| self.state.pending_zap.is_none()),
            attempt: self.state.recovery_attempts,
            audio_track: self.selected_audio_track().map(AudioTrack::display_name),
            quality: selected.quality.map(quality_name),
            copy_label: self.copy.label(),
            feedback,
            focus: if self.fullscreen() {
                self.control
            } else {
                self.dock_focus.then_some(Control::EnterFullscreen)
            }
            .filter(|_| focus_visible),
            volume: self.state.controls.volume,
            muted: self.state.controls.muted,
            embedded: self.live_picture.is_some(),
        }))
    }

    fn retarget_motion(&mut self, cx: &Context<GuideSurface>) {
        let cut = cx.reduce_motion() || self.state.selected.is_none();
        self.motion
            .retarget(self.fullscreen(), self.controls_visible, cut);
    }

    pub(super) fn fullscreen_frame(
        &mut self,
        window: &mut Window,
        reduced: bool,
    ) -> (FullscreenFrame, FullscreenFrame) {
        self.motion.frame(reduced, window)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render(
        &self,
        window: &mut Window,
        layout: guide_view::Layout,
        pointer: Option<PlayerPointer>,
        minute: guide_view::MinuteFrame,
        reduce_motion: bool,
        frame: FullscreenFrame,
        picture_reveal: Duration,
        show_live_picture: bool,
    ) -> Option<Div> {
        self.overlay(
            minute,
            reduce_motion,
            picture_reveal,
            layout.focus_visible,
            |overlay| {
                if overlay.still.is_some()
                    && let Some(handoff) = self.state.still_handoff.clone()
                {
                    window.on_next_frame(move |_, _| handoff.painted());
                }

                if overlay.roll.is_some() {
                    window.request_animation_frame();
                }
                gpui::div()
                    .absolute()
                    .inset_0()
                    .when_some(
                        guide_view::playback::dock_lock(layout, overlay, frame),
                        |root, lock| root.child(lock),
                    )
                    // The tuning field fades off an embedded first picture, so the
                    // live picture sits under it.
                    .when_some(
                        self.live_picture
                            .as_ref()
                            .filter(|_| {
                                show_live_picture
                                    && self.state.pending_zap.is_none()
                                    && matches!(
                                        overlay.stage,
                                        PlaybackStage::Playing | PlaybackStage::Paused
                                    )
                            })
                            .cloned(),
                        |root, image| root.child(video_picture(layout, frame, image, true)),
                    )
                    .when(overlay.show_field(), |root| {
                        root.child(tuning_field(layout, overlay, frame))
                    })
                    .when(frame.caption > 0., |root| {
                        root.child(if layout.compact_pocket() {
                            let details = self.pocket_details(minute.now.to_utc());
                            pocket::chrome(
                                layout,
                                overlay,
                                &details,
                                frame,
                                minute,
                                pointer.as_ref(),
                            )
                            .unwrap_or_else(gpui::div)
                        } else {
                            docked_playback(layout, overlay, frame)
                        })
                    })
                    .when(frame.band > 0., |root| {
                        root.child(fullscreen_band(
                            layout,
                            overlay,
                            &self.fullscreen_actions(),
                            frame.band,
                            if reduce_motion
                                || overlay.show_field()
                                || self.viewports.is_none_or(|v| v[1].is_none())
                            {
                                0.
                            } else {
                                self.picture_bottom as f32 / window.scale_factor()
                            },
                            minute,
                            pointer.as_ref(),
                        ))
                    })
                    // Above the band, where the native picture it stands in for would be.
                    .when_some(self.flight.image().cloned(), |root, image| {
                        root.child(video_picture(layout, frame, image, false))
                    })
                    .children(self.menu_panel(layout, pointer.as_ref()))
            },
        )
    }

    fn menu_panel(
        &self,
        layout: guide_view::Layout,
        pointer: Option<&PlayerPointer>,
    ) -> Option<Div> {
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
            PlayerMenu::More(index) => (
                *index,
                self.more_actions()
                    .into_iter()
                    .map(|action| match action {
                        Control::Restart => "Restart",
                        Control::Quality => "Quality",
                        _ => self.copy.label(),
                    })
                    .collect(),
            ),
        };
        Some(picker(
            layout,
            focused,
            options.into_iter().map(str::to_owned),
            pointer,
        ))
    }
}

impl GuideSurface {
    /// Commits an external Tune or Stop on the UI thread, superseding pending
    /// Guide tune and zap reads as a local playback action would.
    pub fn external_playback_intent(&mut self, command: Command) -> bool {
        if matches!(command, Command::Tune { .. }) && self.activation.is_none() {
            return false;
        }
        if self.send_playback(command) {
            self.playback.cancel_zap();
            true
        } else {
            false
        }
    }

    pub(super) fn picture_reveal_duration(&self) -> Duration {
        self.catalog
            .playback()
            .map_or(Duration::ZERO, PlaybackHost::picture_reveal_duration)
    }

    fn send_playback(&self, command: Command) -> bool {
        self.catalog
            .runner()
            .is_some_and(|runner| runner.commands().try_send(command).is_ok())
    }

    pub(super) fn playback_update(
        &mut self,
        mut snapshot: TvPlaybackSnapshot,
        cx: &mut Context<Self>,
    ) {
        if snapshot.visible_host != Some(self.host.id()) {
            snapshot.still_handoff = None;
        }
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
        let first_picture = snapshot.first_picture.is_some()
            && snapshot.first_picture != self.playback.state.first_picture;
        if first_picture && cx.reduce_motion() {
            // A static reduced-motion field has no animation tick to retire it.
            let overlap = self.picture_reveal_duration();
            cx.spawn(async move |view, cx| {
                cx.background_executor().timer(overlap).await;
                let _ = view.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        if began_playing {
            let outgoing = self.playback.stage();
            let lands = match outgoing {
                PlaybackStage::Reconnecting => true,
                PlaybackStage::Tuning => {
                    first_picture && self.playback.start_origin != Some(PlaybackStage::Paused)
                }
                _ => false,
            };
            self.playback.signal_landing = lands.then(|| (Instant::now(), outgoing));
        }
        // A resume passes through Starting and must not replay the tuning landing.
        self.playback.start_origin = if snapshot.phase == TvPlaybackPhase::Starting
            && snapshot.selected == self.playback.state.selected
        {
            match self.playback.state.phase {
                TvPlaybackPhase::Starting => self.playback.start_origin,
                _ => Some(self.playback.stage()),
            }
        } else {
            None
        };
        let previous = self.playback.state.recovery_frame.as_ref();
        if let Some(frame) = snapshot.recovery_frame.as_ref()
            && previous.is_none_or(|old| !std::sync::Arc::ptr_eq(old, frame))
        {
            self.playback.release_still = None;
            self.playback.drop_still(cx);
            self.playback.hold_started = Some(Instant::now());
            self.playback.still = image::RgbaImage::from_raw(
                frame.width,
                frame.height,
                frame.bgra.clone(),
            )
            .map(|pixels| std::sync::Arc::new(gpui::RenderImage::new([image::Frame::new(pixels)])));
        } else if self.playback.still.is_some() && snapshot.recovery_frame.is_none() {
            if began_playing
                && !cx.reduce_motion()
                && snapshot.selected == self.playback.state.selected
            {
                // Keep the GPUI underlay through the native first-frame reveal.
                let overlap = self.picture_reveal_duration();
                self.playback.release_still = Some(cx.spawn(async move |view, cx| {
                    cx.background_executor().timer(overlap).await;
                    let _ = view.update(cx, |view, cx| {
                        view.playback.drop_still(cx);
                        cx.notify();
                    });
                }));
            } else if snapshot.phase != TvPlaybackPhase::Playing
                || self.playback.release_still.is_none()
            {
                self.playback.release_still = None;
                self.playback.drop_still(cx);
            }
        }
        self.playback.state = snapshot;
        let fullscreen = self.playback.fullscreen();
        let entering = !was_fullscreen && fullscreen;
        let exiting = was_fullscreen && !fullscreen;
        if (entering || exiting) && self.host.form_factor() == FormFactor::Handheld {
            self.host.set_immersive(fullscreen);
        }
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
            self.playback.menu = None;
        }
        if !fullscreen {
            // The Pocket Watch screen keeps its menus while docked.
            if exiting || self.playback.state.selected.is_none() {
                self.playback.menu = None;
            }
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
        } else if self.playback.stage() != PlaybackStage::Playing {
            self.playback.controls_visible = true;
            self.playback.idle = None;
        }
        let subject_changed =
            prior_subject.as_ref() != self.playback.shown().map(|target| &target.channel);
        if subject_changed {
            if let Some(roll) = &mut self.playback.roll {
                roll.show_number();
            }
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
        self.playback.retarget_motion(cx);
        let closed_menu = self
            .playback
            .reconcile_fullscreen_controls(subject_changed || audio_replaced);
        if fullscreen && (entering || began_playing || zap_settled || closed_menu) {
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
        if let Some(incoming) = &subject
            && self
                .playback
                .subject
                .as_ref()
                .is_some_and(|old| old.id != incoming.id)
            && let Some(roll) = &mut self.playback.roll
        {
            roll.show_subject();
        }
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
        let excluded = self.screen.exclusions().clone();
        let requested_channel = channel_id.clone();
        let Some(query) = self.catalog.playback_subject_query(move |core| {
            let number = core.channel_number(generation, &channel_id).ok()??;
            let family = core.channel_family(generation, number).ok()??;
            let channel = family
                .variants()
                .iter()
                .find(|channel| channel.id() == &channel_id)?;
            let title = channel_title(channel).to_owned();
            // The current Programme and four after it for the Pocket Up Next list.
            let limit = PageRequest::first(PageLimit::new(5).ok()?);
            let schedule = family_order(channel, family.variants())
                .filter_map(|candidate| {
                    core.schedule(
                        ScheduleQuery::new(candidate.id().clone(), limit.clone()).with_from(from),
                    )
                    .ok()
                })
                .find(|page| page.generation() == generation && !page.items().is_empty());
            let reel = [ChannelDirection::Previous, ChannelDirection::Next].map(|direction| {
                let neighbour = core
                    .adjacent_channel_family(generation, family.number(), direction, &excluded)
                    .ok()??;
                Some((
                    neighbour.number(),
                    channel_title(neighbour.variants().first()?).to_owned(),
                ))
            });
            Some(PlaybackSubject {
                reel,
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

    /// The TV Search desk hides the picture. Pocket sheets sit clear of it.
    pub(super) fn dock_hidden(&self) -> bool {
        self.search.state().view() == SearchView::Desk
            && !matches!(self.profile, guide_view::Profile::Pocket(..))
    }

    pub(super) fn ambient_exposed(&self) -> bool {
        !self.playback.fullscreen() || self.playback.controls_visible
    }

    pub(super) fn sync_ambient(&mut self, reduced: bool) {
        let visible = self.activation.is_some()
            && self.setup.is_none()
            && self.ambient_light()
            && !self.dock_hidden();
        let exposed = self.ambient_exposed();
        let player = &mut self.playback;
        let stage = (visible && player.shown().is_some()).then(|| player.stage());
        let sampling = exposed && stage == Some(PlaybackStage::Playing);
        if sampling != player.ambient_epoch.is_some() {
            static NEXT_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            player.ambient_epoch =
                sampling.then(|| NEXT_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
            if let Some(host) = self.catalog.playback() {
                host.set_ambient_sampling(self.host.id(), player.ambient_epoch.unwrap_or(0));
            }
        }
        match stage {
            Some(PlaybackStage::Playing | PlaybackStage::Paused) => {}
            Some(PlaybackStage::Tuning) => player.ambient.field(Some(0x438eb6), reduced),
            Some(PlaybackStage::Reconnecting) => player.ambient.field(Some(0x636974), reduced),
            _ => player.ambient.field(None, reduced),
        }
    }

    pub(super) fn ambient_sample(&mut self, epoch: u64, colors: [u32; 3], cx: &mut Context<Self>) {
        if Some(epoch) == self.playback.ambient_epoch
            && self.playback.ambient.sample(colors, cx.reduce_motion())
        {
            cx.notify();
        }
    }

    /// Steps the flight still and reports whether the native picture should wait
    /// offscreen.
    fn update_flight_still(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let flying = self.playback.motion.flying();
        // A zap, a retune, a reconnect or a recovery still takes the picture
        // over until the flight ends.
        let available = matches!(
            self.playback.stage(),
            PlaybackStage::Playing | PlaybackStage::Paused
        ) && self.playback.still.is_none();
        let owned = available && self.playback.state.selected == self.playback.flight_channel;
        let settled = if flying {
            FlightStill::Unavailable
        } else {
            FlightStill::Idle
        };
        self.playback.flight = match std::mem::take(&mut self.playback.flight) {
            FlightStill::Flying { image, .. } | FlightStill::Landing(image) if !owned => {
                self.playback.flight_landing = None;
                cx.drop_image(image, Some(window));
                settled
            }
            FlightStill::Requested(_) if !owned => settled,
            FlightStill::Idle if flying && available => self.request_flight_still(),
            FlightStill::Requested(_) | FlightStill::Unavailable if !flying => FlightStill::Idle,
            FlightStill::Flying { image, .. } if !flying => {
                let lands = self.activation.is_some()
                    && self.playback.state.selected.is_some()
                    && (self.playback.fullscreen() || !self.dock_hidden());
                if lands {
                    self.release_flight_still(FLIGHT_LANDING_FALLBACK, cx);
                    FlightStill::Landing(image)
                } else {
                    cx.drop_image(image, Some(window));
                    FlightStill::Idle
                }
            }
            // The live picture has played since this still; copy it again.
            FlightStill::Landing(image) if flying => {
                self.playback.flight_landing = None;
                cx.drop_image(image, Some(window));
                self.request_flight_still()
            }
            other => other,
        };
        if let FlightStill::Flying { painted: false, .. } = self.playback.flight {
            let view = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = view.update(cx, |view, cx| {
                    if let FlightStill::Flying { painted, .. } = &mut view.playback.flight {
                        *painted = true;
                        cx.notify();
                    }
                });
            });
        }
        matches!(
            self.playback.flight,
            FlightStill::Flying { painted: true, .. }
        )
    }

    fn request_flight_still(&mut self) -> FlightStill {
        self.playback.flight_channel = self.playback.state.selected.clone();
        self.playback.flight_request += 1;
        let request = self.playback.flight_request;
        if self.catalog.playback().is_some_and(|host| {
            host.capture_flight_still(self.host.id(), request, FLIGHT_STILL_SIZE)
        }) {
            FlightStill::Requested(request)
        } else {
            FlightStill::Unavailable
        }
    }

    /// Drops a landed still after `delay`, replacing any earlier release.
    fn release_flight_still(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.playback.flight_landing = Some(cx.spawn(async move |view, cx| {
            cx.background_executor().timer(delay).await;
            let _ = view.update(cx, |view, cx| {
                if matches!(view.playback.flight, FlightStill::Landing(_))
                    && let FlightStill::Landing(image) = std::mem::take(&mut view.playback.flight)
                {
                    cx.drop_image(image, None);
                    cx.notify();
                }
            });
        }));
    }

    /// Android placed the native picture again; it covers the landed still shortly.
    pub(super) fn flight_picture_placed(&mut self, cx: &mut Context<Self>) {
        if matches!(self.playback.flight, FlightStill::Landing(_)) {
            self.release_flight_still(FLIGHT_LANDING, cx);
        }
    }

    pub(super) fn flight_still_captured(
        &mut self,
        request: u64,
        frame: Option<image::RgbaImage>,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.playback.flight, FlightStill::Requested(id) if id == request) {
            return;
        }
        let [max_width, max_height] = FLIGHT_STILL_SIZE;
        let frame =
            frame.filter(|pixels| pixels.width() <= max_width && pixels.height() <= max_height);
        self.playback.flight = match frame {
            Some(pixels) => FlightStill::Flying {
                image: std::sync::Arc::new(gpui::RenderImage::new([image::Frame::new(pixels)])),
                painted: false,
            },
            None => FlightStill::Unavailable,
        };
        cx.notify();
    }

    pub(super) fn update_playback_viewports(
        &mut self,
        window: &mut Window,
        frame: FullscreenFrame,
        cx: &mut Context<Self>,
    ) {
        let embedded = self
            .catalog
            .playback()
            .is_some_and(PlaybackHost::embedded_picture);
        let in_pip = self.playback.picture_in_picture;
        let parked = !in_pip && !embedded && self.update_flight_still(window, cx);
        let Some(host) = self
            .catalog
            .playback()
            .filter(|_| self.activation.is_some() && self.catalog.runner().is_some())
        else {
            self.playback.viewports = None;
            return;
        };
        let rect = |frame| picture_rect(self.layout(window), window.scale_factor(), frame);
        let viewports = if in_pip {
            // Android fills the PiP window. Keep a viewport available to the
            // runner so Play can reopen the transport even with a hidden dock.
            let viewport = rect(FullscreenFrame {
                fullscreen: 1.,
                ..Default::default()
            });
            [Some(viewport); 2]
        } else if self.playback.state.pending_zap.is_some()
            || parked
            || (embedded && (self.setup.is_some() || !window.is_visible()))
        {
            [None, None]
        } else if embedded {
            // GPUI moves the live image through the flight. Send only physical
            // endpoints to libmpv, including the endpoint for the control band.
            let docked = (!self.dock_hidden()).then(|| rect(FullscreenFrame::default()));
            let fullscreen = rect(FullscreenFrame {
                fullscreen: 1.,
                band: f32::from(self.playback.controls_visible),
                caption: 0.,
                stripe: 0.,
            });
            [docked, Some(fullscreen)]
        } else {
            let rect = rect(frame);
            [(!self.dock_hidden()).then_some(rect), Some(rect)]
        };
        if self.playback.viewports != Some(viewports) {
            host.set_picture_bounds(self.host.id(), viewports[0], viewports[1]);
            self.playback.viewports = Some(viewports);
        }
    }

    pub(super) fn picture_in_picture_changed(
        &mut self,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.playback.picture_in_picture = active;
        self.playback.viewports = None;
        if active {
            self.update_playback_viewports(window, FullscreenFrame::default(), cx);
        }
        cx.notify();
    }

    pub(super) fn playback_back(&mut self, cx: &mut Context<Self>) -> bool {
        if self.playback.menu.take().is_some() {
            self.arm_controls(cx);
            return true;
        }
        if self.playback.fullscreen() {
            self.send_playback(Command::ExitFullscreen);
            return true;
        }
        if self.pocket.leave_watch(self.pocket_mode()) {
            self.restore_guide_after_playback(false, cx);
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

    pub(super) fn media_key(&mut self, key: HostKey, held: bool, cx: &mut Context<Self>) -> bool {
        if matches!(key, HostKey::Channel(_))
            && (self.source.is_some()
                || self.screen.group_chooser().is_some()
                || self.search.state().view() != SearchView::Closed
                || self.playback.menu.is_some())
        {
            return true;
        }
        match key {
            HostKey::Channel(direction) => self.zap(direction, held, cx),
            HostKey::TogglePause | HostKey::Stop | HostKey::Pause | HostKey::Resume => {
                if !held {
                    let command = match key {
                        HostKey::Stop => Command::Stop,
                        HostKey::Resume
                            if self.host.form_factor() == FormFactor::Handheld
                                && matches!(
                                    self.playback.state.phase,
                                    TvPlaybackPhase::Failed {
                                        can_restart: true,
                                        ..
                                    }
                                ) =>
                        {
                            Command::Restart
                        }
                        HostKey::Resume => Command::Resume,
                        HostKey::TogglePause
                            if matches!(self.playback.state.phase, TvPlaybackPhase::Paused(_)) =>
                        {
                            Command::Resume
                        }
                        _ => Command::Pause,
                    };
                    if matches!(command, Command::Stop) {
                        self.playback.cancel_zap();
                    }
                    self.send_playback(command);
                }
            }
            _ => return false,
        }
        true
    }

    pub(super) fn player_key(
        &mut self,
        key: ScreenKey,
        repeat: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.playback.fullscreen() {
            if self.playback.dock_focus {
                match key {
                    ScreenKey::Activate => {
                        self.playback.dock_focus = false;
                        self.send_playback(Command::Fullscreen);
                    }
                    ScreenKey::Arrow(_) => self.playback.dock_focus = false,
                }
                cx.notify();
                return true;
            }
            if self.playback.docked()
                && key == ScreenKey::Arrow(Direction::Left)
                && self.screen.focus() == ScreenFocus::Toolbar(ToolbarFocus::Search)
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
            ScreenKey::Arrow(Direction::Up) => self.zap(ChannelDirection::Previous, repeat, cx),
            ScreenKey::Arrow(Direction::Down) => self.zap(ChannelDirection::Next, repeat, cx),
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
                    self.activate_player(action, cx);
                }
            }
        }
        self.playback.retarget_motion(cx);
        self.arm_controls(cx);
        true
    }

    fn activate_player(&mut self, action: Control, cx: &mut Context<Self>) {
        let command = match action {
            Control::EnterFullscreen => {
                self.pocket.chose_fullscreen();
                Command::Fullscreen
            }
            Control::PictureInPicture => {
                if !self.host.enter_picture_in_picture() {
                    self.show_feedback(Some("Picture-in-picture is unavailable."), cx);
                }
                return;
            }
            Control::More => {
                self.playback.menu = Some(PlayerMenu::More(0));
                return;
            }
            Control::ExitFullscreen => Command::ExitFullscreen,
            Control::Pause => Command::Pause,
            Control::Resume => Command::Resume,
            Control::SetMuted(_) => Command::ToggleMute,
            Control::Volume => return,
            Control::Restart => Command::Restart,
            Control::CopyDiagnostics => {
                self.copy_playback_diagnostics(cx);
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
        self.send_playback(command);
    }

    fn copy_playback_diagnostics(&mut self, cx: &mut Context<Self>) {
        let playback = &mut self.playback;
        if matches!(playback.copy, DiagnosticCopy::Pending(_)) {
            return;
        }
        let Some(text) = playback_diagnostics(&playback.state) else {
            return;
        };
        playback.copy_sequence += 1;
        let attempt = playback.copy_sequence;
        playback.copy =
            if self
                .host
                .copy_diagnostics(ClipboardTarget::Playback { attempt }, &text, cx)
            {
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
        let more = self.playback.more_actions();
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
                    self.send_playback(Command::SelectAudio {
                        token,
                        track: track.id().clone(),
                    });
                }
                self.playback.menu = None;
            }
            (Some(PlayerMenu::More(index)), ScreenKey::Arrow(direction)) => {
                if let Some(next) = step(Some(*index), more.len(), direction) {
                    *index = next;
                }
            }
            (Some(PlayerMenu::More(index)), ScreenKey::Activate) => {
                let action = more.get(*index).copied();
                self.playback.menu = None;
                if let Some(action) = action {
                    self.activate_player(action, cx);
                }
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

    /// Band controls take pointer presses directly; the band is never cached.
    pub(super) fn player_pointer(&self, cx: &Context<Self>) -> Option<PlayerPointer> {
        if self.host.form_factor() == crate::FormFactor::Television {
            return None;
        }
        let view = cx.weak_entity();
        Some(std::rc::Rc::new(move |click, window, cx| {
            let _ = view.update(cx, |view, cx| view.player_click(click, window, cx));
        }))
    }

    fn player_click(&mut self, click: PlayerClick, window: &mut Window, cx: &mut Context<Self>) {
        match click {
            PlayerClick::Variant(index) => {
                let choice = self.playback.shown_subject().and_then(|subject| {
                    let channel = subject.family.variants().get(index)?;
                    Some((subject.family.generation(), channel.id().clone()))
                });
                if let Some((generation, channel)) = choice {
                    self.tune(generation, channel, true, false, cx);
                }
            }
            PlayerClick::Zap(direction) => self.zap(direction, false, cx),
            PlayerClick::Guide => {
                self.pocket.leave_watch(self.pocket_mode());
                self.playback.menu = None;
                self.restore_guide_after_playback(false, cx);
            }
            PlayerClick::Watch => {
                self.pocket.show_watch();
                cx.notify();
            }
            PlayerClick::Search => {
                self.playback.menu = None;
                self.search_from_watch(window, cx);
            }
            PlayerClick::Control(action) => {
                if self.playback.fullscreen() {
                    self.playback.control = Some(action);
                }
                self.activate_player(action, cx);
            }
            PlayerClick::Pick(index) => {
                match &mut self.playback.menu {
                    Some(
                        PlayerMenu::Audio(current)
                        | PlayerMenu::Quality(current)
                        | PlayerMenu::More(current),
                    ) => {
                        *current = index;
                    }
                    None => return,
                }
                self.menu_key(ScreenKey::Activate, cx);
            }
            PlayerClick::Volume(level) => {
                self.send_playback(Command::SetVolume(level));
            }
        }
        self.playback.retarget_motion(cx);
        self.arm_controls(cx);
    }

    /// Pointer movement over fullscreen shows the band and restarts its idle timer.
    pub(super) fn reveal_player_controls(&mut self, cx: &mut Context<Self>) {
        if !self.playback.fullscreen() {
            return;
        }
        if !self.playback.controls_visible {
            self.playback.controls_visible = true;
            self.playback.retarget_motion(cx);
        }
        self.arm_controls(cx);
    }

    /// A tap on a handheld's fullscreen picture shows the band, or hides it
    /// while playback runs without a menu.
    pub(super) fn toggle_player_controls(&mut self, cx: &mut Context<Self>) {
        let hides = self.playback.controls_visible
            && self.playback.menu.is_none()
            && self.playback.stage() == PlaybackStage::Playing;
        if !self.playback.fullscreen() || !hides {
            self.reveal_player_controls(cx);
            return;
        }
        self.playback.controls_visible = false;
        self.playback.control = None;
        self.playback.idle = None;
        self.playback.retarget_motion(cx);
        cx.notify();
    }

    pub(super) fn set_volume_step(&mut self, step: f32) {
        let volume = (self.playback.state.controls.volume + step).clamp(0., 1.);
        self.send_playback(Command::SetVolume(volume));
    }

    pub(super) fn toggle_mute(&mut self) {
        if self.playback.state.selected.is_some() {
            self.send_playback(Command::ToggleMute);
        }
    }

    pub(super) fn toggle_pause(&mut self) {
        let command = match self.playback.state.phase {
            TvPlaybackPhase::Paused(_) => Command::Resume,
            TvPlaybackPhase::Playing => Command::Pause,
            _ => return,
        };
        self.send_playback(command);
    }

    pub(super) fn clear_dock_focus(&mut self) {
        self.playback.dock_focus = false;
    }

    /// Returns whether the request changes the picture's placement.
    pub(super) fn playback_set_fullscreen(&mut self, fullscreen: bool) -> bool {
        if self.playback.state.selected.is_none() || self.playback.fullscreen() == fullscreen {
            return false;
        }
        self.pocket.chose_fullscreen();
        self.playback.dock_focus = false;
        self.send_playback(if fullscreen {
            Command::Fullscreen
        } else {
            Command::ExitFullscreen
        })
    }

    /// Turning the phone cuts straight to or from fullscreen: the system
    /// already animates the rotation, so no still flies.
    pub(super) fn turn_fullscreen(&mut self, turn: super::pocket_ui::Turn, cx: &mut Context<Self>) {
        let enter = turn == super::pocket_ui::Turn::Enter;
        if self.send_playback(if enter {
            Command::Fullscreen
        } else {
            Command::ExitFullscreen
        }) {
            self.playback.motion.retarget(enter, false, true);
            cx.notify();
        }
    }

    pub(super) fn player_zap(&mut self, direction: ChannelDirection, cx: &mut Context<Self>) {
        self.zap(direction, false, cx);
    }

    fn arm_controls(&mut self, cx: &mut Context<Self>) {
        self.playback.idle = None;
        if self.playback.controls_visible
            && self.playback.menu.is_none()
            && self.playback.stage() == PlaybackStage::Playing
        {
            self.playback.idle = Some(cx.spawn(async move |view, cx| {
                cx.background_executor().timer(CONTROLS_IDLE).await;
                let _ = view.update(cx, |view, cx| {
                    if view.pointer_holds_band() {
                        view.arm_controls(cx);
                    } else if view.playback.fullscreen()
                        && view.playback.stage() == PlaybackStage::Playing
                    {
                        view.playback.controls_visible = false;
                        view.playback.retarget_motion(cx);
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
