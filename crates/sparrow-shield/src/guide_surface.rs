use std::{collections::HashSet, sync::Mutex, time::Duration};

use chrono::{DateTime, FixedOffset, Local, Timelike, Utc};
use gpui::{
    AnyWindowHandle, App, Context, FocusHandle, IntoElement, KeyDownEvent, Render, SharedString,
    Task, Window, WindowOptions, div, prelude::*, px, rgb,
};
use sparrow_core::{CatalogStatus, ChannelGroupView, CoreError, GuideWindowChannel, Page};
use sparrow_tv_guide::{
    GroupPageRequest, GuidePageRequest, PageStatus, PageUpdate,
    guide_screen::{
        GROUP_PREFETCH_DELAY, GroupPrefetchTicket, GuideMode, GuideRead, GuideScreen, LaneFocus,
        ScreenAction, ScreenFocus, ScreenKey, ToolbarFocus,
    },
    now_focus::{Direction, NowFocus},
    preview::{DescriptionPage, PreviewProgramme},
    timeline_focus::TimelineFocus,
};
use tokio::task::AbortHandle;

use crate::{catalog, guide_view};

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
    screen: GuideScreen,
    focus: FocusHandle,
    now_lane: guide_view::LaneViewport,
    timeline_rail: guide_view::RailViewport,
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
    _catalog_updates: Task<()>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreviewFrame {
    Idle,
    WaitingForPaint,
    Scheduled,
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
        Self {
            screen,
            focus,
            now_lane: guide_view::LaneViewport::default(),
            timeline_rail: guide_view::RailViewport::default(),
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
            _catalog_updates: catalog_updates,
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
            self.group_dwell = None;
            self.group_ticket = None;
            self.retire_preview_read();
            self.preview_frame = PreviewFrame::Idle;
            self.feedback = None;
            self.feedback_timeout = None;
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
                                view.set_clock(clock);
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
            self.status_revision = Some(revision);
            self.screen.catalog_status(status);
            self.sync_view(cx);
        }
    }

    fn set_clock(&mut self, clock: DateTime<FixedOffset>) {
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
        self.screen.set_clock(clock.to_utc(), *clock.offset());
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = match event.keystroke.key.as_str() {
            "up" => ScreenKey::Arrow(Direction::Up),
            "down" => ScreenKey::Arrow(Direction::Down),
            "left" => ScreenKey::Arrow(Direction::Left),
            "right" => ScreenKey::Arrow(Direction::Right),
            "enter" => ScreenKey::Activate,
            _ => return,
        };
        if self.activation.is_none() {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if matches!(key, ScreenKey::Arrow(_))
            && (modifiers.control || modifiers.alt || modifiers.platform)
        {
            return;
        }
        let now = Local::now().fixed_offset();
        let prior_mode = self.screen.mode();
        let action = self.screen.press(key, now.to_utc(), *now.offset());
        if self.screen.mode() != prior_mode {
            self.now_lane = guide_view::LaneViewport::default();
            self.timeline_rail = guide_view::RailViewport::default();
        }
        self.feedback = match action {
            ScreenAction::Unavailable => Some("This action is unavailable."),
            ScreenAction::None => None,
        };
        self.feedback_timeout = self.feedback.map(|_| {
            cx.spawn(async |view, cx| {
                cx.background_executor().timer(FEEDBACK_DURATION).await;
                let _ = view.update(cx, |view, cx| {
                    view.feedback = None;
                    cx.notify();
                });
            })
        });
        self.sync_view(cx);
    }

    fn sync_view(&mut self, cx: &mut Context<Self>) {
        let Some(activation) = self.activation else {
            // In-flight reads complete into the reducer while hidden. Retain any
            // newly queued requests so their pager tickets remain valid on resume.
            return;
        };
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

fn publish_diagnostics(screen: &GuideScreen) {
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

impl Render for GuideSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.screen
            .set_surface_height(guide_view::surface_height(window.viewport_size()));
        self.schedule_preview_frame(window, cx);
        publish_diagnostics(&self.screen);
        div()
            .id("sparrow-guide")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .child(guide_view::render(
                &self.screen,
                self.now,
                self.presented_preview.as_ref(),
                self.description.as_ref(),
                (&mut self.now_lane, &mut self.timeline_rail),
                window,
            ))
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
            })
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
