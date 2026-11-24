//! Focus and host effects for the Shield Guide's inline Search and Channel desk.

use std::{collections::HashSet, sync::Arc};

use sparrow_core::CatalogGeneration;
use sparrow_tv_guide::{
    PageUpdate, VariantPreferences,
    guide_screen::ScreenKey,
    now_focus::Direction,
    search::{
        SearchCompletion, SearchDebounce, SearchRead, SearchSelection, SearchState, SearchStatus,
        SearchTune, SearchView,
    },
};

use crate::guide_view::SearchFocus;

const PREFETCH_TAIL_ROWS: usize = 6;

pub(super) enum SearchEffect {
    None,
    Edit,
    Debounce(SearchDebounce),
    Read(SearchRead),
    ReconcileGeneration,
    Dismiss,
    GuideKey(ScreenKey),
    Tune(SearchTune),
}

/// Owns surface focus and the editor's displayed text. SearchState owns the
/// normalized term, result pages, request identities and worker cancellation.
pub(super) struct SearchFlow {
    state: SearchState,
    term: String,
    focus: SearchFocus,
    has_exclusions: bool,
    initial_desk_focus: bool,
    more_anchor: Option<usize>,
}

impl SearchFlow {
    pub(super) fn new(generation: Option<CatalogGeneration>) -> Self {
        Self {
            state: SearchState::new(generation),
            term: String::new(),
            focus: SearchFocus::Field,
            has_exclusions: false,
            initial_desk_focus: false,
            more_anchor: None,
        }
    }

    pub(super) fn state(&self) -> &SearchState {
        &self.state
    }

    pub(super) fn term(&self) -> &str {
        &self.term
    }

    pub(super) fn focus(&self) -> SearchFocus {
        self.focus
    }

    pub(super) fn has_exclusions(&self) -> bool {
        self.has_exclusions
    }

    /// Opens the toolbar Search before the Activity takes editor focus.
    pub(super) fn open_inline(&mut self) -> Option<SearchDebounce> {
        self.focus = SearchFocus::Field;
        self.clear_handoff();
        self.state.set_view(SearchView::Inline)
    }

    /// Accepts text from the current Activity editor. The host owns IME focus
    /// and rejects updates from superseded editor sessions.
    pub(super) fn edit(&mut self, text: String) -> Option<SearchDebounce> {
        if self.state.view() == SearchView::Closed || self.term == text {
            return None;
        }
        self.term = text;
        self.focus = SearchFocus::Field;
        self.clear_handoff();
        self.state.set_term(&self.term)
    }

    pub(super) fn set_generation(
        &mut self,
        generation: Option<CatalogGeneration>,
    ) -> Option<SearchDebounce> {
        let timer = self.state.set_generation(generation);
        if timer.is_some() {
            self.more_anchor = None;
        }
        self.reconcile_focus();
        timer
    }

    pub(super) fn set_preferences(
        &mut self,
        excluded: HashSet<String>,
        preferences: Arc<VariantPreferences>,
    ) -> SearchEffect {
        let programme = match self.focus {
            SearchFocus::Programme(index) => self.state.programme_key(index),
            _ => None,
        };
        self.has_exclusions = !excluded.is_empty();
        self.state.set_preferences(excluded, preferences);
        if let Some(programme) = programme {
            self.focus = self
                .state
                .programme_index(programme)
                .map_or(SearchFocus::Field, SearchFocus::Programme);
        }
        self.reconcile_focus();
        self.advance_hidden()
    }

    pub(super) fn finish_debounce(&mut self, ticket: SearchDebounce) -> Option<SearchRead> {
        self.state.finish_debounce(ticket)
    }

    /// A current completion may advance a hidden-only desk or hand off focus.
    /// Superseded completions cannot move it.
    pub(super) fn complete(&mut self, completion: SearchCompletion) -> (PageUpdate, SearchEffect) {
        let update = self.state.complete(completion);
        if update == PageUpdate::Superseded {
            return (update, SearchEffect::None);
        }

        let previous_focus = self.focus;
        self.settle_handoff();
        self.reconcile_focus();
        let effect = if self.focus == previous_focus {
            self.advance_hidden()
        } else {
            self.request_for_focus()
        };
        (update, effect)
    }

    pub(super) fn key(&mut self, key: ScreenKey) -> SearchEffect {
        match (self.state.view(), key) {
            (SearchView::Closed, _) => SearchEffect::GuideKey(key),
            (_, ScreenKey::Activate) => self.activate(self.focus),
            (SearchView::Inline, ScreenKey::Arrow(direction)) => self.inline_arrow(direction),
            (SearchView::Desk, ScreenKey::Arrow(direction)) => {
                if let Some(target) = self.desk_target(direction) {
                    self.move_focus(target)
                } else {
                    SearchEffect::None
                }
            }
        }
    }

    /// Activates a typed renderer target. Stale or hidden choices have no effect.
    pub(super) fn activate(&mut self, target: SearchFocus) -> SearchEffect {
        if !self.available(target) {
            return SearchEffect::None;
        }
        self.focus = target;
        self.clear_handoff();
        match target {
            SearchFocus::Field => SearchEffect::Edit,
            SearchFocus::Clear => {
                self.term.clear();
                self.state.set_term("");
                self.back()
            }
            SearchFocus::Close => self.back(),
            SearchFocus::Desk => self.open_desk(),
            SearchFocus::Retry if self.state.status() == SearchStatus::ReconcileGeneration => {
                SearchEffect::ReconcileGeneration
            }
            SearchFocus::Retry => self
                .state
                .retry()
                .map_or(SearchEffect::None, SearchEffect::Read),
            SearchFocus::IncludeHidden => {
                self.state.set_include_hidden(!self.state.include_hidden());
                self.advance_hidden()
            }
            SearchFocus::More => {
                self.more_anchor = Some(self.state.channels().len());
                self.state
                    .request_more()
                    .map_or(SearchEffect::None, SearchEffect::Read)
            }
            SearchFocus::Channel(number) => self.tune(SearchSelection::Channel(number)),
            SearchFocus::Programme(index) => match self.state.programme_key(index) {
                Some(key) => self.tune(SearchSelection::Programme(key)),
                None => SearchEffect::None,
            },
        }
    }

    /// Close the overlay and restore the Guide toolbar's Search field. The
    /// Activity handles Back from an open IME before passing it here.
    pub(super) fn back(&mut self) -> SearchEffect {
        if self.state.view() == SearchView::Closed {
            return SearchEffect::None;
        }
        self.state.set_view(SearchView::Closed);
        self.focus = SearchFocus::Field;
        self.clear_handoff();
        SearchEffect::Dismiss
    }

    fn open_desk(&mut self) -> SearchEffect {
        let timer = self.state.set_view(SearchView::Desk);
        self.state.set_include_hidden(false);
        self.focus = SearchFocus::Close;
        self.initial_desk_focus = self.state.status() == SearchStatus::Loading;
        timer.map_or(SearchEffect::None, SearchEffect::Debounce)
    }

    fn tune(&mut self, selection: SearchSelection) -> SearchEffect {
        let Some(tune) = self.state.tune(selection) else {
            return SearchEffect::None;
        };
        self.term.clear();
        self.focus = SearchFocus::Field;
        SearchEffect::Tune(tune)
    }

    fn inline_arrow(&mut self, direction: Direction) -> SearchEffect {
        if matches!(self.focus, SearchFocus::Field | SearchFocus::Clear) {
            let target = match (self.focus, direction) {
                (SearchFocus::Field, Direction::Right) if self.available(SearchFocus::Clear) => {
                    Some(SearchFocus::Clear)
                }
                (SearchFocus::Clear, Direction::Left) => Some(SearchFocus::Field),
                (_, Direction::Down) => self.inline_options().next(),
                (SearchFocus::Clear, Direction::Right) => return SearchEffect::None,
                _ => None,
            };
            if let Some(target) = target {
                return self.move_focus(target);
            }
            self.back();
            return SearchEffect::GuideKey(ScreenKey::Arrow(direction));
        }
        if matches!(direction, Direction::Left | Direction::Right) {
            return SearchEffect::None;
        }

        let target = {
            let mut previous = SearchFocus::Field;
            let mut options = self.inline_options();
            loop {
                let Some(option) = options.next() else {
                    break Some(SearchFocus::Field);
                };
                if option == self.focus {
                    break if direction == Direction::Up {
                        Some(previous)
                    } else {
                        options.next()
                    };
                }
                previous = option;
            }
        };
        target.map_or(SearchEffect::None, |target| self.move_focus(target))
    }

    fn inline_options(&self) -> impl Iterator<Item = SearchFocus> + '_ {
        self.available(SearchFocus::Retry)
            .then_some(SearchFocus::Retry)
            .into_iter()
            .chain(
                self.available(SearchFocus::Desk)
                    .then_some(SearchFocus::Desk),
            )
            .chain(
                self.state
                    .channels()
                    .iter()
                    .map(|row| SearchFocus::Channel(row.channel().number())),
            )
            .chain((0..self.state.programmes().len()).map(SearchFocus::Programme))
    }

    fn desk_target(&self, direction: Direction) -> Option<SearchFocus> {
        use Direction::*;
        use SearchFocus::*;
        match (self.focus, direction) {
            (Close, Down) => Some(Field),
            (Field, Up) => Some(Close),
            (Field, Right) if self.has_exclusions => Some(IncludeHidden),
            (Field, Down) if self.has_exclusions => Some(IncludeHidden),
            (Field | IncludeHidden, Down) => self.first_desk_result(),
            (IncludeHidden, Up | Left) => Some(Field),
            (Retry, Up) => Some(self.desk_toolbar_tail()),
            (Channel(number), Up) => {
                let index = self.channel_index(number)?;
                Some(
                    index
                        .checked_sub(1)
                        .and_then(|index| self.channel_at(index))
                        .unwrap_or_else(|| self.desk_toolbar_tail()),
                )
            }
            (Channel(number), Down) => self
                .channel_at(self.channel_index(number)? + 1)
                .or_else(|| self.available(More).then_some(More)),
            (More, Up) => self.last_channel(),
            _ => None,
        }
    }

    fn desk_toolbar_tail(&self) -> SearchFocus {
        if self.has_exclusions {
            SearchFocus::IncludeHidden
        } else {
            SearchFocus::Field
        }
    }

    fn first_desk_result(&self) -> Option<SearchFocus> {
        if self.available(SearchFocus::Retry) {
            Some(SearchFocus::Retry)
        } else if self.state.status() == SearchStatus::Ready {
            self.channel_at(0)
        } else {
            None
        }
    }

    fn available(&self, target: SearchFocus) -> bool {
        let view = self.state.view();
        if view == SearchView::Closed {
            return false;
        }
        let status = self.state.status();
        match target {
            SearchFocus::Field => true,
            SearchFocus::Clear => view == SearchView::Inline && !self.term.is_empty(),
            SearchFocus::Close => view == SearchView::Desk,
            SearchFocus::Desk => {
                view == SearchView::Inline
                    && !self.term.trim().is_empty()
                    && !matches!(
                        status,
                        SearchStatus::Idle | SearchStatus::Invalid | SearchStatus::Unavailable
                    )
            }
            SearchFocus::Retry => {
                status == SearchStatus::ReconcileGeneration
                    || (view == SearchView::Desk && status == SearchStatus::Failed)
            }
            SearchFocus::IncludeHidden => view == SearchView::Desk && self.has_exclusions,
            SearchFocus::Channel(number) => {
                status == SearchStatus::Ready && self.channel_index(number).is_some()
            }
            SearchFocus::Programme(index) => {
                view == SearchView::Inline
                    && status == SearchStatus::Ready
                    && index < self.state.programmes().len()
            }
            SearchFocus::More => {
                view == SearchView::Desk && status == SearchStatus::Ready && self.state.has_more()
            }
        }
    }

    fn move_focus(&mut self, target: SearchFocus) -> SearchEffect {
        if target == self.focus {
            return SearchEffect::None;
        }
        self.focus = target;
        self.clear_handoff();
        if target == SearchFocus::More {
            self.more_anchor = Some(self.state.channels().len());
        }
        self.request_for_focus()
    }

    fn request_for_focus(&mut self) -> SearchEffect {
        if self.state.view() != SearchView::Desk {
            return SearchEffect::None;
        }
        let near_tail = match self.focus {
            SearchFocus::Channel(number) => self
                .channel_index(number)
                .is_some_and(|index| self.state.channels().len() - index <= PREFETCH_TAIL_ROWS),
            SearchFocus::More => true,
            _ => false,
        };
        if near_tail && let Some(read) = self.state.request_more() {
            return SearchEffect::Read(read);
        }
        self.advance_hidden()
    }

    fn advance_hidden(&mut self) -> SearchEffect {
        if self.state.should_advance_hidden()
            && let Some(read) = self.state.request_more()
        {
            return SearchEffect::Read(read);
        }
        SearchEffect::None
    }

    fn settle_handoff(&mut self) {
        if self.initial_desk_focus && self.state.status() != SearchStatus::Loading {
            self.initial_desk_focus = false;
            if self.focus == SearchFocus::Close
                && let Some(target) = self.first_desk_result()
            {
                self.focus = target;
            }
        }
        if let Some(anchor) = self.more_anchor.take()
            && self.focus == SearchFocus::More
        {
            if self.available(SearchFocus::Retry) {
                self.focus = SearchFocus::Retry;
            } else if let Some(first_new_row) = self.channel_at(anchor) {
                self.focus = first_new_row;
            } else if !self.state.has_more() {
                self.focus = self.last_channel().unwrap_or(SearchFocus::Field);
            }
        }
    }

    fn reconcile_focus(&mut self) {
        if self.available(self.focus) {
            return;
        }
        self.clear_handoff();
        self.focus = if self.available(SearchFocus::Retry) {
            SearchFocus::Retry
        } else {
            SearchFocus::Field
        };
    }

    fn clear_handoff(&mut self) {
        self.initial_desk_focus = false;
        self.more_anchor = None;
    }

    fn channel_index(&self, number: u32) -> Option<usize> {
        self.state
            .channels()
            .iter()
            .position(|row| row.channel().number() == number)
    }

    fn channel_at(&self, index: usize) -> Option<SearchFocus> {
        Some(SearchFocus::Channel(
            self.state.channels().get(index)?.channel().number(),
        ))
    }

    fn last_channel(&self) -> Option<SearchFocus> {
        Some(SearchFocus::Channel(
            self.state.channels().last()?.channel().number(),
        ))
    }
}
