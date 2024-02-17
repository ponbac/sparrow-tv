use std::{
    fmt::{self, Display, Formatter},
    str::FromStr,
};

use itertools::Itertools;

#[derive(Debug, PartialEq)]
pub struct PlaylistEntry {
    pub duration: i32,
    pub tvg_id: String,
    pub tvg_name: String,
    pub tvg_logo: String,
    pub group_title: String,
    pub name: String,
    pub url: String,
}

#[derive(Debug)]
pub struct Playlist {
    pub entries: Vec<PlaylistEntry>,
}

impl Playlist {
    pub fn to_m3u(&self) -> String {
        format!(
            "#EXTM3U\n{}",
            self.entries
                .iter()
                .map(|entry| entry.to_string())
                .join("\n")
        )
    }
}

impl FromStr for Playlist {
    type Err = nom::error::Error<&'static str>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let entries = s
            .lines()
            .skip(1)
            .chunks(2)
            .into_iter()
            .map(|chunk| {
                let joined_chunk = chunk.collect::<Vec<&str>>().join("\n");
                let entry = match PlaylistEntry::parse(joined_chunk.trim()) {
                    Ok(entry) => entry,
                    Err(e) => panic!("Failed to parse playlist entry: {:?}\n{}", e, joined_chunk),
                };
                entry
            })
            .collect::<Vec<PlaylistEntry>>();

        Ok(Playlist { entries })
    }
}

impl PlaylistEntry {
    pub fn parse(i: &str) -> anyhow::Result<PlaylistEntry> {
        todo!()
    }
}

impl Display for PlaylistEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "#EXTINF:{} xui-id=\"{{XUI_ID}}\" tvg-id=\"{}\" tvg-name=\"{}\" tvg-logo=\"{}\" group-title=\"{}\",{}\n{}",
            self.duration, self.tvg_id, self.tvg_name, self.tvg_logo, self.group_title, self.name, self.url
        )
    }
}

enum EntryKey {
    Duration,
    TvgId,
    TvgName,
    TvgLogo,
    GroupTitle,
    Name,
    Url,
}

fn get(i: &str, key: &EntryKey) -> anyhow::Result<String> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("#EXTINF:-1"), Ok(("", -1)));
    }

    #[test]
    fn test_parse_tvg_id() {
        assert_eq!(parse_tvg_id("tvg-id=\"\""), Ok(("", "")));
    }

    #[test]
    fn test_parse_tvg_name() {
        assert_eq!(parse_tvg_name("tvg-name=\"\""), Ok(("", "")));
    }

    #[test]
    fn test_parse_tvg_logo() {
        assert_eq!(parse_tvg_logo("tvg-logo=\"\""), Ok(("", "")));
    }

    #[test]
    fn test_parse_group_title() {
        assert_eq!(parse_group_title("group-title=\"\""), Ok(("", "")));
    }

    #[test]
    fn test_parse_name_and_url() {
        assert_eq!(
            parse_name_and_url("ABC ‧ TEST\nhttp://abc.xyz:8080/user/pass/168917"),
            Ok(("", ("ABC ‧ TEST", "http://abc.xyz:8080/user/pass/168917")))
        );
    }

    #[test]
    fn test_parse_playlist_entry() {
        let test_channel = r#"
#EXTINF:-1 xui-id="{XUI_ID}" tvg-id="ABC.se" tvg-name="ABC FHD SE" tvg-logo="https://logo.com" group-title="Sweden",ABC FHD SE
http://abc.xyz:8080/user/pass/360
        "#;
        assert_eq!(
            dbg!(PlaylistEntry::parse(test_channel.trim())).unwrap(),
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
}
