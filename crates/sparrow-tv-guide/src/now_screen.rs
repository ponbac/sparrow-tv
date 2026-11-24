//! Remote-control focus, viewport, and catalog reads for the television Now screen.

use std::{
    collections::{HashSet, VecDeque},
    ops::Range,
    time::Duration,
};

use sparrow_core::{
    CatalogGeneration, CatalogStatus, ChannelGroupFilter, ChannelGroupView, ChannelId, CoreError,
    GuideWindowChannel, Page,
};

use crate::{
    BoardView, ChannelGroupPages, GroupPageRequest, GuideFamily, GuidePageRequest, GuidePages,
    GuideWindow, PageStatus, PageUpdate,
    now_focus::{Direction, NowFocus, NowMove, NowNavigation},
    viewport::VerticalViewport,
};

pub const NOW_COLUMNS: usize = 3;
pub const NOW_ROW_TOP: f32 = 4.;
pub const NOW_ROW_HEIGHT: f32 = 100.;
pub const NOW_ROW_PITCH: f32 = 111.;
pub const NOW_FOOTER_GAP: f32 = 18.;
pub const NOW_RETAINED_RETRY_HEIGHT: f32 = 78.;
pub const NOW_MORE_HEIGHT: f32 = 54.;

const FOCUS_MARGIN: f32 = 24.;
const BOARD_BOTTOM_PADDING: f32 = 24.;
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
pub enum ScreenFocus {
    None,
    Toolbar(ToolbarFocus),
    GroupLane(LaneFocus),
    Board(NowFocus),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenKey {
    Arrow(Direction),
    Activate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenAction {
    None,
    Unavailable,
    OpenTimeline,
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

pub enum NowRead {
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
struct PreviewFocus {
    index: usize,
    number: u32,
}

struct BoardTarget {
    index: usize,
    number: u32,
    variant: Option<ChannelId>,
}

/// The host dispatches read intents off the UI thread and delivers the results on it.
/// Focus movement over loaded families never waits for a catalog read.
pub struct NowScreen {
    window: GuideWindow,
    exclusions: HashSet<String>,
    configured: Option<bool>,
    open_failed: bool,
    generation: Option<CatalogGeneration>,
    group_pages: Option<ChannelGroupPages>,
    visible_groups: Vec<ChannelGroupView>,
    selected: Option<ChannelGroupFilter>,
    selected_lane: LaneFocus,
    pages: Option<GuidePages>,
    cached: VecDeque<CachedGroup>,
    navigation: NowNavigation,
    focus: ScreenFocus,
    preview: Option<PreviewFocus>,
    focus_epoch: u64,
    viewport: VerticalViewport,
    reads: Vec<NowRead>,
}

impl NowScreen {
    pub fn new(window: GuideWindow, exclusions: HashSet<String>) -> Self {
        Self {
            window,
            exclusions,
            configured: None,
            open_failed: false,
            generation: None,
            group_pages: None,
            visible_groups: Vec::new(),
            selected: None,
            selected_lane: LaneFocus::All,
            pages: None,
            cached: VecDeque::new(),
            navigation: NowNavigation::default(),
            focus: ScreenFocus::None,
            preview: None,
            focus_epoch: 0,
            viewport: VerticalViewport::default(),
            reads: Vec::new(),
        }
    }

    pub fn generation(&self) -> Option<CatalogGeneration> {
        self.generation
    }

    pub fn focus(&self) -> ScreenFocus {
        self.focus
    }

    pub fn selected_group(&self) -> Option<&ChannelGroupFilter> {
        self.selected.as_ref()
    }

    pub fn groups(&self) -> &[ChannelGroupView] {
        &self.visible_groups
    }

    pub fn families(&self) -> &[GuideFamily] {
        self.pages.as_ref().map_or(&[], GuidePages::families)
    }

    /// Keeps the last focused Channel in the preview while browsing other controls.
    pub fn preview_family(&self) -> Option<&GuideFamily> {
        let families = self.families();
        self.preview
            .and_then(|preview| {
                families
                    .get(preview.index)
                    .filter(|family| family.number() == preview.number)
            })
            .or_else(|| families.first())
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

    /// Height of the clipped board, in the 1920-wide Guide reference units.
    pub fn set_viewport_height(&mut self, height: f32) {
        let height = height.max(0.);
        if self.viewport.height != height {
            self.viewport.height = height;
            self.reveal_focus();
        }
    }

    pub fn scroll_offset(&self) -> f32 {
        self.viewport.offset
    }

    pub fn first_visible_row(&self) -> usize {
        self.first_row_after(self.scroll_offset())
    }

    /// Family indices for intersecting rows, including overscan on each side.
    pub fn visible_families(&self, overscan: usize) -> Range<usize> {
        let available = self.families().len();
        let first = self.first_visible_row().saturating_sub(overscan) * NOW_COLUMNS;
        let bottom = self.scroll_offset() + self.viewport.height;
        let end = ((bottom - NOW_ROW_TOP) / NOW_ROW_PITCH).ceil() as usize;
        first.min(available)
            ..end
                .saturating_add(overscan)
                .saturating_mul(NOW_COLUMNS)
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

    pub fn take_reads(&mut self) -> Vec<NowRead> {
        std::mem::take(&mut self.reads)
    }

    /// Call on catalog notifications, or let a rejected cursor queue the same read.
    pub fn request_status(&mut self) {
        if !self
            .reads
            .iter()
            .any(|read| matches!(read, NowRead::Status))
        {
            self.reads.push(NowRead::Status);
        }
    }

    pub fn catalog_open_failed(&mut self) {
        self.open_failed = true;
        self.recover_focus();
    }

    pub fn catalog_status(&mut self, status: &CatalogStatus) {
        self.open_failed = false;
        self.configured = Some(status.configuration().is_configured());
        let generation = status.generation();
        if generation != self.generation {
            self.generation = generation;
            self.preview = None;
            self.group_pages = generation.map(ChannelGroupPages::new);
            self.visible_groups.clear();
            self.selected_lane = LaneFocus::All;
            self.cached.clear();
            self.viewport.offset = 0.;
            self.focus_epoch = self.focus_epoch.wrapping_add(1);
            self.navigation = NowNavigation::default();
            self.reads.clear();
            match self.focus {
                ScreenFocus::Board(_) => self.move_focus(ScreenFocus::None),
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
                self.visible_groups.clear();
                self.selected_lane = LaneFocus::All;
                if matches!(self.focus, ScreenFocus::GroupLane(LaneFocus::Group(_))) {
                    self.move_focus(ScreenFocus::GroupLane(LaneFocus::All));
                }
            }
            if let Some(pages) = &mut self.pages
                && pages.status() == PageStatus::ReconcileGeneration
            {
                pages.select(generation, self.selected.clone(), self.window);
            }
        }
        self.recover_focus();
        self.schedule_reads();
    }

    pub fn set_window(&mut self, window: GuideWindow) {
        if self.window == window {
            return;
        }
        self.window = window;
        self.focus_epoch = self.focus_epoch.wrapping_add(1);
        self.cached.clear();
        self.reads.retain(|read| !matches!(read, NowRead::Guide(_)));
        if let (Some(generation), Some(pages)) = (self.generation, &mut self.pages) {
            pages.select(generation, self.selected.clone(), window);
        }
        self.recover_focus();
        self.schedule_reads();
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
            let Some(pages) = &mut self.pages else {
                return PageUpdate::Superseded;
            };
            let update = pages.complete(request, result);
            if update != PageUpdate::Superseded {
                self.restore_board_target(focused);
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
        (group != self.selected && !self.cached.iter().any(|entry| entry.key == group))
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
                self.reads.push(NowRead::Guide(request));
            }
            self.cache_group(group, pages);
        }
    }

    pub fn press(&mut self, key: ScreenKey) -> ScreenAction {
        match key {
            ScreenKey::Activate => {
                self.navigation = NowNavigation::default();
                self.activate()
            }
            ScreenKey::Arrow(direction) => {
                self.arrow(direction);
                self.schedule_reads();
                ScreenAction::None
            }
        }
    }

    fn activate(&mut self) -> ScreenAction {
        match self.focus {
            ScreenFocus::None => ScreenAction::None,
            ScreenFocus::GroupLane(LaneFocus::Phase) => ScreenAction::OpenTimeline,
            ScreenFocus::GroupLane(LaneFocus::All | LaneFocus::Group(_)) => {
                if let Some(group) = self.focused_group() {
                    self.select_group(group);
                }
                ScreenAction::None
            }
            ScreenFocus::Board(NowFocus::Retry) => {
                self.retry();
                ScreenAction::None
            }
            ScreenFocus::Toolbar(_)
            | ScreenFocus::GroupLane(LaneFocus::ChooseGroups)
            | ScreenFocus::Board(_) => ScreenAction::Unavailable,
        }
    }

    fn arrow(&mut self, direction: Direction) {
        match self.focus {
            ScreenFocus::None => self.recover_focus(),
            ScreenFocus::Toolbar(toolbar) => {
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
            ScreenFocus::GroupLane(lane) => self.lane_arrow(lane, direction),
            ScreenFocus::Board(focus) => {
                if let Some(board) = self.board() {
                    let count = self.variant_count(focus);
                    let movement = self.navigation.press(focus, direction, count, board);
                    self.apply_movement(movement);
                }
            }
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
                let tolerance = BOARD_ENTRY_TOLERANCE.min(self.viewport.height);
                let first = self.first_row_after(self.scroll_offset() + tolerance) * NOW_COLUMNS;
                let choices = self.families().len().saturating_sub(first).min(NOW_COLUMNS);
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

    fn variant_count(&self, focus: NowFocus) -> usize {
        focus
            .family()
            .and_then(|family| self.families().get(family))
            .map_or(0, |family| {
                let count = family.quality_variants().count();
                if count > 1 { count } else { 0 }
            })
    }

    fn board(&self) -> Option<BoardView> {
        let mut board = self.pages.as_ref()?.board();
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
        match self.focus {
            ScreenFocus::GroupLane(LaneFocus::All) => Some(None),
            ScreenFocus::GroupLane(LaneFocus::Group(index)) => self
                .visible_groups
                .get(index)
                .and_then(|group| ChannelGroupFilter::parse(group.name()).ok())
                .map(Some),
            _ => None,
        }
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
        let previous_key = std::mem::replace(&mut self.selected, group);
        self.update_selected_lane();
        if let Some(previous) = self.pages.take() {
            next.keep_compatible_rows(&previous);
            self.cache_group(previous_key, previous);
        }
        self.pages = Some(next);
        self.navigation = NowNavigation::default();
        self.viewport.offset = 0.;
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
        if update == PageUpdate::ReconcileGeneration {
            self.request_status();
        }
        self.recover_focus();
        if let Some(board) = self.board() {
            let focus = match self.focus {
                ScreenFocus::Board(focus) => Some(focus),
                _ => None,
            };
            let movement = self.navigation.settle(focus, board);
            self.apply_movement(movement);
        }
        self.schedule_reads();
    }

    fn board_target(&self) -> Option<BoardTarget> {
        let ScreenFocus::Board(focus) = self.focus else {
            return None;
        };
        let index = focus.family()?;
        let family = self.families().get(index)?;
        let variant = if let NowFocus::Variant { chip, .. } = focus {
            family
                .quality_variants()
                .nth(chip)
                .map(|row| row.channel().id().clone())
        } else {
            None
        };
        Some(BoardTarget {
            index,
            number: family.number(),
            variant,
        })
    }

    fn restore_board_target(&mut self, prior: Option<BoardTarget>) {
        let Some(prior) = prior else {
            return;
        };
        let families = self.families();
        let index = if families
            .get(prior.index)
            .is_some_and(|family| family.number() == prior.number)
        {
            Some(prior.index)
        } else {
            families
                .iter()
                .position(|family| family.number() == prior.number)
        };
        let next = index.map(|index| {
            let family = &families[index];
            if let Some(variant) = &prior.variant
                && family.quality_variants().count() > 1
                && let Some(chip) = family
                    .quality_variants()
                    .position(|row| row.channel().id() == variant)
            {
                return NowFocus::Variant {
                    family: index,
                    chip,
                };
            }
            NowFocus::Channel(index)
        });
        self.move_focus(next.map_or(ScreenFocus::None, ScreenFocus::Board));
    }

    fn recover_focus(&mut self) {
        match self.focus {
            ScreenFocus::None => {
                if let Some(board) = self.board() {
                    let movement = self.navigation.recover(board);
                    self.apply_movement(movement);
                }
                if self.focus == ScreenFocus::None && self.notice() != Some(ScreenNotice::Loading) {
                    self.move_focus(ScreenFocus::GroupLane(self.selected_lane()));
                }
            }
            ScreenFocus::Board(NowFocus::Retry) if !self.retry_available() => {
                if let Some(last) = self.families().len().checked_sub(1) {
                    self.move_focus(ScreenFocus::Board(NowFocus::Channel(last)));
                } else {
                    self.move_focus(ScreenFocus::GroupLane(self.selected_lane()));
                }
            }
            ScreenFocus::Board(focus)
                if focus
                    .family()
                    .is_some_and(|index| index >= self.families().len()) =>
            {
                self.move_focus(ScreenFocus::None);
                self.recover_focus();
            }
            _ => {}
        }
        self.reconcile_preview();
        self.reveal_focus();
    }

    fn reconcile_preview(&mut self) {
        if let Some(preview) = self.preview {
            let families = self.families();
            if families
                .get(preview.index)
                .is_none_or(|family| family.number() != preview.number)
            {
                self.preview = families
                    .iter()
                    .position(|family| family.number() == preview.number)
                    .map(|index| PreviewFocus { index, ..preview });
            }
        }
    }

    fn apply_movement(&mut self, movement: NowMove) {
        match movement {
            NowMove::Stay => {}
            NowMove::Focus(focus) => self.move_focus(ScreenFocus::Board(focus)),
            NowMove::ToGroupLane => self.move_focus(ScreenFocus::GroupLane(self.selected_lane())),
            NowMove::LoadMore => self.load_guide_page(),
        }
    }

    fn move_focus(&mut self, focus: ScreenFocus) {
        if let ScreenFocus::Board(focus) = focus
            && let Some(index) = focus.family()
            && let Some(family) = self.families().get(index)
        {
            self.preview = Some(PreviewFocus {
                index,
                number: family.number(),
            });
        }
        if self.focus != focus {
            self.focus = focus;
            self.focus_epoch = self.focus_epoch.wrapping_add(1);
        }
        self.reveal_focus();
    }

    fn first_row_after(&self, top: f32) -> usize {
        let first = ((top - NOW_ROW_TOP - NOW_ROW_HEIGHT) / NOW_ROW_PITCH).floor() + 1.;
        (first as usize).min(self.families().len().div_ceil(NOW_COLUMNS))
    }

    fn rows_bottom(&self) -> f32 {
        let rows = self.families().len().div_ceil(NOW_COLUMNS);
        rows.checked_sub(1)
            .map_or(0., |last| row_top(last) + NOW_ROW_HEIGHT)
    }

    fn retained_retry(&self) -> bool {
        !self.families().is_empty() && self.retry_available()
    }

    fn reveal_focus(&mut self) {
        let rows_bottom = self.rows_bottom();
        let retained_retry = self.retained_retry();
        let footer_top = rows_bottom + NOW_FOOTER_GAP;
        let content_bottom = if retained_retry {
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
        };
        let (target, covered_bottom) = match self.focus {
            ScreenFocus::Board(NowFocus::Retry) if retained_retry => {
                (Some(footer_top..content_bottom), 0.)
            }
            ScreenFocus::Board(focus) => {
                let target = focus.family().map(|family| {
                    let top = row_top(family / NOW_COLUMNS);
                    top..top + NOW_ROW_HEIGHT
                });
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
            content_bottom + BOARD_BOTTOM_PADDING,
            target,
            FOCUS_MARGIN,
            FOCUS_MARGIN + covered_bottom,
        );
    }

    fn schedule_reads(&mut self) {
        if let Some(groups) = &mut self.group_pages
            && let Some(request) = groups.request_next()
        {
            self.reads.push(NowRead::Groups(request));
        }
        let focus = match self.focus {
            ScreenFocus::Board(focus) => focus.family(),
            _ => None,
        };
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
            self.reads.push(NowRead::Guide(request));
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
            self.reads.push(NowRead::Groups(request));
        }
        if let Some(request) = self.pages.as_mut().and_then(GuidePages::retry) {
            self.reads.push(NowRead::Guide(request));
        }
        if self.families().is_empty() && !self.retry_available() {
            self.move_focus(ScreenFocus::None);
        }
        self.recover_focus();
    }
}

fn row_top(row: usize) -> f32 {
    NOW_ROW_TOP + row as f32 * NOW_ROW_PITCH
}
