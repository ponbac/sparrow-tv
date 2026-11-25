//! Generation-bound Shield Search. The host owns the debounce timer and worker.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use chrono::{DateTime, Utc};
use sparrow_bounded_blocking::BlockingTaskCancellation;
use sparrow_core::{
    CatalogAccess, CatalogGeneration, ChannelId, ChannelQuality, ChannelSummary, CoreError,
    GuideProgramme, InputReason, Page, PageCursor, PageLimit, PageRequest, ProgrammeSearchHit,
    SearchRequest, SearchResults, SearchTerm,
};

use crate::{GuideFamily, PageUpdate, VariantPreferences, time::programme_at};

pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(90);
const PAGE_SIZE: u16 = 40;
const INLINE_LIMIT: usize = 8;
const MAX_FIELD_BYTES: usize = 256;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchView {
    Closed,
    Inline,
    Desk,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchStatus {
    Idle,
    Invalid,
    Unavailable,
    Loading,
    Ready,
    Hidden,
    Empty,
    Failed,
    ReconcileGeneration,
}

/// Return this ticket when the host's debounce timer elapses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchDebounce(u64);

/// A single bounded read for the host's serial Search worker.
pub struct SearchRead {
    id: u64,
    generation: CatalogGeneration,
    term: SearchTerm,
    branch: Branch,
    cancelled: BlockingTaskCancellation,
}

enum Branch {
    Inline,
    Desk(Option<PageCursor>),
}

pub struct SearchCompletion {
    id: u64,
    generation: CatalogGeneration,
    result: Result<ReadResult, CoreError>,
}

enum ReadResult {
    Inline(SearchResults),
    Desk(Page<ChannelSummary>),
}

impl SearchRead {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.is_cancelled()
    }

    /// The worker can use the same cancellation while waiting for its read permit.
    pub fn cancellation(&self) -> BlockingTaskCancellation {
        self.cancelled.clone()
    }

    /// Performs the query. The caller must run it off the render thread.
    pub fn run(self, catalog: &CatalogAccess) -> SearchCompletion {
        let limit = PageLimit::new(PAGE_SIZE).expect("valid Search page size");
        let result = if self.is_cancelled() {
            Err(CoreError::Cancelled)
        } else {
            let cancelled = &self.cancelled;
            let is_cancelled = || cancelled.is_cancelled();
            match self.branch {
                Branch::Inline => {
                    let page = PageRequest::first(limit);
                    let request = SearchRequest::new(self.term, page.clone(), page);
                    catalog
                        .search_with_cancellation(request, is_cancelled)
                        .map(ReadResult::Inline)
                }
                Branch::Desk(cursor) => catalog
                    .search_channels_with_cancellation(
                        self.term,
                        PageRequest::new(cursor, limit),
                        is_cancelled,
                    )
                    .map(ReadResult::Desk),
            }
        };
        SearchCompletion {
            id: self.id,
            generation: self.generation,
            result,
        }
    }

    /// Returns a failed read if the worker could not take the request.
    pub fn unavailable(&self) -> SearchCompletion {
        SearchCompletion {
            id: self.id,
            generation: self.generation,
            result: Err(CoreError::Cancelled),
        }
    }
}

/// Identifies a Programme in an accepted inline Search read across changes to
/// visibility. Replacing the read invalidates its keys.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SearchProgrammeKey {
    read: u64,
    rank: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchSelection {
    Channel(u32),
    Programme(SearchProgrammeKey),
}

/// A search tune leaves the durable Quality Variant preference alone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchTune {
    pub generation: CatalogGeneration,
    pub channel: ChannelId,
}

/// The matched variants of a Channel, folded at its first ranked position.
pub struct SearchChannelRow {
    variants: Vec<ChannelSummary>,
    selected: usize,
    hidden: bool,
}

impl SearchChannelRow {
    pub fn channel(&self) -> &ChannelSummary {
        &self.variants[self.selected]
    }

    pub fn variants(&self) -> &[ChannelSummary] {
        &self.variants
    }

    pub fn title(&self) -> &str {
        channel_title(self.channel())
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Reads the matched variants' already loaded Guide Programmes, starting
    /// with the Channel this hit tunes. It never triggers another catalog read.
    pub fn programme_on_now<'a>(
        &self,
        family: &'a GuideFamily,
        now: DateTime<Utc>,
    ) -> Option<&'a GuideProgramme> {
        let selected = self.channel();
        std::iter::once(selected)
            .chain(
                self.variants
                    .iter()
                    .filter(|variant| variant.id() != selected.id()),
            )
            .find_map(|variant| {
                let row = family
                    .variants()
                    .iter()
                    .find(|row| row.channel().id() == variant.id())?;
                programme_at(row.programmes(), now)
            })
    }
}

#[derive(Default)]
struct Hits {
    raw_channels: Vec<ChannelSummary>,
    raw_programmes: Vec<ProgrammeSearchHit>,
    rows: RankedRows,
    programmes: Vec<ProgrammeSearchHit>,
    programme_ranks: Vec<usize>,
    programme_read: u64,
}

#[derive(Default)]
struct RankedRows {
    rows: Vec<SearchChannelRow>,
    by_number: HashMap<u32, usize>,
    hidden_count: usize,
}

impl RankedRows {
    fn append(
        &mut self,
        channel: &ChannelSummary,
        hidden: bool,
        include_hidden: bool,
        preferences: &VariantPreferences,
    ) {
        if hidden && !include_hidden {
            self.hidden_count += 1;
            return;
        }
        let index = *self.by_number.entry(channel.number()).or_insert_with(|| {
            let index = self.rows.len();
            self.rows.push(SearchChannelRow {
                variants: Vec::new(),
                selected: 0,
                hidden,
            });
            index
        });
        let row = &mut self.rows[index];
        let incoming_quality = quality(channel);
        let order = row
            .variants
            .partition_point(|existing| quality(existing) <= incoming_quality);
        row.variants.insert(order, channel.clone());
        let first = &row.variants[0];
        let wanted = preferences.quality(first.group(), channel_title(first));
        let available = row.variants.iter().position(|variant| {
            let candidate = quality(variant);
            candidate.is_some() && candidate == wanted
        });
        row.selected = available.unwrap_or_else(|| {
            let best = row.variants.last().and_then(quality);
            row.variants
                .partition_point(|variant| quality(variant) < best)
        });
    }
}

fn quality(channel: &ChannelSummary) -> Option<ChannelQuality> {
    channel.variant().map(|variant| variant.quality())
}

fn channel_title(channel: &ChannelSummary) -> &str {
    channel
        .variant()
        .map_or_else(|| channel.name(), |variant| variant.base_name())
}

#[derive(Default, Eq, PartialEq)]
enum Input {
    #[default]
    Empty,
    Valid(SearchTerm),
    Invalid,
}

impl Input {
    fn from_field(field: &str) -> Self {
        if field.len() > MAX_FIELD_BYTES {
            return Self::Invalid;
        }
        match SearchTerm::parse(field) {
            Ok(term) => Self::Valid(term),
            Err(CoreError::InvalidInput {
                reason: InputReason::Required,
                ..
            }) => Self::Empty,
            Err(_) => Self::Invalid,
        }
    }
}

#[derive(Default)]
enum Pending {
    #[default]
    Idle,
    Debouncing(SearchDebounce),
    Reading {
        id: u64,
        cancelled: BlockingTaskCancellation,
    },
    Failed,
    ReconcileGeneration,
}

/// Owns inline and desk Search for one viewer. The search field itself belongs
/// to the host, which passes committed edits and retains its editor focus.
pub struct SearchState {
    view: SearchView,
    generation: Option<CatalogGeneration>,
    input: Input,
    pending: Pending,
    hits: Hits,
    next: Option<PageCursor>,
    include_hidden: bool,
    excluded: HashSet<String>,
    preferences: Arc<VariantPreferences>,
}

impl SearchState {
    pub fn new(generation: Option<CatalogGeneration>) -> Self {
        Self {
            view: SearchView::Closed,
            generation,
            input: Input::Empty,
            pending: Pending::Idle,
            hits: Hits::default(),
            next: None,
            include_hidden: false,
            excluded: HashSet::new(),
            preferences: Arc::default(),
        }
    }

    pub fn view(&self) -> SearchView {
        self.view
    }

    /// Changes surface while retaining the field's committed term. Include
    /// hidden applies only to the current visit to the Search Desk.
    pub fn set_view(&mut self, view: SearchView) -> Option<SearchDebounce> {
        if self.view == view {
            return None;
        }
        self.view = view;
        self.include_hidden = false;
        self.restart()
    }

    pub fn set_term(&mut self, field: &str) -> Option<SearchDebounce> {
        let input = Input::from_field(field);
        if self.input == input {
            return None;
        }
        self.input = input;
        self.restart()
    }

    /// Apply the current status after a publication or stale result.
    pub fn set_generation(
        &mut self,
        generation: Option<CatalogGeneration>,
    ) -> Option<SearchDebounce> {
        if self.generation == generation && !matches!(self.pending, Pending::ReconcileGeneration) {
            return None;
        }
        self.generation = generation;
        self.restart()
    }

    pub fn set_preferences(
        &mut self,
        excluded: HashSet<String>,
        preferences: Arc<VariantPreferences>,
    ) {
        self.excluded = excluded;
        self.preferences = preferences;
        self.project_hits();
    }

    /// Reprojects loaded ranks without restarting or advancing the core query.
    pub fn set_include_hidden(&mut self, include: bool) {
        if self.view == SearchView::Desk && self.include_hidden != include {
            self.include_hidden = include;
            self.project_hits();
        }
    }

    pub fn include_hidden(&self) -> bool {
        self.view == SearchView::Desk && self.include_hidden
    }

    pub fn status(&self) -> SearchStatus {
        if self.view == SearchView::Closed {
            return SearchStatus::Idle;
        }
        if self.input == Input::Invalid {
            return SearchStatus::Invalid;
        }
        if self.generation.is_none() {
            return SearchStatus::Unavailable;
        }
        if self.input == Input::Empty {
            return SearchStatus::Idle;
        }
        let visible = !self.hits.rows.rows.is_empty() || !self.hits.programmes.is_empty();
        match self.pending {
            Pending::Debouncing(_) => SearchStatus::Loading,
            Pending::Reading { .. } if !visible => SearchStatus::Loading,
            Pending::Failed => SearchStatus::Failed,
            Pending::ReconcileGeneration => SearchStatus::ReconcileGeneration,
            _ if visible => SearchStatus::Ready,
            _ if self.should_advance_hidden() => SearchStatus::Loading,
            _ if self.hidden_count() > 0 => SearchStatus::Hidden,
            _ => SearchStatus::Empty,
        }
    }

    pub fn channels(&self) -> &[SearchChannelRow] {
        let rows = &self.hits.rows.rows;
        match self.view {
            SearchView::Closed => &[],
            SearchView::Inline => &rows[..rows.len().min(INLINE_LIMIT)],
            SearchView::Desk => rows,
        }
    }

    pub fn programmes(&self) -> &[ProgrammeSearchHit] {
        let rows = &self.hits.programmes;
        if self.view == SearchView::Inline {
            &rows[..rows.len().min(INLINE_LIMIT)]
        } else {
            &[]
        }
    }

    /// Returns the identity to carry with a visible Programme focus target.
    pub fn programme_key(&self, index: usize) -> Option<SearchProgrammeKey> {
        self.programmes().get(index)?;
        Some(SearchProgrammeKey {
            read: self.hits.programme_read,
            rank: self.hits.programme_ranks[index],
        })
    }

    /// Finds a remembered target in the current visible Programme results.
    pub fn programme_index(&self, key: SearchProgrammeKey) -> Option<usize> {
        if key.read != self.hits.programme_read {
            return None;
        }
        self.hits
            .programme_ranks
            .iter()
            .take(self.programmes().len())
            .position(|&rank| rank == key.rank)
    }

    /// Counts raw hits removed by the group filter before family folding.
    pub fn hidden_count(&self) -> usize {
        self.hits.rows.hidden_count + self.hits.raw_programmes.len() - self.hits.programmes.len()
    }

    pub fn has_more(&self) -> bool {
        self.view == SearchView::Desk && self.next.is_some()
    }

    pub fn loading_more(&self) -> bool {
        self.has_more() && matches!(self.pending, Pending::Reading { .. })
    }

    pub fn should_advance_hidden(&self) -> bool {
        self.has_more()
            && !self.include_hidden
            && !self.hits.raw_channels.is_empty()
            && self.hits.rows.rows.is_empty()
            && matches!(self.pending, Pending::Idle)
    }

    pub fn finish_debounce(&mut self, ticket: SearchDebounce) -> Option<SearchRead> {
        if !matches!(self.pending, Pending::Debouncing(current) if ticket == current) {
            return None;
        }
        self.request()
    }

    pub fn request_more(&mut self) -> Option<SearchRead> {
        if self.has_more() && matches!(self.pending, Pending::Idle) {
            self.request()
        } else {
            None
        }
    }

    pub fn retry(&mut self) -> Option<SearchRead> {
        if matches!(self.pending, Pending::Failed) {
            self.request()
        } else {
            None
        }
    }

    pub fn complete(&mut self, outcome: SearchCompletion) -> PageUpdate {
        if self.generation != Some(outcome.generation)
            || !matches!(self.pending, Pending::Reading { id, .. } if id == outcome.id)
        {
            return PageUpdate::Superseded;
        }
        self.pending = Pending::Idle;
        match outcome.result {
            Ok(ReadResult::Inline(results)) if results.generation() == outcome.generation => {
                self.hits.programme_read = outcome.id;
                self.hits
                    .raw_programmes
                    .extend_from_slice(results.programmes().items());
                self.project_programmes();
                self.append_channels(results.channels().items());
            }
            Ok(ReadResult::Desk(page)) if page.generation() == outcome.generation => {
                self.next = page.next().cloned();
                self.append_channels(page.items());
            }
            Ok(_) | Err(CoreError::StaleCursor { .. }) => {
                self.hits = Hits::default();
                self.next = None;
                self.pending = Pending::ReconcileGeneration;
                return PageUpdate::ReconcileGeneration;
            }
            Err(_) => {
                self.pending = Pending::Failed;
                return PageUpdate::Failed;
            }
        }
        PageUpdate::Applied
    }

    /// Resolves a currently visible hit and clears Search after a tune.
    pub fn tune(&mut self, selection: SearchSelection) -> Option<SearchTune> {
        if self.status() != SearchStatus::Ready {
            return None;
        }
        let channel = match selection {
            SearchSelection::Channel(number) => self
                .channels()
                .iter()
                .find(|row| row.channel().number() == number)?
                .channel(),
            SearchSelection::Programme(key) => {
                self.programmes()[self.programme_index(key)?].channel()
            }
        };
        let intent = SearchTune {
            generation: self.generation?,
            channel: channel.id().clone(),
        };
        self.input = Input::Empty;
        self.set_view(SearchView::Closed);
        Some(intent)
    }

    fn restart(&mut self) -> Option<SearchDebounce> {
        self.cancel_read();
        self.hits = Hits::default();
        self.next = None;
        self.pending = Pending::Idle;
        if self.view == SearchView::Closed
            || self.generation.is_none()
            || !matches!(self.input, Input::Valid(_))
        {
            return None;
        }
        let ticket = SearchDebounce(NEXT_ID.fetch_add(1, Ordering::Relaxed));
        self.pending = Pending::Debouncing(ticket);
        Some(ticket)
    }

    fn request(&mut self) -> Option<SearchRead> {
        let Input::Valid(term) = &self.input else {
            return None;
        };
        let branch = match self.view {
            SearchView::Closed => return None,
            SearchView::Inline => Branch::Inline,
            SearchView::Desk => Branch::Desk(self.next.clone()),
        };
        let generation = self.generation?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let cancelled = BlockingTaskCancellation::new();
        self.pending = Pending::Reading {
            id,
            cancelled: cancelled.clone(),
        };
        Some(SearchRead {
            id,
            generation,
            term: term.clone(),
            branch,
            cancelled,
        })
    }

    fn append_channels(&mut self, channels: &[ChannelSummary]) {
        let include_hidden = self.include_hidden();
        for channel in channels {
            let hidden = self.excluded.contains(channel.group());
            self.hits
                .rows
                .append(channel, hidden, include_hidden, &self.preferences);
        }
        self.hits.raw_channels.extend_from_slice(channels);
    }

    fn project_hits(&mut self) {
        let include_hidden = self.include_hidden();
        self.hits.rows = RankedRows::default();
        for channel in &self.hits.raw_channels {
            let hidden = self.excluded.contains(channel.group());
            self.hits
                .rows
                .append(channel, hidden, include_hidden, &self.preferences);
        }
        self.project_programmes();
    }

    fn project_programmes(&mut self) {
        self.hits.programmes.clear();
        self.hits.programme_ranks.clear();
        for (rank, hit) in self.hits.raw_programmes.iter().enumerate() {
            if !self.excluded.contains(hit.channel().group()) {
                self.hits.programmes.push(hit.clone());
                self.hits.programme_ranks.push(rank);
            }
        }
    }

    fn cancel_read(&self) {
        if let Pending::Reading { cancelled, .. } = &self.pending {
            cancelled.cancel();
        }
    }
}

impl Drop for SearchState {
    fn drop(&mut self) {
        self.cancel_read();
    }
}
