use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

use crate::{identity, xmltv::ParsedGuide};

use super::PendingChannel;

pub(super) fn build_programmes(
    channels: &[PendingChannel],
    guide: Option<&ParsedGuide>,
) -> (Vec<CatalogProgramme>, Vec<(u32, u32)>) {
    let mut ranges = vec![(0_u32, 0_u32); channels.len()];
    let Some(guide) = guide else {
        return (Vec::new(), ranges);
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

    let mut m3u_name_counts = HashMap::<&str, usize>::with_capacity(channels.len());
    for channel in channels {
        *m3u_name_counts
            .entry(channel.name_order.as_str())
            .or_default() += 1;
    }

    let mut matched_channels = HashMap::<Arc<str>, Vec<u32>>::new();
    for (channel_index, channel) in channels.iter().enumerate() {
        let exact_id = channel.tvg_id.trim();
        let guide_id = if exact_id.is_empty() {
            (m3u_name_counts.get(channel.name_order.as_str()) == Some(&1))
                .then(|| guide_names.get(&channel.name_order).and_then(Clone::clone))
                .flatten()
        } else {
            guide_ids.get(exact_id).map(Arc::clone)
        };
        if let Some(guide_id) = guide_id {
            let index = u32::try_from(channel_index)
                .expect("the bounded M3U payload cannot contain more than u32::MAX Channels");
            matched_channels.entry(guide_id).or_default().push(index);
        }
    }

    let mut pending = Vec::new();
    for (source_index, programme) in guide.programmes.iter().enumerate() {
        let Some(channel_indices) = matched_channels.get(&programme.guide_channel_id) else {
            continue;
        };
        let source_index = u32::try_from(source_index)
            .expect("the bounded EPG payload cannot contain more than u32::MAX Programmes");
        for &channel_index in channel_indices {
            pending.push(CatalogProgramme {
                channel_index,
                source_index,
            });
        }
    }
    pending.sort_unstable_by(|left, right| compare_programmes(left, right, channels, guide));

    let programmes = pending;
    let mut start = 0;
    while start < programmes.len() {
        let channel_index = programmes[start].channel_index;
        let mut end = start + 1;
        while end < programmes.len() && programmes[end].channel_index == channel_index {
            end += 1;
        }
        let start_u32 = u32::try_from(start).expect("catalogued Programme range fits in u32");
        let end_u32 = u32::try_from(end).expect("catalogued Programme range fits in u32");
        debug_assert_eq!(ranges[channel_index as usize], (0, 0));
        ranges[channel_index as usize] = (start_u32, end_u32);
        start = end;
    }

    (programmes, ranges)
}

pub(super) struct ScheduleOverlapIndex {
    // Each entry points at the Programme with the latest end in its Channel's
    // schedule prefix. The pointed-to end times are therefore monotonic within
    // each schedule range and can be binary-searched without losing a long
    // Programme that starts before shorter, already-ended entries.
    pub(super) prefix_max_end_sources: Box<[u32]>,
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
        let mut max_end_source = 0_u32;

        for (index, programme) in programmes.iter().enumerate() {
            if index != 0 && programmes[index - 1].channel_index == programme.channel_index {
                if guide.programmes[programme.source_index as usize].ends_at
                    > guide.programmes[max_end_source as usize].ends_at
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
            + self.prefix_max_end_sources[schedule.clone()].partition_point(|&source_index| {
                prefix_ended_by_window_start(source_index as usize)
            })
    }
}

#[derive(Clone, Copy)]
pub(super) struct CatalogProgramme {
    pub(super) channel_index: u32,
    pub(super) source_index: u32,
}

fn compare_programmes(
    left: &CatalogProgramme,
    right: &CatalogProgramme,
    channels: &[PendingChannel],
    guide: &ParsedGuide,
) -> Ordering {
    let left_source = &guide.programmes[left.source_index as usize];
    let right_source = &guide.programmes[right.source_index as usize];
    channels[left.channel_index as usize]
        .id
        .as_str()
        .cmp(channels[right.channel_index as usize].id.as_str())
        .then_with(|| left_source.starts_at.cmp(&right_source.starts_at))
        .then_with(|| left_source.ends_at.cmp(&right_source.ends_at))
        .then_with(|| left_source.title.cmp(&right_source.title))
        .then_with(|| left_source.description.cmp(&right_source.description))
        .then_with(|| left.source_index.cmp(&right.source_index))
}
