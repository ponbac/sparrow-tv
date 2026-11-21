use std::collections::HashMap;

use crate::domain::{CoreError, PageLimit, SearchTerm, SelectionPrefix};

use super::flat::{ABSENT_U32, FlatCatalog};

pub(super) const SEARCH_CANCELLATION_CHECKPOINT_BYTES: usize = 4 * 1024;

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
    term_bytes: &'term [u8],
    token_lookup: HashMap<&'term [u8], usize>,
    matched_tokens: Vec<bool>,
    longest_token: usize,
    substring_fallback: Vec<usize>,
}

impl<'term> SearchMatcher<'term> {
    pub(super) fn new(term: &'term SearchTerm) -> Self {
        let term = term.as_str();
        let term_bytes = term.as_bytes();
        let token_count = term_bytes.split(|&b| b == b' ').count();
        let mut token_lookup = HashMap::with_capacity(token_count);
        for token in term_bytes.split(|&b| b == b' ') {
            let next_index = token_lookup.len();
            token_lookup.entry(token).or_insert(next_index);
        }
        let longest_token = token_lookup
            .keys()
            .map(|token| token.len())
            .max()
            .unwrap_or(0);
        let matched_tokens = vec![false; token_lookup.len()];
        let substring_fallback = substring_fallback(term_bytes);
        Self {
            term,
            term_bytes,
            token_lookup,
            matched_tokens,
            longest_token,
            substring_fallback,
        }
    }

    pub(super) fn field_rank(
        &mut self,
        field: &[u8],
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<MatchCategory>, CoreError> {
        if field == self.term_bytes {
            return Ok(Some(MatchCategory::Exact));
        }
        if field.starts_with(self.term_bytes) {
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
        field: &[u8],
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, CoreError> {
        if is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        self.matched_tokens.fill(false);
        let mut missing = self.matched_tokens.len();
        let mut token_start = 0;

        for (chunk_index, chunk) in field
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

    fn mark_token(&mut self, token: &[u8], missing: &mut usize) -> bool {
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
        field: &[u8],
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, CoreError> {
        if is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let needle = self.term_bytes;
        let mut matched = 0;
        for (chunk_index, chunk) in field
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

impl RankBucket {
    fn record(&mut self, item_index: usize, prefix_len: usize) {
        self.total += 1;
        if self.prefix.len() < prefix_len {
            if self.prefix.capacity() == 0 {
                self.prefix
                    .reserve_exact(prefix_len.min(usize::from(PageLimit::MAX)));
            }
            self.prefix.push(item_index);
        }
    }
}

const SEARCH_FIELDS: usize = 2;
const RANK_BUCKETS: usize = 4 * SEARCH_FIELDS;

fn finalize_buckets(
    buckets: [RankBucket; RANK_BUCKETS],
    prefix_len: usize,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<SelectionPrefix, CoreError> {
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

/// Ranks Channels by scanning groups in catalog order. Each group's normalized
/// name is ranked once instead of once per Channel, avoiding faulting
/// `channels_hot` during Channel search.
pub(super) fn ranked_channel_selection(
    catalog: &FlatCatalog,
    matcher: &mut SearchMatcher<'_>,
    requested_prefix_len: usize,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<SelectionPrefix, CoreError> {
    let spans = catalog.channel_search_spans();
    let arena = catalog.channel_search_arena();
    let prefix_len = requested_prefix_len.min(spans.len());
    let mut buckets: [RankBucket; RANK_BUCKETS] = std::array::from_fn(|_| RankBucket::default());

    for (group_index, group) in catalog.groups().iter().enumerate() {
        let start = group.channel_start.get() as usize;
        let end = group.channel_end.get() as usize;
        if start == end {
            continue;
        }
        if is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let group_norm = catalog.group_search_name_bytes(group_index);
        let secondary_rank = matcher
            .field_rank(group_norm, is_cancelled)?
            .map(|category| SearchRank {
                category,
                field_index: 1,
            });
        for (item_index, span) in spans[start..end].iter().enumerate() {
            let item_index = start + item_index;
            if is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            let off = span.off.get() as usize;
            let len = span.len.get() as usize;
            let primary_rank = matcher
                .field_rank(&arena[off..off + len], is_cancelled)?
                .map(|category| SearchRank {
                    category,
                    field_index: 0,
                });
            let best = match (primary_rank, secondary_rank) {
                (Some(p), Some(s)) => Some(p.min(s)),
                (Some(p), None) => Some(p),
                (None, Some(s)) => Some(s),
                (None, None) => None,
            };
            if let Some(rank) = best {
                buckets[rank.bucket_index()].record(item_index, prefix_len);
            }
        }
    }

    finalize_buckets(buckets, prefix_len, is_cancelled)
}

/// Ranks Programmes in exact `schedule_entries` order (ChannelId order, then
/// schedule order) so equal-ranked ties preserve the exact pagination contract.
pub(super) fn ranked_programme_selection(
    catalog: &FlatCatalog,
    matcher: &mut SearchMatcher<'_>,
    requested_prefix_len: usize,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<SelectionPrefix, CoreError> {
    let entries = catalog.schedule_entries();
    let spans = catalog.programme_search_spans();
    let arena = catalog.programme_search_arena();
    let prefix_len = requested_prefix_len.min(entries.len());
    let mut buckets: [RankBucket; RANK_BUCKETS] = std::array::from_fn(|_| RankBucket::default());

    for (item_index, entry) in entries.iter().enumerate() {
        if is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let doc = &spans[entry.matched_prog_index.get() as usize];
        let p_off = doc.primary_off.get() as usize;
        let p_len = doc.primary_len.get() as usize;
        let mut best = matcher
            .field_rank(&arena[p_off..p_off + p_len], is_cancelled)?
            .map(|category| SearchRank {
                category,
                field_index: 0,
            });
        if doc.secondary_off.get() != ABSENT_U32 {
            let s_off = doc.secondary_off.get() as usize;
            let s_len = doc.secondary_len.get() as usize;
            if let Some(category) =
                matcher.field_rank(&arena[s_off..s_off + s_len], is_cancelled)?
            {
                let candidate = SearchRank {
                    category,
                    field_index: 1,
                };
                best = Some(best.map_or(candidate, |current| current.min(candidate)));
            }
        }
        if let Some(rank) = best {
            buckets[rank.bucket_index()].record(item_index, prefix_len);
        }
    }

    finalize_buckets(buckets, prefix_len, is_cancelled)
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
