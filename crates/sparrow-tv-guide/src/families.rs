use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use sparrow_core::{ChannelQuality, GuideProgramme, GuideWindowChannel};
use unicode_normalization::UnicodeNormalization;

/// One Channel row and its adjacent Quality Variants in catalog order.
#[derive(Clone)]
pub struct GuideFamily {
    variants: Arc<Vec<GuideWindowChannel>>,
}

impl GuideFamily {
    pub fn number(&self) -> u32 {
        self.variants[0].channel().number()
    }

    pub fn title(&self) -> &str {
        let channel = self.variants[0].channel();
        channel
            .variant()
            .map_or_else(|| channel.name(), |variant| variant.base_name())
    }

    pub fn group(&self) -> &str {
        self.variants[0].channel().group()
    }

    pub fn preference_title(&self) -> String {
        normalize_title(self.title())
    }

    pub fn variants(&self) -> &[GuideWindowChannel] {
        &self.variants
    }

    /// Selects the viewer's available Quality Variant or the highest quality.
    pub fn preferred_variant(&self, preferences: &VariantPreferences) -> &GuideWindowChannel {
        let wanted = preferences.quality(self.group(), self.title());
        let mut best = &self.variants[0];
        for variant in self.variants.iter() {
            let quality = variant_quality(variant);
            if quality.is_some() && quality == wanted {
                return variant;
            }
            if quality > variant_quality(best) {
                best = variant;
            }
        }
        best
    }

    pub fn quality_variants(&self) -> impl Iterator<Item = &GuideWindowChannel> {
        self.variants
            .iter()
            .filter(|row| variant_quality(row).is_some())
    }

    /// Selects a variant for display and the row supplying its best available schedule.
    pub fn preferred_with_schedule(
        &self,
        preferences: &VariantPreferences,
    ) -> (&GuideWindowChannel, &GuideWindowChannel) {
        let preferred = self.preferred_variant(preferences);
        let source = if preferred.programmes().is_empty() {
            self.variants
                .iter()
                .find(|row| !row.programmes().is_empty())
                .unwrap_or(preferred)
        } else {
            preferred
        };
        (preferred, source)
    }

    /// Shows the preferred variant's Programmes, falling back in catalog order.
    pub fn programmes(&self, preferences: &VariantPreferences) -> &[GuideProgramme] {
        self.preferred_with_schedule(preferences).1.programmes()
    }
}

/// Picture quality choices by exact Channel Group and normalized family title.
#[derive(Default, Eq, PartialEq)]
pub struct VariantPreferences(HashMap<String, HashMap<String, ChannelQuality>>);

impl VariantPreferences {
    pub fn from_normalized(
        choices: impl IntoIterator<Item = (String, String, ChannelQuality)>,
    ) -> Self {
        let mut preferences = Self::default();
        for (group, title, quality) in choices {
            preferences
                .0
                .entry(group)
                .or_default()
                .insert(title, quality);
        }
        preferences
    }

    pub fn choose(&mut self, group: &str, title: &str, quality: ChannelQuality) {
        self.0
            .entry(group.to_owned())
            .or_default()
            .insert(normalize_title(title), quality);
    }

    pub fn quality(&self, group: &str, title: &str) -> Option<ChannelQuality> {
        self.0.get(group)?.get(&normalize_title(title)).copied()
    }
}

pub(crate) fn append_families(
    families: &mut Vec<GuideFamily>,
    rows: &[GuideWindowChannel],
    excluded_groups: &HashSet<String>,
) {
    for row in rows {
        let channel = row.channel();
        if excluded_groups.contains(channel.group()) {
            continue;
        }
        if let Some(family) = families.last_mut()
            && family.number() == channel.number()
        {
            Arc::make_mut(&mut family.variants).push(row.clone());
        } else {
            families.push(GuideFamily {
                variants: Arc::new(vec![row.clone()]),
            });
        }
    }
}

fn variant_quality(row: &GuideWindowChannel) -> Option<ChannelQuality> {
    row.channel().variant().map(|variant| variant.quality())
}

fn normalize_title(title: &str) -> String {
    let normalized = title.nfkc().collect::<String>().to_lowercase();
    // Match the existing guide's JavaScript whitespace rule for preference keys.
    let is_space = |character: char| {
        (character.is_whitespace() && character != '\u{85}') || character == '\u{feff}'
    };
    let mut key = String::with_capacity(normalized.len());
    for word in normalized.split(is_space).filter(|word| !word.is_empty()) {
        if !key.is_empty() {
            key.push(' ');
        }
        key.push_str(word);
    }
    key
}
