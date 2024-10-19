use serde::Deserialize;
use std::fs::File;
use std::io::BufReader;

#[derive(Debug, Deserialize)]
struct Epg {
    #[serde(rename = "channel", default)]
    channels: Vec<Channel>,
    #[serde(rename = "programme", default)]
    programmes: Vec<Programme>,
}

#[derive(Debug, Deserialize)]
struct Channel {
    #[serde(rename = "id")]
    id: String,
    #[serde(rename = "display-name")]
    display_name: String,
    #[serde(default)]
    icon: Option<Icon>,
}

#[derive(Debug, Deserialize)]
struct Icon {
    #[serde(rename = "src")]
    src: String,
}

#[derive(Debug, Deserialize)]
struct Programme {
    #[serde(rename = "start")]
    start: String,
    #[serde(rename = "stop")]
    stop: String,
    #[serde(rename = "channel")]
    channel: String,
    title: String,
    desc: String,
}

pub fn parse_epg() -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open("examples/epg.xml")?;
    let reader = BufReader::new(file);

    let epg: Epg = serde_xml_rs::from_reader(reader)?;

    println!("Channels:");
    for channel in &epg.channels {
        println!("  ID: {}", channel.id);
        println!("  Name: {}", channel.display_name);
        println!(
            "  Icon: {}",
            channel
                .icon
                .as_ref()
                .map(|icon| &icon.src)
                .unwrap_or(&String::new())
        );
        println!();
    }

    println!("Programmes:");
    for programme in &epg.programmes {
        println!("  Channel: {}", programme.channel);
        println!("  Start: {}", programme.start);
        println!("  Stop: {}", programme.stop);
        println!("  Title: {}", programme.title);
        println!("  Description: {}", programme.desc);
        println!();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_epg() -> Result<(), Box<dyn std::error::Error>> {
        parse_epg()
    }
}
