use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

use chrono::{DateTime, Utc};

use crate::identity;

use super::flat::UNMATCHED_PROG_BIT;

pub(super) struct ScheduleBuildChannel<'a> {
    pub(super) tvg_id: &'a str,
    pub(super) name_order: &'a str,
}

pub(super) trait GuideSource {
    fn channel_count(&self) -> usize;
    fn channel_id(&self, index: usize) -> &str;
    fn channel_display_name_count(&self, index: usize) -> usize;
    fn channel_display_name(&self, channel_index: usize, name_index: usize) -> &str;
    fn programme_count(&self) -> usize;
    fn programme_guide_channel_id(&self, index: usize) -> &str;
    fn programme_title(&self, index: usize) -> &str;
    fn programme_description(&self, index: usize) -> Option<&str>;
    fn programme_starts_at(&self, index: usize) -> DateTime<Utc>;
    fn programme_ends_at(&self, index: usize) -> DateTime<Utc>;
}

#[derive(Clone, Copy)]
pub(super) struct BuiltScheduleEntry {
    pub(super) channel_index: u32,
    pub(super) matched_prog_index: u32,
}

pub(super) struct BuiltProgrammes {
    pub(super) channel_schedules: Vec<(u32, u32)>,
    pub(super) schedule_entries: Vec<BuiltScheduleEntry>,
    pub(super) schedule_overlap: Vec<u32>,
    pub(super) matched_source_indices: Vec<u32>,
    pub(super) unmatched_source_indices: Vec<u32>,
    pub(super) guide_prog_order: Vec<u32>,
}

pub(super) fn build_programmes<G: GuideSource>(
    channels: &[ScheduleBuildChannel<'_>],
    channels_by_id: &[u32],
    guide: Option<&G>,
) -> BuiltProgrammes {
    let mut channel_schedules = vec![(0_u32, 0_u32); channels.len()];
    let Some(guide) = guide else {
        return BuiltProgrammes {
            channel_schedules,
            schedule_entries: Vec::new(),
            schedule_overlap: Vec::new(),
            matched_source_indices: Vec::new(),
            unmatched_source_indices: Vec::new(),
            guide_prog_order: Vec::new(),
        };
    };

    let guide_channel_count = guide.channel_count();
    let mut guide_ids: HashSet<&str> = HashSet::with_capacity(guide_channel_count);
    let mut guide_names: HashMap<String, Option<&str>> = HashMap::new();
    for ch_idx in 0..guide_channel_count {
        let id = guide.channel_id(ch_idx);
        guide_ids.insert(id);
        for name_idx in 0..guide.channel_display_name_count(ch_idx) {
            let normalized =
                identity::normalize_identity_field(guide.channel_display_name(ch_idx, name_idx));
            if normalized.is_empty() {
                continue;
            }
            guide_names
                .entry(normalized)
                .and_modify(|candidate| {
                    if *candidate != Some(id) {
                        *candidate = None;
                    }
                })
                .or_insert(Some(id));
        }
    }

    let mut m3u_name_counts = HashMap::<&str, usize>::with_capacity(channels.len());
    for channel in channels {
        *m3u_name_counts.entry(channel.name_order).or_default() += 1;
    }

    let mut matched_channels: HashMap<&str, Vec<u32>> = HashMap::new();
    for (channel_index, channel) in channels.iter().enumerate() {
        let exact_id = channel.tvg_id.trim();
        let guide_id = if exact_id.is_empty() {
            (m3u_name_counts.get(channel.name_order) == Some(&1))
                .then(|| guide_names.get(channel.name_order).copied().flatten())
                .flatten()
        } else {
            guide_ids.get(exact_id).copied()
        };
        if let Some(guide_id) = guide_id {
            let index = u32::try_from(channel_index)
                .expect("the bounded M3U payload cannot contain more than u32::MAX Channels");
            matched_channels.entry(guide_id).or_default().push(index);
        }
    }

    let total_progs = guide.programme_count();
    let mut matched_source_indices = Vec::new();
    let mut unmatched_source_indices = Vec::new();
    let mut guide_prog_order = Vec::with_capacity(total_progs);
    let mut pending = Vec::new();

    for source_index in 0..total_progs {
        let source_u32 = u32::try_from(source_index)
            .expect("the bounded EPG payload cannot contain more than u32::MAX Programmes");
        if let Some(channel_indices) =
            matched_channels.get(guide.programme_guide_channel_id(source_index))
        {
            let matched_idx = u32::try_from(matched_source_indices.len())
                .expect("matched Programme count fits in u32");
            assert!(
                matched_idx < UNMATCHED_PROG_BIT,
                "matched Programme count must fit in 31 bits"
            );
            matched_source_indices.push(source_u32);
            guide_prog_order.push(matched_idx);
            for &channel_index in channel_indices {
                pending.push(BuiltScheduleEntry {
                    channel_index,
                    matched_prog_index: matched_idx,
                });
            }
        } else {
            let unmatched_idx = u32::try_from(unmatched_source_indices.len())
                .expect("unmatched Programme count fits in u32");
            assert!(
                unmatched_idx < UNMATCHED_PROG_BIT,
                "unmatched Programme count must fit in 31 bits"
            );
            unmatched_source_indices.push(source_u32);
            guide_prog_order.push(UNMATCHED_PROG_BIT | unmatched_idx);
        }
    }

    // Build O(1) ChannelId rank lookup so sorting matches lexicographic ChannelId order exactly.
    let mut id_rank = vec![0_u32; channels.len()];
    for (rank, &ch_idx) in channels_by_id.iter().enumerate() {
        id_rank[ch_idx as usize] = rank as u32;
    }

    pending.sort_unstable_by(|left, right| {
        compare_entries(left, right, &id_rank, &matched_source_indices, guide)
    });

    // Assign contiguous (start, end) ranges in ChannelId order (empty schedules get (cursor, cursor)).
    let mut cursor = 0_usize;
    for &ch_idx in channels_by_id {
        let start = cursor;
        while cursor < pending.len() && pending[cursor].channel_index == ch_idx {
            cursor += 1;
        }
        channel_schedules[ch_idx as usize] = (
            u32::try_from(start).expect("catalogued Programme range fits in u32"),
            u32::try_from(cursor).expect("catalogued Programme range fits in u32"),
        );
    }
    debug_assert_eq!(cursor, pending.len());

    // Build prefix-maximum end index per channel schedule.
    let mut schedule_overlap = Vec::with_capacity(pending.len());
    let mut max_end_matched = 0_u32;
    let mut max_end_time = DateTime::<Utc>::MIN_UTC;
    for (index, entry) in pending.iter().enumerate() {
        let entry_end = guide
            .programme_ends_at(matched_source_indices[entry.matched_prog_index as usize] as usize);
        if index != 0 && pending[index - 1].channel_index == entry.channel_index {
            if entry_end > max_end_time {
                max_end_matched = entry.matched_prog_index;
                max_end_time = entry_end;
            }
        } else {
            max_end_matched = entry.matched_prog_index;
            max_end_time = entry_end;
        }
        schedule_overlap.push(max_end_matched);
    }

    BuiltProgrammes {
        channel_schedules,
        schedule_entries: pending,
        schedule_overlap,
        matched_source_indices,
        unmatched_source_indices,
        guide_prog_order,
    }
}

fn compare_entries<G: GuideSource>(
    left: &BuiltScheduleEntry,
    right: &BuiltScheduleEntry,
    id_rank: &[u32],
    matched_source_indices: &[u32],
    guide: &G,
) -> Ordering {
    let left_source = matched_source_indices[left.matched_prog_index as usize] as usize;
    let right_source = matched_source_indices[right.matched_prog_index as usize] as usize;
    id_rank[left.channel_index as usize]
        .cmp(&id_rank[right.channel_index as usize])
        .then_with(|| {
            guide
                .programme_starts_at(left_source)
                .cmp(&guide.programme_starts_at(right_source))
        })
        .then_with(|| {
            guide
                .programme_ends_at(left_source)
                .cmp(&guide.programme_ends_at(right_source))
        })
        .then_with(|| {
            guide
                .programme_title(left_source)
                .cmp(guide.programme_title(right_source))
        })
        .then_with(|| {
            guide
                .programme_description(left_source)
                .cmp(&guide.programme_description(right_source))
        })
        .then_with(|| left_source.cmp(&right_source))
}
