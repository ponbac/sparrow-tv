use std::{io::BufRead, sync::Arc};

use chrono::{DateTime, Datelike, Timelike, Utc};
use quick_xml::{
    XmlVersion,
    events::{BytesDecl, BytesRef, BytesStart, Event},
    reader::Reader,
};

use crate::{
    domain::{EpgFailureKind, SafeFailure, SnapshotOperation, SourceKind, StoreError},
    m3u::normalize_presentation,
};

pub(crate) struct ParsedGuide {
    pub(crate) channels: Vec<ParsedGuideChannel>,
    pub(crate) programmes: Vec<ParsedProgramme>,
}

pub(crate) struct ParsedGuideChannel {
    pub(crate) id: Arc<str>,
    pub(crate) display_names: Vec<Arc<str>>,
}

pub(crate) struct ParsedProgramme {
    pub(crate) guide_channel_id: Arc<str>,
    pub(crate) title: Arc<str>,
    pub(crate) description: Option<Arc<str>>,
    pub(crate) starts_at: DateTime<Utc>,
    pub(crate) ends_at: DateTime<Utc>,
}

pub(crate) fn parse(reader: &mut dyn BufRead) -> Result<ParsedGuide, SafeFailure> {
    let mut parser = GuideParser::default();
    let mut xml = Reader::from_reader(reader);
    xml.config_mut().check_comments = true;
    xml.config_mut().expand_empty_elements = true;
    let mut buffer = Vec::new();

    loop {
        let event = xml.read_event_into(&mut buffer).map_err(map_xml_error)?;
        match event {
            Event::Start(element) => parser.start(&element)?,
            Event::End(_) => parser.end()?,
            Event::Text(text) => parser.text(text.as_ref())?,
            Event::CData(text) => parser.cdata(text.as_ref())?,
            Event::GeneralRef(reference) => parser.reference(&reference)?,
            Event::Decl(declaration) => parser.declaration(&declaration)?,
            Event::DocType(_) => parser.doctype()?,
            Event::Eof => break,
            Event::PI(_) | Event::Comment(_) => parser.misc(),
            Event::Empty(_) => unreachable!("empty XML elements are expanded by the reader"),
        }
        buffer.clear();
    }

    parser.finish()
}

#[derive(Default)]
struct GuideParser {
    depth: usize,
    root_seen: bool,
    root_closed: bool,
    declaration_seen: bool,
    doctype_seen: bool,
    prolog_misc_seen: bool,
    channels: Vec<ParsedGuideChannel>,
    programmes: Vec<ParsedProgramme>,
    record: Option<PendingRecord>,
    capture: Option<CapturedText>,
}

impl GuideParser {
    fn start(&mut self, element: &BytesStart<'_>) -> Result<(), SafeFailure> {
        let name = element.local_name();
        let name = name.as_ref();

        if self.capture.is_some() {
            return Err(invalid_epg(EpgFailureKind::MalformedXml));
        }

        match self.depth {
            0 => {
                validate_attributes(element)?;
                if self.root_seen || self.root_closed || name != "tv" {
                    return Err(invalid_epg(EpgFailureKind::MalformedXml));
                }
                self.root_seen = true;
            }
            1 if name == "channel" => {
                if self.record.is_some() {
                    return Err(invalid_epg(EpgFailureKind::MalformedXml));
                }
                self.record = Some(PendingRecord::Channel(parse_channel(element)?));
            }
            1 if name == "programme" => {
                if self.record.is_some() {
                    return Err(invalid_epg(EpgFailureKind::MalformedXml));
                }
                self.record = Some(PendingRecord::Programme(parse_programme(element)?));
            }
            2 => {
                validate_attributes(element)?;
                self.capture = match (&self.record, name) {
                    (Some(PendingRecord::Channel(_)), "display-name") => {
                        Some(CapturedText::new(CaptureKind::DisplayName))
                    }
                    (Some(PendingRecord::Programme(_)), "title") => {
                        Some(CapturedText::new(CaptureKind::Title))
                    }
                    (Some(PendingRecord::Programme(_)), "desc") => {
                        Some(CapturedText::new(CaptureKind::Description))
                    }
                    _ => None,
                };
            }
            _ => validate_attributes(element)?,
        }

        self.depth = self.depth.saturating_add(1);
        Ok(())
    }

    fn end(&mut self) -> Result<(), SafeFailure> {
        self.depth = self
            .depth
            .checked_sub(1)
            .ok_or_else(|| invalid_epg(EpgFailureKind::MalformedXml))?;

        if self.depth == 2 {
            if let Some(capture) = self.capture.take() {
                self.finish_capture(capture);
            }
        } else if self.depth == 1 {
            if let Some(record) = self.record.take() {
                self.finish_record(record)?;
            }
        } else if self.depth == 0 {
            self.root_closed = true;
        }

        Ok(())
    }

    fn text(&mut self, value: &str) -> Result<(), SafeFailure> {
        if self.depth == 0 && !is_xml_whitespace(value) {
            return Err(invalid_epg(EpgFailureKind::MalformedXml));
        }
        if self.depth == 0 && !self.root_seen && !value.is_empty() {
            self.prolog_misc_seen = true;
        }
        if let Some(capture) = self.capture.as_mut() {
            capture.value.push_str(value);
        }
        Ok(())
    }

    fn cdata(&mut self, value: &str) -> Result<(), SafeFailure> {
        if self.depth == 0 {
            return Err(invalid_epg(EpgFailureKind::MalformedXml));
        }
        self.text(value)
    }

    fn declaration(&mut self, declaration: &BytesDecl<'_>) -> Result<(), SafeFailure> {
        if self.root_seen
            || self.root_closed
            || self.declaration_seen
            || self.doctype_seen
            || self.prolog_misc_seen
        {
            return Err(invalid_epg(EpgFailureKind::MalformedXml));
        }
        let version = declaration
            .version()
            .map_err(|_| invalid_epg(EpgFailureKind::MalformedXml))?;
        if version != "1.0" {
            return Err(invalid_epg(EpgFailureKind::MalformedXml));
        }
        validate_declaration_attributes(declaration)?;
        self.declaration_seen = true;
        Ok(())
    }

    fn doctype(&mut self) -> Result<(), SafeFailure> {
        if self.root_seen || self.root_closed || self.doctype_seen {
            return Err(invalid_epg(EpgFailureKind::MalformedXml));
        }
        self.doctype_seen = true;
        self.prolog_misc_seen = true;
        Ok(())
    }

    fn misc(&mut self) {
        if self.depth == 0 && !self.root_seen {
            self.prolog_misc_seen = true;
        }
    }

    fn reference(&mut self, reference: &BytesRef<'_>) -> Result<(), SafeFailure> {
        if self.depth == 0 {
            return Err(invalid_epg(EpgFailureKind::MalformedXml));
        }
        if let Some(character) = reference
            .resolve_char_ref()
            .map_err(|_| invalid_epg(EpgFailureKind::MalformedXml))?
        {
            if let Some(capture) = self.capture.as_mut() {
                capture.value.push(character);
            }
            return Ok(());
        }

        let resolved = match reference.as_ref() {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "apos" => '\'',
            "quot" => '"',
            _ => return Err(invalid_epg(EpgFailureKind::MalformedXml)),
        };
        if let Some(capture) = self.capture.as_mut() {
            capture.value.push(resolved);
        }
        Ok(())
    }

    fn finish_capture(&mut self, capture: CapturedText) {
        let value = normalize_presentation(&capture.value);
        match (&mut self.record, capture.kind) {
            (Some(PendingRecord::Channel(channel)), CaptureKind::DisplayName)
                if !value.is_empty() =>
            {
                channel.display_names.push(value);
            }
            (Some(PendingRecord::Programme(programme)), CaptureKind::Title)
                if programme.title.is_none() && !value.is_empty() =>
            {
                programme.title = Some(value);
            }
            (Some(PendingRecord::Programme(programme)), CaptureKind::Description)
                if programme.description.is_none() && !value.is_empty() =>
            {
                programme.description = Some(value);
            }
            _ => {}
        }
    }

    fn finish_record(&mut self, record: PendingRecord) -> Result<(), SafeFailure> {
        match record {
            PendingRecord::Channel(channel) => {
                if let Some(id) = channel.id {
                    self.channels.push(ParsedGuideChannel {
                        id: Arc::from(id),
                        display_names: channel.display_names.into_iter().map(Arc::from).collect(),
                    });
                }
            }
            PendingRecord::Programme(programme) => {
                let Some(guide_channel_id) = programme.guide_channel_id else {
                    return Ok(());
                };
                let Some(starts_at) = programme.start.as_deref().and_then(parse_timestamp) else {
                    return Ok(());
                };
                let Some(ends_at) = programme.stop.as_deref().and_then(parse_timestamp) else {
                    return Ok(());
                };
                let Some(title) = programme.title else {
                    return Ok(());
                };
                if ends_at <= starts_at {
                    return Ok(());
                }
                self.programmes.push(ParsedProgramme {
                    guide_channel_id: Arc::from(guide_channel_id),
                    title: Arc::from(title),
                    description: programme.description.map(Arc::from),
                    starts_at,
                    ends_at,
                });
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<ParsedGuide, SafeFailure> {
        if !self.root_seen
            || !self.root_closed
            || self.depth != 0
            || self.record.is_some()
            || self.capture.is_some()
        {
            return Err(invalid_epg(EpgFailureKind::MalformedXml));
        }
        if self.channels.is_empty() {
            return Err(SafeFailure::NoEpgChannels);
        }
        Ok(ParsedGuide {
            channels: self.channels,
            programmes: self.programmes,
        })
    }
}

enum PendingRecord {
    Channel(PendingChannel),
    Programme(PendingProgramme),
}

struct PendingChannel {
    id: Option<String>,
    display_names: Vec<String>,
}

struct PendingProgramme {
    guide_channel_id: Option<String>,
    start: Option<String>,
    stop: Option<String>,
    title: Option<String>,
    description: Option<String>,
}

#[derive(Clone, Copy)]
enum CaptureKind {
    DisplayName,
    Title,
    Description,
}

struct CapturedText {
    kind: CaptureKind,
    value: String,
}

impl CapturedText {
    fn new(kind: CaptureKind) -> Self {
        Self {
            kind,
            value: String::new(),
        }
    }
}

fn parse_channel(element: &BytesStart<'_>) -> Result<PendingChannel, SafeFailure> {
    let mut id = None;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|_| invalid_epg(EpgFailureKind::MalformedXml))?;
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|_| invalid_epg(EpgFailureKind::MalformedXml))?;
        if attribute.key.as_ref() == "id" {
            id = Some(value.trim().to_owned());
        }
    }
    Ok(PendingChannel {
        id: id.filter(|id| !id.is_empty()),
        display_names: Vec::new(),
    })
}

fn parse_programme(element: &BytesStart<'_>) -> Result<PendingProgramme, SafeFailure> {
    let mut start = None;
    let mut stop = None;
    let mut guide_channel_id = None;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|_| invalid_epg(EpgFailureKind::MalformedXml))?;
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|_| invalid_epg(EpgFailureKind::MalformedXml))?;
        match attribute.key.as_ref() {
            "start" => start = Some(value.trim().to_owned()),
            "stop" => stop = Some(value.trim().to_owned()),
            "channel" => guide_channel_id = Some(value.trim().to_owned()),
            _ => {}
        }
    }

    Ok(PendingProgramme {
        guide_channel_id: guide_channel_id.filter(|id| !id.is_empty()),
        start: start.filter(|value| !value.is_empty()),
        stop: stop.filter(|value| !value.is_empty()),
        title: None,
        description: None,
    })
}

fn validate_attributes(element: &BytesStart<'_>) -> Result<(), SafeFailure> {
    for attribute in element.attributes() {
        attribute
            .map_err(|_| invalid_epg(EpgFailureKind::MalformedXml))?
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|_| invalid_epg(EpgFailureKind::MalformedXml))?;
    }
    Ok(())
}

fn validate_declaration_attributes(declaration: &BytesDecl<'_>) -> Result<(), SafeFailure> {
    let element = BytesStart::from_content(declaration.as_ref(), 3);
    let mut position = 0_u8;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|_| invalid_epg(EpgFailureKind::MalformedXml))?;
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|_| invalid_epg(EpgFailureKind::MalformedXml))?;
        position = match (position, attribute.key.as_ref()) {
            (0, "version") if value == "1.0" => 1,
            (1, "encoding") if is_encoding_name(value.as_ref()) => 2,
            (1 | 2, "standalone") if matches!(value.as_ref(), "yes" | "no") => 3,
            _ => return Err(invalid_epg(EpgFailureKind::MalformedXml)),
        };
    }
    (position > 0)
        .then_some(())
        .ok_or_else(|| invalid_epg(EpgFailureKind::MalformedXml))
}

fn is_encoding_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    let timestamp = [
        "%Y%m%d%H%M%S %z",
        "%Y%m%d%H%M%S%z",
        "%Y%m%d%H%M%S %:z",
        "%Y%m%d%H%M%S%:z",
    ]
    .into_iter()
    .find_map(|format| DateTime::parse_from_str(value, format).ok())
    .map(|time| time.with_timezone(&Utc))
    .or_else(|| {
        chrono::NaiveDateTime::parse_from_str(value, "%Y%m%d%H%M%S")
            .ok()
            .map(|time| time.and_utc())
    })?;

    normalize_browser_instant(timestamp)
}

fn normalize_browser_instant(timestamp: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let timestamp = if timestamp.nanosecond() >= 1_000_000_000 {
        let unix_seconds = timestamp.timestamp().checked_add(1)?;
        let nanoseconds = timestamp.nanosecond() - 1_000_000_000;
        DateTime::from_timestamp(unix_seconds, nanoseconds)?
    } else {
        timestamp
    };

    (0..=9999).contains(&timestamp.year()).then_some(timestamp)
}

fn is_xml_whitespace(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
}

fn map_xml_error(error: quick_xml::Error) -> SafeFailure {
    match error {
        quick_xml::Error::Encoding(_) => SafeFailure::InvalidEncoding {
            kind: SourceKind::Epg,
        },
        quick_xml::Error::Io(_) => SafeFailure::Snapshot {
            kind: SourceKind::Epg,
            operation: SnapshotOperation::ReadStage,
            reason: StoreError::Unavailable,
        },
        _ => invalid_epg(EpgFailureKind::MalformedXml),
    }
}

fn invalid_epg(reason: EpgFailureKind) -> SafeFailure {
    SafeFailure::InvalidEpgFormat { reason }
}

#[cfg(test)]
mod tests {
    use chrono::{SecondsFormat, TimeZone, Utc};

    use super::{normalize_browser_instant, parse_timestamp};

    #[test]
    fn leap_seconds_are_carried_into_the_next_utc_instant() {
        let at_utc =
            parse_timestamp("20161231235960 +0000").expect("a leap second at UTC is accepted");
        let across_offset = parse_timestamp("20170101005960 +0100")
            .expect("a leap second with an offset is accepted");

        for timestamp in [at_utc, across_offset] {
            assert_eq!(
                timestamp.to_rfc3339_opts(SecondsFormat::AutoSi, true),
                "2017-01-01T00:00:00Z"
            );
            assert!(timestamp.timestamp_subsec_nanos() < 1_000_000_000);
        }
    }

    #[test]
    fn accepted_timestamps_stay_inside_the_four_digit_browser_year_range() {
        let lower = parse_timestamp("00000101000000 +0000")
            .expect("the lower four-digit year boundary is accepted");
        let upper = parse_timestamp("99991231235959 +0000")
            .expect("the upper four-digit year boundary is accepted");

        assert_eq!(
            lower.to_rfc3339_opts(SecondsFormat::AutoSi, true),
            "0000-01-01T00:00:00Z"
        );
        assert_eq!(
            upper.to_rfc3339_opts(SecondsFormat::AutoSi, true),
            "9999-12-31T23:59:59Z"
        );
    }

    #[test]
    fn utc_conversion_and_leap_carry_cannot_escape_browser_year_bounds() {
        assert!(parse_timestamp("00000101000000 +0100").is_none());
        assert!(parse_timestamp("99991231235959 -0100").is_none());
        assert!(parse_timestamp("99991231235960 +0000").is_none());

        let before_browser_range = Utc
            .with_ymd_and_hms(-1, 12, 31, 23, 59, 59)
            .single()
            .expect("Chrono supports years before the browser contract");
        let after_browser_range = Utc
            .with_ymd_and_hms(10_000, 1, 1, 0, 0, 0)
            .single()
            .expect("Chrono supports years after the browser contract");
        assert!(normalize_browser_instant(before_browser_range).is_none());
        assert!(normalize_browser_instant(after_browser_range).is_none());
    }
}
