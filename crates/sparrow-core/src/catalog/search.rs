use std::collections::HashMap;

use crate::domain::{CoreError, SearchTerm, SelectionPrefix, normalize_search_text};

pub(super) const SEARCH_CANCELLATION_CHECKPOINT_BYTES: usize = 4 * 1024;

pub(super) struct SearchIndex {
    // Normalized fields share one allocation; each document retains only offsets
    // instead of one or two independently allocated Strings.
    pub(super) arena: String,
    pub(super) documents: Box<[SearchDocument]>,
}

impl SearchIndex {
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.documents.len()
    }
}

pub(super) struct SearchIndexBuilder {
    arena: String,
    documents: Vec<SearchDocument>,
}

impl SearchIndexBuilder {
    pub(super) fn with_capacity(document_capacity: usize, byte_capacity: usize) -> Self {
        Self {
            arena: String::with_capacity(byte_capacity),
            documents: Vec::with_capacity(document_capacity),
        }
    }

    pub(super) fn push(&mut self, primary: &str, secondary: Option<&str>) {
        let primary = self.push_field(primary);
        let secondary = secondary.map(|value| self.push_field(value));
        self.documents.push(SearchDocument::new(primary, secondary));
    }

    fn push_field(&mut self, value: &str) -> SearchField {
        let normalized = normalize_search_text(value);
        let start = self.arena.len();
        self.arena.push_str(&normalized);
        let end = self.arena.len();
        SearchField { start, end }
    }

    pub(super) fn finish(self) -> SearchIndex {
        SearchIndex {
            arena: self.arena,
            documents: self.documents.into_boxed_slice(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct SearchField {
    start: usize,
    end: usize,
}

pub(super) struct SearchDocument {
    pub(super) primary: SearchField,
    secondary_start: usize,
    secondary_end: usize,
}

impl SearchDocument {
    fn new(primary: SearchField, secondary: Option<SearchField>) -> Self {
        let (secondary_start, secondary_end) =
            secondary.map_or((usize::MAX, usize::MAX), |field| (field.start, field.end));
        Self {
            primary,
            secondary_start,
            secondary_end,
        }
    }

    fn rank(
        &self,
        arena: &str,
        matcher: &mut SearchMatcher<'_>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<SearchRank>, CoreError> {
        let mut best: Option<SearchRank> = None;
        for (field_index, field) in std::iter::once(self.primary.get(arena))
            .chain(self.secondary().map(|field| field.get(arena)))
            .enumerate()
        {
            let Some(category) = matcher.field_rank(field, is_cancelled)? else {
                continue;
            };
            let candidate = SearchRank {
                category,
                field_index,
            };
            best = Some(best.map_or(candidate, |current| current.min(candidate)));
        }
        Ok(best)
    }

    pub(super) fn secondary(&self) -> Option<SearchField> {
        (self.secondary_start != usize::MAX).then_some(SearchField {
            start: self.secondary_start,
            end: self.secondary_end,
        })
    }
}

impl SearchField {
    pub(super) fn get(self, arena: &str) -> &str {
        &arena[self.start..self.end]
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct SearchRank {
    category: MatchCategory,
    field_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum MatchCategory {
    Exact,
    Prefix,
    Token,
    Substring,
}

// Request-local because a Search Term may contain private caller text. The
// compiled lookup and scratch are reused across both result lanes, then dropped.
pub(super) struct SearchMatcher<'term> {
    pub(super) term: &'term str,
    token_lookup: HashMap<&'term str, usize>,
    matched_tokens: Vec<bool>,
    longest_token: usize,
    substring_fallback: Vec<usize>,
}

impl<'term> SearchMatcher<'term> {
    pub(super) fn new(term: &'term SearchTerm) -> Self {
        let term = term.as_str();
        let token_count = term.split(' ').count();
        let mut token_lookup = HashMap::with_capacity(token_count);
        for token in term.split(' ') {
            let next_index = token_lookup.len();
            token_lookup.entry(token).or_insert(next_index);
        }
        let longest_token = token_lookup
            .keys()
            .map(|token| token.len())
            .max()
            .unwrap_or(0);
        let matched_tokens = vec![false; token_lookup.len()];
        let substring_fallback = substring_fallback(term.as_bytes());
        Self {
            term,
            token_lookup,
            matched_tokens,
            longest_token,
            substring_fallback,
        }
    }

    pub(super) fn field_rank(
        &mut self,
        field: &str,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<MatchCategory>, CoreError> {
        if field == self.term {
            return Ok(Some(MatchCategory::Exact));
        }
        if field.starts_with(self.term) {
            return Ok(Some(MatchCategory::Prefix));
        }
        if self.contains_all_tokens(field, is_cancelled)? {
            return Ok(Some(MatchCategory::Token));
        }
        Ok(self
            .contains_substring(field, is_cancelled)?
            .then_some(MatchCategory::Substring))
    }

    fn contains_all_tokens(
        &mut self,
        field: &str,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, CoreError> {
        if is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        self.matched_tokens.fill(false);
        let mut missing = self.matched_tokens.len();
        let mut token_start = 0;

        for (chunk_index, chunk) in field
            .as_bytes()
            .chunks(SEARCH_CANCELLATION_CHECKPOINT_BYTES)
            .enumerate()
        {
            if chunk_index != 0 && is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            let chunk_start = chunk_index * SEARCH_CANCELLATION_CHECKPOINT_BYTES;
            for (offset, byte) in chunk.iter().enumerate() {
                if *byte != b' ' {
                    continue;
                }
                let token_end = chunk_start + offset;
                if token_end - token_start <= self.longest_token
                    && self.mark_token(&field[token_start..token_end], &mut missing)
                {
                    return Ok(true);
                }
                token_start = token_end + 1;
            }
        }

        if field.len() - token_start <= self.longest_token {
            self.mark_token(&field[token_start..], &mut missing);
        }
        Ok(missing == 0)
    }

    fn mark_token(&mut self, token: &str, missing: &mut usize) -> bool {
        let Some(&index) = self.token_lookup.get(token) else {
            return false;
        };
        if !self.matched_tokens[index] {
            self.matched_tokens[index] = true;
            *missing -= 1;
        }
        *missing == 0
    }

    pub(super) fn contains_substring(
        &self,
        field: &str,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, CoreError> {
        if is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let needle = self.term.as_bytes();
        let mut matched = 0;
        for (chunk_index, chunk) in field
            .as_bytes()
            .chunks(SEARCH_CANCELLATION_CHECKPOINT_BYTES)
            .enumerate()
        {
            if chunk_index != 0 && is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            for byte in chunk {
                while matched > 0 && *byte != needle[matched] {
                    matched = self.substring_fallback[matched - 1];
                }
                if *byte == needle[matched] {
                    matched += 1;
                    if matched == needle.len() {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }
}

#[derive(Default)]
struct RankBucket {
    prefix: Vec<usize>,
    total: usize,
}

pub(super) fn ranked_selection(
    index: &SearchIndex,
    matcher: &mut SearchMatcher<'_>,
    requested_prefix_len: usize,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<SelectionPrefix, CoreError> {
    const SEARCH_FIELDS: usize = 2;
    const RANK_BUCKETS: usize = 4 * SEARCH_FIELDS;
    let prefix_len = requested_prefix_len.min(index.documents.len());

    // SearchRank has a small finite range. Bucketing keeps the same total order
    // as sorting `(SearchRank, catalog index)`. Each bucket retains only the
    // prefix that could contribute to this page while still counting all matches.
    let mut buckets: [RankBucket; RANK_BUCKETS] = std::array::from_fn(|_| RankBucket::default());
    for (item_index, document) in index.documents.iter().enumerate() {
        if is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        if let Some(rank) = document.rank(&index.arena, matcher, is_cancelled)? {
            let bucket = &mut buckets[rank.bucket_index()];
            bucket.total += 1;
            if bucket.prefix.len() < prefix_len {
                bucket.prefix.push(item_index);
            }
        }
    }

    let match_count = buckets.iter().map(|bucket| bucket.total).sum();
    let mut ranked = Vec::with_capacity(prefix_len.min(match_count));
    for bucket in buckets {
        let remaining = prefix_len.saturating_sub(ranked.len());
        for item_index in bucket.prefix.into_iter().take(remaining) {
            if is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            ranked.push(item_index);
        }
        if ranked.len() == prefix_len {
            break;
        }
    }
    debug_assert_eq!(ranked.len(), prefix_len.min(match_count));
    Ok(SelectionPrefix::new(ranked, match_count))
}

impl SearchRank {
    fn bucket_index(self) -> usize {
        let category = match self.category {
            MatchCategory::Exact => 0,
            MatchCategory::Prefix => 1,
            MatchCategory::Token => 2,
            MatchCategory::Substring => 3,
        };
        category * 2 + self.field_index
    }
}

pub(super) fn never_cancelled() -> bool {
    false
}

fn substring_fallback(needle: &[u8]) -> Vec<usize> {
    let mut fallback = vec![0; needle.len()];
    let mut prefix_length = 0;
    for index in 1..needle.len() {
        while prefix_length > 0 && needle[index] != needle[prefix_length] {
            prefix_length = fallback[prefix_length - 1];
        }
        if needle[index] == needle[prefix_length] {
            prefix_length += 1;
            fallback[index] = prefix_length;
        }
    }
    fallback
}
