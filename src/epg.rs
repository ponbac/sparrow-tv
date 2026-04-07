use chrono::{DateTime, FixedOffset};
use rayon::prelude::*;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, HashSet};
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
    pub fn empty() -> Self {
        Self {
            channels: Vec::new(),
            programmes: Vec::new(),
        }
    }

    pub fn from_reader(reader: impl Read) -> Result<Epg, Box<dyn std::error::Error>> {
        let mut xml = String::new();
        let mut reader = reader;
        reader.read_to_string(&mut xml)?;
        Self::from_xml_str(&xml)
    }

    pub fn from_xml_str(input: &str) -> Result<Epg, Box<dyn std::error::Error>> {
        let input = strip_utf8_bom(input);
        match serde_xml_rs::from_str(input) {
            Ok(epg) => Ok(epg),
            Err(original_error) => {
                let sanitized = sanitize_epg_xml(input);
                if sanitized == input {
                    return Err(Box::new(original_error));
                }

                serde_xml_rs::from_str(&sanitized).map_err(|_| Box::new(original_error) as _)
            }
        }
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

        let mut matching_programmes: Vec<Programme> = self
            .programmes
            .par_iter()
            .filter(|p| {
                p.stop >= time_now
                    && (p.title.to_lowercase().contains(&search_term)
                        || p.desc.to_lowercase().contains(&search_term))
            })
            .cloned()
            .collect();
        matching_programmes.sort_by_key(|p| p.start);

        matching_programmes
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
        let channels_to_keep: HashSet<&String> = channels_to_keep.iter().collect();
        self.channels.retain(|c| channels_to_keep.contains(&c.id));
        self.programmes
            .retain(|p| channels_to_keep.contains(&p.channel));
    }
}

fn strip_utf8_bom(input: &str) -> &str {
    input.strip_prefix('\u{feff}').unwrap_or(input)
}

fn sanitize_epg_xml(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;

    while let Some(relative_start) = input[cursor..].find('<') {
        let start = cursor + relative_start;
        output.push_str(&input[cursor..start]);

        let Some(end) = find_tag_end(input, start) else {
            output.push_str(&input[start..]);
            return output;
        };

        let tag = &input[start..=end];
        output.push_str(&sanitize_tag(tag));
        cursor = end + 1;
    }

    output.push_str(&input[cursor..]);
    output
}

fn find_tag_end(input: &str, start: usize) -> Option<usize> {
    let mut in_quotes = false;
    let mut quote_char = '\0';

    for (offset, ch) in input[start..].char_indices() {
        match ch {
            '"' | '\'' => {
                if in_quotes && ch == quote_char {
                    in_quotes = false;
                    quote_char = '\0';
                } else if !in_quotes {
                    in_quotes = true;
                    quote_char = ch;
                }
            }
            '>' if !in_quotes => return Some(start + offset),
            _ => {}
        }
    }

    None
}

fn sanitize_tag(tag: &str) -> String {
    if tag.len() < 3
        || tag.starts_with("</")
        || tag.starts_with("<?")
        || tag.starts_with("<!")
    {
        return tag.to_string();
    }

    let inner = &tag[1..tag.len() - 1];
    let trimmed = inner.trim();
    let self_closing = trimmed.ends_with('/');
    let content = if self_closing {
        trimmed[..trimmed.len() - 1].trim_end()
    } else {
        trimmed
    };

    let Some(name_end) = content.find(char::is_whitespace) else {
        return tag.to_string();
    };
    let tag_name = &content[..name_end];
    let attrs = &content[name_end..];

    match sanitize_attributes(attrs) {
        Some(sanitized_attrs) => {
            let mut rebuilt = String::from("<");
            rebuilt.push_str(tag_name);
            if !sanitized_attrs.is_empty() {
                rebuilt.push(' ');
                rebuilt.push_str(&sanitized_attrs);
            }
            if self_closing {
                rebuilt.push_str("/>");
            } else {
                rebuilt.push('>');
            }
            rebuilt
        }
        None => tag.to_string(),
    }
}

fn sanitize_attributes(input: &str) -> Option<String> {
    let mut rest = input.trim();
    let mut seen = HashSet::new();
    let mut attributes = Vec::new();

    while !rest.is_empty() {
        let eq_index = rest.find('=')?;
        let key = rest[..eq_index].trim();
        if key.is_empty() {
            return None;
        }

        let mut value_rest = rest[eq_index + 1..].trim_start();
        let quote = value_rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        value_rest = &value_rest[quote.len_utf8()..];
        let end_quote = value_rest.find(quote)?;
        let value = &value_rest[..end_quote];

        if seen.insert(key.to_string()) {
            attributes.push(format!(r#"{key}={quote}{value}{quote}"#));
        }

        rest = value_rest[end_quote + quote.len_utf8()..].trim_start();
    }

    Some(attributes.join(" "))
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
    use super::*;

    const SAMPLE_EPG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE tv SYSTEM "xmltv.dtd">
<tv generator-info-name="NXT" generator-info-url="nxtplay.xyz">
    <channel id="example.com">
        <display-name>Example Channel</display-name>
        <icon src="https://example.com/logo.png"/>
    </channel>
    <programme start="20241017130900 +0100" stop="20241017140000 +0100" channel="example.com">
        <title>Test Programme</title>
        <desc>Test Description</desc>
    </programme>
</tv>"#;

    #[test]
    fn test_parse_epg() -> Result<(), Box<dyn std::error::Error>> {
        let _ = Epg::from_reader(SAMPLE_EPG.as_bytes())?;
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
        let epg = Epg::from_reader(SAMPLE_EPG.as_bytes())?;
        let _ = epg.to_xml()?;
        Ok(())
    }

    #[test]
    fn test_parse_epg_repairs_duplicate_programme_attributes() -> Result<(), Box<dyn std::error::Error>>
    {
        let malformed = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE tv SYSTEM "xmltv.dtd">
<tv generator-info-name="NXT" generator-info-url="nxtplay.bz">
    <channel id="example.com">
        <display-name>Example Channel</display-name>
    </channel>
    <programme start="20241017130900 +0100" stop="20241017140000 +0100" stop="20241017150000 +0100" channel="example.com">
        <title>Test Programme</title>
        <desc>Test Description</desc>
    </programme>
</tv>"#;

        let epg = Epg::from_xml_str(malformed)?;
        assert_eq!(epg.programmes.len(), 1);
        assert_eq!(epg.programmes[0].stop.to_rfc3339(), "2024-10-17T14:00:00+01:00");
        Ok(())
    }
}
