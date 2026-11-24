use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use sparrow_core::{
    CatalogGeneration, ChannelGroupFilter, ChannelGroupView, ChannelQuery, CoreError,
    GuideWindowChannel, GuideWindowQuery, Page, PageCursor, PageLimit, PageRequest,
};

use crate::{BoardView, GuideFamily, GuideWindow, families::append_families};

const FIRST_GUIDE_PAGE: u16 = 40;
const CONTINUATION_PAGE: u16 = 100;
const PREFETCH_FAMILIES: usize = 8;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

// A host may replace a pager while its worker still owns an old read.
fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageStatus {
    Ready,
    Loading,
    Failed,
    ReconcileGeneration,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageUpdate {
    Applied,
    Failed,
    ReconcileGeneration,
    Superseded,
}

/// One pending Guide read. Return it to the owner along with the core's result.
pub struct GuidePageRequest {
    id: u64,
    query: GuideWindowQuery,
}

impl GuidePageRequest {
    pub fn query(&self) -> &GuideWindowQuery {
        &self.query
    }
}

/// Owns the selected window and forward pages. The host runs pending reads off its UI thread.
pub struct GuidePages {
    pager: Pager,
    selection: u64,
    group: Option<ChannelGroupFilter>,
    excluded_groups: HashSet<String>,
    active: LoadedWindow,
    displayed_previous: Option<RetainedWindow>,
}

struct RetainedWindow {
    data: LoadedWindow,
    group_source: Option<LoadedWindow>,
    status: PageStatus,
    replace_after: usize,
}

#[derive(Clone)]
struct LoadedWindow {
    window: GuideWindow,
    page_count: usize,
    families: Arc<Vec<GuideFamily>>,
}

impl LoadedWindow {
    fn empty(window: GuideWindow) -> Self {
        Self {
            window,
            page_count: 0,
            families: Arc::default(),
        }
    }
}

impl GuidePages {
    pub fn new(
        generation: CatalogGeneration,
        group: Option<ChannelGroupFilter>,
        window: GuideWindow,
        excluded_groups: HashSet<String>,
    ) -> Self {
        let group = group.filter(|name| !excluded_groups.contains(name.as_str()));
        Self {
            pager: Pager::new(generation),
            selection: next_id(),
            group,
            excluded_groups,
            active: LoadedWindow::empty(window),
            displayed_previous: None,
        }
    }

    /// Changes the active query and invalidates pending requests. A same-group clock
    /// move keeps the old view until the new window has caught up to its page depth.
    pub fn select(
        &mut self,
        generation: CatalogGeneration,
        group: Option<ChannelGroupFilter>,
        window: GuideWindow,
    ) {
        let group = group.filter(|name| !self.excluded_groups.contains(name.as_str()));
        let same_generation = self.pager.generation == generation;
        let same_group = self.group == group;
        if same_generation
            && same_group
            && self.active.window == window
            && self.status() != PageStatus::ReconcileGeneration
        {
            return;
        }
        let prior = std::mem::replace(&mut self.active, LoadedWindow::empty(window));
        if !same_generation {
            self.displayed_previous = None;
        } else if same_group {
            if self.displayed_previous.is_none() && prior.page_count > 0 {
                // The old request is gone. Show catch-up until the new read settles.
                let status = match self.status() {
                    PageStatus::Failed | PageStatus::ReconcileGeneration => PageStatus::Loading,
                    status => status,
                };
                self.displayed_previous = Some(RetainedWindow {
                    status,
                    group_source: None,
                    replace_after: prior.page_count,
                    data: prior,
                });
            }
        } else {
            let source = self
                .displayed_previous
                .take()
                .map_or(prior, |shown| shown.group_source.unwrap_or(shown.data));
            self.displayed_previous = Self::group_placeholder(source, group.as_ref());
        }
        self.group = group;
        self.selection = next_id();
        self.pager.reset(generation);
    }

    /// A group switch can display compatible rows from a pager kept in the cache.
    pub(crate) fn keep_compatible_rows(&mut self, previous: &Self) {
        if self.active.page_count != 0
            || self
                .displayed_previous
                .as_ref()
                .is_some_and(|prior| prior.group_source.is_none())
            || !matches!(self.status(), PageStatus::Ready | PageStatus::Loading)
        {
            return;
        }
        let source = previous
            .displayed_previous
            .as_ref()
            .map_or(&previous.active, |shown| {
                shown.group_source.as_ref().unwrap_or(&shown.data)
            });
        self.displayed_previous = Self::group_placeholder(source.clone(), self.group.as_ref());
    }

    fn group_placeholder(
        source: LoadedWindow,
        group: Option<&ChannelGroupFilter>,
    ) -> Option<RetainedWindow> {
        if source.page_count == 0 {
            return None;
        }
        let mut data = source.clone();
        if let Some(group) = group {
            data.families = Arc::new(
                source
                    .families
                    .iter()
                    .filter(|family| family.group() == group.as_str())
                    .cloned()
                    .collect(),
            );
        }
        let status = if data.families.is_empty() {
            PageStatus::Loading
        } else {
            PageStatus::Complete
        };
        Some(RetainedWindow {
            data,
            group_source: Some(source),
            status,
            replace_after: 1,
        })
    }

    pub fn status(&self) -> PageStatus {
        self.pager.status()
    }

    /// Availability for the rows on screen while the requested window catches up.
    pub fn displayed_status(&self) -> PageStatus {
        self.displayed_previous
            .as_ref()
            .map_or_else(|| self.status(), |window| window.status)
    }

    pub fn board(&self) -> BoardView {
        BoardView {
            selection: self.selection,
            families: self.families().len(),
            page: self.displayed_status(),
        }
    }

    pub fn displayed_window(&self) -> GuideWindow {
        self.displayed().window
    }

    pub fn families(&self) -> &[GuideFamily] {
        &self.displayed().families
    }

    fn displayed(&self) -> &LoadedWindow {
        self.displayed_previous
            .as_ref()
            .map_or(&self.active, |window| &window.data)
    }

    pub fn is_replacing_window(&self) -> bool {
        self.displayed_previous.is_some()
    }

    /// Opens new or hidden-only views without waiting for a focus move.
    pub fn needs_automatic_page(&self) -> bool {
        self.status() == PageStatus::Ready
            && (self.is_replacing_window() || self.active.families.is_empty())
    }

    pub fn should_prefetch(&self, focused_family: usize) -> bool {
        let available = self.active.families.len();
        self.status() == PageStatus::Ready
            && !self.is_replacing_window()
            && focused_family < available
            && available - focused_family - 1 < PREFETCH_FAMILIES
    }

    pub fn request_next(&mut self) -> Option<GuidePageRequest> {
        let (id, page) = self.pager.request(FIRST_GUIDE_PAGE)?;
        Some(self.guide_request(id, page))
    }

    pub fn retry(&mut self) -> Option<GuidePageRequest> {
        let (id, page) = self.pager.retry(FIRST_GUIDE_PAGE)?;
        Some(self.guide_request(id, page))
    }

    fn guide_request(&self, id: u64, page: PageRequest) -> GuidePageRequest {
        let channels = match &self.group {
            Some(group) => ChannelQuery::in_group(group.clone(), page),
            None => ChannelQuery::all(page),
        };
        let query = GuideWindowQuery::new(
            self.active.window.starts_at(),
            self.active.window.ends_at(),
            channels,
        )
        .expect("a GuideWindow is a bounded core query interval");
        GuidePageRequest { id, query }
    }

    pub fn complete(
        &mut self,
        request: GuidePageRequest,
        result: Result<Page<GuideWindowChannel>, CoreError>,
    ) -> PageUpdate {
        let rows = self.pager.complete(request.id, result);
        match rows {
            Ok(page) => {
                append_families(
                    Arc::make_mut(&mut self.active.families),
                    page.items(),
                    &self.excluded_groups,
                );
                self.active.page_count += 1;
                if self
                    .displayed_previous
                    .as_ref()
                    .is_some_and(|prior| self.active.page_count >= prior.replace_after)
                {
                    self.displayed_previous = None;
                }
                PageUpdate::Applied
            }
            Err(outcome) => {
                if outcome != PageUpdate::Superseded {
                    self.displayed_previous = None;
                }
                outcome
            }
        }
    }
}

pub struct GroupPageRequest {
    id: u64,
    page: PageRequest,
}

impl GroupPageRequest {
    pub fn query(&self) -> &PageRequest {
        &self.page
    }
}

/// Retains source order while pulling all bounded Channel Group pages for the lane.
pub struct ChannelGroupPages {
    pager: Pager,
    groups: Vec<ChannelGroupView>,
}

impl ChannelGroupPages {
    pub fn new(generation: CatalogGeneration) -> Self {
        Self {
            pager: Pager::new(generation),
            groups: Vec::new(),
        }
    }

    pub fn select(&mut self, generation: CatalogGeneration) {
        if self.pager.generation != generation || self.status() == PageStatus::ReconcileGeneration {
            self.groups.clear();
            self.pager.reset(generation);
        }
    }

    pub fn groups(&self) -> &[ChannelGroupView] {
        &self.groups
    }

    pub fn status(&self) -> PageStatus {
        self.pager.status()
    }

    pub fn request_next(&mut self) -> Option<GroupPageRequest> {
        let (id, page) = self.pager.request(CONTINUATION_PAGE)?;
        Some(GroupPageRequest { id, page })
    }

    pub fn retry(&mut self) -> Option<GroupPageRequest> {
        let (id, page) = self.pager.retry(CONTINUATION_PAGE)?;
        Some(GroupPageRequest { id, page })
    }

    pub fn complete(
        &mut self,
        request: GroupPageRequest,
        result: Result<Page<ChannelGroupView>, CoreError>,
    ) -> PageUpdate {
        match self.pager.complete(request.id, result) {
            Ok(page) => {
                self.groups.extend_from_slice(page.items());
                PageUpdate::Applied
            }
            Err(outcome) => outcome,
        }
    }
}

struct Pager {
    generation: CatalogGeneration,
    state: ReadState,
}

enum ReadState {
    Ready(Option<PageCursor>),
    Loading { id: u64, cursor: Option<PageCursor> },
    Failed(Option<PageCursor>),
    ReconcileGeneration,
    Complete,
}

impl Pager {
    fn new(generation: CatalogGeneration) -> Self {
        Self {
            generation,
            state: ReadState::Ready(None),
        }
    }

    fn reset(&mut self, generation: CatalogGeneration) {
        self.generation = generation;
        self.state = ReadState::Ready(None);
    }

    fn status(&self) -> PageStatus {
        match self.state {
            ReadState::Ready(_) => PageStatus::Ready,
            ReadState::Loading { .. } => PageStatus::Loading,
            ReadState::Failed(_) => PageStatus::Failed,
            ReadState::ReconcileGeneration => PageStatus::ReconcileGeneration,
            ReadState::Complete => PageStatus::Complete,
        }
    }

    fn request(&mut self, first: u16) -> Option<(u64, PageRequest)> {
        let ReadState::Ready(cursor) = &self.state else {
            return None;
        };
        Some(self.start(first, cursor.clone()))
    }

    fn retry(&mut self, first: u16) -> Option<(u64, PageRequest)> {
        let ReadState::Failed(cursor) = &self.state else {
            return None;
        };
        Some(self.start(first, cursor.clone()))
    }

    fn start(&mut self, first: u16, cursor: Option<PageCursor>) -> (u64, PageRequest) {
        let id = next_id();
        let limit = if cursor.is_some() {
            CONTINUATION_PAGE
        } else {
            first
        };
        let limit = PageLimit::new(limit).expect("guide page sizes fit the core limit");
        self.state = ReadState::Loading {
            id,
            cursor: cursor.clone(),
        };
        (id, PageRequest::new(cursor, limit))
    }

    fn complete<T>(
        &mut self,
        id: u64,
        result: Result<Page<T>, CoreError>,
    ) -> Result<Page<T>, PageUpdate> {
        let ReadState::Loading { id: active, cursor } = &mut self.state else {
            return Err(PageUpdate::Superseded);
        };
        if *active != id {
            return Err(PageUpdate::Superseded);
        }
        let cursor = cursor.take();
        match result {
            Ok(page) if page.generation() == self.generation => {
                self.state = match page.next() {
                    Some(next) => ReadState::Ready(Some(next.clone())),
                    None => ReadState::Complete,
                };
                Ok(page)
            }
            Ok(_) | Err(CoreError::StaleCursor { .. }) => {
                self.state = ReadState::ReconcileGeneration;
                Err(PageUpdate::ReconcileGeneration)
            }
            Err(_) => {
                self.state = ReadState::Failed(cursor);
                Err(PageUpdate::Failed)
            }
        }
    }
}
