use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

use crate::{domain::ChannelId, identity, xmltv::ParsedGuide};

use super::PendingChannel;

pub(super) fn build_programmes(
    channels: &[PendingChannel],
    guide: Option<&ParsedGuide>,
) -> (Vec<CatalogProgramme>, HashMap<ChannelId, Range<usize>>) {
    let Some(guide) = guide else {
        return (Vec::new(), HashMap::new());
    };
    let mut guide_ids = HashSet::with_capacity(guide.channels.len());
    let mut guide_names = HashMap::<String, Option<Arc<str>>>::new();
    for channel in &guide.channels {
        guide_ids.insert(Arc::clone(&channel.id));
        for display_name in &channel.display_names {
            let normalized = identity::normalize_identity_field(display_name);
            if normalized.is_empty() {
                continue;
            }
            guide_names
                .entry(normalized)
                .and_modify(|candidate| {
                    if candidate.as_deref() != Some(channel.id.as_ref()) {
                        *candidate = None;
                    }
                })
                .or_insert_with(|| Some(Arc::clone(&channel.id)));
        }
    }

    let mut m3u_name_counts = HashMap::<String, usize>::new();
    for channel in channels {
        *m3u_name_counts
            .entry(channel.name_order.clone())
            .or_default() += 1;
    }

    let mut matched_channels = HashMap::<Arc<str>, Vec<ChannelId>>::new();
    for channel in channels {
        let exact_id = channel.tvg_id.trim();
        let guide_id = if exact_id.is_empty() {
            (m3u_name_counts.get(&channel.name_order) == Some(&1))
                .then(|| guide_names.get(&channel.name_order).and_then(Clone::clone))
                .flatten()
        } else {
            guide_ids.contains(exact_id).then(|| Arc::from(exact_id))
        };
        if let Some(guide_id) = guide_id {
            matched_channels
                .entry(guide_id)
                .or_default()
                .push(channel.id.clone());
        }
    }

    let mut pending = Vec::new();
    for (source_index, programme) in guide.programmes.iter().enumerate() {
        let Some(channel_ids) = matched_channels.get(&programme.guide_channel_id) else {
            continue;
        };
        for channel_id in channel_ids {
            pending.push(CatalogProgramme {
                channel_id: channel_id.clone(),
                source_index,
            });
        }
    }
    pending.sort_unstable_by(|left, right| compare_programmes(left, right, guide));

    let programmes = pending;
    let mut ranges = HashMap::new();
    let mut start = 0;
    while start < programmes.len() {
        let channel_id = programmes[start].channel_id.clone();
        let mut end = start + 1;
        while end < programmes.len() && programmes[end].channel_id == channel_id {
            end += 1;
        }
        let previous = ranges.insert(channel_id, start..end);
        debug_assert!(previous.is_none());
        start = end;
    }

    (programmes, ranges)
}

pub(super) struct ScheduleOverlapIndex {
    // Each entry points at the Programme with the latest end in its Channel's
    // schedule prefix. The pointed-to end times are therefore monotonic within
    // each schedule range and can be binary-searched without losing a long
    // Programme that starts before shorter, already-ended entries.
    pub(super) prefix_max_end_sources: Box<[usize]>,
}

impl ScheduleOverlapIndex {
    pub(super) fn build(programmes: &[CatalogProgramme], guide: Option<&ParsedGuide>) -> Self {
        let Some(guide) = guide else {
            debug_assert!(programmes.is_empty());
            return Self {
                prefix_max_end_sources: Box::new([]),
            };
        };
        let mut prefix_max_end_sources = Vec::with_capacity(programmes.len());
        let mut max_end_source = 0;

        for (index, programme) in programmes.iter().enumerate() {
            if index != 0 && programmes[index - 1].channel_id == programme.channel_id {
                if guide.programmes[programme.source_index].ends_at
                    > guide.programmes[max_end_source].ends_at
                {
                    max_end_source = programme.source_index;
                }
            } else {
                max_end_source = programme.source_index;
            }
            prefix_max_end_sources.push(max_end_source);
        }

        Self {
            prefix_max_end_sources: prefix_max_end_sources.into_boxed_slice(),
        }
    }

    pub(super) fn first_possible_overlap(
        &self,
        schedule: &Range<usize>,
        mut prefix_ended_by_window_start: impl FnMut(usize) -> bool,
    ) -> usize {
        debug_assert!(schedule.end <= self.prefix_max_end_sources.len());
        schedule.start
            + self.prefix_max_end_sources[schedule.clone()]
                .partition_point(|source_index| prefix_ended_by_window_start(*source_index))
    }
}

pub(super) struct CatalogProgramme {
    pub(super) channel_id: ChannelId,
    pub(super) source_index: usize,
}

fn compare_programmes(
    left: &CatalogProgramme,
    right: &CatalogProgramme,
    guide: &ParsedGuide,
) -> Ordering {
    let left_source = &guide.programmes[left.source_index];
    let right_source = &guide.programmes[right.source_index];
    left.channel_id
        .as_str()
        .cmp(right.channel_id.as_str())
        .then_with(|| left_source.starts_at.cmp(&right_source.starts_at))
        .then_with(|| left_source.ends_at.cmp(&right_source.ends_at))
        .then_with(|| left_source.title.cmp(&right_source.title))
        .then_with(|| left_source.description.cmp(&right_source.description))
        .then_with(|| left.source_index.cmp(&right.source_index))
}
