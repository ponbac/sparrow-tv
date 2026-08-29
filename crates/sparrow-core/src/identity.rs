use blake3::Hasher;
use unicode_normalization::UnicodeNormalization;

use crate::domain::{CHANNEL_ID_PREFIX, ChannelId, SourceConfigurationFingerprint};

const SEED_DOMAIN: &[u8] = b"sparrow-channel-identity-seed-v1\0";
const CHANNEL_ID_DOMAIN: &[u8] = b"sparrow-channel-identifier-v1\0";

/// Builds the playback-independent identity shared by recognizable entries.
pub(crate) fn seed(tvg_id: &str, name: &str, group: &str) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(SEED_DOMAIN);
    hash_identity_field(&mut hasher, tvg_id);
    hash_identity_field(&mut hasher, name);
    hash_identity_field(&mut hasher, group);
    *hasher.finalize().as_bytes()
}

/// Namespaces a duplicate-aware identity to one Source Configuration.
pub(crate) fn channel_id(
    fingerprint: &SourceConfigurationFingerprint,
    seed: &[u8; 32],
    occurrence: u32,
) -> ChannelId {
    let mut hasher = Hasher::new();
    hasher.update(CHANNEL_ID_DOMAIN);
    hash_field(&mut hasher, fingerprint.as_bytes());
    hash_field(&mut hasher, seed);
    hash_field(&mut hasher, &occurrence.to_le_bytes());

    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(CHANNEL_ID_PREFIX.len() + 64);
    encoded.push_str(CHANNEL_ID_PREFIX);
    encoded.push_str(digest.to_hex().as_str());
    ChannelId::generated(encoded)
}

fn hash_identity_field(hasher: &mut Hasher, value: &str) {
    let normalized = normalize_identity_field(value);
    hash_field(hasher, normalized.as_bytes());
}

fn hash_field(hasher: &mut Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

/// Returns the canonical comparison key used by identity and browse ordering.
pub(crate) fn normalize_identity_field(value: &str) -> String {
    let compatibility_normalized = value.nfkc().collect::<String>();
    let mut normalized = String::with_capacity(compatibility_normalized.len());

    for (index, word) in compatibility_normalized.split_whitespace().enumerate() {
        if index != 0 {
            normalized.push(' ');
        }
        normalized.extend(word.chars().flat_map(char::to_lowercase));
    }

    normalized
}

#[cfg(test)]
mod tests {
    use crate::domain::{SourceConfiguration, SourceConfigurationInput};

    use super::{channel_id, seed};

    #[test]
    fn identity_fields_use_nfkc_collapsed_whitespace_and_lowercase() {
        assert_eq!(
            seed("  ＴＶ\u{a0}ONE ", "CAFÉ\tNEWS", "  WＯRLD  NEWS "),
            seed("tv one", "café news", "world news"),
        );
    }

    #[test]
    fn length_prefixes_preserve_field_boundaries() {
        assert_ne!(seed("ab", "c", ""), seed("a", "bc", ""));
    }

    #[test]
    fn channel_ids_are_configuration_scoped_and_duplicate_aware() {
        let first_configuration = SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://first.invalid/channels.m3u",
            None::<String>,
        ))
        .expect("test Source Configuration is valid");
        let second_configuration = SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://second.invalid/channels.m3u",
            None::<String>,
        ))
        .expect("test Source Configuration is valid");
        let changed_epg_configuration = SourceConfiguration::parse(SourceConfigurationInput::new(
            "https://first.invalid/channels.m3u",
            Some("https://first.invalid/guide.xml"),
        ))
        .expect("test Source Configuration with EPG is valid");
        let identity_seed = seed("news.one", "News One", "News");

        let first = channel_id(&first_configuration.fingerprint, &identity_seed, 0);
        let repeated = channel_id(&first_configuration.fingerprint, &identity_seed, 0);
        let duplicate = channel_id(&first_configuration.fingerprint, &identity_seed, 1);
        let other_configuration = channel_id(&second_configuration.fingerprint, &identity_seed, 0);
        let changed_epg = channel_id(&changed_epg_configuration.fingerprint, &identity_seed, 0);

        assert_eq!(first, repeated);
        assert_ne!(first, duplicate);
        assert_ne!(first, other_configuration);
        assert_ne!(first, changed_epg);
        assert_eq!(first.as_str().len(), 68);
        assert!(first.as_str().starts_with("ch1_"));
    }
}
