use std::{ops::Range, sync::Arc};

use chrono::{DateTime, Utc};
use url::Url;

use crate::{
    domain::{
        CHANNEL_ID_PREFIX, CatalogGeneration, ChannelDetails, ChannelGroupView, ChannelIcon,
        ChannelId, ChannelQuality, ChannelQuery, ChannelSummary, ChannelVariant, CoreError,
        CursorQueryHash, GuideProgramme, GuideWindowChannel, GuideWindowQuery, Page, PageLimit,
        PageRequest, PageTextArena, ProgrammeSearchHit, ProgrammeSummary, ResolvedPlaybackSource,
        ScheduleQuery, SearchRequest, SearchResults, SearchTerm, SecretPlaybackLocation,
        SelectionPrefix, SourceConfiguration, TextSlot,
    },
    m3u::ParsedChannel,
    ports::{CatalogCacheMapping, SnapshotCandidate, SnapshotStore},
    xmltv::ParsedGuide,
};

mod flat;
mod schedule;
mod search;
mod variants;

pub(crate) use flat::cache_key;
use flat::{FlatCatalog, decode_quality};
#[cfg(test)]
use search::{MatchCategory, SEARCH_CANCELLATION_CHECKPOINT_BYTES};
use search::{
    SearchMatcher, never_cancelled, ranked_channel_selection, ranked_programme_selection,
};

const CURSOR_QUERY_DOMAIN: &[u8] = b"sparrow-page-query-v1\0";
const GROUPS_QUERY_TAG: u8 = 0;
const ALL_CHANNELS_QUERY_TAG: u8 = 1;
const GROUP_CHANNELS_QUERY_TAG: u8 = 2;
const SCHEDULE_QUERY_TAG: u8 = 3;
const CHANNEL_SEARCH_QUERY_TAG: u8 = 4;
const PROGRAMME_SEARCH_QUERY_TAG: u8 = 5;
const GUIDE_WINDOW_QUERY_TAG: u8 = 6;
const MAX_PAGE_ITEMS: usize = PageLimit::MAX as usize;
const CHANNEL_ID_HEX_LEN: usize = CHANNEL_ID_PREFIX.len() + 64;

pub(crate) struct ChannelCatalog {
    flat: FlatCatalog,
}

impl ChannelCatalog {
    pub(crate) fn from_parsed(
        configuration: &SourceConfiguration,
        parsed: &[ParsedChannel],
        guide: Option<&ParsedGuide>,
        generation: CatalogGeneration,
    ) -> Self {
        Self {
            flat: FlatCatalog::from_parsed(configuration, parsed, guide, generation),
        }
    }

    pub(crate) fn from_parsed_with_existing_guide(
        configuration: &SourceConfiguration,
        parsed: &[ParsedChannel],
        existing: &ChannelCatalog,
        generation: CatalogGeneration,
    ) -> Self {
        Self {
            flat: FlatCatalog::from_parsed_with_existing_guide(
                configuration,
                parsed,
                &existing.flat,
                generation,
            ),
        }
    }

    pub(crate) fn with_updated_guide(
        &self,
        guide: &ParsedGuide,
        generation: CatalogGeneration,
    ) -> Self {
        Self {
            flat: self.flat.with_updated_guide(guide, generation),
        }
    }

    pub(crate) fn open_from_cache(
        store: &dyn SnapshotStore,
        key: &[u8; 32],
        candidates: &[&SnapshotCandidate],
        generation: CatalogGeneration,
    ) -> Option<Self> {
        FlatCatalog::open_with_store(store, key, candidates, generation).map(|flat| Self { flat })
    }

    pub(crate) fn promote_to_mapping(&self, mapping: CatalogCacheMapping) -> Option<Self> {
        self.flat
            .promote_to_mapping(mapping)
            .map(|flat| Self { flat })
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.flat.as_bytes()
    }

    pub(crate) const fn is_mapped(&self) -> bool {
        self.flat.is_mapped()
    }

    pub(crate) const fn has_epg(&self) -> bool {
        self.flat.has_epg()
    }

    pub(crate) const fn generation(&self) -> CatalogGeneration {
        self.flat.generation()
    }

    pub(crate) fn groups_page(
        &self,
        request: &PageRequest,
    ) -> Result<Page<ChannelGroupView>, CoreError> {
        let group_count = self.flat.groups().len();
        Page::from_range_window(
            self.generation(),
            0..group_count,
            request,
            query_hash(GROUPS_QUERY_TAG, None),
            |range| self.project_groups_window(range),
        )
    }

    pub(crate) fn channels_page(
        &self,
        query: &ChannelQuery,
    ) -> Result<Page<ChannelSummary>, CoreError> {
        let query_hash = match query.group() {
            None => query_hash(ALL_CHANNELS_QUERY_TAG, None),
            Some(group) => query_hash(GROUP_CHANNELS_QUERY_TAG, Some(group)),
        };
        Page::from_range_window(
            self.generation(),
            self.channel_range(query.group()),
            query.page(),
            query_hash,
            |range| self.project_channels_range(range),
        )
    }

    pub(crate) fn channel(&self, id: &ChannelId) -> Result<ChannelDetails, CoreError> {
        let index = self
            .flat
            .channel_index_by_digest(&id.digest())
            .ok_or_else(|| CoreError::ChannelNotFound { id: id.clone() })?;
        Ok(self.project_channel_details(index))
    }

    pub(crate) fn resolve_playback(
        &self,
        id: &ChannelId,
    ) -> Result<ResolvedPlaybackSource, CoreError> {
        self.flat
            .channel_index_by_digest(&id.digest())
            .and_then(|index| {
                let url = Url::parse(self.flat.channel_playback(index)).ok()?;
                Some(ResolvedPlaybackSource::new(SecretPlaybackLocation::new(
                    Arc::new(url),
                )))
            })
            .ok_or_else(|| CoreError::ChannelNotFound { id: id.clone() })
    }

    pub(crate) fn schedule(
        &self,
        query: &ScheduleQuery,
    ) -> Result<Page<ProgrammeSummary>, CoreError> {
        let Some(channel_index) = self
            .flat
            .channel_index_by_digest(&query.channel_id().digest())
        else {
            return Err(CoreError::ChannelNotFound {
                id: query.channel_id().clone(),
            });
        };
        let collection = self.flat.channel_schedule_range(channel_index);
        let Some(from) = query.from() else {
            return Page::from_range_window(
                self.generation(),
                collection,
                query.page(),
                schedule_query_hash(query),
                |range| self.project_programme_summaries_range(range),
            );
        };
        Page::from_bounded_selection_window(
            self.generation(),
            self.flat.schedule_entries().len(),
            query.page(),
            schedule_query_hash(query),
            |prefix_len| Ok(self.programmes_ending_after(&collection, from, prefix_len)),
            |indices| self.project_programme_summaries_indices(indices),
        )
    }

    /// Selects the Programmes of one schedule that end after `from`, in start
    /// order. A long Programme can outlast shorter ones that start after it,
    /// so the selection is not a contiguous range and every candidate is
    /// checked.
    fn programmes_ending_after(
        &self,
        schedule: &Range<usize>,
        from: DateTime<Utc>,
        prefix_len: usize,
    ) -> SelectionPrefix {
        let first_possible = self.flat.first_possible_overlap(schedule, from);
        let entries = self.flat.schedule_entries();
        let mut indices = Vec::with_capacity(prefix_len.min(schedule.end - first_possible));
        let mut total_len = 0;
        for (offset, entry) in entries[first_possible..schedule.end].iter().enumerate() {
            let index = first_possible + offset;
            if self
                .flat
                .matched_programme_ends_at(entry.matched_prog_index.get() as usize)
                > from
            {
                total_len += 1;
                if indices.len() < prefix_len {
                    indices.push(index);
                }
            }
        }
        SelectionPrefix::new(indices, total_len)
    }

    pub(crate) fn guide_window(
        &self,
        query: &GuideWindowQuery,
    ) -> Result<Page<GuideWindowChannel>, CoreError> {
        let channels = query.channels();
        let Some(around) = query.around_channel() else {
            return Page::from_range_window(
                self.generation(),
                self.channel_range(channels.group()),
                channels.page(),
                guide_window_query_hash(query),
                |range| self.project_guide_window_range(range, query),
            );
        };
        let position = self
            .flat
            .channel_index_by_digest(&around.digest())
            .ok_or_else(|| CoreError::ChannelNotFound { id: around.clone() })?;
        let limit = channels.page().limit();
        Ok(Page::from_position_window(
            self.generation(),
            self.flat.channels_hot().len(),
            position.saturating_sub(usize::from(limit.get()) / 2),
            limit,
            guide_window_query_hash(query),
            |range| self.project_guide_window_range(range, query),
        ))
    }

    pub(crate) fn search(&self, request: &SearchRequest) -> Result<SearchResults, CoreError> {
        self.search_with_cancellation(request, &never_cancelled)
    }

    pub(crate) fn search_with_cancellation(
        &self,
        request: &SearchRequest,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<SearchResults, CoreError> {
        let mut matcher = SearchMatcher::new(request.term());
        let channel_page =
            self.search_channels_with_matcher(request.channels(), &mut matcher, is_cancelled)?;
        let programme_page = Page::from_bounded_selection_window(
            self.generation(),
            self.flat.schedule_entries().len(),
            request.programmes(),
            query_hash(PROGRAMME_SEARCH_QUERY_TAG, Some(matcher.term)),
            |prefix_len| {
                ranked_programme_selection(&self.flat, &mut matcher, prefix_len, is_cancelled)
            },
            |indices| self.project_programme_search_hits(indices),
        )?;

        Ok(SearchResults::new(channel_page, programme_page))
    }

    pub(crate) fn search_channels(
        &self,
        term: &SearchTerm,
        page: &PageRequest,
    ) -> Result<Page<ChannelSummary>, CoreError> {
        self.search_channels_with_cancellation(term, page, &never_cancelled)
    }

    pub(crate) fn search_channels_with_cancellation(
        &self,
        term: &SearchTerm,
        page: &PageRequest,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Page<ChannelSummary>, CoreError> {
        let mut matcher = SearchMatcher::new(term);
        self.search_channels_with_matcher(page, &mut matcher, is_cancelled)
    }

    fn search_channels_with_matcher(
        &self,
        page: &PageRequest,
        matcher: &mut SearchMatcher<'_>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Page<ChannelSummary>, CoreError> {
        Page::from_bounded_selection_window(
            self.generation(),
            self.flat.channels_hot().len(),
            page,
            query_hash(CHANNEL_SEARCH_QUERY_TAG, Some(matcher.term)),
            |prefix_len| ranked_channel_selection(&self.flat, matcher, prefix_len, is_cancelled),
            |indices| self.project_channels_indices(indices),
        )
    }

    pub(crate) fn search_programmes(
        &self,
        term: &SearchTerm,
        page: &PageRequest,
    ) -> Result<Page<ProgrammeSummary>, CoreError> {
        self.search_programmes_with_cancellation(term, page, &never_cancelled)
    }

    pub(crate) fn search_programmes_with_cancellation(
        &self,
        term: &SearchTerm,
        page: &PageRequest,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Page<ProgrammeSummary>, CoreError> {
        let mut matcher = SearchMatcher::new(term);
        Page::from_bounded_selection_window(
            self.generation(),
            self.flat.schedule_entries().len(),
            page,
            query_hash(PROGRAMME_SEARCH_QUERY_TAG, Some(matcher.term)),
            |prefix_len| {
                ranked_programme_selection(&self.flat, &mut matcher, prefix_len, is_cancelled)
            },
            |indices| self.project_programme_summaries_indices(indices),
        )
    }

    fn channel_range(&self, group: Option<&str>) -> Range<usize> {
        match group {
            None => 0..self.flat.channels_hot().len(),
            Some(group) => self.flat.group_channel_range(group),
        }
    }

    fn project_groups_window(&self, range: Range<usize>) -> Arc<[ChannelGroupView]> {
        if range.is_empty() {
            return Arc::from([]);
        }
        let groups = self.flat.groups();
        let total_bytes: usize = range
            .clone()
            .map(|idx| self.flat.group_name(idx).len())
            .sum();
        let mut arena = PageTextArena::with_capacity(total_bytes);
        let mut slots = [TextSlot::empty(); MAX_PAGE_ITEMS];
        let count = range.len();
        debug_assert!(count <= MAX_PAGE_ITEMS);
        for (slot, idx) in slots[..count].iter_mut().zip(range.clone()) {
            *slot = arena.push_str(self.flat.group_name(idx));
        }
        let frozen = arena.finish();
        range
            .enumerate()
            .map(|(offset, idx)| {
                let g = &groups[idx];
                ChannelGroupView::new(
                    frozen.resolve(slots[offset]),
                    g.channel_end.get() - g.channel_start.get(),
                )
            })
            .collect()
    }

    fn project_channels_range(&self, range: Range<usize>) -> Arc<[ChannelSummary]> {
        if range.is_empty() {
            return Arc::from([]);
        }
        let mut indices = [0_usize; MAX_PAGE_ITEMS];
        let count = range.len();
        debug_assert!(count <= MAX_PAGE_ITEMS);
        for (dst, idx) in indices[..count].iter_mut().zip(range) {
            *dst = idx;
        }
        self.project_channels_indices(&indices[..count])
    }

    fn project_channels_indices(&self, indices: &[usize]) -> Arc<[ChannelSummary]> {
        if indices.is_empty() {
            return Arc::from([]);
        }
        debug_assert!(indices.len() <= MAX_PAGE_ITEMS);
        let icon_bytes: usize = indices
            .iter()
            .map(|&idx| self.flat.channel_icon_len(idx))
            .sum();
        let mut arena = PageTextArena::with_capacity(indices.len().saturating_mul(96) + icon_bytes);
        let mut group_slots = [(0_u32, empty_slot()); MAX_PAGE_ITEMS];
        let mut group_slot_count = 0_usize;
        let mut pending = [ChannelSlots::empty(); MAX_PAGE_ITEMS];

        for (out, &ch_idx) in pending[..indices.len()].iter_mut().zip(indices) {
            *out = self.intern_channel_slots(
                ch_idx,
                &mut arena,
                &mut group_slots,
                &mut group_slot_count,
            );
        }

        let frozen = arena.finish();
        pending[..indices.len()]
            .iter()
            .map(|item| resolve_channel_summary(item, &frozen))
            .collect()
    }

    fn intern_channel_slots(
        &self,
        ch_idx: usize,
        arena: &mut PageTextArena,
        group_slots: &mut [(u32, TextSlot); MAX_PAGE_ITEMS],
        group_slot_count: &mut usize,
    ) -> ChannelSlots {
        let ch = &self.flat.channels_hot()[ch_idx];
        let name = self.flat.channel_name(ch_idx);
        let id = arena.push_channel_id_digest(&ch.id_digest);
        let name_slot = arena.push_str(name);
        let group_slot = intern_stack_slot(
            group_slots,
            group_slot_count,
            ch.group_index.get(),
            |g_idx| arena.push_str(self.flat.group_name(g_idx as usize)),
        );
        let quality = decode_quality(ch.quality).expect("validated channel quality");
        let variant = quality.and_then(|q| {
            let (_, base_name) = variants::split(name)?;
            Some((q, arena.push_str(&base_name)))
        });
        let icon = self
            .flat
            .channel_icon(ch_idx)
            .map(|icon| arena.push_str(icon));
        ChannelSlots {
            id,
            name: name_slot,
            group: group_slot,
            number: ch.number.get(),
            variant,
            icon,
        }
    }

    fn project_channel_details(&self, ch_idx: usize) -> ChannelDetails {
        let ch = &self.flat.channels_hot()[ch_idx];
        let name = self.flat.channel_name(ch_idx);
        let group = self.flat.group_name(ch.group_index.get() as usize);
        let quality = decode_quality(ch.quality).expect("validated channel quality");
        let base_name = quality.and_then(|q| variants::split(name).map(|(_, base)| (q, base)));
        let icon = self.flat.channel_icon(ch_idx);
        let total_bytes = CHANNEL_ID_HEX_LEN
            + name.len()
            + group.len()
            + base_name.as_ref().map_or(0, |(_, b)| b.len())
            + icon.map_or(0, str::len);
        let mut arena = PageTextArena::with_capacity(total_bytes);
        let id_slot = arena.push_channel_id_digest(&ch.id_digest);
        let name_slot = arena.push_str(name);
        let group_slot = arena.push_str(group);
        let variant_slot = base_name.map(|(q, base)| (q, arena.push_str(&base)));
        let icon_slot = icon.map(|icon| arena.push_str(icon));
        let frozen = arena.finish();
        ChannelDetails::new(
            ChannelId::from_shared(frozen.resolve(id_slot)),
            frozen.resolve(name_slot),
            frozen.resolve(group_slot),
            ch.number.get(),
            variant_slot.map(|(q, slot)| ChannelVariant::new(q, frozen.resolve(slot))),
            icon_slot.map(|slot| ChannelIcon::new(frozen.resolve(slot))),
        )
    }

    fn project_programme_summaries_range(&self, range: Range<usize>) -> Arc<[ProgrammeSummary]> {
        if range.is_empty() {
            return Arc::from([]);
        }
        let mut indices = [0_usize; MAX_PAGE_ITEMS];
        let count = range.len();
        debug_assert!(count <= MAX_PAGE_ITEMS);
        for (dst, idx) in indices[..count].iter_mut().zip(range) {
            *dst = idx;
        }
        self.project_programme_summaries_indices(&indices[..count])
    }

    fn project_programme_summaries_indices(&self, indices: &[usize]) -> Arc<[ProgrammeSummary]> {
        if indices.is_empty() {
            return Arc::from([]);
        }
        debug_assert!(indices.len() <= MAX_PAGE_ITEMS);
        let entries = self.flat.schedule_entries();
        let channels_hot = self.flat.channels_hot();

        // Deduplicate channels AND matched programmes so shared descriptions
        // across multiple channels on a page are copied into the arena once.
        let mut arena =
            PageTextArena::with_capacity(CHANNEL_ID_HEX_LEN + indices.len().saturating_mul(96));
        let mut channel_id_slots = [(0_u32, empty_slot()); MAX_PAGE_ITEMS];
        let mut channel_id_count = 0_usize;
        let mut prog_slots = [(0_u32, (empty_slot(), None)); MAX_PAGE_ITEMS];
        let mut prog_slot_count = 0_usize;

        #[derive(Clone, Copy)]
        struct PendingProg {
            ch_id: TextSlot,
            title: TextSlot,
            description: Option<TextSlot>,
            starts_at: DateTime<Utc>,
            ends_at: DateTime<Utc>,
        }
        let mut pending = [PendingProg {
            ch_id: empty_slot(),
            title: empty_slot(),
            description: None,
            starts_at: DateTime::<Utc>::UNIX_EPOCH,
            ends_at: DateTime::<Utc>::UNIX_EPOCH,
        }; MAX_PAGE_ITEMS];

        for (out, &entry_idx) in pending[..indices.len()].iter_mut().zip(indices) {
            let entry = &entries[entry_idx];
            let ch_idx = entry.channel_index.get();
            let prog_idx = entry.matched_prog_index.get();
            let ch_id =
                intern_stack_slot(&mut channel_id_slots, &mut channel_id_count, ch_idx, |c| {
                    arena.push_channel_id_digest(&channels_hot[c as usize].id_digest)
                });
            let (title, description) =
                intern_stack_slot(&mut prog_slots, &mut prog_slot_count, prog_idx, |p| {
                    let title = arena.push_str(self.flat.matched_programme_title(p as usize));
                    let desc = self
                        .flat
                        .matched_programme_description(p as usize)
                        .map(|d| arena.push_str(d));
                    (title, desc)
                });
            *out = PendingProg {
                ch_id,
                title,
                description,
                starts_at: self.flat.matched_programme_starts_at(prog_idx as usize),
                ends_at: self.flat.matched_programme_ends_at(prog_idx as usize),
            };
        }

        let frozen = arena.finish();
        pending[..indices.len()]
            .iter()
            .map(|item| {
                ProgrammeSummary::new(
                    ChannelId::from_shared(frozen.resolve(item.ch_id)),
                    frozen.resolve(item.title),
                    item.description.map(|slot| frozen.resolve(slot)),
                    item.starts_at,
                    item.ends_at,
                )
            })
            .collect()
    }

    fn project_programme_search_hits(&self, indices: &[usize]) -> Arc<[ProgrammeSearchHit]> {
        if indices.is_empty() {
            return Arc::from([]);
        }
        debug_assert!(indices.len() <= MAX_PAGE_ITEMS);
        let entries = self.flat.schedule_entries();

        // Deduplicate groups, channels, and bounded programme titles into one page arena.
        let mut arena =
            PageTextArena::with_capacity(CHANNEL_ID_HEX_LEN + indices.len().saturating_mul(56));
        let mut group_slots = [(0_u32, empty_slot()); MAX_PAGE_ITEMS];
        let mut group_slot_count = 0_usize;
        let mut channel_slots = [(0_u32, ChannelSlots::empty()); MAX_PAGE_ITEMS];
        let mut channel_slot_count = 0_usize;
        let mut prog_title_slots = [(0_u32, (empty_slot(), false)); MAX_PAGE_ITEMS];
        let mut prog_title_count = 0_usize;

        #[derive(Clone, Copy)]
        struct PendingHit {
            ch: ChannelSlots,
            title: TextSlot,
            title_truncated: bool,
            starts_at: DateTime<Utc>,
            ends_at: DateTime<Utc>,
        }
        let mut pending = [PendingHit {
            ch: ChannelSlots::empty(),
            title: empty_slot(),
            title_truncated: false,
            starts_at: DateTime::<Utc>::UNIX_EPOCH,
            ends_at: DateTime::<Utc>::UNIX_EPOCH,
        }; MAX_PAGE_ITEMS];

        for (out, &entry_idx) in pending[..indices.len()].iter_mut().zip(indices) {
            let entry = &entries[entry_idx];
            let ch_idx = entry.channel_index.get();
            let prog_idx = entry.matched_prog_index.get();
            let ch = intern_stack_slot(&mut channel_slots, &mut channel_slot_count, ch_idx, |c| {
                self.intern_channel_slots(
                    c as usize,
                    &mut arena,
                    &mut group_slots,
                    &mut group_slot_count,
                )
            });
            let (title, title_truncated) = intern_stack_slot(
                &mut prog_title_slots,
                &mut prog_title_count,
                prog_idx,
                |p| {
                    let (bounded, truncated) = self.flat.matched_programme_title_bounded(
                        p as usize,
                        ProgrammeSearchHit::MAX_TITLE_BYTES,
                    );
                    (arena.push_str(bounded), truncated)
                },
            );
            *out = PendingHit {
                ch,
                title,
                title_truncated,
                starts_at: self.flat.matched_programme_starts_at(prog_idx as usize),
                ends_at: self.flat.matched_programme_ends_at(prog_idx as usize),
            };
        }

        let frozen = arena.finish();
        pending[..indices.len()]
            .iter()
            .map(|item| {
                let channel = resolve_channel_summary(&item.ch, &frozen);
                ProgrammeSearchHit::from_bounded(
                    channel,
                    frozen.resolve(item.title),
                    item.title_truncated,
                    item.starts_at,
                    item.ends_at,
                )
            })
            .collect()
    }

    fn project_guide_window_range(
        &self,
        range: Range<usize>,
        query: &GuideWindowQuery,
    ) -> Arc<[GuideWindowChannel]> {
        if range.is_empty() {
            return Arc::from([]);
        }
        let starts_at = query.starts_at();
        let ends_at = query.ends_at();
        let entries = self.flat.schedule_entries();
        let channel_summaries = self.project_channels_range(range.clone());

        // Collect overlapping matched programme indices per channel in one pass,
        // deduplicating bounded titles into a single page-owned text arena.
        let mut per_channel_progs: Vec<Vec<u32>> = Vec::with_capacity(range.len());
        let mut title_map: std::collections::HashMap<u32, (TextSlot, bool)> =
            std::collections::HashMap::new();
        let mut arena = PageTextArena::with_capacity(range.len().saturating_mul(128));

        for ch_idx in range {
            let schedule = self.flat.channel_schedule_range(ch_idx);
            let first_possible = self.flat.first_possible_overlap(&schedule, starts_at);
            let mut matched_indices = Vec::new();
            for entry in &entries[first_possible..schedule.end] {
                let prog_idx = entry.matched_prog_index.get();
                if self.flat.matched_programme_starts_at(prog_idx as usize) >= ends_at {
                    break;
                }
                if self.flat.matched_programme_ends_at(prog_idx as usize) > starts_at {
                    title_map.entry(prog_idx).or_insert_with(|| {
                        let (bounded, truncated) = self.flat.matched_programme_title_bounded(
                            prog_idx as usize,
                            GuideProgramme::MAX_TITLE_BYTES,
                        );
                        (arena.push_str(bounded), truncated)
                    });
                    matched_indices.push(prog_idx);
                    if matched_indices.len() == GuideWindowChannel::MAX_PROGRAMMES + 1 {
                        break;
                    }
                }
            }
            per_channel_progs.push(matched_indices);
        }

        let frozen = arena.finish();
        channel_summaries
            .iter()
            .cloned()
            .zip(per_channel_progs)
            .map(|(summary, prog_indices)| {
                let programmes = prog_indices
                    .into_iter()
                    .map(|prog_idx| {
                        let (slot, truncated) = title_map[&prog_idx];
                        GuideProgramme::from_bounded(
                            frozen.resolve(slot),
                            truncated,
                            self.flat.matched_programme_starts_at(prog_idx as usize),
                            self.flat.matched_programme_ends_at(prog_idx as usize),
                        )
                    })
                    .collect();
                GuideWindowChannel::new(summary, programmes)
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn channel_ids_in_order(&self) -> Vec<String> {
        self.flat
            .channels_hot()
            .iter()
            .map(|ch| {
                let hex = blake3::Hash::from_bytes(ch.id_digest).to_hex();
                format!("{CHANNEL_ID_PREFIX}{}", hex.as_str())
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn channel_names_and_ids(&self) -> Vec<(String, String)> {
        self.flat
            .channels_hot()
            .iter()
            .enumerate()
            .map(|(idx, ch)| {
                let hex = blake3::Hash::from_bytes(ch.id_digest).to_hex();
                (
                    self.flat.channel_name(idx).to_owned(),
                    format!("{CHANNEL_ID_PREFIX}{}", hex.as_str()),
                )
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn from_cache_mapping(
        mapping: CatalogCacheMapping,
        generation: CatalogGeneration,
    ) -> Option<Self> {
        FlatCatalog::from_mapping(mapping, generation).map(|flat| Self { flat })
    }
}

#[derive(Clone, Copy)]
struct ChannelSlots {
    id: TextSlot,
    name: TextSlot,
    group: TextSlot,
    number: u32,
    variant: Option<(ChannelQuality, TextSlot)>,
    icon: Option<TextSlot>,
}

impl ChannelSlots {
    const fn empty() -> Self {
        Self {
            id: empty_slot(),
            name: empty_slot(),
            group: empty_slot(),
            number: 0,
            variant: None,
            icon: None,
        }
    }
}

fn resolve_channel_summary(
    item: &ChannelSlots,
    frozen: &crate::domain::FrozenPageTextArena,
) -> ChannelSummary {
    ChannelSummary::new(
        ChannelId::from_shared(frozen.resolve(item.id)),
        frozen.resolve(item.name),
        frozen.resolve(item.group),
        item.number,
        item.variant
            .map(|(quality, base)| ChannelVariant::new(quality, frozen.resolve(base))),
        item.icon.map(|icon| ChannelIcon::new(frozen.resolve(icon))),
    )
}

const fn empty_slot() -> TextSlot {
    TextSlot::empty()
}

fn intern_stack_slot<V: Copy>(
    table: &mut [(u32, V); MAX_PAGE_ITEMS],
    len: &mut usize,
    key: u32,
    compute: impl FnOnce(u32) -> V,
) -> V {
    for &(existing_key, value) in &table[..*len] {
        if existing_key == key {
            return value;
        }
    }
    let value = compute(key);
    table[*len] = (key, value);
    *len += 1;
    value
}

fn query_hash(tag: u8, discriminator: Option<&str>) -> CursorQueryHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CURSOR_QUERY_DOMAIN);
    hasher.update(&[tag]);
    if let Some(discriminator) = discriminator {
        hasher.update(&(discriminator.len() as u64).to_le_bytes());
        hasher.update(discriminator.as_bytes());
    }
    CursorQueryHash::new(*hasher.finalize().as_bytes())
}

/// Scopes a schedule cursor to its Channel and to the instant the schedule is
/// read from, so a continuation cannot be replayed against another selection.
fn schedule_query_hash(query: &ScheduleQuery) -> CursorQueryHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CURSOR_QUERY_DOMAIN);
    hasher.update(&[SCHEDULE_QUERY_TAG]);
    let channel_id = query.channel_id().as_str();
    hasher.update(&(channel_id.len() as u64).to_le_bytes());
    hasher.update(channel_id.as_bytes());
    match query.from() {
        Some(from) => {
            hasher.update(&[1]);
            hasher.update(&from.timestamp().to_le_bytes());
            hasher.update(&from.timestamp_subsec_nanos().to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
    CursorQueryHash::new(*hasher.finalize().as_bytes())
}

/// A page placed around a Channel shares this hash with the ordinary
/// all-Channels window, which is what makes its continuation an ordinary cursor.
fn guide_window_query_hash(query: &GuideWindowQuery) -> CursorQueryHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CURSOR_QUERY_DOMAIN);
    hasher.update(&[GUIDE_WINDOW_QUERY_TAG]);
    match query.channels().group() {
        Some(group) => {
            hasher.update(&[1]);
            hasher.update(&(group.len() as u64).to_le_bytes());
            hasher.update(group.as_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
    for instant in [query.starts_at(), query.ends_at()] {
        hasher.update(&instant.timestamp().to_le_bytes());
        hasher.update(&instant.timestamp_subsec_nanos().to_le_bytes());
    }
    CursorQueryHash::new(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests;
