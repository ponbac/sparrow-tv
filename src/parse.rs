use std::{
    fmt::{self, Display, Formatter},
    str::FromStr,
};

use itertools::Itertools;
use nom::{
    bytes::complete::{tag, take_till, take_until},
    character::complete::{char, space0},
    combinator::map_res,
    sequence::{delimited, preceded},
    IResult,
};

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
                    Ok((_, entry)) => entry,
                    Err(e) => panic!("Failed to parse playlist entry: {:?}\n{}", e, joined_chunk),
                };
                entry
            })
            .collect::<Vec<PlaylistEntry>>();

        Ok(Playlist { entries })
    }
}

impl PlaylistEntry {
    pub fn parse(i: &str) -> IResult<&str, PlaylistEntry> {
        let (i, duration) = parse_duration(i)?;
        let (i, _) = space0(i)?;
        let (i, _) = parse_xui_id(i)?;
        let (i, _) = space0(i)?;
        let (i, tvg_id) = parse_tvg_id(i)?;
        let (i, _) = space0(i)?;
        let (i, tvg_name) = parse_tvg_name(i)?;
        let (i, _) = space0(i)?;
        let (i, tvg_logo) = parse_tvg_logo(i)?;
        let (i, _) = space0(i)?;
        let (i, group_title) = parse_group_title(i)?;
        let (i, _) = char(',')(i)?;
        let (i, (name, url)) = parse_name_and_url(i)?;

        Ok((
            i,
            PlaylistEntry {
                duration,
                tvg_id: tvg_id.to_string(),
                tvg_name: tvg_name.to_string(),
                tvg_logo: tvg_logo.to_string(),
                group_title: group_title.to_string(),
                name: name.to_string(),
                url: url.to_string(),
            },
        ))
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

fn parse_duration(input: &str) -> IResult<&str, i32> {
    map_res(preceded(tag("#EXTINF:"), take_until(" ")), |s: &str| {
        s.parse::<i32>()
    })(input)
}

fn parse_tvg_id(input: &str) -> IResult<&str, &str> {
    delimited(tag("tvg-id=\""), take_until("\""), tag("\""))(input)
}

fn parse_xui_id(input: &str) -> IResult<&str, &str> {
    delimited(tag("xui-id=\""), take_until("\""), tag("\""))(input)
}

fn parse_tvg_name(input: &str) -> IResult<&str, &str> {
    delimited(tag("tvg-name=\""), take_until("\""), tag("\""))(input)
}

fn parse_tvg_logo(input: &str) -> IResult<&str, &str> {
    delimited(tag("tvg-logo=\""), take_until("\""), tag("\""))(input)
}

fn parse_group_title(input: &str) -> IResult<&str, &str> {
    delimited(tag("group-title=\""), take_until("\""), tag("\""))(input)
}

fn parse_name_and_url(input: &str) -> IResult<&str, (&str, &str)> {
    let (input, name) = take_until("\n")(input)?;
    let (input, _) = char('\n')(input)?;
    let (input, url) = take_till(|c| c == '\n' || c == '\0')(input)?;
    Ok((input, (name, url)))
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
            dbg!(PlaylistEntry::parse(test_channel.trim())),
            Ok((
                "",
                PlaylistEntry {
                    duration: -1,
                    tvg_id: "ABC.se".to_string(),
                    tvg_name: "ABC FHD SE".to_string(),
                    tvg_logo: "https://logo.com".to_string(),
                    group_title: "Sweden".to_string(),
                    name: "ABC FHD SE".to_string(),
                    url: "http://abc.xyz:8080/user/pass/360".to_string()
                }
            ))
        );
    }
}
