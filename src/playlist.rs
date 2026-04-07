use std::{
    collections::HashMap,
    fmt::{self, Display, Formatter},
    num::ParseIntError,
    str::FromStr,
};

use itertools::Itertools;
use thiserror::Error;

#[derive(Debug, PartialEq, Clone)]
pub struct PlaylistEntry {
    pub duration: i32,
    pub tvg_id: String,
    pub tvg_name: String,
    pub tvg_logo: String,
    pub group_title: String,
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub entries: Vec<PlaylistEntry>,
    pub filtered_entries: Vec<PlaylistEntry>,
}

#[derive(Debug, Error)]
pub enum PlaylistParseError {
    #[error("playlist is empty or missing #EXTM3U header")]
    MissingHeader,
    #[error("playlist entry {entry_index} is incomplete:\n{chunk}")]
    IncompleteEntry { entry_index: usize, chunk: String },
    #[error("playlist entry {entry_index} has invalid duration `{value}`")]
    InvalidDuration {
        entry_index: usize,
        value: String,
        #[source]
        source: ParseIntError,
    },
    #[error("playlist entry {entry_index} is malformed: {reason}\n{chunk}")]
    MalformedEntry {
        entry_index: usize,
        reason: String,
        chunk: String,
    },
}

impl Playlist {
    pub fn new(entries: Vec<PlaylistEntry>) -> Self {
        Self {
            entries: entries.clone(),
            filtered_entries: entries,
        }
    }

    pub fn filtered_groups(&self) -> Vec<String> {
        self.filtered_entries
            .iter()
            .map(|entry| entry.group_title.clone())
            .unique()
            .collect()
    }

    pub fn to_m3u(&self) -> String {
        format!(
            "#EXTM3U\n{}",
            self.filtered_entries
                .iter()
                .map(|entry| entry.to_string())
                .join("\n")
        )
    }

    pub fn exclude_groups(&mut self, groups_to_exclude: Vec<&str>) {
        self.filtered_entries
            .retain(|entry| !groups_to_exclude.contains(&entry.group_title.as_str()));
    }

    pub fn exclude_containing(&mut self, snippets: Vec<&str>) {
        self.filtered_entries.retain(|entry| {
            let is_excluded = snippets
                .iter()
                .any(|snippet| entry.group_title.contains(snippet));
            if is_excluded {
                tracing::debug!(
                    "Excluding entry {} with group title: {}",
                    entry.name,
                    entry.group_title
                );
            }
            !is_excluded
        });
    }

    pub fn exclude_all_extensions(&mut self) {
        self.filtered_entries.retain(|entry| {
            entry
                .url
                .rsplit('/')
                .next()
                .is_some_and(|segment| !segment.contains('.'))
        });
    }
}

impl FromStr for Playlist {
    type Err = PlaylistParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut lines = s
            .lines()
            .map(|line| line.trim_end_matches('\r'))
            .filter(|line| !line.trim().is_empty());

        let Some(header) = lines.next() else {
            return Err(PlaylistParseError::MissingHeader);
        };
        if header.trim() != "#EXTM3U" {
            return Err(PlaylistParseError::MissingHeader);
        }

        let mut entries = Vec::new();
        let mut entry_index = 1;

        loop {
            let Some(info_line) = lines.next() else {
                break;
            };
            let url_line = lines
                .next()
                .ok_or_else(|| PlaylistParseError::IncompleteEntry {
                    entry_index,
                    chunk: info_line.to_string(),
                })?;
            let chunk = format!("{info_line}\n{url_line}");
            let entry = PlaylistEntry::parse(entry_index, &chunk)?;
            entries.push(entry);
            entry_index += 1;
        }

        Ok(Playlist::new(entries))
    }
}

impl PlaylistEntry {
    pub fn parse(entry_index: usize, input: &str) -> Result<PlaylistEntry, PlaylistParseError> {
        let mut lines = input.lines().map(|line| line.trim_end_matches('\r'));

        let info_line = lines
            .next()
            .ok_or_else(|| PlaylistParseError::IncompleteEntry {
                entry_index,
                chunk: input.to_string(),
            })?;
        let url_line = lines
            .next()
            .ok_or_else(|| PlaylistParseError::IncompleteEntry {
                entry_index,
                chunk: input.to_string(),
            })?;

        let info = info_line.strip_prefix("#EXTINF:").ok_or_else(|| {
            PlaylistParseError::MalformedEntry {
                entry_index,
                reason: "missing #EXTINF prefix".to_string(),
                chunk: input.to_string(),
            }
        })?;

        let (metadata, name) =
            split_extinf_metadata(info).ok_or_else(|| PlaylistParseError::MalformedEntry {
                entry_index,
                reason: "missing channel name separator".to_string(),
                chunk: input.to_string(),
            })?;

        let (duration_str, attrs_str) = split_duration_and_attrs(metadata);
        let duration =
            duration_str
                .parse::<i32>()
                .map_err(|source| PlaylistParseError::InvalidDuration {
                    entry_index,
                    value: duration_str.to_string(),
                    source,
                })?;

        let attrs =
            parse_attributes(attrs_str).map_err(|reason| PlaylistParseError::MalformedEntry {
                entry_index,
                reason,
                chunk: input.to_string(),
            })?;

        Ok(PlaylistEntry {
            duration,
            tvg_id: attrs.get("tvg-id").cloned().unwrap_or_default(),
            tvg_name: attrs
                .get("tvg-name")
                .cloned()
                .unwrap_or_else(|| name.to_string()),
            tvg_logo: attrs.get("tvg-logo").cloned().unwrap_or_default(),
            group_title: attrs.get("group-title").cloned().unwrap_or_default(),
            name: name.to_string(),
            url: url_line.trim().to_string(),
        })
    }
}

impl Display for PlaylistEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "#EXTINF:{} xui-id=\"{{XUI_ID}}\" tvg-id=\"{}\" tvg-name=\"{}\" tvg-logo=\"{}\" group-title=\"{}\",{}\n{}",
            self.duration,
            self.tvg_id,
            self.tvg_name,
            self.tvg_logo,
            self.group_title,
            self.name,
            self.url
        )
    }
}

fn split_extinf_metadata(input: &str) -> Option<(&str, &str)> {
    let mut in_quotes = false;
    for (index, ch) in input.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                let metadata = input[..index].trim();
                let name = input[index + 1..].trim();
                return Some((metadata, name));
            }
            _ => {}
        }
    }
    None
}

fn split_duration_and_attrs(input: &str) -> (&str, &str) {
    let trimmed = input.trim();
    let split_index = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let duration = &trimmed[..split_index];
    let attrs = trimmed[split_index..].trim();
    (duration, attrs)
}

fn parse_attributes(input: &str) -> Result<HashMap<String, String>, String> {
    let mut rest = input.trim();
    let mut attributes = HashMap::new();

    while !rest.is_empty() {
        let Some(eq_index) = rest.find('=') else {
            return Err(format!("missing `=` in attribute segment `{rest}`"));
        };
        let key = rest[..eq_index].trim();
        if key.is_empty() {
            return Err(format!("missing attribute key in segment `{rest}`"));
        }

        let value_start = &rest[eq_index + 1..];
        let Some(value_start) = value_start.strip_prefix('"') else {
            return Err(format!("attribute `{key}` is not quoted"));
        };
        let Some(end_quote) = value_start.find('"') else {
            return Err(format!(
                "attribute `{key}` has an unterminated quoted value"
            ));
        };

        let value = &value_start[..end_quote];
        attributes.insert(key.to_string(), value.to_string());
        rest = value_start[end_quote + 1..].trim_start();
    }

    Ok(attributes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_playlist_entry() {
        let test_channel = r#"
#EXTINF:-1 xui-id="{XUI_ID}" tvg-id="ABC.se" tvg-name="ABC FHD SE" tvg-logo="https://logo.com" group-title="Sweden",ABC FHD SE
http://abc.xyz:8080/user/pass/360
        "#;
        assert_eq!(
            PlaylistEntry::parse(1, test_channel.trim()).unwrap(),
            PlaylistEntry {
                duration: -1,
                tvg_id: "ABC.se".to_string(),
                tvg_name: "ABC FHD SE".to_string(),
                tvg_logo: "https://logo.com".to_string(),
                group_title: "Sweden".to_string(),
                name: "ABC FHD SE".to_string(),
                url: "http://abc.xyz:8080/user/pass/360".to_string()
            }
        );
    }

    #[test]
    fn test_parse_playlist_entry_without_xui_id() {
        let test_channel = r#"
#EXTINF:-1 tvg-id="ABC.se" tvg-name="ABC FHD SE" tvg-logo="https://logo.com" group-title="Sweden",ABC FHD SE
http://abc.xyz:8080/user/pass/360
        "#;
        assert_eq!(
            PlaylistEntry::parse(1, test_channel.trim()).unwrap(),
            PlaylistEntry {
                duration: -1,
                tvg_id: "ABC.se".to_string(),
                tvg_name: "ABC FHD SE".to_string(),
                tvg_logo: "https://logo.com".to_string(),
                group_title: "Sweden".to_string(),
                name: "ABC FHD SE".to_string(),
                url: "http://abc.xyz:8080/user/pass/360".to_string()
            }
        );
    }

    #[test]
    fn test_parse_playlist_returns_error_instead_of_panicking() {
        let invalid_playlist = "#EXTM3U\n<head><title>502 Bad Gateway</title></head>\n<body>";
        let error = invalid_playlist.parse::<Playlist>().unwrap_err();
        assert!(matches!(
            error,
            PlaylistParseError::MalformedEntry { entry_index: 1, .. }
        ));
    }
}
