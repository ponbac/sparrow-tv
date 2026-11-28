use std::{collections::HashSet, io::BufRead, sync::Arc};

use unicode_normalization::UnicodeNormalization;
use url::Url;

use crate::domain::{
    ChannelGroupFilter, M3uFailureKind, SafeFailure, SnapshotOperation, SourceKind, StoreError,
    is_channel_icon_location,
};

pub(crate) struct ParsedChannel {
    pub(crate) tvg_id: Arc<str>,
    pub(crate) name: Arc<str>,
    pub(crate) group: Arc<str>,
    /// Validated HTTP(S) playback location; parsed into `url::Url` lazily on tune.
    pub(crate) playback: Arc<str>,
    /// The `tvg-logo` location exactly as given, when it is a usable absolute
    /// HTTP(S) URL. Equal icons share one interned allocation.
    pub(crate) icon: Option<Arc<str>>,
}

struct PendingEntry {
    number: u32,
    tvg_id: String,
    name: String,
    group: String,
    icon: Option<String>,
}

#[derive(Default)]
struct MetadataAttributes {
    tvg_id: Option<String>,
    tvg_name: Option<String>,
    group_title: Option<String>,
    tvg_logo: Option<String>,
}

pub(crate) fn parse(reader: &mut dyn BufRead) -> Result<Vec<ParsedChannel>, SafeFailure> {
    let mut channels = Vec::new();
    let empty: Arc<str> = Arc::from("");
    let mut interned_groups: HashSet<Arc<str>> = HashSet::new();
    interned_groups.insert(Arc::clone(&empty));
    let mut interned_icons: HashSet<Arc<str>> = HashSet::new();
    let mut pending: Option<PendingEntry> = None;
    let mut completed_entries = 0;
    let mut header_seen = false;
    let mut first_line = true;
    let mut line_bytes = Vec::new();

    loop {
        line_bytes.clear();
        let bytes_read =
            reader
                .read_until(b'\n', &mut line_bytes)
                .map_err(|_| SafeFailure::Snapshot {
                    kind: SourceKind::M3u,
                    operation: SnapshotOperation::ReadStage,
                    reason: StoreError::Unavailable,
                })?;
        if bytes_read == 0 {
            break;
        }

        let encoded_line = if first_line {
            first_line = false;
            line_bytes
                .strip_prefix(&[0xef, 0xbb, 0xbf])
                .unwrap_or(&line_bytes)
        } else {
            &line_bytes
        };
        let line = std::str::from_utf8(encoded_line)
            .map_err(|_| SafeFailure::InvalidEncoding {
                kind: SourceKind::M3u,
            })?
            .trim();

        if !header_seen {
            if line.is_empty() {
                continue;
            }
            if !is_header(line) {
                return Err(invalid_format(None, M3uFailureKind::MissingHeader));
            }
            header_seen = true;
            continue;
        }

        if line.is_empty() {
            continue;
        }

        if is_extinf_directive(line) {
            if let Some(pending) = pending.as_ref() {
                return Err(invalid_format(
                    Some(pending.number),
                    M3uFailureKind::IncompleteEntry,
                ));
            }

            let entry = next_entry_number(completed_entries);
            let metadata = line
                .strip_prefix("#EXTINF:")
                .ok_or_else(|| invalid_format(Some(entry), M3uFailureKind::MalformedMetadata))?;
            pending = Some(parse_metadata(metadata, entry)?);
            continue;
        }

        if line.starts_with('#') {
            continue;
        }

        let entry = pending.take().ok_or_else(|| {
            invalid_format(
                Some(next_entry_number(completed_entries)),
                M3uFailureKind::UnexpectedLocation,
            )
        })?;
        let playback = parse_playback(line, entry.number)?;
        completed_entries += 1;
        if is_non_live(&playback, &entry.group) {
            continue;
        }
        let tvg_id = if entry.tvg_id.is_empty() {
            Arc::clone(&empty)
        } else {
            Arc::from(entry.tvg_id)
        };
        let group = if let Some(existing) = interned_groups.get(entry.group.as_str()) {
            Arc::clone(existing)
        } else if let Ok(filter) = ChannelGroupFilter::parse(entry.group) {
            let group: Arc<str> = Arc::from(filter.as_str());
            interned_groups.insert(Arc::clone(&group));
            group
        } else {
            Arc::clone(&empty)
        };
        let icon = entry.icon.map(|icon| {
            if let Some(existing) = interned_icons.get(icon.as_str()) {
                Arc::clone(existing)
            } else {
                let icon: Arc<str> = Arc::from(icon);
                interned_icons.insert(Arc::clone(&icon));
                icon
            }
        });
        channels.push(ParsedChannel {
            tvg_id,
            name: Arc::from(entry.name),
            group,
            playback: Arc::from(playback.as_str()),
            icon,
        });
    }

    if !header_seen {
        return Err(invalid_format(None, M3uFailureKind::MissingHeader));
    }
    if let Some(pending) = pending {
        return Err(invalid_format(
            Some(pending.number),
            M3uFailureKind::IncompleteEntry,
        ));
    }
    if channels.is_empty() {
        return Err(SafeFailure::NoPlayableChannels);
    }

    Ok(channels)
}

fn is_header(line: &str) -> bool {
    line.strip_prefix("#EXTM3U").is_some_and(|attributes| {
        attributes.is_empty() || attributes.chars().next().is_some_and(char::is_whitespace)
    })
}

fn is_extinf_directive(line: &str) -> bool {
    line.strip_prefix("#EXTINF").is_some_and(|remainder| {
        remainder.is_empty()
            || remainder.starts_with(':')
            || remainder.chars().next().is_some_and(char::is_whitespace)
    })
}

fn next_entry_number(completed_entries: usize) -> u32 {
    u32::try_from(completed_entries)
        .unwrap_or(u32::MAX - 1)
        .saturating_add(1)
}

fn parse_metadata(metadata: &str, entry: u32) -> Result<PendingEntry, SafeFailure> {
    let separator = find_title_separator(metadata, entry)?;
    let metadata_fields = metadata[..separator].trim();
    let display_name = &metadata[separator + 1..];

    let duration_end = metadata_fields
        .char_indices()
        .find_map(|(index, character)| character.is_whitespace().then_some(index))
        .unwrap_or(metadata_fields.len());
    let duration = &metadata_fields[..duration_end];
    if duration.is_empty() || duration.parse::<i64>().is_err() {
        return Err(invalid_format(
            Some(entry),
            M3uFailureKind::MalformedMetadata,
        ));
    }

    let attributes = parse_attributes(&metadata_fields[duration_end..], entry)?;
    let display_name = normalize_presentation(display_name);
    let name = if display_name.is_empty() {
        attributes
            .tvg_name
            .as_deref()
            .map(normalize_presentation)
            .unwrap_or_default()
    } else {
        display_name
    };
    if name.is_empty() {
        return Err(invalid_format(Some(entry), M3uFailureKind::EmptyName));
    }

    Ok(PendingEntry {
        number: entry,
        tvg_id: attributes
            .tvg_id
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .to_owned(),
        name,
        group: attributes
            .group_title
            .as_deref()
            .map(normalize_presentation)
            .unwrap_or_default(),
        icon: attributes.tvg_logo.and_then(|logo| {
            let logo = logo.trim();
            is_channel_icon_location(logo).then(|| logo.to_owned())
        }),
    })
}

fn find_title_separator(metadata: &str, entry: u32) -> Result<usize, SafeFailure> {
    let mut quoted = false;
    let mut escaped = false;

    for (index, character) in metadata.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"'
                && is_quoted_value_terminator(metadata, index + character.len_utf8(), true)
            {
                quoted = false;
            }
        } else if character == '"' {
            quoted = true;
        } else if character == ',' {
            return Ok(index);
        }
    }

    Err(invalid_format(
        Some(entry),
        if quoted {
            M3uFailureKind::UnterminatedQuote
        } else {
            M3uFailureKind::MalformedMetadata
        },
    ))
}

fn parse_attributes(attributes: &str, entry: u32) -> Result<MetadataAttributes, SafeFailure> {
    let mut parsed = MetadataAttributes::default();
    let mut cursor = 0;

    while skip_whitespace(attributes, &mut cursor) {
        let key_start = cursor;
        while let Some(character) = character_at(attributes, cursor) {
            if character == '=' || character.is_whitespace() {
                break;
            }
            cursor += character.len_utf8();
        }
        let key_end = cursor;
        skip_whitespace(attributes, &mut cursor);

        if key_start == key_end || character_at(attributes, cursor) != Some('=') {
            return Err(invalid_format(
                Some(entry),
                M3uFailureKind::MalformedMetadata,
            ));
        }
        cursor += '='.len_utf8();
        skip_whitespace(attributes, &mut cursor);

        let value = match character_at(attributes, cursor) {
            Some('"') => {
                cursor += '"'.len_utf8();
                parse_quoted_value(attributes, &mut cursor, entry)?
            }
            Some(_) => {
                let value_start = cursor;
                while let Some(character) = character_at(attributes, cursor) {
                    if character.is_whitespace() {
                        break;
                    }
                    cursor += character.len_utf8();
                }
                attributes[value_start..cursor].to_owned()
            }
            None => {
                return Err(invalid_format(
                    Some(entry),
                    M3uFailureKind::MalformedMetadata,
                ));
            }
        };

        let key = &attributes[key_start..key_end];
        if key.eq_ignore_ascii_case("tvg-id") {
            parsed.tvg_id = Some(value);
        } else if key.eq_ignore_ascii_case("tvg-name") {
            parsed.tvg_name = Some(value);
        } else if key.eq_ignore_ascii_case("group-title") {
            parsed.group_title = Some(value);
        } else if key.eq_ignore_ascii_case("tvg-logo") {
            parsed.tvg_logo = Some(value);
        }
    }

    Ok(parsed)
}

fn parse_quoted_value(
    attributes: &str,
    cursor: &mut usize,
    entry: u32,
) -> Result<String, SafeFailure> {
    let mut value = String::new();

    loop {
        let Some(character) = character_at(attributes, *cursor) else {
            return Err(invalid_format(
                Some(entry),
                M3uFailureKind::UnterminatedQuote,
            ));
        };
        *cursor += character.len_utf8();

        match character {
            '"' => {
                if is_quoted_value_terminator(attributes, *cursor, false) {
                    return Ok(value);
                }
                value.push('"');
            }
            '\\' => match character_at(attributes, *cursor) {
                Some(next @ ('"' | '\\')) => {
                    value.push(next);
                    *cursor += next.len_utf8();
                }
                _ => value.push('\\'),
            },
            _ => value.push(character),
        }
    }
}

/// Provider feeds sometimes place an unescaped quoted phrase inside a quoted
/// attribute value. A quote closes the value only when what follows is the
/// metadata separator, the end of the attribute list, or another `key=value`
/// attribute. Other quotes remain presentation text.
fn is_quoted_value_terminator(input: &str, after_quote: usize, comma_terminates: bool) -> bool {
    let Some(next) = character_at(input, after_quote) else {
        return true;
    };
    if comma_terminates && next == ',' {
        return true;
    }
    if !next.is_whitespace() {
        return false;
    }

    let mut cursor = after_quote;
    skip_whitespace(input, &mut cursor);
    let Some(next) = character_at(input, cursor) else {
        return true;
    };
    if comma_terminates && next == ',' {
        return true;
    }

    let key_start = cursor;
    while let Some(character) = character_at(input, cursor) {
        if character == '=' || character.is_whitespace() || character == ',' {
            break;
        }
        cursor += character.len_utf8();
    }
    let key_end = cursor;
    skip_whitespace(input, &mut cursor);
    key_start < key_end && character_at(input, cursor) == Some('=')
}

fn skip_whitespace(input: &str, cursor: &mut usize) -> bool {
    while let Some(character) = character_at(input, *cursor) {
        if !character.is_whitespace() {
            break;
        }
        *cursor += character.len_utf8();
    }
    *cursor < input.len()
}

fn character_at(input: &str, cursor: usize) -> Option<char> {
    input.get(cursor..)?.chars().next()
}

pub(crate) fn normalize_presentation(value: &str) -> String {
    if value.is_ascii() {
        let mut normalized = String::with_capacity(value.len());
        let mut whitespace_pending = false;
        for byte in value.bytes() {
            // Match `char::is_whitespace()` for all ASCII bytes: 0x09..=0x0D and 0x20.
            if matches!(byte, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r') {
                whitespace_pending = !normalized.is_empty();
            } else {
                if whitespace_pending {
                    normalized.push(' ');
                    whitespace_pending = false;
                }
                normalized.push(byte as char);
            }
        }
        return normalized;
    }

    let mut normalized = String::with_capacity(value.len());
    let mut whitespace_pending = false;

    for character in value.nfkc() {
        if character.is_whitespace() {
            whitespace_pending = !normalized.is_empty();
        } else {
            if whitespace_pending {
                normalized.push(' ');
                whitespace_pending = false;
            }
            normalized.push(character);
        }
    }

    normalized
}

fn parse_playback(location: &str, entry: u32) -> Result<Url, SafeFailure> {
    let playback = Url::parse(location)
        .map_err(|_| invalid_format(Some(entry), M3uFailureKind::UnsupportedPlaybackSource))?;
    if !matches!(playback.scheme(), "http" | "https") || !playback.has_host() {
        return Err(invalid_format(
            Some(entry),
            M3uFailureKind::UnsupportedPlaybackSource,
        ));
    }

    Ok(playback)
}

/// Only explicit non-live paths, file extensions or group tags exclude an entry.
/// Plain movie/series group names and live stream formats are left alone.
fn is_non_live(playback: &Url, group: &str) -> bool {
    let path = playback.path();
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    if ["movie", "series", "vod", "vods", "timeshift"]
        .iter()
        .any(|segment| first.eq_ignore_ascii_case(segment))
    {
        return true;
    }

    let file = path.rsplit('/').next().unwrap_or("");
    if let Some((_, extension)) = file.rsplit_once('.')
        && [
            "mkv", "mp4", "avi", "mov", "m4v", "wmv", "webm", "mpg", "mpeg", "divx", "vob", "iso",
            "3gp", "ogv",
        ]
        .iter()
        .any(|container| extension.eq_ignore_ascii_case(container))
    {
        return true;
    }

    if !group.contains(['(', '[']) {
        return false;
    }
    let group = group.to_ascii_lowercase();
    [
        "(vod)", "[vod]", "(serie)", "[serie]", "(series)", "[series]",
    ]
    .iter()
    .any(|tag| group.contains(tag))
}

fn invalid_format(entry: Option<u32>, reason: M3uFailureKind) -> SafeFailure {
    SafeFailure::InvalidFormat { entry, reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_playlists_keep_only_live_channels() {
        let mut playlist = String::from("#EXTM3U\n");
        let live = [
            ("/123", "Movies"),
            ("/live/123.ts", "Sky Movies"),
            ("/123.m3u8?next=/movie/file.mp4", "Series"),
            ("/live/series/123", "Drama (Series HD)"),
            ("/123.m2ts", "Live"),
            ("/123.mpegts", "Live"),
            ("/123.m3u", "Live"),
            ("/123.mpd", "Live"),
        ];
        let on_demand = [
            ("//MoViE/123.txt", "Movies"),
            ("/series/123", "Drama"),
            ("/vod/123.m3u8", "Drama"),
            ("/vods/123", "Drama"),
            ("/timeshift/123.ts", "Live"),
            ("/123.MKV?format=ts", "Movies"),
            ("/123.mp4#live", "Movies"),
            ("/123.avi", "Movies"),
            ("/123.ts", "Movies (VOD)"),
            ("/123", "Movies [Vod]"),
            ("/123", "Drama （SeRiE）"),
            ("/123", "Drama [Serie]"),
            ("/123", "Drama (Series)"),
            ("/123", "Drama [Series]"),
        ];
        for (index, (path, group)) in live.into_iter().chain(on_demand).enumerate() {
            use std::fmt::Write as _;
            writeln!(
                playlist,
                "#EXTINF:-1 group-title=\"{group}\",Entry {index}\nhttps://fixture.invalid{path}"
            )
            .expect("writing to a string succeeds");
        }

        let channels = parse(&mut playlist.as_bytes()).expect("playlist is valid");
        let retained: Vec<_> = channels
            .iter()
            .map(|channel| channel.group.as_ref())
            .collect();
        assert_eq!(retained, live.map(|(_, group)| group));
    }

    #[test]
    fn on_demand_only_playlists_have_no_playable_channels() {
        let playlist = b"#EXTM3U\n#EXTINF:-1,Episode\nhttps://fixture.invalid/series/123\n";
        assert_eq!(
            parse(&mut playlist.as_slice()).err(),
            Some(SafeFailure::NoPlayableChannels)
        );
    }

    #[test]
    fn malformed_entries_are_validated_and_numbered_in_playlist_order() {
        for (tail, reason) in [
            (
                "#EXTINF:invalid group-title=\"(VOD)\",Movie\nhttps://fixture.invalid/movie/2\n",
                M3uFailureKind::MalformedMetadata,
            ),
            (
                "#EXTINF:-1 group-title=\"(VOD)\",Movie\nftp://fixture.invalid/movie/2\n",
                M3uFailureKind::UnsupportedPlaybackSource,
            ),
            (
                "#EXTINF:-1 group-title=\"(VOD)\",Movie\n",
                M3uFailureKind::IncompleteEntry,
            ),
            (
                "https://fixture.invalid/movie/2\n",
                M3uFailureKind::UnexpectedLocation,
            ),
        ] {
            let playlist =
                format!("#EXTM3U\n#EXTINF:-1,Movie\nhttps://fixture.invalid/movie/1\n{tail}");
            assert_eq!(
                parse(&mut playlist.as_bytes()).err(),
                Some(SafeFailure::InvalidFormat {
                    entry: Some(2),
                    reason
                })
            );
        }
    }
}
