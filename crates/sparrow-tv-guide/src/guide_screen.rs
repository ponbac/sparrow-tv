//! Remote-control focus, viewport, and catalog reads for the television Guide.

use std::{
    collections::{HashSet, VecDeque},
    ops::Range,
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use sparrow_core::{
    CatalogGeneration, CatalogStatus, ChannelGroupFilter, ChannelGroupView, ChannelId, CoreError,
    GuideProgramme, GuideWindowChannel, Page,
};

use crate::{
    BoardView, ChannelGroupPages, GroupPageRequest, GuideFamily, GuidePageRequest, GuidePages,
    GuideWindow, PageStatus, PageUpdate, VariantPreferences,
    group_chooser::{GroupChooser, GroupChooserFocus, GroupChooserIntent, GroupRosterCounts},
    now_focus::{Direction, NowFocus, NowMove, NowNavigation},
    time::{ProgrammeFocus, ProgrammeStep, Timeline},
    timeline_focus::{TimelineFocus, TimelineMove, TimelineNavigation},
    viewport::VerticalViewport,
};

pub const NOW_FOOTER_GAP: f32 = 18.;
pub const NOW_RETAINED_RETRY_HEIGHT: f32 = 78.;
pub const NOW_MORE_HEIGHT: f32 = 54.;

const FOCUS_MARGIN: f32 = 24.;
const BOARD_ENTRY_TOLERANCE: f32 = 8.;
pub const GROUP_PREFETCH_DELAY: Duration = Duration::from_millis(150);
const CACHED_GROUPS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolbarFocus {
    Search,
    Sources,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaneFocus {
    Phase,
    All,
    Group(usize),
    ChooseGroups,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuideMode {
    Now,
    Timeline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenFocus {
    None,
    Toolbar(ToolbarFocus),
    GroupLane(LaneFocus),
    Board(NowFocus),
    TimelineBoard(TimelineFocus),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenKey {
    Arrow(Direction),
    Activate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScreenAction {
    None,
    Unavailable,
    EditGroupFilter,
    GroupVisibilityChanged(GroupVisibilityChange),
}

/// A Channel Group visibility edit already applied optimistically to the Guide.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GroupVisibilityChange {
    Set {
        name: String,
        hidden: bool,
    },
    /// Clears every exclusion, including groups absent from the current catalog.
    ShowAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenNotice {
    Loading,
    NotConfigured,
    CatalogUnavailable,
    Empty,
    AllGroupsHidden,
    ReadFailed,
}

pub enum GuideRead {
    Status,
    Groups(GroupPageRequest),
    Guide(GuidePageRequest),
}

/// Remains valid only while the same Channel Group control holds focus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupPrefetchTicket(u64);

struct CachedGroup {
    key: Option<ChannelGroupFilter>,
    pages: GuidePages,
}

#[derive(Clone, Copy)]
struct FamilyTarget {
    index: usize,
    number: u32,
}

impl FamilyTarget {
    fn resolve(self, families: &[GuideFamily]) -> Option<usize> {
        if families
            .get(self.index)
            .is_some_and(|family| family.number() == self.number)
        {
            Some(self.index)
        } else {
            families
                .iter()
                .position(|family| family.number() == self.number)
        }
    }
}

#[derive(Clone)]
struct ProgrammeTarget {
    family: FamilyTarget,
    programme: ProgrammeFocus,
    cell: Option<usize>,
}

struct PreviewFocus {
    family: FamilyTarget,
    programme: Option<ProgrammeTarget>,
}

struct BoardTarget {
    family: FamilyTarget,
    variant: Option<ChannelId>,
    programme: Option<ProgrammeFocus>,
}

#[derive(Clone, Copy)]
enum FocusZone {
    None,
    Toolbar(ToolbarFocus),
    GroupLane(LaneFocus),
    Board,
}

enum BoardMode {
    Now {
        navigation: NowNavigation,
        focus: NowFocus,
    },
    Timeline {
        navigation: TimelineNavigation,
        focus: TimelineFocus,
        axis: Timeline,
        return_target: Option<ProgrammeTarget>,
    },
}

/// Row geometry of one Guide board, in the host's layout units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoardGeometry {
    /// Surface y of the board's top edge.
    pub top: f32,
    pub row_top: f32,
    pub row_height: f32,
    pub pitch: f32,
    pub columns: usize,
    /// Board height hidden under a pinned header, such as the Timeline ruler.
    pub covered_top: f32,
    pub bottom_padding: f32,
}

/// Geometry for both boards and the surface they sit on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoardMetrics {
    pub now: BoardGeometry,
    pub timeline: BoardGeometry,
    pub surface_height: f32,
}

impl BoardMetrics {
    /// The television Now board in 1920-wide reference units.
    pub const TV_NOW: BoardGeometry = BoardGeometry {
        top: 522.,
        row_top: 4.,
        row_height: 100.,
        pitch: 111.,
        columns: 3,
        covered_top: 0.,
        bottom_padding: 24.,
    };
    /// The television Timeline board in 1920-wide reference units.
    pub const TV_TIMELINE: BoardGeometry = BoardGeometry {
        top: 444.,
        row_top: 48.,
        row_height: 83.,
        pitch: 83.,
        columns: 1,
        covered_top: 48.,
        bottom_padding: 60.,
    };

    pub const fn tv(surface_height: f32) -> Self {
        Self {
            now: Self::TV_NOW,
            timeline: Self::TV_TIMELINE,
            surface_height,
        }
    }

    pub const fn board(&self, mode: GuideMode) -> BoardGeometry {
        match mode {
            GuideMode::Now => self.now,
            GuideMode::Timeline => self.timeline,
        }
    }
}

impl BoardGeometry {
    pub fn row(&self, index: usize) -> Range<f32> {
        let top = self.row_top + (index / self.columns) as f32 * self.pitch;
        top..top + self.row_height
    }
}

/// The host dispatches read intents off the UI thread and delivers the results on it.
/// Focus movement over loaded families never waits for a catalog read.
pub struct GuideScreen {
    content_revision: u64,
    window: GuideWindow,
    exclusions: HashSet<String>,
    preferences: Arc<VariantPreferences>,
    configured: Option<bool>,
    open_failed: bool,
    generation: Option<CatalogGeneration>,
    group_pages: Option<ChannelGroupPages>,
    visible_groups: Vec<ChannelGroupView>,
    group_chooser: Option<GroupChooser>,
    selected: Option<ChannelGroupFilter>,
    selected_lane: LaneFocus,
    pages: Option<GuidePages>,
    cached: VecDeque<CachedGroup>,
    mode: BoardMode,
    zone: FocusZone,
    extended: bool,
    viewing_instant: Option<DateTime<Utc>>,
    preview: Option<PreviewFocus>,
    focus_epoch: u64,
    viewport: VerticalViewport,
    metrics: BoardMetrics,
    /// Arrow keys reveal focus; pointer scrolling owns the offset until the next one.
    follow_focus: bool,
    reads: Vec<GuideRead>,
}

impl GuideScreen {
    pub fn new(now: DateTime<Utc>, offset: FixedOffset, exclusions: HashSet<String>) -> Self {
        Self {
            content_revision: 0,
            window: GuideWindow::at(now, offset, false),
            exclusions,
            preferences: Arc::default(),
            configured: None,
            open_failed: false,
            generation: None,
            group_pages: None,
            visible_groups: Vec::new(),
            group_chooser: None,
            selected: None,
            selected_lane: LaneFocus::All,
            pages: None,
            cached: VecDeque::new(),
            mode: BoardMode::Now {
                navigation: NowNavigation::default(),
                focus: NowFocus::Channel(0),
            },
            zone: FocusZone::None,
            extended: false,
            viewing_instant: None,
            preview: None,
            focus_epoch: 0,
            viewport: VerticalViewport::default(),
            metrics: BoardMetrics::tv(0.),
            follow_focus: true,
            reads: Vec::new(),
        }
    }

    /// Changes when catalog data displayed by the Guide may have changed.
    pub fn content_revision(&self) -> u64 {
        self.content_revision
    }

    fn content_changed(&mut self) {
        self.content_revision = self.content_revision.wrapping_add(1);
    }

    pub fn generation(&self) -> Option<CatalogGeneration> {
        self.generation
    }

    pub fn mode(&self) -> GuideMode {
        match self.mode {
            BoardMode::Now { .. } => GuideMode::Now,
            BoardMode::Timeline { .. } => GuideMode::Timeline,
        }
    }

    pub fn timeline(&self) -> Option<Timeline> {
        match self.mode {
            BoardMode::Now { .. } => None,
            BoardMode::Timeline { axis, .. } => Some(axis),
        }
    }

    pub fn focus(&self) -> ScreenFocus {
        match self.zone {
            FocusZone::None => ScreenFocus::None,
            FocusZone::Toolbar(focus) => ScreenFocus::Toolbar(focus),
            FocusZone::GroupLane(focus) => ScreenFocus::GroupLane(focus),
            FocusZone::Board => match self.mode {
                BoardMode::Now { focus, .. } => ScreenFocus::Board(focus),
                BoardMode::Timeline { focus, .. } => ScreenFocus::TimelineBoard(focus),
            },
        }
    }

    /// Reveals the preferred Channel in the loaded Guide, preserving its current
    /// focusable control when returning from fullscreen.
    pub fn restore_playback_focus(&mut self, channel: Option<&ChannelId>, preserve_origin: bool) {
        let family = channel.and_then(|channel| {
            self.families().iter().position(|family| {
                family
                    .variants()
                    .iter()
                    .any(|variant| variant.channel().id() == channel)
            })
        });
        let current = self.focus();
        self.cancel_pending_navigation();
        self.follow_focus = true;
        if preserve_origin && family.is_some() && family == board_family(current) {
            self.reveal_focus();
        } else if let Some(index) = family.or_else(|| (!self.families().is_empty()).then_some(0)) {
            self.move_focus(match self.mode() {
                GuideMode::Now => ScreenFocus::Board(NowFocus::Channel(index)),
                GuideMode::Timeline => ScreenFocus::TimelineBoard(TimelineFocus::Channel(index)),
            });
        } else {
            self.move_focus(ScreenFocus::None);
            self.recover_focus();
        }
        self.schedule_reads();
    }

    pub fn selected_group(&self) -> Option<&ChannelGroupFilter> {
        self.selected.as_ref()
    }

    pub fn clear_group_selection(&mut self) {
        self.select_group(None);
    }

    pub fn groups(&self) -> &[ChannelGroupView] {
        &self.visible_groups
    }

    pub fn exclusions(&self) -> &HashSet<String> {
        &self.exclusions
    }

    pub fn variant_preferences(&self) -> &Arc<VariantPreferences> {
        &self.preferences
    }

    pub fn set_variant_preferences(&mut self, preferences: Arc<VariantPreferences>) {
        if self.preferences == preferences {
            return;
        }
        let focused = self.board_target();
        if self.timeline().is_some() {
            self.cancel_pending_navigation();
        }
        self.content_changed();
        self.preferences = preferences;
        self.reconcile_timeline_return();
        self.reconcile_preview();
        self.restore_board_target(focused);
    }

    pub fn group_chooser(&self) -> Option<&GroupChooser> {
        self.group_chooser.as_ref()
    }

    /// The unfiltered catalog roster, including hidden groups.
    pub fn group_roster(&self) -> &[ChannelGroupView] {
        self.group_pages
            .as_ref()
            .map_or(&[], ChannelGroupPages::groups)
    }

    pub fn group_roster_counts(&self) -> GroupRosterCounts {
        let shown = self.visible_groups.len();
        GroupRosterCounts {
            shown,
            hidden: self.group_roster().len() - shown,
        }
    }

    pub fn group_roster_status(&self) -> Option<PageStatus> {
        self.group_pages.as_ref().map(ChannelGroupPages::status)
    }

    pub fn group_chooser_can_show_all(&self) -> bool {
        !self.exclusions.is_empty()
    }

    /// Applies a complete preference snapshot without changing the active Guide mode.
    pub fn set_group_exclusions(&mut self, exclusions: HashSet<String>) -> bool {
        self.apply_group_exclusions(exclusions, self.selected.clone())
    }

    pub fn group_chooser_intent(&mut self, intent: GroupChooserIntent) -> ScreenAction {
        if self.group_chooser.is_none() {
            return ScreenAction::None;
        }
        match intent {
            GroupChooserIntent::Close => {
                self.close_group_chooser();
                ScreenAction::None
            }
            GroupChooserIntent::Filter(filter) => {
                let groups = self
                    .group_pages
                    .as_ref()
                    .map_or(&[][..], ChannelGroupPages::groups);
                if let Some(chooser) = &mut self.group_chooser {
                    chooser.change_filter(filter, groups);
                }
                self.focus_epoch = self.focus_epoch.wrapping_add(1);
                ScreenAction::None
            }
            GroupChooserIntent::Activate(target) => self.activate_group_chooser(target),
        }
    }

    pub fn families(&self) -> &[GuideFamily] {
        self.pages.as_ref().map_or(&[], GuidePages::families)
    }

    /// Keeps the last focused Channel in the preview while browsing other controls.
    pub fn preview_family(&self) -> Option<&GuideFamily> {
        self.remembered_preview_family()
            .or_else(|| self.families().first())
    }

    /// Index into the same schedule used to render the preview family.
    pub fn preview_cell(&self) -> Option<usize> {
        self.remembered_preview_family()?;
        self.preview.as_ref()?.programme.as_ref()?.cell
    }

    fn remembered_preview_family(&self) -> Option<&GuideFamily> {
        let target = self.preview.as_ref()?.family;
        self.families()
            .get(target.index)
            .filter(|family| family.number() == target.number)
    }

    pub fn displayed_window(&self) -> GuideWindow {
        self.pages
            .as_ref()
            .map_or(self.window, GuidePages::displayed_window)
    }

    pub fn page_status(&self) -> Option<PageStatus> {
        self.pages.as_ref().map(GuidePages::displayed_status)
    }

    pub fn notice(&self) -> Option<ScreenNotice> {
        if self.open_failed {
            return Some(ScreenNotice::CatalogUnavailable);
        }
        if !self.families().is_empty() {
            return None;
        }
        match self.configured {
            None => return Some(ScreenNotice::Loading),
            Some(false) => return Some(ScreenNotice::NotConfigured),
            Some(true) if self.generation.is_none() => {
                return Some(ScreenNotice::CatalogUnavailable);
            }
            Some(true) => {}
        }
        if self.retry_available() {
            return Some(ScreenNotice::ReadFailed);
        }
        if self.all_groups_hidden() {
            return Some(ScreenNotice::AllGroupsHidden);
        }
        match self.page_status() {
            Some(PageStatus::Complete) => Some(ScreenNotice::Empty),
            _ => Some(ScreenNotice::Loading),
        }
    }

    pub fn retry_available(&self) -> bool {
        self.board().is_some_and(BoardView::failed)
    }

    pub fn board_metrics(&self) -> BoardMetrics {
        self.metrics
    }

    /// Board geometry and surface height, in the host's layout units.
    pub fn set_board_metrics(&mut self, metrics: BoardMetrics) {
        if self.metrics != metrics {
            self.metrics = metrics;
            self.viewport.height = (metrics.surface_height - self.geometry().top).max(0.);
            self.reveal_focus();
        }
    }

    fn geometry(&self) -> BoardGeometry {
        self.metrics.board(self.mode())
    }

    fn resize_board(&mut self) {
        let height = (self.metrics.surface_height - self.geometry().top).max(0.);
        if self.viewport.height != height {
            self.viewport.height = height;
            self.reveal_focus();
        }
    }

    pub fn scroll_offset(&self) -> f32 {
        self.viewport.offset
    }

    pub fn first_visible_row(&self) -> usize {
        let geometry = self.geometry();
        if self.viewport.height <= geometry.covered_top {
            return self.families().len().div_ceil(geometry.columns);
        }
        self.first_row_after(self.scroll_offset() + geometry.covered_top)
    }

    /// Family indices for intersecting rows, including overscan on each side.
    pub fn visible_families(&self, overscan: usize) -> Range<usize> {
        let available = self.families().len();
        let geometry = self.geometry();
        if self.viewport.height <= geometry.covered_top {
            return available..available;
        }
        let first = self.first_visible_row().saturating_sub(overscan) * geometry.columns;
        let bottom = self.scroll_offset() + self.viewport.height;
        let end = ((bottom - geometry.row_top) / geometry.pitch).ceil() as usize;
        first.min(available)
            ..end
                .saturating_add(overscan)
                .saturating_mul(geometry.columns)
                .min(available)
    }

    /// The board-relative top of the footer, pinned to the viewport for a retained error.
    pub fn footer_top(&self) -> f32 {
        let top = self.rows_bottom() + NOW_FOOTER_GAP - self.scroll_offset();
        if self.retained_retry() {
            top.min((self.viewport.height - NOW_RETAINED_RETRY_HEIGHT).max(0.))
        } else {
            top
        }
    }

    pub fn take_reads(&mut self) -> Vec<GuideRead> {
        std::mem::take(&mut self.reads)
    }

    /// Call on catalog notifications, or let a rejected cursor queue the same read.
    pub fn request_status(&mut self) {
        if !self
            .reads
            .iter()
            .any(|read| matches!(read, GuideRead::Status))
        {
            self.reads.push(GuideRead::Status);
        }
    }

    pub fn catalog_open_failed(&mut self) {
        self.open_failed = true;
        self.recover_focus();
    }

    pub fn catalog_status(&mut self, status: &CatalogStatus) {
        let displayed = self.displayed_window();
        self.content_changed();
        self.open_failed = false;
        self.configured = Some(status.configuration().is_configured());
        let generation = status.generation();
        if generation != self.generation {
            self.generation = generation;
            self.preview = None;
            self.clear_timeline_return();
            self.group_pages = generation.map(ChannelGroupPages::new);
            self.refresh_chooser_matches();
            self.visible_groups.clear();
            self.selected_lane = LaneFocus::All;
            self.cached.clear();
            self.viewport.offset = 0.;
            self.follow_focus = true;
            self.focus_epoch = self.focus_epoch.wrapping_add(1);
            self.cancel_pending_navigation();
            self.reads.clear();
            match self.focus() {
                ScreenFocus::Board(_) | ScreenFocus::TimelineBoard(_) => {
                    self.move_focus(ScreenFocus::None)
                }
                ScreenFocus::GroupLane(LaneFocus::Group(_)) => {
                    self.move_focus(ScreenFocus::GroupLane(LaneFocus::All));
                }
                _ => {}
            }
            self.pages = generation.map(|generation| {
                GuidePages::new(
                    generation,
                    self.selected.clone(),
                    self.window,
                    self.exclusions.clone(),
                )
            });
        } else if let Some(generation) = generation {
            if let Some(groups) = &mut self.group_pages
                && groups.status() == PageStatus::ReconcileGeneration
            {
                groups.select(generation);
                self.refresh_chooser_matches();
                self.visible_groups.clear();
                self.selected_lane = LaneFocus::All;
                if matches!(self.focus(), ScreenFocus::GroupLane(LaneFocus::Group(_))) {
                    self.move_focus(ScreenFocus::GroupLane(LaneFocus::All));
                }
            }
            if let Some(pages) = &mut self.pages
                && pages.status() == PageStatus::ReconcileGeneration
            {
                pages.select(generation, self.selected.clone(), self.window);
            }
        }
        self.reanchor_timeline(displayed);
        self.recover_focus();
        self.schedule_reads();
    }

    /// Extends this Guide's reads to eight hours, retaining the visible rows
    /// while the wider window loads. Returns whether the query window changed.
    pub fn enable_extended_window(&mut self, now: DateTime<Utc>, offset: FixedOffset) -> bool {
        self.extended = true;
        self.set_clock(now, offset)
    }

    /// The selected time chip. `None` follows the Guide clock.
    pub fn viewing_instant(&self) -> Option<DateTime<Utc>> {
        self.viewing_instant
    }

    /// Selects a time chip without changing the query window.
    pub fn set_viewing_instant(&mut self, instant: Option<DateTime<Utc>>, now: DateTime<Utc>) {
        self.viewing_instant = instant.filter(|instant| *instant > now);
    }

    /// Advances the Guide clock and reports whether it requested a new window.
    pub fn set_clock(&mut self, now: DateTime<Utc>, offset: FixedOffset) -> bool {
        self.set_viewing_instant(self.viewing_instant, now);
        let window = GuideWindow::at(now, offset, self.extended);
        if self.window == window {
            return false;
        }
        let displayed = self.displayed_window();
        self.window = window;
        self.focus_epoch = self.focus_epoch.wrapping_add(1);
        self.cached.clear();
        self.reads
            .retain(|read| !matches!(read, GuideRead::Guide(_)));
        if let (Some(generation), Some(pages)) = (self.generation, &mut self.pages) {
            pages.select(generation, self.selected.clone(), window);
        }
        self.reanchor_timeline(displayed);
        self.recover_focus();
        self.schedule_reads();
        true
    }

    pub fn groups_completed(
        &mut self,
        request: GroupPageRequest,
        result: Result<Page<ChannelGroupView>, CoreError>,
    ) -> PageUpdate {
        let Some(pages) = &mut self.group_pages else {
            return PageUpdate::Superseded;
        };
        let prior_len = pages.groups().len();
        let update = pages.complete(request, result);
        if update == PageUpdate::Applied {
            self.visible_groups.extend(
                pages.groups()[prior_len..]
                    .iter()
                    .filter(|group| !self.exclusions.contains(group.name()))
                    .cloned(),
            );
            let groups_complete = pages.status() == PageStatus::Complete;
            self.refresh_chooser_matches();
            if self.selected_lane == LaneFocus::All {
                self.update_selected_lane();
            }
            if groups_complete && self.selected.is_some() && self.selected_lane == LaneFocus::All {
                self.select_group(None);
            }
        }
        self.apply_update(update);
        update
    }

    pub fn guide_completed(
        &mut self,
        request: GuidePageRequest,
        result: Result<Page<GuideWindowChannel>, CoreError>,
    ) -> PageUpdate {
        let group = request.query().channels().group();
        let active = group == self.selected.as_ref().map(ChannelGroupFilter::as_str);
        if active {
            let focused = self.board_target();
            let displayed = self.displayed_window();
            let Some(pages) = &mut self.pages else {
                return PageUpdate::Superseded;
            };
            let update = pages.complete(request, result);
            if update != PageUpdate::Superseded {
                self.reanchor_timeline(displayed);
                self.restore_board_target(focused);
                self.reconcile_timeline_return();
            }
            self.apply_update(update);
            return update;
        }
        let Some(index) = self
            .cached
            .iter()
            .position(|entry| entry.key.as_ref().map(ChannelGroupFilter::as_str) == group)
        else {
            return PageUpdate::Superseded;
        };
        let update = self.cached[index].pages.complete(request, result);
        if matches!(update, PageUpdate::Failed | PageUpdate::ReconcileGeneration) {
            self.cached.remove(index);
        }
        if update == PageUpdate::ReconcileGeneration {
            self.request_status();
        }
        update
    }

    pub fn prefetch_ticket(&self) -> Option<GroupPrefetchTicket> {
        self.generation?;
        let group = self.focused_group()?;
        (group != self.selected
            && group
                .as_ref()
                .is_none_or(|group| !self.exclusions.contains(group.as_str()))
            && !self.cached.iter().any(|entry| entry.key == group))
        .then_some(GroupPrefetchTicket(self.focus_epoch))
    }

    /// The host calls this when `GROUP_PREFETCH_DELAY` has elapsed.
    pub fn prefetch(&mut self, ticket: GroupPrefetchTicket) {
        if self.prefetch_ticket() != Some(ticket) {
            return;
        }
        if let (Some(generation), Some(group)) = (self.generation, self.focused_group()) {
            let mut pages = GuidePages::new(
                generation,
                group.clone(),
                self.window,
                self.exclusions.clone(),
            );
            if let Some(request) = pages.request_next() {
                self.reads.push(GuideRead::Guide(request));
            }
            self.cache_group(group, pages);
        }
    }

    pub fn press(
        &mut self,
        key: ScreenKey,
        now: DateTime<Utc>,
        offset: FixedOffset,
    ) -> ScreenAction {
        if key != ScreenKey::Arrow(Direction::Down) {
            self.cancel_pending_navigation();
        }
        if matches!(key, ScreenKey::Arrow(_)) {
            self.follow_focus = true;
        }
        if let Some(focus) = self.group_chooser.as_ref().map(GroupChooser::focus) {
            return match key {
                ScreenKey::Activate => self.activate_group_chooser(focus),
                ScreenKey::Arrow(direction) => {
                    let can_show_all = self.group_chooser_can_show_all();
                    if let Some(chooser) = &mut self.group_chooser {
                        chooser.arrow(direction, can_show_all);
                        if chooser.focus() != focus {
                            self.focus_epoch = self.focus_epoch.wrapping_add(1);
                        }
                    }
                    ScreenAction::None
                }
            };
        }
        match key {
            ScreenKey::Activate => self.activate(now, offset),
            ScreenKey::Arrow(direction) => {
                self.arrow(direction, now);
                self.schedule_reads();
                ScreenAction::None
            }
        }
    }

    fn cancel_pending_navigation(&mut self) {
        match &mut self.mode {
            BoardMode::Now { navigation, .. } => *navigation = NowNavigation::default(),
            BoardMode::Timeline { navigation, .. } => navigation.cancel_pending(),
        }
    }

    fn activate(&mut self, now: DateTime<Utc>, offset: FixedOffset) -> ScreenAction {
        match self.focus() {
            ScreenFocus::None => ScreenAction::None,
            ScreenFocus::GroupLane(LaneFocus::Phase) => {
                self.toggle_mode(now, offset);
                ScreenAction::None
            }
            ScreenFocus::GroupLane(LaneFocus::All | LaneFocus::Group(_)) => {
                if let Some(group) = self.focused_group() {
                    self.select_group(group);
                }
                ScreenAction::None
            }
            ScreenFocus::Board(NowFocus::Retry)
            | ScreenFocus::TimelineBoard(TimelineFocus::Retry) => {
                self.retry();
                ScreenAction::None
            }
            ScreenFocus::GroupLane(LaneFocus::ChooseGroups) => {
                self.group_chooser = Some(GroupChooser::new(
                    self.group_roster(),
                    self.selected_group(),
                ));
                self.focus_epoch = self.focus_epoch.wrapping_add(1);
                ScreenAction::None
            }
            ScreenFocus::Toolbar(_) | ScreenFocus::Board(_) | ScreenFocus::TimelineBoard(_) => {
                ScreenAction::Unavailable
            }
        }
    }

    /// Returns the board to Now on a layout that offers no Timeline. Returns
    /// whether the board was showing Timeline.
    pub fn leave_timeline(&mut self, now: DateTime<Utc>, offset: FixedOffset) -> bool {
        let timeline = self.mode() == GuideMode::Timeline;
        if timeline {
            self.toggle_mode(now, offset);
        }
        timeline
    }

    fn toggle_mode(&mut self, now: DateTime<Utc>, offset: FixedOffset) {
        self.mode = match self.mode {
            BoardMode::Now { .. } => {
                self.extended = true;
                BoardMode::Timeline {
                    navigation: TimelineNavigation::default(),
                    focus: TimelineFocus::Channel(0),
                    axis: Timeline::new(self.displayed_window()),
                    return_target: None,
                }
            }
            BoardMode::Timeline { .. } => BoardMode::Now {
                navigation: NowNavigation::default(),
                focus: NowFocus::Channel(0),
            },
        };
        self.resize_board();
        self.set_clock(now, offset);
        self.reveal_focus();
    }

    fn arrow(&mut self, direction: Direction, now: DateTime<Utc>) {
        match self.focus() {
            ScreenFocus::None => self.recover_focus(),
            ScreenFocus::Toolbar(toolbar) => {
                if direction == Direction::Down && self.mode() == GuideMode::Timeline {
                    let focus = self.timeline_entry(now).map_or(
                        ScreenFocus::GroupLane(self.selected_lane()),
                        ScreenFocus::TimelineBoard,
                    );
                    self.move_focus(focus);
                    return;
                }
                let focus = match (toolbar, direction) {
                    (ToolbarFocus::Search, Direction::Right) => {
                        ScreenFocus::Toolbar(ToolbarFocus::Sources)
                    }
                    (ToolbarFocus::Sources, Direction::Left) => {
                        ScreenFocus::Toolbar(ToolbarFocus::Search)
                    }
                    (ToolbarFocus::Search, Direction::Down) => {
                        ScreenFocus::GroupLane(self.selected_lane())
                    }
                    (ToolbarFocus::Sources, Direction::Down) => {
                        ScreenFocus::GroupLane(LaneFocus::ChooseGroups)
                    }
                    _ => return,
                };
                self.move_focus(focus);
            }
            ScreenFocus::GroupLane(lane) => match self.mode() {
                GuideMode::Now => self.lane_arrow(lane, direction),
                GuideMode::Timeline => self.rail_arrow(lane, direction, now),
            },
            ScreenFocus::Board(focus) => {
                if let Some(board) = self.board() {
                    let count = focus
                        .family()
                        .map_or(0, |family| self.variant_count(family));
                    if let BoardMode::Now { navigation, .. } = &mut self.mode {
                        let movement = navigation.press(focus, direction, count, board);
                        self.apply_movement(movement);
                    }
                }
            }
            ScreenFocus::TimelineBoard(focus) => self.timeline_arrow(focus, direction, now),
        }
    }

    fn rail_arrow(&mut self, lane: LaneFocus, direction: Direction, now: DateTime<Utc>) {
        let last = self.visible_groups.len().checked_sub(1);
        let lane = match (lane, direction) {
            (_, Direction::Left) => return,
            (_, Direction::Right) => {
                if let Some(focus) = self.timeline_entry(now) {
                    self.move_focus(ScreenFocus::TimelineBoard(focus));
                }
                return;
            }
            (LaneFocus::Phase, Direction::Up) => {
                self.move_focus(ScreenFocus::Toolbar(ToolbarFocus::Sources));
                return;
            }
            (LaneFocus::All, Direction::Up) => LaneFocus::Phase,
            (LaneFocus::Group(0), Direction::Up) => LaneFocus::All,
            (LaneFocus::Group(index), Direction::Up) => LaneFocus::Group(index - 1),
            (LaneFocus::ChooseGroups, Direction::Up) => {
                last.map_or(LaneFocus::All, LaneFocus::Group)
            }
            (LaneFocus::Phase, Direction::Down) => LaneFocus::All,
            (LaneFocus::All, Direction::Down) => {
                last.map_or(LaneFocus::ChooseGroups, |_| LaneFocus::Group(0))
            }
            (LaneFocus::Group(index), Direction::Down) if Some(index) != last => {
                LaneFocus::Group(index + 1)
            }
            (LaneFocus::Group(_), Direction::Down) => LaneFocus::ChooseGroups,
            (LaneFocus::ChooseGroups, Direction::Down) => return,
        };
        self.move_focus(ScreenFocus::GroupLane(lane));
    }

    fn timeline_entry(&self, now: DateTime<Utc>) -> Option<TimelineFocus> {
        let BoardMode::Timeline {
            axis,
            return_target,
            ..
        } = &self.mode
        else {
            return None;
        };
        if let Some(target) = return_target
            && let Some(cell) = target.cell
        {
            return Some(TimelineFocus::Programme {
                family: target.family.index,
                cell,
            });
        }
        let first = self.board_entry_row();
        if let Some(family) = self.families().get(first) {
            let anchor = axis.anchor(now, None);
            let target = axis.cell_at(family.programmes(&self.preferences), anchor);
            Some(timeline_cell(first, target))
        } else {
            self.retry_available().then_some(TimelineFocus::Retry)
        }
    }

    fn timeline_arrow(&mut self, focus: TimelineFocus, direction: Direction, now: DateTime<Utc>) {
        let Some(axis) = self.timeline() else {
            return;
        };
        let current = focus
            .family()
            .and_then(|family| self.families().get(family));
        let focused = match focus {
            TimelineFocus::Programme { cell, .. } => {
                current.and_then(|family| family.programmes(&self.preferences).get(cell))
            }
            _ => None,
        };
        let anchor = axis.anchor(now, focused);
        match direction {
            Direction::Up | Direction::Down => {
                let Some(board) = self.board() else {
                    return;
                };
                let BoardMode::Timeline { navigation, .. } = &mut self.mode else {
                    return;
                };
                let movement = if direction == Direction::Up {
                    navigation.up(focus, anchor, board)
                } else {
                    navigation.down(focus, anchor, board)
                };
                if movement == TimelineMove::ToGroupLane && focus == TimelineFocus::Retry {
                    self.move_focus(ScreenFocus::GroupLane(self.selected_lane()));
                } else {
                    self.apply_timeline_movement(movement);
                }
            }
            Direction::Left | Direction::Right => {
                if let Some(index) = focus.family()
                    && let Some(family) = current
                {
                    let count = self.variant_count(index);
                    let cells = family.programmes(&self.preferences);
                    let target = self
                        .timeline_horizontal_target(focus, direction, anchor, axis, cells, count);
                    if target != self.focus() {
                        self.move_focus(target);
                    }
                }
            }
        }
    }

    fn timeline_horizontal_target(
        &self,
        focus: TimelineFocus,
        direction: Direction,
        anchor: DateTime<Utc>,
        axis: Timeline,
        cells: &[GuideProgramme],
        count: usize,
    ) -> ScreenFocus {
        let current = ScreenFocus::TimelineBoard(focus);
        let Some(family) = focus.family() else {
            return current;
        };
        let rail = ScreenFocus::GroupLane(self.selected_lane());
        let programme = |cell| TimelineFocus::Programme { family, cell };
        let variant = |chip| TimelineFocus::Variant { family, chip };
        let here = || axis.cell_at(cells, anchor).map(programme);
        let board = ScreenFocus::TimelineBoard;
        match (focus, direction) {
            (TimelineFocus::Channel(_), Direction::Left)
            | (TimelineFocus::Variant { chip: 0, .. }, Direction::Left) => rail,
            (TimelineFocus::Channel(_), Direction::Right) => here()
                .or_else(|| (count > 0).then(|| variant(0)))
                .map_or(current, board),
            (TimelineFocus::Programme { cell, .. }, Direction::Left) => axis
                .step_programme(cells, cell, ProgrammeStep::Earlier)
                .map(programme)
                .or_else(|| count.checked_sub(1).map(variant))
                .map_or(rail, board),
            (TimelineFocus::Programme { cell, .. }, Direction::Right) => axis
                .step_programme(cells, cell, ProgrammeStep::Later)
                .map(programme)
                .map_or(current, board),
            (TimelineFocus::Variant { chip, .. }, Direction::Left) => board(variant(chip - 1)),
            (TimelineFocus::Variant { chip, .. }, Direction::Right) if chip + 1 < count => {
                board(variant(chip + 1))
            }
            (TimelineFocus::Variant { .. }, Direction::Right) => here().map_or(current, board),
            _ => current,
        }
    }

    fn lane_arrow(&mut self, lane: LaneFocus, direction: Direction) {
        let last = self.visible_groups.len().checked_sub(1);
        let target = match (lane, direction) {
            (_, Direction::Up) => {
                self.move_focus(ScreenFocus::Toolbar(ToolbarFocus::Sources));
                return;
            }
            (_, Direction::Down) => {
                let columns = self.metrics.now.columns;
                let first = self.board_entry_row() * columns;
                let choices = self.families().len().saturating_sub(first).min(columns);
                if choices > 0 {
                    let offset = if lane == LaneFocus::ChooseGroups {
                        choices - 1
                    } else {
                        0
                    };
                    self.move_focus(ScreenFocus::Board(NowFocus::Channel(first + offset)));
                } else if self.retry_available() {
                    self.move_focus(ScreenFocus::Board(NowFocus::Retry));
                }
                return;
            }
            (LaneFocus::All, Direction::Left) => LaneFocus::Phase,
            (LaneFocus::Group(0), Direction::Left) => LaneFocus::All,
            (LaneFocus::Group(index), Direction::Left) => LaneFocus::Group(index - 1),
            (LaneFocus::ChooseGroups, Direction::Left) => {
                last.map_or(LaneFocus::All, LaneFocus::Group)
            }
            (LaneFocus::Phase, Direction::Right) => LaneFocus::All,
            (LaneFocus::All, Direction::Right) => {
                last.map_or(LaneFocus::ChooseGroups, |_| LaneFocus::Group(0))
            }
            (LaneFocus::Group(index), Direction::Right) if Some(index) != last => {
                LaneFocus::Group(index + 1)
            }
            (LaneFocus::Group(_), Direction::Right) => LaneFocus::ChooseGroups,
            _ => return,
        };
        self.move_focus(ScreenFocus::GroupLane(target));
    }

    fn board_entry_row(&self) -> usize {
        let tolerance = BOARD_ENTRY_TOLERANCE.min(self.viewport.height);
        self.first_row_after(self.scroll_offset() + tolerance)
    }

    fn variant_count(&self, index: usize) -> usize {
        self.families().get(index).map_or(0, |family| {
            let count = family.quality_variants().count();
            if count > 1 { count } else { 0 }
        })
    }

    fn board(&self) -> Option<BoardView> {
        let mut board = self.pages.as_ref()?.board(self.geometry().columns);
        if board.page == PageStatus::Complete
            && self.group_pages.as_ref().is_some_and(|groups| {
                matches!(
                    groups.status(),
                    PageStatus::Failed | PageStatus::ReconcileGeneration
                )
            })
        {
            board.page = PageStatus::Failed;
        }
        Some(board)
    }

    fn all_groups_hidden(&self) -> bool {
        self.visible_groups.is_empty()
            && self.group_pages.as_ref().is_some_and(|pages| {
                pages.status() == PageStatus::Complete && !pages.groups().is_empty()
            })
    }

    pub fn selected_lane(&self) -> LaneFocus {
        self.selected_lane
    }

    fn update_selected_lane(&mut self) {
        self.selected_lane = self
            .selected
            .as_ref()
            .and_then(|selected| {
                self.visible_groups
                    .iter()
                    .position(|group| group.name() == selected.as_str())
            })
            .map_or(LaneFocus::All, LaneFocus::Group);
    }

    fn focused_group(&self) -> Option<Option<ChannelGroupFilter>> {
        if let Some(chooser) = self.group_chooser() {
            return match chooser.focus() {
                GroupChooserFocus::Pick(0) => Some(None),
                GroupChooserFocus::Pick(row) => self.chooser_group(row).map(Some),
                _ => None,
            };
        }
        match self.focus() {
            ScreenFocus::GroupLane(LaneFocus::All) => Some(None),
            ScreenFocus::GroupLane(LaneFocus::Group(index)) => self
                .visible_groups
                .get(index)
                .and_then(|group| ChannelGroupFilter::parse(group.name()).ok())
                .map(Some),
            _ => None,
        }
    }

    fn refresh_chooser_matches(&mut self) {
        if let Some(chooser) = &mut self.group_chooser {
            let groups = self
                .group_pages
                .as_ref()
                .map_or(&[][..], ChannelGroupPages::groups);
            let focus = chooser.focus();
            chooser.rebuild_matches(groups);
            if chooser.focus() != focus {
                self.focus_epoch = self.focus_epoch.wrapping_add(1);
            }
        }
    }

    fn chooser_group(&self, row: usize) -> Option<ChannelGroupFilter> {
        let index = self.group_chooser()?.group_index(row)?;
        ChannelGroupFilter::parse(self.group_roster().get(index)?.name()).ok()
    }

    fn close_group_chooser(&mut self) {
        self.group_chooser = None;
        self.focus_epoch = self.focus_epoch.wrapping_add(1);
        self.move_focus(ScreenFocus::GroupLane(LaneFocus::ChooseGroups));
    }

    fn activate_group_chooser(&mut self, target: GroupChooserFocus) -> ScreenAction {
        let can_show_all = self.group_chooser_can_show_all();
        if !self
            .group_chooser
            .as_mut()
            .is_some_and(|chooser| chooser.focus_control(target, can_show_all))
        {
            return ScreenAction::None;
        }
        self.focus_epoch = self.focus_epoch.wrapping_add(1);
        let change = match target {
            GroupChooserFocus::Close => {
                self.close_group_chooser();
                None
            }
            GroupChooserFocus::Filter => return ScreenAction::EditGroupFilter,
            GroupChooserFocus::Pick(0) => {
                self.select_group(None);
                self.close_group_chooser();
                None
            }
            GroupChooserFocus::Pick(row) => {
                let Some(group) = self.chooser_group(row) else {
                    return ScreenAction::None;
                };
                let change = if self.exclusions.contains(group.as_str()) {
                    let name = group.as_str().to_owned();
                    let mut exclusions = self.exclusions.clone();
                    exclusions.remove(&name);
                    self.apply_group_exclusions(exclusions, Some(group));
                    Some(GroupVisibilityChange::Set {
                        name,
                        hidden: false,
                    })
                } else {
                    self.select_group(Some(group));
                    None
                };
                self.close_group_chooser();
                change
            }
            GroupChooserFocus::Toggle(row) => {
                let Some(group) = self.chooser_group(row) else {
                    return ScreenAction::None;
                };
                let name = group.as_str().to_owned();
                let mut exclusions = self.exclusions.clone();
                let hidden = !exclusions.remove(&name);
                if hidden {
                    exclusions.insert(name.clone());
                }
                self.set_group_exclusions(exclusions);
                Some(GroupVisibilityChange::Set { name, hidden })
            }
            GroupChooserFocus::ShowAll => {
                self.set_group_exclusions(HashSet::new());
                if let Some(chooser) = &mut self.group_chooser {
                    let groups = self
                        .group_pages
                        .as_ref()
                        .map_or(&[][..], ChannelGroupPages::groups);
                    chooser.focus_selected(groups, self.selected.as_ref());
                }
                Some(GroupVisibilityChange::ShowAll)
            }
        };
        change.map_or(ScreenAction::None, ScreenAction::GroupVisibilityChanged)
    }

    fn apply_group_exclusions(
        &mut self,
        exclusions: HashSet<String>,
        group: Option<ChannelGroupFilter>,
    ) -> bool {
        if exclusions == self.exclusions {
            return false;
        }
        let lane_group = match self.focus() {
            ScreenFocus::GroupLane(LaneFocus::Group(index)) => {
                self.visible_groups.get(index).cloned()
            }
            _ => None,
        };
        let focused = self.board_target();
        let displayed = self.displayed_window();
        self.content_changed();
        self.exclusions = exclusions;
        if let Some(chooser) = &mut self.group_chooser {
            chooser.reconcile_show_all(!self.exclusions.is_empty());
        }
        let selected = group.filter(|group| !self.exclusions.contains(group.as_str()));
        if selected != self.selected {
            self.clear_timeline_return();
        }
        self.selected = selected;
        self.visible_groups = self
            .group_roster()
            .iter()
            .filter(|group| !self.exclusions.contains(group.name()))
            .cloned()
            .collect();
        self.update_selected_lane();
        self.cached.clear();
        self.reads
            .retain(|read| !matches!(read, GuideRead::Guide(_)));
        self.focus_epoch = self.focus_epoch.wrapping_add(1);
        self.cancel_pending_navigation();
        if let Some(generation) = self.generation {
            let mut pages = GuidePages::new(
                generation,
                self.selected.clone(),
                self.window,
                self.exclusions.clone(),
            );
            if let Some(previous) = &self.pages {
                pages.keep_compatible_rows(previous);
            }
            self.pages = Some(pages);
        }
        if let Some(group) = lane_group {
            let focus = self
                .visible_groups
                .iter()
                .position(|visible| visible.name() == group.name())
                .map_or(self.selected_lane(), LaneFocus::Group);
            self.move_focus(ScreenFocus::GroupLane(focus));
        }
        self.reanchor_timeline(displayed);
        self.restore_board_target(focused);
        self.reconcile_timeline_return();
        self.recover_focus();
        self.schedule_reads();
        true
    }

    fn select_group(&mut self, group: Option<ChannelGroupFilter>) {
        let Some(generation) = self.generation else {
            return;
        };
        if self.selected == group {
            return;
        }
        let mut next = self
            .cached
            .iter()
            .position(|entry| entry.key == group)
            .and_then(|index| self.cached.remove(index))
            .map_or_else(
                || {
                    GuidePages::new(
                        generation,
                        group.clone(),
                        self.window,
                        self.exclusions.clone(),
                    )
                },
                |entry| entry.pages,
            );
        let focused = self.board_target();
        let displayed = self.displayed_window();
        let previous_key = std::mem::replace(&mut self.selected, group);
        self.update_selected_lane();
        if let Some(previous) = self.pages.take() {
            next.keep_compatible_rows(&previous);
            self.cache_group(previous_key, previous);
        }
        self.pages = Some(next);
        self.viewport.offset = 0.;
        self.follow_focus = true;
        self.cancel_pending_navigation();
        self.clear_timeline_return();
        self.reanchor_timeline(displayed);
        self.focus_epoch = self.focus_epoch.wrapping_add(1);
        self.restore_board_target(focused);
        self.recover_focus();
        self.schedule_reads();
    }

    fn cache_group(&mut self, key: Option<ChannelGroupFilter>, pages: GuidePages) {
        if matches!(
            pages.status(),
            PageStatus::Failed | PageStatus::ReconcileGeneration
        ) {
            return;
        }
        if self.cached.len() == CACHED_GROUPS {
            // Keep the main board warm while browsing individual Channel Groups.
            let oldest_group = self.cached.iter().position(|entry| entry.key.is_some());
            self.cached.remove(oldest_group.unwrap_or(0));
        }
        self.cached.push_back(CachedGroup { key, pages });
    }

    fn apply_update(&mut self, update: PageUpdate) {
        if update == PageUpdate::Superseded {
            return;
        }
        self.content_changed();
        if update == PageUpdate::ReconcileGeneration {
            self.request_status();
        }
        self.recover_focus();
        if let Some(board) = self.board() {
            let focus = self.focus();
            match &mut self.mode {
                BoardMode::Now { navigation, .. } => {
                    let target = match focus {
                        ScreenFocus::Board(target) => Some(target),
                        _ => None,
                    };
                    let movement = navigation.settle(target, board);
                    self.apply_movement(movement);
                }
                BoardMode::Timeline { navigation, .. } => {
                    let target = match focus {
                        ScreenFocus::TimelineBoard(target) => Some(target),
                        _ => None,
                    };
                    let movement = navigation.settle(target, board);
                    self.apply_timeline_movement(movement);
                }
            }
        }
        self.schedule_reads();
    }

    fn board_target(&self) -> Option<BoardTarget> {
        let focus = self.focus();
        let index = board_family(focus)?;
        let family = self.families().get(index)?;
        let chip = match focus {
            ScreenFocus::Board(NowFocus::Variant { chip, .. })
            | ScreenFocus::TimelineBoard(TimelineFocus::Variant { chip, .. }) => Some(chip),
            _ => None,
        };
        let variant = chip
            .and_then(|chip| family.quality_variants().nth(chip))
            .map(|row| row.channel().id().clone());
        let programme = match focus {
            ScreenFocus::TimelineBoard(TimelineFocus::Programme { cell, .. }) => {
                ProgrammeFocus::at(family.programmes(&self.preferences), cell)
            }
            _ => None,
        };
        Some(BoardTarget {
            family: FamilyTarget {
                index,
                number: family.number(),
            },
            variant,
            programme,
        })
    }

    fn restore_board_target(&mut self, prior: Option<BoardTarget>) {
        let Some(prior) = prior else {
            return;
        };
        let previous = self.focus();
        let mut retained_programme = None;
        let families = self.families();
        let next = prior
            .family
            .resolve(families)
            .map_or(ScreenFocus::None, |index| {
                let family = &families[index];
                let chip = prior
                    .variant
                    .as_ref()
                    .filter(|_| self.variant_count(index) > 0)
                    .and_then(|variant| {
                        family
                            .quality_variants()
                            .position(|row| row.channel().id() == variant)
                    });
                if let Some(mut axis) = self.timeline() {
                    let target = if let Some(chip) = chip {
                        TimelineFocus::Variant {
                            family: index,
                            chip,
                        }
                    } else {
                        let cell = prior.programme.as_ref().and_then(|programme| {
                            let cells = family.programmes(&self.preferences);
                            if let Some(cell) = axis.exact_focus(cells, programme) {
                                retained_programme = Some(TimelineFocus::Programme {
                                    family: index,
                                    cell,
                                });
                                Some(cell)
                            } else {
                                axis.restore_focus(cells, programme)
                            }
                        });
                        timeline_cell(index, cell)
                    };
                    ScreenFocus::TimelineBoard(target)
                } else {
                    ScreenFocus::Board(chip.map_or(NowFocus::Channel(index), |chip| {
                        NowFocus::Variant {
                            family: index,
                            chip,
                        }
                    }))
                }
            });
        if let ScreenFocus::TimelineBoard(origin @ TimelineFocus::Programme { .. }) = previous
            && let BoardMode::Timeline { navigation, .. } = &mut self.mode
        {
            navigation.rebase_programme(origin, retained_programme);
        }
        self.move_focus(next);
    }

    fn recover_focus(&mut self) {
        let focus = self.focus();
        match focus {
            ScreenFocus::None => {
                if let Some(board) = self.board() {
                    match &mut self.mode {
                        BoardMode::Now { navigation, .. } => {
                            let movement = navigation.recover(board);
                            self.apply_movement(movement);
                        }
                        BoardMode::Timeline { navigation, .. } => {
                            let movement = navigation.recover(board);
                            self.apply_timeline_movement(movement);
                        }
                    }
                }
                if self.focus() == ScreenFocus::None && self.notice() != Some(ScreenNotice::Loading)
                {
                    self.move_focus(ScreenFocus::GroupLane(self.selected_lane()));
                }
            }
            ScreenFocus::Board(NowFocus::Retry)
            | ScreenFocus::TimelineBoard(TimelineFocus::Retry)
                if !self.retry_available() =>
            {
                let last = self.families().len().checked_sub(1);
                let next = match (self.mode(), last) {
                    (GuideMode::Now, Some(last)) => ScreenFocus::Board(NowFocus::Channel(last)),
                    (GuideMode::Timeline, Some(last)) => {
                        ScreenFocus::TimelineBoard(TimelineFocus::Channel(last))
                    }
                    (_, None) => ScreenFocus::GroupLane(self.selected_lane()),
                };
                self.move_focus(next);
            }
            _ if board_family(focus).is_some_and(|index| index >= self.families().len()) => {
                self.move_focus(ScreenFocus::None);
                self.recover_focus();
            }
            _ => {}
        }
        self.reconcile_preview();
        self.reveal_focus();
    }

    fn reanchor_timeline(&mut self, old_window: GuideWindow) {
        let window = self.displayed_window();
        if old_window != window
            && let BoardMode::Timeline { axis, .. } = &mut self.mode
        {
            axis.reanchor(window);
        }
    }

    fn clear_timeline_return(&mut self) {
        if let BoardMode::Timeline { return_target, .. } = &mut self.mode {
            *return_target = None;
        }
    }

    fn reconcile_timeline_return(&mut self) {
        let BoardMode::Timeline {
            axis,
            return_target,
            ..
        } = &self.mode
        else {
            return;
        };
        let Some(mut target) = return_target.clone() else {
            return;
        };
        let next = locate_programme(*axis, self.families(), &self.preferences, &target).map(
            |(index, cell)| {
                target.family.index = index;
                target.cell = Some(cell);
                target
            },
        );
        if let BoardMode::Timeline { return_target, .. } = &mut self.mode {
            *return_target = next;
        }
    }

    fn reconcile_preview(&mut self) {
        let Some(mut preview) = self.preview.take() else {
            return;
        };
        let families = self.families();
        let index = preview.family.resolve(families);
        if let Some(index) = index {
            preview.family.index = index;
        }
        if let Some(target) = &mut preview.programme {
            target.cell = index.and_then(|index| {
                target.family.index = index;
                Timeline::new(self.displayed_window()).exact_focus(
                    families[index].programmes(&self.preferences),
                    &target.programme,
                )
            });
        }
        self.preview = Some(preview);
    }

    fn apply_movement(&mut self, movement: NowMove) {
        match movement {
            NowMove::Stay => {}
            NowMove::Focus(focus) => self.move_focus(ScreenFocus::Board(focus)),
            NowMove::ToGroupLane => self.move_focus(ScreenFocus::GroupLane(self.selected_lane())),
            NowMove::LoadMore => self.load_guide_page(),
        }
    }

    fn apply_timeline_movement(&mut self, movement: TimelineMove) {
        match movement {
            TimelineMove::Stay => {}
            TimelineMove::Focus(focus) => self.move_focus(ScreenFocus::TimelineBoard(focus)),
            TimelineMove::Row { family, anchor } => {
                if let Some(axis) = self.timeline()
                    && let Some(row) = self.families().get(family)
                {
                    let cell = axis.cell_at(row.programmes(&self.preferences), anchor);
                    self.move_focus(ScreenFocus::TimelineBoard(timeline_cell(family, cell)));
                }
            }
            TimelineMove::ToGroupLane => self.move_focus(ScreenFocus::GroupLane(LaneFocus::Phase)),
            TimelineMove::LoadMore => self.load_guide_page(),
        }
    }

    fn move_focus(&mut self, focus: ScreenFocus) {
        let previous = self.focus();
        let mut timeline_update = None;
        if let Some(index) = board_family(focus)
            && let Some(row) = self.families().get(index)
        {
            let family = FamilyTarget {
                index,
                number: row.number(),
            };
            let mut preview = PreviewFocus {
                family,
                programme: None,
            };
            if let ScreenFocus::TimelineBoard(TimelineFocus::Programme { cell, .. }) = focus
                && let Some(mut axis) = self.timeline()
            {
                let cells = row.programmes(&self.preferences);
                if let Some(programme) = ProgrammeFocus::at(cells, cell) {
                    axis.reveal(&cells[cell]);
                    let target = ProgrammeTarget {
                        family,
                        programme,
                        cell: Some(cell),
                    };
                    timeline_update = Some((axis, target.clone()));
                    preview.programme = Some(target);
                }
            }
            self.preview = Some(preview);
        }
        self.zone = match focus {
            ScreenFocus::None => FocusZone::None,
            ScreenFocus::Toolbar(target) => FocusZone::Toolbar(target),
            ScreenFocus::GroupLane(target) => FocusZone::GroupLane(target),
            ScreenFocus::Board(target) => {
                let BoardMode::Now { focus, .. } = &mut self.mode else {
                    return;
                };
                *focus = target;
                FocusZone::Board
            }
            ScreenFocus::TimelineBoard(target) => {
                let BoardMode::Timeline {
                    focus,
                    axis,
                    return_target,
                    ..
                } = &mut self.mode
                else {
                    return;
                };
                *focus = target;
                if let Some((pan, target)) = timeline_update {
                    *axis = pan;
                    *return_target = Some(target);
                }
                FocusZone::Board
            }
        };
        if previous != focus {
            self.focus_epoch = self.focus_epoch.wrapping_add(1);
        }
        self.reveal_focus();
    }

    fn first_row_after(&self, top: f32) -> usize {
        let geometry = self.geometry();
        let first = ((top - geometry.row_top - geometry.row_height) / geometry.pitch).floor() + 1.;
        (first as usize).min(self.families().len().div_ceil(geometry.columns))
    }

    fn rows_bottom(&self) -> f32 {
        let geometry = self.geometry();
        self.families()
            .len()
            .checked_sub(1)
            .map_or(0., |last| geometry.row(last).end)
    }

    fn retained_retry(&self) -> bool {
        !self.families().is_empty() && self.retry_available()
    }

    /// Board content height, including the footer below the rows.
    fn content_bottom(&self) -> f32 {
        let rows_bottom = self.rows_bottom();
        let footer_top = rows_bottom + NOW_FOOTER_GAP;
        if self.retained_retry() {
            footer_top + NOW_RETAINED_RETRY_HEIGHT
        } else if rows_bottom > 0.
            && matches!(
                self.page_status(),
                Some(PageStatus::Ready | PageStatus::Loading)
            )
        {
            footer_top + NOW_MORE_HEIGHT
        } else {
            rows_bottom
        }
    }

    fn reveal_focus(&mut self) {
        let footer_top = self.rows_bottom() + NOW_FOOTER_GAP;
        let retained_retry = self.retained_retry();
        let content_bottom = self.content_bottom();
        let geometry = self.geometry();
        let focus = self.focus();
        let (target, covered_bottom) = match focus {
            ScreenFocus::Board(NowFocus::Retry)
            | ScreenFocus::TimelineBoard(TimelineFocus::Retry)
                if retained_retry =>
            {
                (Some(footer_top..content_bottom), 0.)
            }
            ScreenFocus::Board(_) | ScreenFocus::TimelineBoard(_) => {
                let target = board_family(focus).map(|family| geometry.row(family));
                let covered_bottom = if retained_retry {
                    NOW_RETAINED_RETRY_HEIGHT
                } else {
                    0.
                };
                (target, covered_bottom)
            }
            _ => (None, 0.),
        };
        self.viewport.reveal(
            content_bottom + geometry.bottom_padding,
            target.filter(|_| self.follow_focus),
            FOCUS_MARGIN + geometry.covered_top,
            FOCUS_MARGIN + covered_bottom,
        );
    }

    /// Moves focus to a control under the pointer without scrolling the board.
    /// Returns false when the control is not on screen in the current mode.
    pub fn set_focus(&mut self, focus: ScreenFocus) -> bool {
        let families = self.families().len();
        let available = match focus {
            ScreenFocus::None => false,
            ScreenFocus::Toolbar(_) => true,
            ScreenFocus::GroupLane(LaneFocus::Group(index)) => index < self.visible_groups.len(),
            ScreenFocus::GroupLane(_) => true,
            ScreenFocus::Board(target) => {
                self.mode() == GuideMode::Now
                    && match target {
                        NowFocus::Channel(family) => family < families,
                        NowFocus::Variant { family, chip } => chip < self.variant_count(family),
                        NowFocus::Retry => self.retry_available(),
                    }
            }
            ScreenFocus::TimelineBoard(target) => {
                self.mode() == GuideMode::Timeline
                    && match target {
                        TimelineFocus::Channel(family) => family < families,
                        TimelineFocus::Variant { family, chip } => {
                            chip < self.variant_count(family)
                        }
                        TimelineFocus::Programme { family, cell } => self
                            .families()
                            .get(family)
                            .is_some_and(|row| cell < row.programmes(&self.preferences).len()),
                        TimelineFocus::Retry => self.retry_available(),
                    }
            }
        };
        if !available || self.group_chooser.is_some() {
            return false;
        }
        self.cancel_pending_navigation();
        self.follow_focus = false;
        self.move_focus(focus);
        self.schedule_reads();
        true
    }

    /// Scrolls the board under pointer control and loads the next page near its end.
    /// Returns whether the board moved.
    pub fn scroll_by(&mut self, delta: f32) -> bool {
        let before = self.viewport.offset;
        let limit =
            (self.content_bottom() + self.geometry().bottom_padding - self.viewport.height).max(0.);
        self.follow_focus = false;
        self.viewport.offset = (before + delta).clamp(0., limit);
        let last_shown = self.visible_families(1).end.checked_sub(1);
        if !self.all_groups_hidden()
            && self.pages.as_ref().is_some_and(|pages| {
                pages.needs_automatic_page()
                    || last_shown.is_some_and(|family| pages.should_prefetch(family))
            })
        {
            self.load_guide_page();
        }
        self.viewport.offset != before
    }

    /// Pans the Timeline by whole half hours inside the loaded Guide window.
    pub fn pan_timeline(&mut self, half_hours: i64) -> bool {
        TimeDelta::try_minutes(half_hours.saturating_mul(30))
            .is_some_and(|delta| self.pan_timeline_by(delta))
    }

    /// Pans the Timeline by elapsed time, for continuous pointer input.
    pub fn pan_timeline_by(&mut self, delta: TimeDelta) -> bool {
        let BoardMode::Timeline { axis, .. } = &mut self.mode else {
            return false;
        };
        let before = axis.starts_at();
        axis.pan_by(delta);
        axis.starts_at() != before
    }

    fn schedule_reads(&mut self) {
        if let Some(groups) = &mut self.group_pages
            && let Some(request) = groups.request_next()
        {
            self.reads.push(GuideRead::Groups(request));
        }
        let focus = board_family(self.focus());
        if !self.all_groups_hidden()
            && self.pages.as_ref().is_some_and(|pages| {
                pages.needs_automatic_page()
                    || focus.is_some_and(|family| pages.should_prefetch(family))
            })
        {
            self.load_guide_page();
        }
    }

    fn load_guide_page(&mut self) {
        if let Some(request) = self.pages.as_mut().and_then(GuidePages::request_next) {
            self.reads.push(GuideRead::Guide(request));
        }
    }

    fn retry(&mut self) {
        if self
            .pages
            .as_ref()
            .is_some_and(|pages| pages.status() == PageStatus::ReconcileGeneration)
            || self
                .group_pages
                .as_ref()
                .is_some_and(|groups| groups.status() == PageStatus::ReconcileGeneration)
        {
            self.request_status();
        }
        if let Some(request) = self.group_pages.as_mut().and_then(ChannelGroupPages::retry) {
            self.reads.push(GuideRead::Groups(request));
        }
        if let Some(request) = self.pages.as_mut().and_then(GuidePages::retry) {
            self.reads.push(GuideRead::Guide(request));
        }
        if self.families().is_empty() && !self.retry_available() {
            self.move_focus(ScreenFocus::None);
        }
        self.recover_focus();
    }
}

fn board_family(focus: ScreenFocus) -> Option<usize> {
    match focus {
        ScreenFocus::Board(focus) => focus.family(),
        ScreenFocus::TimelineBoard(focus) => focus.family(),
        _ => None,
    }
}

fn timeline_cell(family: usize, cell: Option<usize>) -> TimelineFocus {
    cell.map_or(TimelineFocus::Channel(family), |cell| {
        TimelineFocus::Programme { family, cell }
    })
}

fn locate_programme(
    axis: Timeline,
    families: &[GuideFamily],
    preferences: &VariantPreferences,
    target: &ProgrammeTarget,
) -> Option<(usize, usize)> {
    let index = target.family.resolve(families)?;
    axis.exact_focus(families[index].programmes(preferences), &target.programme)
        .map(|cell| (index, cell))
}
