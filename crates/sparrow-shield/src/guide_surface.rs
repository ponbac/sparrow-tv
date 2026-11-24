use std::{collections::HashSet, sync::Mutex, time::Duration};

use chrono::{DateTime, FixedOffset, Local, Timelike, Utc};
use gpui::{
    AnyWindowHandle, App, Context, FocusHandle, IntoElement, KeyDownEvent, Render, SharedString,
    Task, Window, WindowOptions, div, prelude::*, px, rgb,
};
use sparrow_core::{
    CatalogGeneration, CatalogStatus, ChannelGroupView, ChannelId, CoreError, GuideWindowChannel,
    Page, PageLimit, PageRequest, ScheduleQuery,
};
use sparrow_tv_guide::{
    GroupPageRequest, GuidePageRequest, GuideWindow, PageStatus, PageUpdate,
    now_focus::{Direction, NowFocus},
    now_screen::{
        GROUP_PREFETCH_DELAY, GroupPrefetchTicket, LaneFocus, NowRead, NowScreen, ScreenAction,
        ScreenFocus, ScreenKey, ToolbarFocus,
    },
};

use crate::{catalog, now_view};

const PREVIEW_DWELL: Duration = Duration::from_millis(300);
const FEEDBACK_DURATION: Duration = Duration::from_millis(1500);
const MAX_PREVIEW_DESCRIPTION_BYTES: usize = 1024;

#[derive(Clone, Copy)]
pub(super) struct GuideDiagnostics {
    pub(super) focus: &'static str,
    pub(super) channel: Option<u32>,
    pub(super) group_index: Option<usize>,
    pub(super) chip_index: Option<usize>,
    pub(super) first_row: usize,
    pub(super) scroll_offset_units: u32,
    pub(super) loaded_families: usize,
    pub(super) page_status: Option<PageStatus>,
}

pub(super) static GUIDE_DIAGNOSTICS: Mutex<GuideDiagnostics> = Mutex::new(GuideDiagnostics {
    focus: "None",
    channel: None,
    group_index: None,
    chip_index: None,
    first_row: 0,
    scroll_offset_units: 0,
    loaded_families: 0,
    page_status: None,
});

pub(super) struct GuideSurface {
    screen: NowScreen,
    focus: FocusHandle,
    lane_viewport: now_view::LaneViewport,
    now: DateTime<FixedOffset>,
    description: Option<SharedString>,
    preview: Option<PreviewKey>,
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

#[derive(Clone, PartialEq, Eq)]
struct PreviewKey {
    generation: CatalogGeneration,
    channel: ChannelId,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    ordinal: usize,
    from: DateTime<Utc>,
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
        let mut screen = NowScreen::new(
            GuideWindow::at(now.to_utc(), *now.offset(), false),
            HashSet::new(),
        );
        screen.set_viewport_height(now_view::board_height(window.viewport_size()));
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
            lane_viewport: now_view::LaneViewport::default(),
            now,
            description: None,
            preview: None,
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
            self.preview_dwell = None;
            if self.description.is_none() {
                self.preview = None;
            }
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
        self.now = clock;
        self.screen
            .set_window(GuideWindow::at(clock.to_utc(), *clock.offset(), false));
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
        let action = self.screen.press(key);
        self.feedback = match action {
            ScreenAction::Unavailable => Some("This action is unavailable."),
            ScreenAction::OpenTimeline => Some("Timeline is unavailable."),
            _ => None,
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
        self.refresh_description(cx);
        cx.notify();
    }

    fn dispatch_reads(&mut self, cx: &Context<Self>) {
        for read in self.screen.take_reads() {
            let task = match read {
                NowRead::Status => {
                    catalog::reconcile_status();
                    continue;
                }
                NowRead::Groups(request) => catalog::read(move |core| {
                    let result = core.list_groups(request.query().clone());
                    ReadResult::Groups(request, result)
                }),
                NowRead::Guide(request) => catalog::read(move |core| {
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

    fn refresh_description(&mut self, cx: &Context<Self>) {
        let key = self.screen.generation().and_then(|generation| {
            let (row, programme) = now_view::preview_programme(&self.screen, self.now)?;
            let starts_at = programme.starts_at();
            let ends_at = programme.ends_at();
            let ordinal = row
                .programmes()
                .iter()
                .take_while(|candidate| !std::ptr::eq(*candidate, programme))
                .filter(|candidate| {
                    candidate.starts_at() == starts_at && candidate.ends_at() == ends_at
                })
                .count();
            Some(PreviewKey {
                generation,
                channel: row.channel().id().clone(),
                starts_at,
                ends_at,
                ordinal,
                from: starts_at.max(self.screen.displayed_window().starts_at()),
            })
        });
        if self.preview == key {
            return;
        }
        self.preview.clone_from(&key);
        self.description = None;
        self.preview_dwell = key.map(|key| {
            cx.spawn(async move |view, cx| {
                cx.background_executor().timer(PREVIEW_DWELL).await;
                let description = read_description(&key).await;
                let _ = view.update(cx, |view, cx| {
                    if view.activation.is_some() && view.preview.as_ref() == Some(&key) {
                        view.description = description;
                        cx.notify();
                    }
                });
            })
        });
    }
}

async fn read_description(key: &PreviewKey) -> Option<SharedString> {
    let limit = PageLimit::new(4).ok()?;
    let mut cursor = None;
    let mut remaining = key.ordinal;
    loop {
        let query = ScheduleQuery::new(key.channel.clone(), PageRequest::new(cursor, limit))
            .with_from(key.from);
        let page = catalog::read(move |core| core.schedule(query))?
            .await
            .ok()?
            .ok()?;
        if page.generation() != key.generation {
            return None;
        }
        for programme in page.items() {
            if programme.starts_at() > key.starts_at {
                return None;
            }
            if programme.starts_at() == key.starts_at && programme.ends_at() == key.ends_at {
                if remaining == 0 {
                    return programme.description().map(|description| {
                        let end = description.floor_char_boundary(MAX_PREVIEW_DESCRIPTION_BYTES);
                        SharedString::from(&description[..end])
                    });
                }
                remaining -= 1;
            }
        }
        cursor = Some(page.next()?.clone());
    }
}

fn publish_diagnostics(screen: &NowScreen) {
    let (focus, family, group_index, chip_index) = match screen.focus() {
        ScreenFocus::None => ("None", None, None, None),
        ScreenFocus::Toolbar(ToolbarFocus::Search) => ("Search", None, None, None),
        ScreenFocus::Toolbar(ToolbarFocus::Sources) => ("Sources", None, None, None),
        ScreenFocus::GroupLane(LaneFocus::Phase) => ("Now", None, None, None),
        ScreenFocus::GroupLane(LaneFocus::All) => ("All", None, None, None),
        ScreenFocus::GroupLane(LaneFocus::Group(index)) => ("Group", None, Some(index), None),
        ScreenFocus::GroupLane(LaneFocus::ChooseGroups) => ("ChooseGroups", None, None, None),
        ScreenFocus::Board(NowFocus::Channel(family)) => ("Channel", Some(family), None, None),
        ScreenFocus::Board(NowFocus::Variant { family, chip }) => {
            ("Quality", Some(family), None, Some(chip))
        }
        ScreenFocus::Board(NowFocus::Retry) => ("Retry", None, None, None),
    };
    let channel = family
        .and_then(|index| screen.families().get(index))
        .map(|row| row.number());
    *GUIDE_DIAGNOSTICS.lock().unwrap() = GuideDiagnostics {
        focus,
        channel,
        group_index,
        chip_index,
        first_row: screen.first_visible_row(),
        scroll_offset_units: screen.scroll_offset().round() as u32,
        loaded_families: screen.families().len(),
        page_status: screen.page_status(),
    };
}

impl Render for GuideSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.screen
            .set_viewport_height(now_view::board_height(window.viewport_size()));
        publish_diagnostics(&self.screen);
        div()
            .id("sparrow-guide")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .child(now_view::render(
                &self.screen,
                self.now,
                self.description.as_ref(),
                &mut self.lane_viewport,
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
