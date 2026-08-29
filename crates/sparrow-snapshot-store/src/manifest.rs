use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::layout::{DiskKind, MAX_VALIDATOR_BYTES, Slot};

const MANIFEST_VERSION: u16 = 1;
const POINTER_VERSION: u16 = 1;
const DIGEST_HEX_BYTES: usize = 64;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct DiskSource {
    pub(crate) kind: DiskKind,
    pub(crate) key: [u8; 32],
}

#[derive(Clone, Default, Eq, PartialEq)]
pub(crate) struct DiskValidators {
    pub(crate) etag: Option<String>,
    pub(crate) last_modified: Option<String>,
}

impl DiskValidators {
    pub(crate) fn new(
        etag: Option<String>,
        last_modified: Option<String>,
    ) -> Result<Self, ManifestError> {
        validate_validator(etag.as_deref())?;
        validate_validator(last_modified.as_deref())?;
        Ok(Self {
            etag,
            last_modified,
        })
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct DiskMetadata {
    pub(crate) source: DiskSource,
    pub(crate) decoded_bytes: u64,
    pub(crate) checksum: [u8; 32],
    pub(crate) validated_at: DateTime<Utc>,
    pub(crate) validators: DiskValidators,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManifestError {
    Invalid,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestDocument {
    version: u16,
    source_kind: String,
    source_key: String,
    decoded_bytes: u64,
    checksum: String,
    validated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PointerDocument {
    version: u16,
    slot: String,
    checksum: String,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct DiskPointer {
    pub(crate) slot: Slot,
    pub(crate) checksum: [u8; 32],
}

pub(crate) fn encode_manifest(metadata: &DiskMetadata) -> Result<Vec<u8>, ManifestError> {
    validate_validator(metadata.validators.etag.as_deref())?;
    validate_validator(metadata.validators.last_modified.as_deref())?;
    serde_json::to_vec(&ManifestDocument {
        version: MANIFEST_VERSION,
        source_kind: metadata.source.kind.manifest_name().to_owned(),
        source_key: encode_digest(&metadata.source.key),
        decoded_bytes: metadata.decoded_bytes,
        checksum: encode_digest(&metadata.checksum),
        validated_at: canonical_timestamp(metadata.validated_at),
        etag: metadata.validators.etag.clone(),
        last_modified: metadata.validators.last_modified.clone(),
    })
    .map_err(|_| ManifestError::Invalid)
}

pub(crate) fn decode_manifest(bytes: &[u8]) -> Result<DiskMetadata, ManifestError> {
    let document: ManifestDocument =
        serde_json::from_slice(bytes).map_err(|_| ManifestError::Invalid)?;
    if document.version != MANIFEST_VERSION {
        return Err(ManifestError::Invalid);
    }
    let kind =
        DiskKind::parse_manifest_name(&document.source_kind).ok_or(ManifestError::Invalid)?;
    let source_key = decode_digest(&document.source_key)?;
    let checksum = decode_digest(&document.checksum)?;
    let validated_at = parse_canonical_timestamp(&document.validated_at)?;
    let validators = DiskValidators::new(document.etag, document.last_modified)?;
    Ok(DiskMetadata {
        source: DiskSource {
            kind,
            key: source_key,
        },
        decoded_bytes: document.decoded_bytes,
        checksum,
        validated_at,
        validators,
    })
}

pub(crate) fn encode_pointer(slot: Slot, checksum: &[u8; 32]) -> Result<Vec<u8>, ManifestError> {
    serde_json::to_vec(&PointerDocument {
        version: POINTER_VERSION,
        slot: slot.name().to_owned(),
        checksum: encode_digest(checksum),
    })
    .map_err(|_| ManifestError::Invalid)
}

pub(crate) fn decode_pointer(bytes: &[u8]) -> Result<DiskPointer, ManifestError> {
    let document: PointerDocument =
        serde_json::from_slice(bytes).map_err(|_| ManifestError::Invalid)?;
    if document.version != POINTER_VERSION {
        return Err(ManifestError::Invalid);
    }
    Ok(DiskPointer {
        slot: Slot::parse(&document.slot).ok_or(ManifestError::Invalid)?,
        checksum: decode_digest(&document.checksum)?,
    })
}

fn validate_validator(value: Option<&str>) -> Result<(), ManifestError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_empty()
        || value.len() > MAX_VALIDATOR_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '\r' | '\n'))
    {
        return Err(ManifestError::Invalid);
    }
    Ok(())
}

fn encode_digest(value: &[u8; 32]) -> String {
    blake3::Hash::from_bytes(*value).to_hex().to_string()
}

fn decode_digest(value: &str) -> Result<[u8; 32], ManifestError> {
    if value.len() != DIGEST_HEX_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ManifestError::Invalid);
    }
    blake3::Hash::from_hex(value)
        .map(|hash| *hash.as_bytes())
        .map_err(|_| ManifestError::Invalid)
}

fn canonical_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn parse_canonical_timestamp(value: &str) -> Result<DateTime<Utc>, ManifestError> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|_| ManifestError::Invalid)?
        .with_timezone(&Utc);
    if value != canonical_timestamp(parsed) {
        return Err(ManifestError::Invalid);
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Timelike, Utc};

    use super::*;

    #[test]
    fn manifest_round_trips_only_canonical_private_metadata() {
        let metadata = fixture_metadata();
        let encoded = encode_manifest(&metadata).expect("fixture manifest encodes");
        assert!(decode_manifest(&encoded).expect("encoded manifest decodes") == metadata);

        let text = std::str::from_utf8(&encoded).expect("JSON is UTF-8");
        assert!(text.contains("2026-08-29T12:34:56.123456789Z"));
        assert!(!text.contains("+00:00"));
    }

    #[test]
    fn strict_documents_reject_unknown_duplicate_and_noncanonical_fields() {
        let canonical = encode_manifest(&fixture_metadata()).expect("fixture manifest encodes");
        let canonical = String::from_utf8(canonical).expect("JSON is UTF-8");

        for malformed in [
            canonical.replacen("{", "{\"unknown\":true,", 1),
            canonical.replacen("\"version\":1", "\"version\":1,\"version\":1", 1),
            canonical.replacen("\"version\":1", "\"version\":2", 1),
            canonical.replacen("source_key\":\"00", "source_key\":\"AA", 1),
            canonical.replacen("T12:34:56.123456789Z", "T12:34:56.123456789+00:00", 1),
        ] {
            assert!(matches!(
                decode_manifest(malformed.as_bytes()),
                Err(ManifestError::Invalid)
            ));
        }

        for malformed in [
            br#"{"version":1,"slot":"a","checksum":"0000000000000000000000000000000000000000000000000000000000000000","extra":true}"#.as_slice(),
            br#"{"version":1,"version":1,"slot":"a","checksum":"0000000000000000000000000000000000000000000000000000000000000000"}"#.as_slice(),
            br#"{"version":2,"slot":"a","checksum":"0000000000000000000000000000000000000000000000000000000000000000"}"#.as_slice(),
            br#"{"version":1,"slot":"A","checksum":"0000000000000000000000000000000000000000000000000000000000000000"}"#.as_slice(),
            br#"{"version":1,"slot":"a","checksum":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#.as_slice(),
        ] {
            assert!(matches!(
                decode_pointer(malformed),
                Err(ManifestError::Invalid)
            ));
        }

        let pointer = encode_pointer(Slot::B, &[7; 32]).expect("pointer encodes");
        let decoded = decode_pointer(&pointer).expect("pointer decodes");
        assert_eq!(decoded.slot, Slot::B);
        assert_eq!(decoded.checksum, [7; 32]);
    }

    #[test]
    fn validators_are_bounded_and_header_safe() {
        assert!(DiskValidators::new(Some("etag".to_owned()), None).is_ok());
        for invalid in [
            String::new(),
            "line\nbreak".to_owned(),
            "x".repeat(MAX_VALIDATOR_BYTES + 1),
        ] {
            assert!(matches!(
                DiskValidators::new(Some(invalid), None),
                Err(ManifestError::Invalid)
            ));
        }
    }

    fn fixture_metadata() -> DiskMetadata {
        DiskMetadata {
            source: DiskSource {
                kind: DiskKind::M3u,
                key: [0; 32],
            },
            decoded_bytes: 42,
            checksum: [1; 32],
            validated_at: Utc
                .with_ymd_and_hms(2026, 8, 29, 12, 34, 56)
                .single()
                .expect("fixture timestamp")
                .with_nanosecond(123_456_789)
                .expect("fixture nanoseconds"),
            validators: DiskValidators::new(
                Some("\"opaque-etag\"".to_owned()),
                Some("Sat, 29 Aug 2026 12:34:56 GMT".to_owned()),
            )
            .expect("fixture validators"),
        }
    }
}
