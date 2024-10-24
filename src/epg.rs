use chrono::{DateTime, FixedOffset};
use itertools::Itertools;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::io::Read;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Epg {
    #[serde(rename = "channel", default)]
    pub channels: Vec<Channel>,
    #[serde(rename = "programme", default)]
    pub programmes: Vec<Programme>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Channel {
    #[serde(rename(deserialize = "id"))]
    pub id: String,
    #[serde(rename(deserialize = "display-name"))]
    pub display_name: String,
    #[serde(default)]
    pub icon: Option<Icon>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Icon {
    pub src: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Programme {
    #[serde(deserialize_with = "deserialize_datetime")]
    pub start: DateTime<FixedOffset>,
    #[serde(deserialize_with = "deserialize_datetime")]
    pub stop: DateTime<FixedOffset>,
    pub channel: String,
    pub title: String,
    pub desc: String,
}

impl Epg {
    pub fn from_reader(reader: impl Read) -> Result<Epg, Box<dyn std::error::Error>> {
        let epg: Epg = serde_xml_rs::from_reader(reader)?;
        Ok(epg)
    }

    pub async fn from_url(url: &str) -> Result<Epg, Box<dyn std::error::Error>> {
        let response = reqwest::get(url).await?;
        let body = response.text().await?;
        Epg::from_reader(body.as_bytes())
    }

    pub fn channel_map(&self) -> HashMap<String, Channel> {
        self.channels
            .clone()
            .into_iter()
            .map(|c| (c.id.clone(), c))
            .collect()
    }

    pub fn search(&self, search_term: &str) -> Vec<Programme> {
        let search_term = search_term.to_lowercase();
        let time_now = chrono::Utc::now();

        self.programmes
            .iter()
            .filter(|p| {
                p.stop >= time_now
                    && (p.title.to_lowercase().contains(&search_term)
                        || p.desc.to_lowercase().contains(&search_term))
            })
            .sorted_by_key(|p| p.start)
            .cloned()
            .collect()
    }

    pub fn to_xml(&self) -> Result<String, Box<dyn std::error::Error>> {
        let header = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE tv SYSTEM "xmltv.dtd">
<tv generator-info-name="NXT" generator-info-url="nxtplay.xyz">"#;
        let footer = "\n</tv>";
        let mut body = String::new();
        for channel in &self.channels {
            body.push_str(&format!("\n<channel id=\"{}\">", escape_xml(&channel.id)));
            body.push_str(&format!(
                "\n<display-name>{}</display-name>",
                escape_xml(&channel.display_name)
            ));
            if let Some(icon) = &channel.icon {
                body.push_str(&format!("\n<icon src=\"{}\"/>", escape_xml(&icon.src)));
            }
            body.push_str("\n</channel>");
        }
        for programme in &self.programmes {
            body.push_str(&format!(
                "\n<programme start=\"{}\" stop=\"{}\" channel=\"{}\">",
                programme.start.format("%Y%m%d%H%M%S %z"),
                programme.stop.format("%Y%m%d%H%M%S %z"),
                escape_xml(&programme.channel)
            ));
            body.push_str(&format!(
                "\n<title>{}</title>",
                escape_xml(&programme.title)
            ));
            body.push_str(&format!("\n<desc>{}</desc>", escape_xml(&programme.desc)));
            body.push_str("\n</programme>");
        }
        Ok(format!("{}{}{}", header, body, footer))
    }

    pub fn filter_channels(&mut self, channels_to_keep: &[String]) {
        self.channels = self
            .channels
            .iter()
            .filter(|c| channels_to_keep.contains(&c.id))
            .cloned()
            .collect();
        self.programmes = self
            .programmes
            .iter()
            .filter(|p| channels_to_keep.contains(&p.channel))
            .cloned()
            .collect();
    }
}

fn escape_xml(s: &str) -> String {
    s.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("\"", "&quot;")
        .replace("'", "&apos;")
}

fn deserialize_datetime<'de, D>(deserializer: D) -> Result<DateTime<FixedOffset>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    DateTime::parse_from_str(&s, "%Y%m%d%H%M%S %z").map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use crate::{playlist::Playlist, GROUPS_TO_EXCLUDE, SNIPPETS_TO_EXCLUDE};

    use super::*;

    #[test]
    fn test_parse_epg() -> Result<(), Box<dyn std::error::Error>> {
        let file = File::open("examples/mini-epg.xml")?;
        let _ = Epg::from_reader(file)?;
        Ok(())
    }

    #[test]
    fn test_parse_programme_time() {
        let xml = r#"
            <programme start="20241017130900 +0100" stop="20241017140000 +0100" channel="example.com">
                <title>Test Programme</title>
                <desc>Test Description</desc>
            </programme>
        "#;

        let programme: Programme = serde_xml_rs::from_str(xml).unwrap();
        assert_eq!(programme.start.to_rfc3339(), "2024-10-17T13:09:00+01:00");
        assert_eq!(programme.stop.to_rfc3339(), "2024-10-17T14:00:00+01:00");
    }

    #[test]
    fn test_to_xml() -> Result<(), Box<dyn std::error::Error>> {
        let file = File::open("examples/mini-epg.xml")?;
        let epg = Epg::from_reader(file)?;
        let _ = epg.to_xml()?;
        Ok(())
    }
}
