use serde::Deserialize;
use std::fs::File;
use std::io::BufReader;

#[derive(Debug, Deserialize)]
pub struct EPG {
    pub channels: Vec<Channel>,
}

#[derive(Debug, Deserialize)]
pub struct Channel {
    #[serde(rename = "id")]
    pub id: String,
    pub name: String,
    pub schedule: Vec<Program>,
}

#[derive(Debug, Deserialize)]
pub struct Program {
    pub title: String,
    pub start: String, // You might want to use a DateTime type for better handling
    pub end: String,   // You might want to use a DateTime type for better handling
}

impl EPG {
    pub fn from_xml(file_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let file = File::open(file_path)?;
        let reader = BufReader::new(file);
        let epg = serde_xml_rs::from_reader(reader)?;
        Ok(epg)
    }
}
