use std::{cmp::Ordering, collections::HashMap, fmt, ops::Range, sync::Arc};

use url::Url;

use crate::{
    domain::{
        CatalogGeneration, ChannelDetails, ChannelGroupView, ChannelId, ChannelQuery,
        ChannelSummary, CoreError, CursorQueryHash, Page, PageRequest, SourceConfiguration,
    },
    identity,
    m3u::ParsedChannel,
};

const CURSOR_QUERY_DOMAIN: &[u8] = b"sparrow-page-query-v1\0";
const GROUPS_QUERY_TAG: u8 = 0;
const ALL_CHANNELS_QUERY_TAG: u8 = 1;
const GROUP_CHANNELS_QUERY_TAG: u8 = 2;

pub(crate) struct ChannelCatalog {
    generation: CatalogGeneration,
    groups: Arc<[ChannelGroupView]>,
    summaries: Arc<[ChannelSummary]>,
    records: Vec<ChannelRecord>,
    group_ranges: HashMap<Arc<str>, Range<usize>>,
    by_id: HashMap<ChannelId, usize>,
}

impl ChannelCatalog {
    pub(crate) fn from_parsed(
        configuration: &SourceConfiguration,
        parsed: Vec<ParsedChannel>,
        generation: CatalogGeneration,
    ) -> Self {
        let mut occurrences = HashMap::<[u8; 32], u32>::new();
        let mut pending = Vec::with_capacity(parsed.len());

        for channel in parsed {
            let seed = identity::seed(&channel.tvg_id, &channel.name, &channel.group);
            let occurrence = occurrences.entry(seed).or_default();
            let id = identity::channel_id(&configuration.fingerprint, &seed, *occurrence);
            *occurrence = occurrence.saturating_add(1);

            let name: Arc<str> = Arc::from(channel.name);
            let group: Arc<str> = Arc::from(channel.group);
            pending.push(PendingChannel {
                group_order: identity::normalize_identity_field(&group),
                name_order: identity::normalize_identity_field(&name),
                id,
                name,
                group,
                playback: SecretPlaybackLocation(channel.playback),
            });
        }

        pending.sort_unstable_by(compare_channels);

        let mut summaries = Vec::with_capacity(pending.len());
        let mut records = Vec::with_capacity(pending.len());
        let mut by_id = HashMap::with_capacity(pending.len());
        for channel in pending {
            let summary = ChannelSummary::new(
                channel.id.clone(),
                Arc::clone(&channel.name),
                Arc::clone(&channel.group),
            );
            let details = ChannelDetails::new(channel.id.clone(), channel.name, channel.group);
            let index = records.len();
            debug_assert!(by_id.insert(channel.id, index).is_none());
            summaries.push(summary);
            records.push(ChannelRecord {
                details,
                _playback: channel.playback,
            });
        }

        let mut groups = Vec::new();
        let mut group_ranges = HashMap::new();
        let mut group_start = 0;
        while group_start < summaries.len() {
            let group_name: Arc<str> = Arc::from(summaries[group_start].group());
            let mut group_end = group_start + 1;
            while group_end < summaries.len() && summaries[group_end].group() == group_name.as_ref()
            {
                group_end += 1;
            }
            let channel_count = u32::try_from(group_end - group_start)
                .expect("the bounded M3U payload cannot contain more than u32::MAX Channels");
            groups.push(ChannelGroupView::new(
                Arc::clone(&group_name),
                channel_count,
            ));
            debug_assert!(
                group_ranges
                    .insert(group_name, group_start..group_end)
                    .is_none()
            );
            group_start = group_end;
        }

        Self {
            generation,
            groups: Arc::from(groups),
            summaries: Arc::from(summaries),
            records,
            group_ranges,
            by_id,
        }
    }

    pub(crate) fn groups_page(
        &self,
        request: &PageRequest,
    ) -> Result<Page<ChannelGroupView>, CoreError> {
        Page::from_request(
            self.generation,
            Arc::clone(&self.groups),
            0..self.groups.len(),
            request,
            query_hash(GROUPS_QUERY_TAG, None),
        )
    }

    pub(crate) const fn generation(&self) -> CatalogGeneration {
        self.generation
    }

    pub(crate) fn channels_page(
        &self,
        query: &ChannelQuery,
    ) -> Result<Page<ChannelSummary>, CoreError> {
        let (collection, query_hash) = match query.group() {
            None => (
                0..self.summaries.len(),
                query_hash(ALL_CHANNELS_QUERY_TAG, None),
            ),
            Some(group) => (
                self.group_ranges.get(group).cloned().unwrap_or(0..0),
                query_hash(GROUP_CHANNELS_QUERY_TAG, Some(group)),
            ),
        };
        Page::from_request(
            self.generation,
            Arc::clone(&self.summaries),
            collection,
            query.page(),
            query_hash,
        )
    }

    pub(crate) fn channel(&self, id: &ChannelId) -> Result<ChannelDetails, CoreError> {
        self.by_id
            .get(id)
            .map(|index| self.records[*index].details.clone())
            .ok_or_else(|| CoreError::ChannelNotFound { id: id.clone() })
    }
}

struct PendingChannel {
    group_order: String,
    name_order: String,
    id: ChannelId,
    name: Arc<str>,
    group: Arc<str>,
    playback: SecretPlaybackLocation,
}

fn compare_channels(left: &PendingChannel, right: &PendingChannel) -> Ordering {
    left.group_order
        .cmp(&right.group_order)
        .then_with(|| left.group.cmp(&right.group))
        .then_with(|| left.name_order.cmp(&right.name_order))
        .then_with(|| left.name.cmp(&right.name))
        .then_with(|| left.id.as_str().cmp(right.id.as_str()))
}

fn query_hash(tag: u8, group: Option<&str>) -> CursorQueryHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CURSOR_QUERY_DOMAIN);
    hasher.update(&[tag]);
    if let Some(group) = group {
        hasher.update(&(group.len() as u64).to_le_bytes());
        hasher.update(group.as_bytes());
    }
    CursorQueryHash::new(*hasher.finalize().as_bytes())
}

struct ChannelRecord {
    details: ChannelDetails,
    _playback: SecretPlaybackLocation,
}

struct SecretPlaybackLocation(Url);

impl fmt::Debug for SecretPlaybackLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = &self.0;
        formatter.write_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use proptest::prelude::*;
    use static_assertions::assert_not_impl_any;
    use url::Url;

    use crate::{
        domain::{
            CatalogGeneration, ChannelQuery, CoreError, PageCursor, PageLimit, PageRequest,
            SourceConfiguration, SourceConfigurationInput,
        },
        m3u::ParsedChannel,
    };

    use super::ChannelCatalog;

    assert_not_impl_any!(ChannelCatalog: Clone);

    proptest! {
        #[test]
        fn channel_cursor_round_trips_visit_every_sorted_item_once(
            channel_count in 1_usize..300,
            limit in 1_u16..=100,
        ) {
            let catalog = catalog(
                unique_channels(channel_count, false),
                generation(1),
            );
            let expected = catalog
                .summaries
                .iter()
                .map(|channel| channel.id().as_str().to_owned())
                .collect::<Vec<_>>();
            let mut request = PageRequest::first(PageLimit::new(limit).expect("generated limit is valid"));
            let mut observed = Vec::new();

            loop {
                let page = catalog
                    .channels_page(&ChannelQuery::all(request))
                    .expect("generated page request is valid");
                prop_assert!(page.items().len() <= usize::from(limit));
                observed.extend(
                    page.items()
                        .iter()
                        .map(|channel| channel.id().as_str().to_owned()),
                );
                let Some(next) = page.next() else {
                    break;
                };
                let parsed = PageCursor::parse(next.as_str())
                    .expect("generated cursors round-trip through their transport form");
                request = PageRequest::after(
                    parsed,
                    PageLimit::new(limit).expect("generated limit is valid"),
                );
            }

            prop_assert_eq!(observed, expected);
        }

        #[test]
        fn missing_provider_ids_are_stable_across_source_reordering(
            channel_count in 1_usize..200,
        ) {
            let forward = catalog(
                unique_channels(channel_count, false),
                generation(1),
            );
            let reversed = catalog(
                unique_channels(channel_count, true),
                generation(2),
            );

            prop_assert_eq!(ids_by_name(&forward), ids_by_name(&reversed));
        }

        #[test]
        fn indistinguishable_backup_entries_have_distinct_reorder_stable_id_sets(
            duplicate_count in 2_usize..100,
        ) {
            let forward = catalog(
                duplicate_channels(duplicate_count, false),
                generation(1),
            );
            let reversed = catalog(
                duplicate_channels(duplicate_count, true),
                generation(2),
            );
            let forward_ids = forward
                .summaries
                .iter()
                .map(|channel| channel.id().as_str().to_owned())
                .collect::<HashSet<_>>();
            let reversed_ids = reversed
                .summaries
                .iter()
                .map(|channel| channel.id().as_str().to_owned())
                .collect::<HashSet<_>>();

            prop_assert_eq!(forward_ids.len(), duplicate_count);
            prop_assert_eq!(forward_ids, reversed_ids);
        }
    }

    #[test]
    fn a_cursor_from_an_older_catalog_generation_returns_typed_invalidation() {
        let first = catalog(unique_channels(3, false), generation(1));
        let first_page = first
            .channels_page(&ChannelQuery::all(PageRequest::first(
                PageLimit::new(1).expect("fixture limit is valid"),
            )))
            .expect("first generation is queryable");
        let cursor = first_page.next().expect("more Channels remain").clone();
        let current_generation = generation(2);
        let current = catalog(unique_channels(3, true), current_generation);

        let error = current
            .channels_page(&ChannelQuery::all(PageRequest::after(
                cursor,
                PageLimit::new(1).expect("fixture limit is valid"),
            )))
            .expect_err("an older generation cursor is rejected");
        assert_eq!(
            error,
            CoreError::StaleCursor {
                current: current_generation,
            }
        );
    }

    #[test]
    fn catalog_generation_covers_configuration_m3u_and_optional_epg_content() {
        let first_configuration = configuration();
        let other_configuration = SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://other-provider.fixture.invalid/property.m3u",
            None::<String>,
        ))
        .expect("alternate fixture Source Configuration is valid");
        let baseline = first_configuration.catalog_generation(&[1; 32], None);

        assert_eq!(
            baseline,
            first_configuration.catalog_generation(&[1; 32], None)
        );
        assert_ne!(
            baseline,
            first_configuration.catalog_generation(&[2; 32], None)
        );
        assert_ne!(
            baseline,
            first_configuration.catalog_generation(&[1; 32], Some(&[3; 32]))
        );
        assert_ne!(
            baseline,
            other_configuration.catalog_generation(&[1; 32], None)
        );
        assert_ne!(baseline.get(), 0);
    }

    fn catalog(parsed: Vec<ParsedChannel>, generation: CatalogGeneration) -> ChannelCatalog {
        ChannelCatalog::from_parsed(&configuration(), parsed, generation)
    }

    fn generation(discriminator: u8) -> CatalogGeneration {
        configuration().catalog_generation(&[discriminator; 32], None)
    }

    fn configuration() -> SourceConfiguration {
        SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://provider.fixture.invalid/property.m3u",
            None::<String>,
        ))
        .expect("fixture Source Configuration is valid")
    }

    fn unique_channels(count: usize, reversed: bool) -> Vec<ParsedChannel> {
        let indices: Box<dyn Iterator<Item = usize>> = if reversed {
            Box::new((0..count).rev())
        } else {
            Box::new(0..count)
        };
        indices
            .map(|index| ParsedChannel {
                tvg_id: String::new(),
                name: format!("Channel {index:04}"),
                group: format!("Group {:02}", index % 11),
                playback: Url::parse(&format!(
                    "https://media.fixture.invalid/channel/{index}?token=private-{index}"
                ))
                .expect("generated playback location is valid"),
            })
            .collect()
    }

    fn duplicate_channels(count: usize, reversed: bool) -> Vec<ParsedChannel> {
        let indices: Box<dyn Iterator<Item = usize>> = if reversed {
            Box::new((0..count).rev())
        } else {
            Box::new(0..count)
        };
        indices
            .map(|index| ParsedChannel {
                tvg_id: String::new(),
                name: "Duplicate".to_owned(),
                group: "News".to_owned(),
                playback: Url::parse(&format!(
                    "https://media.fixture.invalid/backup/{index}?token=private-{index}"
                ))
                .expect("generated playback location is valid"),
            })
            .collect()
    }

    fn ids_by_name(catalog: &ChannelCatalog) -> BTreeMap<String, String> {
        catalog
            .summaries
            .iter()
            .map(|channel| (channel.name().to_owned(), channel.id().as_str().to_owned()))
            .collect()
    }
}
