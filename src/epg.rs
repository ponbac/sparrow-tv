use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::io::Read;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Epg {
    #[serde(rename = "channel", default)]
    channels: Vec<Channel>,
    #[serde(rename = "programme", default)]
    programmes: Vec<Programme>,
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

        println!("Found {} channels", epg.channels.len());
        println!("Found {} programmes", epg.programmes.len());

        let channel_map: HashMap<String, Channel> = epg
            .clone()
            .channels
            .into_iter()
            .map(|c| (c.id.clone(), c))
            .collect();

        let search_term = "six kings";
        let time_now = time::Instant::now();
        for programme in &epg.programmes {
            if programme.title.to_lowercase().contains(search_term)
                || programme.desc.to_lowercase().contains(search_term)
            {
                let channel = channel_map.get(&programme.channel).unwrap();
                println!(
                    "{} - {}\nStart: {}\nStop: {}",
                    channel.display_name, programme.title, programme.start, programme.stop
                );
            }
        }
        println!(
            "Time taken: {:?} seconds",
            time_now.elapsed().as_seconds_f32()
        );

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
            .cloned()
            .collect()
    }
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
}
