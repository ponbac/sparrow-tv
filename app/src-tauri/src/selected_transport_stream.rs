use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::{self, Debug, Formatter},
    pin::Pin,
    task::{Context, Poll, ready},
    time::Duration,
};

use bytes::{Bytes, BytesMut};
use futures_util::{FutureExt as _, Stream, StreamExt as _};
use serde::Serialize;
use sparrow_source_http::{PlaybackByteStream, PlaybackReadError};

const TS_PACKET_BYTES: usize = 188;
const SYNC_BYTE: u8 = 0x47;
const MAX_DISCOVERY_BYTES: usize = 4 * 1024 * 1024;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SECTION_BYTES: usize = 1024;
const MAX_LABEL_BYTES: usize = 96;
const MAX_AUDIO_TRACKS: usize = 32;
const AUDIO_TRACK_ID_PREFIX: &str = "atrk1_";
const AUDIO_TRACK_ID_HEX_BYTES: usize = 32;
const AUDIO_TRACK_ID_BYTES: usize = AUDIO_TRACK_ID_PREFIX.len() + AUDIO_TRACK_ID_HEX_BYTES;
const AUDIO_TRACK_ID_DOMAIN: &[u8] = b"sparrow-tv/audio-track/v1\0";

/// A restart-stable opaque identity for one elementary audio stream.
#[derive(Clone, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct AudioTrackId(String);

impl AudioTrackId {
    pub(crate) fn parse(value: String) -> Result<Self, TransportStreamError> {
        let Some(digest) = value.strip_prefix(AUDIO_TRACK_ID_PREFIX) else {
            return Err(TransportStreamError::InvalidSelection);
        };
        if value.len() != AUDIO_TRACK_ID_BYTES
            || digest.len() != AUDIO_TRACK_ID_HEX_BYTES
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(TransportStreamError::InvalidSelection);
        }
        Ok(Self(value))
    }

    fn generated(program_number: u16, elementary_pid: u16, stream_type: u8) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(AUDIO_TRACK_ID_DOMAIN);
        hasher.update(&program_number.to_be_bytes());
        hasher.update(&elementary_pid.to_be_bytes());
        hasher.update(&[stream_type]);
        let digest = hasher.finalize();
        Self(format!(
            "{AUDIO_TRACK_ID_PREFIX}{}",
            &digest.to_hex().as_str()[..AUDIO_TRACK_ID_HEX_BYTES]
        ))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Debug for AudioTrackId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("AudioTrackId(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AudioCodec {
    Mpeg1Audio,
    Mpeg2Audio,
    AacAdts,
    AacLatm,
    Ac3,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AudioTrack {
    id: AudioTrackId,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    codec: AudioCodec,
    selected: bool,
}

impl AudioTrack {
    #[cfg(test)]
    pub(crate) fn fixture(
        id: AudioTrackId,
        language: Option<&str>,
        label: Option<&str>,
        codec: AudioCodec,
        selected: bool,
    ) -> Self {
        Self {
            id,
            language: language.map(str::to_owned),
            label: label.map(str::to_owned),
            codec,
            selected,
        }
    }

    #[cfg(test)]
    fn codec(&self) -> AudioCodec {
        self.codec
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "_tag", rename_all = "kebab-case")]
pub(crate) enum AudioSelection {
    None,
    Selected {
        #[serde(rename = "trackId")]
        track_id: AudioTrackId,
        reason: AudioSelectionReason,
    },
    Fallback {
        #[serde(rename = "trackId", skip_serializing_if = "Option::is_none")]
        track_id: Option<AudioTrackId>,
        missing: MissingAudioSelection,
    },
}

impl AudioSelection {
    pub(crate) fn track_id(&self) -> Option<&AudioTrackId> {
        match self {
            Self::None => None,
            Self::Selected { track_id, .. } => Some(track_id),
            Self::Fallback { track_id, .. } => track_id.as_ref(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AudioSelectionReason {
    Requested,
    SavedPreference,
    CurrentSession,
    FirstAvailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MissingAudioSelection {
    Requested,
    SavedPreference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SelectionRequest {
    Initial {
        saved: Option<AudioTrackId>,
    },
    Continue {
        current: Option<AudioTrackId>,
        saved: Option<AudioTrackId>,
    },
    Requested(AudioTrackId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PreferenceStatus {
    Saved,
    NotSaved,
    Unchanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum TransportStreamError {
    #[error("transport stream discovery exceeded its bound")]
    DiscoveryBound,
    #[error("transport stream discovery timed out")]
    DiscoveryTimedOut,
    #[error("transport stream ended before programme discovery")]
    IncompleteProgramme,
    #[error("transport stream programme metadata is invalid")]
    InvalidProgramme,
    #[error("transport stream uses an unsupported programme layout")]
    UnsupportedProgramme,
    #[error("audio track selection is invalid")]
    InvalidSelection,
    #[error("transport stream read was interrupted")]
    Read(#[source] PlaybackReadError),
}

impl TransportStreamError {
    pub(crate) const fn retryable(self) -> bool {
        matches!(
            self,
            Self::DiscoveryBound
                | Self::DiscoveryTimedOut
                | Self::IncompleteProgramme
                | Self::Read(_)
        )
    }

    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::DiscoveryBound => "programme-discovery-bounded",
            Self::DiscoveryTimedOut => "programme-discovery-timed-out",
            Self::IncompleteProgramme => "programme-incomplete",
            Self::InvalidProgramme => "invalid-programme",
            Self::UnsupportedProgramme => "unsupported-programme",
            Self::InvalidSelection => "invalid-audio-track",
            Self::Read(_) => "unavailable",
        }
    }
}

pub(crate) struct OpenedSelectedTransport {
    pub(crate) stream: SelectedTransportStream,
    pub(crate) tracks: Vec<AudioTrack>,
    pub(crate) selection: AudioSelection,
}

/// Discovers one MPEG-TS programme, selects one audio PID, and exposes only a
/// rewritten PAT/PMT plus the programme's supported video, PCR, and selected
/// audio packets. Callers never need to understand PSI or packet framing.
pub(crate) struct SelectedTransportStream {
    body: PlaybackByteStream,
    framer: PacketFramer,
    selector: PacketSelector,
    ready: VecDeque<Bytes>,
}

impl SelectedTransportStream {
    pub(crate) async fn open(
        body: PlaybackByteStream,
        request: SelectionRequest,
    ) -> Result<OpenedSelectedTransport, TransportStreamError> {
        match tokio::time::timeout(DISCOVERY_TIMEOUT, Self::discover(body, request)).await {
            Ok(result) => result,
            Err(_) => Err(TransportStreamError::DiscoveryTimedOut),
        }
    }

    async fn discover(
        mut body: PlaybackByteStream,
        request: SelectionRequest,
    ) -> Result<OpenedSelectedTransport, TransportStreamError> {
        let mut framer = PacketFramer::default();
        let mut discovery = ProgrammeDiscovery::default();
        let mut packets = Vec::new();
        let mut consumed = 0_usize;

        let programme = 'discovery: loop {
            let bytes = match body.next().await {
                Some(Ok(bytes)) if bytes.is_empty() => continue,
                Some(Ok(bytes)) => bytes,
                Some(Err(error)) => return Err(TransportStreamError::Read(error)),
                None => return Err(TransportStreamError::IncompleteProgramme),
            };
            consumed = consumed
                .checked_add(bytes.len())
                .ok_or(TransportStreamError::DiscoveryBound)?;
            if consumed > MAX_DISCOVERY_BYTES {
                return Err(TransportStreamError::DiscoveryBound);
            }
            let mut found = None;
            for packet in framer.push(&bytes) {
                if found.is_none() {
                    found = discovery.inspect(&packet)?;
                }
                packets.push(packet);
            }
            if let Some(programme) = found {
                break 'discovery programme;
            }
        };

        let (selection, selected_pid, tracks) = programme.select_audio(&request);
        let mut selector = PacketSelector::new(&programme, selected_pid)?;
        let initial = selector.filter(&packets);
        let mut ready = VecDeque::new();
        if !initial.is_empty() {
            ready.push_back(Bytes::from(initial));
        }

        Ok(OpenedSelectedTransport {
            stream: Self {
                body,
                framer,
                selector,
                ready,
            },
            tracks,
            selection,
        })
    }
}

impl Stream for SelectedTransportStream {
    type Item = Result<Bytes, PlaybackReadError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(bytes) = self.ready.pop_front() {
            return Poll::Ready(Some(Ok(bytes)));
        }

        loop {
            match ready!(self.body.next().poll_unpin(context)) {
                Some(Ok(bytes)) if bytes.is_empty() => continue,
                Some(Ok(bytes)) => {
                    let packets = self.framer.push(&bytes);
                    let filtered = self.selector.filter(&packets);
                    if filtered.is_empty() {
                        continue;
                    }
                    return Poll::Ready(Some(Ok(Bytes::from(filtered))));
                }
                Some(Err(error)) => return Poll::Ready(Some(Err(error))),
                None => return Poll::Ready(None),
            }
        }
    }
}

#[derive(Default)]
struct PacketFramer {
    bytes: BytesMut,
    synchronized: bool,
}

impl PacketFramer {
    fn push(&mut self, bytes: &[u8]) -> Vec<[u8; TS_PACKET_BYTES]> {
        self.bytes.extend_from_slice(bytes);
        let mut packets = Vec::new();
        loop {
            if !self.synchronized {
                let Some(offset) = sync_offset(&self.bytes) else {
                    let retain = self.bytes.len().min(TS_PACKET_BYTES * 2);
                    let discard = self.bytes.len() - retain;
                    let _ = self.bytes.split_to(discard);
                    break;
                };
                let _ = self.bytes.split_to(offset);
                self.synchronized = true;
            }
            if self.bytes.len() < TS_PACKET_BYTES {
                break;
            }
            if self.bytes[0] != SYNC_BYTE
                || (self.bytes.len() > TS_PACKET_BYTES && self.bytes[TS_PACKET_BYTES] != SYNC_BYTE)
            {
                let _ = self.bytes.split_to(1);
                self.synchronized = false;
                continue;
            }
            let bytes = self.bytes.split_to(TS_PACKET_BYTES);
            let mut packet = [0_u8; TS_PACKET_BYTES];
            packet.copy_from_slice(&bytes);
            packets.push(packet);
        }
        packets
    }
}

fn sync_offset(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < TS_PACKET_BYTES * 2 + 1 {
        return None;
    }
    (0..=bytes.len() - (TS_PACKET_BYTES * 2 + 1)).find(|offset| {
        bytes[*offset] == SYNC_BYTE
            && bytes[*offset + TS_PACKET_BYTES] == SYNC_BYTE
            && bytes[*offset + TS_PACKET_BYTES * 2] == SYNC_BYTE
    })
}

#[derive(Default)]
struct ProgrammeDiscovery {
    pat: SectionAssembler,
    programmes: Vec<ProgramReference>,
    pmts: HashMap<u16, SectionAssembler>,
    transport_stream_id: Option<u16>,
    pat_version: u8,
}

impl ProgrammeDiscovery {
    fn inspect(
        &mut self,
        packet: &[u8; TS_PACKET_BYTES],
    ) -> Result<Option<Programme>, TransportStreamError> {
        let header = PacketHeader::parse(packet)?;
        if header.pid == 0 {
            for section in self.pat.ingest(header, packet)? {
                let pat = parse_pat(&section)?;
                self.transport_stream_id = Some(pat.transport_stream_id);
                self.pat_version = pat.version;
                self.programmes = pat.programmes;
                self.pmts.retain(|pid, _| {
                    self.programmes
                        .iter()
                        .any(|programme| programme.pmt_pid == *pid)
                });
                for programme in &self.programmes {
                    self.pmts.entry(programme.pmt_pid).or_default();
                }
            }
            return Ok(None);
        }

        let Some(assembler) = self.pmts.get_mut(&header.pid) else {
            return Ok(None);
        };
        for section in assembler.ingest(header, packet)? {
            let Some(reference) = self
                .programmes
                .iter()
                .find(|programme| programme.pmt_pid == header.pid)
            else {
                continue;
            };
            let mut programme = parse_pmt(&section, reference.pmt_pid)?;
            programme.transport_stream_id = self
                .transport_stream_id
                .ok_or(TransportStreamError::InvalidProgramme)?;
            programme.pat_version = self.pat_version;
            if programme.program_number == reference.program_number
                && programme.has_supported_video()
            {
                return Ok(Some(programme));
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Copy)]
struct PacketHeader<'a> {
    pid: u16,
    payload_unit_start: bool,
    continuity: u8,
    payload: Option<&'a [u8]>,
}

impl<'a> PacketHeader<'a> {
    fn parse(packet: &'a [u8; TS_PACKET_BYTES]) -> Result<Self, TransportStreamError> {
        if packet[0] != SYNC_BYTE || packet[1] & 0x80 != 0 {
            return Err(TransportStreamError::InvalidProgramme);
        }
        let pid = (u16::from(packet[1] & 0x1f) << 8) | u16::from(packet[2]);
        let payload_unit_start = packet[1] & 0x40 != 0;
        let adaptation_control = (packet[3] >> 4) & 0x03;
        if adaptation_control == 0 {
            return Err(TransportStreamError::InvalidProgramme);
        }
        let has_payload = adaptation_control & 0x01 != 0;
        let has_adaptation = adaptation_control & 0x02 != 0;
        let mut offset = 4_usize;
        if has_adaptation {
            let adaptation_length = usize::from(packet[offset]);
            offset = offset
                .checked_add(1 + adaptation_length)
                .ok_or(TransportStreamError::InvalidProgramme)?;
            if offset > TS_PACKET_BYTES {
                return Err(TransportStreamError::InvalidProgramme);
            }
        }
        let payload = (has_payload && offset < TS_PACKET_BYTES).then_some(&packet[offset..]);
        Ok(Self {
            pid,
            payload_unit_start,
            continuity: packet[3] & 0x0f,
            payload,
        })
    }
}

#[derive(Default)]
struct SectionAssembler {
    bytes: Vec<u8>,
    expected: Option<usize>,
    continuity: Option<u8>,
}

impl SectionAssembler {
    fn ingest(
        &mut self,
        header: PacketHeader<'_>,
        _packet: &[u8; TS_PACKET_BYTES],
    ) -> Result<Vec<Vec<u8>>, TransportStreamError> {
        let Some(payload) = header.payload else {
            return Ok(Vec::new());
        };
        if self
            .continuity
            .is_some_and(|previous| header.continuity != (previous + 1) & 0x0f)
        {
            self.reset();
        }
        self.continuity = Some(header.continuity);

        let mut completed = Vec::new();
        if header.payload_unit_start {
            let Some((&pointer, remainder)) = payload.split_first() else {
                return Err(TransportStreamError::InvalidProgramme);
            };
            let pointer = usize::from(pointer);
            if pointer > remainder.len() {
                return Err(TransportStreamError::InvalidProgramme);
            }
            if !self.bytes.is_empty() && pointer > 0 {
                self.consume(&remainder[..pointer], &mut completed)?;
            }
            self.reset_section();
            self.consume(&remainder[pointer..], &mut completed)?;
        } else if !self.bytes.is_empty() {
            self.consume(payload, &mut completed)?;
        }
        Ok(completed)
    }

    fn consume(
        &mut self,
        mut bytes: &[u8],
        completed: &mut Vec<Vec<u8>>,
    ) -> Result<(), TransportStreamError> {
        while !bytes.is_empty() {
            if self.bytes.is_empty() && bytes[0] == 0xff {
                return Ok(());
            }
            let needed_for_header = 3_usize.saturating_sub(self.bytes.len());
            if needed_for_header > 0 {
                let take = needed_for_header.min(bytes.len());
                self.bytes.extend_from_slice(&bytes[..take]);
                bytes = &bytes[take..];
                if self.bytes.len() < 3 {
                    return Ok(());
                }
                let section_length =
                    (usize::from(self.bytes[1] & 0x0f) << 8) | usize::from(self.bytes[2]);
                let expected = 3 + section_length;
                if !(8..=MAX_SECTION_BYTES).contains(&expected) {
                    self.reset_section();
                    return Err(TransportStreamError::InvalidProgramme);
                }
                self.expected = Some(expected);
            }
            let expected = self.expected.expect("section header establishes its size");
            let take = (expected - self.bytes.len()).min(bytes.len());
            self.bytes.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.bytes.len() == expected {
                completed.push(std::mem::take(&mut self.bytes));
                self.expected = None;
            }
        }
        Ok(())
    }

    fn reset_section(&mut self) {
        self.bytes.clear();
        self.expected = None;
    }

    fn reset(&mut self) {
        self.reset_section();
        self.continuity = None;
    }
}

struct Pat {
    transport_stream_id: u16,
    version: u8,
    programmes: Vec<ProgramReference>,
}

#[derive(Clone, Copy)]
struct ProgramReference {
    program_number: u16,
    pmt_pid: u16,
}

fn parse_pat(section: &[u8]) -> Result<Pat, TransportStreamError> {
    validate_section(section, 0x00)?;
    if section.len() < 12 || section[5] & 0x01 == 0 {
        return Err(TransportStreamError::InvalidProgramme);
    }
    let entries = &section[8..section.len() - 4];
    let (entries, remainder) = entries.as_chunks::<4>();
    if !remainder.is_empty() {
        return Err(TransportStreamError::InvalidProgramme);
    }
    let transport_stream_id = u16::from_be_bytes([section[3], section[4]]);
    let version = (section[5] >> 1) & 0x1f;
    let mut programmes = Vec::new();
    let mut program_numbers = HashSet::new();
    let mut pmt_pids = HashSet::new();
    for entry in entries {
        let program_number = u16::from_be_bytes([entry[0], entry[1]]);
        if program_number == 0 {
            continue;
        }
        let pmt_pid = (u16::from(entry[2] & 0x1f) << 8) | u16::from(entry[3]);
        if pmt_pid == 0
            || pmt_pid == 0x1fff
            || !program_numbers.insert(program_number)
            || !pmt_pids.insert(pmt_pid)
        {
            return Err(TransportStreamError::InvalidProgramme);
        }
        programmes.push(ProgramReference {
            program_number,
            pmt_pid,
        });
    }
    if programmes.is_empty() {
        return Err(TransportStreamError::InvalidProgramme);
    }
    Ok(Pat {
        transport_stream_id,
        version,
        programmes,
    })
}

struct Programme {
    transport_stream_id: u16,
    pat_version: u8,
    program_number: u16,
    pmt_pid: u16,
    pcr_pid: u16,
    version: u8,
    program_descriptors: Vec<u8>,
    streams: Vec<ElementaryStream>,
}

impl Programme {
    fn has_supported_video(&self) -> bool {
        self.streams
            .iter()
            .any(|stream| matches!(stream.kind, StreamKind::Video))
    }

    fn select_audio(
        &self,
        request: &SelectionRequest,
    ) -> (AudioSelection, Option<u16>, Vec<AudioTrack>) {
        let available = self
            .streams
            .iter()
            .filter_map(|stream| match stream.kind {
                StreamKind::Audio(codec) => Some((stream, codec)),
                StreamKind::Video | StreamKind::Other => None,
            })
            .collect::<Vec<_>>();
        if available.is_empty() {
            return (AudioSelection::None, None, Vec::new());
        }

        let find = |id: &AudioTrackId| {
            available.iter().position(|(stream, _)| {
                AudioTrackId::generated(self.program_number, stream.pid, stream.stream_type) == *id
            })
        };
        let (selected, selection) = match request {
            SelectionRequest::Initial { saved: Some(saved) } => match find(saved) {
                Some(index) => (
                    index,
                    AudioSelection::Selected {
                        track_id: saved.clone(),
                        reason: AudioSelectionReason::SavedPreference,
                    },
                ),
                None => {
                    let id = track_id(self.program_number, available[0].0);
                    (
                        0,
                        AudioSelection::Fallback {
                            track_id: Some(id),
                            missing: MissingAudioSelection::SavedPreference,
                        },
                    )
                }
            },
            SelectionRequest::Initial { saved: None } => {
                let id = track_id(self.program_number, available[0].0);
                (
                    0,
                    AudioSelection::Selected {
                        track_id: id,
                        reason: AudioSelectionReason::FirstAvailable,
                    },
                )
            }
            SelectionRequest::Continue {
                current: Some(current),
                ..
            } if find(current).is_some() => (
                find(current).expect("guard established current track"),
                AudioSelection::Selected {
                    track_id: current.clone(),
                    reason: AudioSelectionReason::CurrentSession,
                },
            ),
            SelectionRequest::Continue {
                saved: Some(saved), ..
            } => match find(saved) {
                Some(index) => (
                    index,
                    AudioSelection::Selected {
                        track_id: saved.clone(),
                        reason: AudioSelectionReason::SavedPreference,
                    },
                ),
                None => {
                    let id = track_id(self.program_number, available[0].0);
                    (
                        0,
                        AudioSelection::Fallback {
                            track_id: Some(id),
                            missing: MissingAudioSelection::SavedPreference,
                        },
                    )
                }
            },
            SelectionRequest::Continue { saved: None, .. } => {
                let id = track_id(self.program_number, available[0].0);
                (
                    0,
                    AudioSelection::Selected {
                        track_id: id,
                        reason: AudioSelectionReason::FirstAvailable,
                    },
                )
            }
            SelectionRequest::Requested(requested) => match find(requested) {
                Some(index) => (
                    index,
                    AudioSelection::Selected {
                        track_id: requested.clone(),
                        reason: AudioSelectionReason::Requested,
                    },
                ),
                None => {
                    let id = track_id(self.program_number, available[0].0);
                    (
                        0,
                        AudioSelection::Fallback {
                            track_id: Some(id),
                            missing: MissingAudioSelection::Requested,
                        },
                    )
                }
            },
        };

        let tracks = available
            .iter()
            .enumerate()
            .map(|(index, (stream, codec))| AudioTrack {
                id: track_id(self.program_number, stream),
                language: stream.language.clone(),
                label: stream.label.clone(),
                codec: *codec,
                selected: index == selected,
            })
            .collect();
        (selection, Some(available[selected].0.pid), tracks)
    }
}

fn track_id(program_number: u16, stream: &ElementaryStream) -> AudioTrackId {
    AudioTrackId::generated(program_number, stream.pid, stream.stream_type)
}

struct ElementaryStream {
    stream_type: u8,
    pid: u16,
    descriptors: Vec<u8>,
    kind: StreamKind,
    language: Option<String>,
    label: Option<String>,
}

#[derive(Clone, Copy)]
enum StreamKind {
    Video,
    Audio(AudioCodec),
    Other,
}

fn parse_pmt(section: &[u8], pmt_pid: u16) -> Result<Programme, TransportStreamError> {
    validate_section(section, 0x02)?;
    if section.len() < 16 || section[5] & 0x01 == 0 {
        return Err(TransportStreamError::InvalidProgramme);
    }
    let program_number = u16::from_be_bytes([section[3], section[4]]);
    let version = (section[5] >> 1) & 0x1f;
    let pcr_pid = (u16::from(section[8] & 0x1f) << 8) | u16::from(section[9]);
    let program_info_length = (usize::from(section[10] & 0x0f) << 8) | usize::from(section[11]);
    let crc_start = section.len() - 4;
    let streams_start = 12_usize
        .checked_add(program_info_length)
        .ok_or(TransportStreamError::InvalidProgramme)?;
    if streams_start > crc_start {
        return Err(TransportStreamError::InvalidProgramme);
    }
    let program_descriptors = section[12..streams_start].to_vec();
    validate_descriptors(&program_descriptors)?;

    let mut streams = Vec::new();
    let mut stream_pids = HashSet::new();
    let mut offset = streams_start;
    while offset < crc_start {
        if crc_start - offset < 5 {
            return Err(TransportStreamError::InvalidProgramme);
        }
        let stream_type = section[offset];
        let pid = (u16::from(section[offset + 1] & 0x1f) << 8) | u16::from(section[offset + 2]);
        let info_length =
            (usize::from(section[offset + 3] & 0x0f) << 8) | usize::from(section[offset + 4]);
        let descriptor_start = offset + 5;
        let descriptor_end = descriptor_start
            .checked_add(info_length)
            .ok_or(TransportStreamError::InvalidProgramme)?;
        if descriptor_end > crc_start || pid == 0 || pid == 0x1fff || !stream_pids.insert(pid) {
            return Err(TransportStreamError::InvalidProgramme);
        }
        let descriptors = section[descriptor_start..descriptor_end].to_vec();
        validate_descriptors(&descriptors)?;
        let kind = classify_stream(stream_type, &descriptors);
        let (language, label) = audio_metadata(&descriptors);
        streams.push(ElementaryStream {
            stream_type,
            pid,
            descriptors,
            kind,
            language,
            label,
        });
        offset = descriptor_end;
    }
    if streams.is_empty() {
        return Err(TransportStreamError::InvalidProgramme);
    }
    if streams
        .iter()
        .filter(|stream| matches!(stream.kind, StreamKind::Audio(_)))
        .count()
        > MAX_AUDIO_TRACKS
    {
        return Err(TransportStreamError::UnsupportedProgramme);
    }
    Ok(Programme {
        transport_stream_id: 0,
        pat_version: 0,
        program_number,
        pmt_pid,
        pcr_pid,
        version,
        program_descriptors,
        streams,
    })
}

fn validate_section(section: &[u8], table_id: u8) -> Result<(), TransportStreamError> {
    if section.len() < 8
        || section[0] != table_id
        || section[1] & 0x80 == 0
        || 3 + ((usize::from(section[1] & 0x0f) << 8) | usize::from(section[2])) != section.len()
        || mpeg_crc32(section) != 0
    {
        return Err(TransportStreamError::InvalidProgramme);
    }
    Ok(())
}

fn validate_descriptors(mut descriptors: &[u8]) -> Result<(), TransportStreamError> {
    while !descriptors.is_empty() {
        if descriptors.len() < 2 {
            return Err(TransportStreamError::InvalidProgramme);
        }
        let length = usize::from(descriptors[1]);
        if descriptors.len() < 2 + length {
            return Err(TransportStreamError::InvalidProgramme);
        }
        descriptors = &descriptors[2 + length..];
    }
    Ok(())
}

fn classify_stream(stream_type: u8, descriptors: &[u8]) -> StreamKind {
    match stream_type {
        0x1b | 0x24 => StreamKind::Video,
        0x03 => StreamKind::Audio(AudioCodec::Mpeg1Audio),
        0x04 => StreamKind::Audio(AudioCodec::Mpeg2Audio),
        0x0f => StreamKind::Audio(AudioCodec::AacAdts),
        0x11 => StreamKind::Audio(AudioCodec::AacLatm),
        0x81 => StreamKind::Audio(AudioCodec::Ac3),
        0x06 if has_ac3_descriptor(descriptors) => StreamKind::Audio(AudioCodec::Ac3),
        _ => StreamKind::Other,
    }
}

fn has_ac3_descriptor(mut descriptors: &[u8]) -> bool {
    while descriptors.len() >= 2 {
        let tag = descriptors[0];
        let length = usize::from(descriptors[1]);
        if descriptors.len() < 2 + length {
            return false;
        }
        let payload = &descriptors[2..2 + length];
        if tag == 0x6a || (tag == 0x05 && payload.starts_with(b"AC-3")) {
            return true;
        }
        descriptors = &descriptors[2 + length..];
    }
    false
}

fn audio_metadata(mut descriptors: &[u8]) -> (Option<String>, Option<String>) {
    let mut language = None;
    let mut label = None;
    while descriptors.len() >= 2 {
        let tag = descriptors[0];
        let length = usize::from(descriptors[1]);
        if descriptors.len() < 2 + length {
            break;
        }
        let payload = &descriptors[2..2 + length];
        match tag {
            0x0a if language.is_none() && payload.len() >= 3 => {
                language = iso_language(&payload[..3]);
            }
            0x50 if payload.len() >= 6 => {
                if language.is_none() {
                    language = iso_language(&payload[3..6]);
                }
                if label.is_none() {
                    label = safe_label(&payload[6..]);
                }
            }
            _ => {}
        }
        descriptors = &descriptors[2 + length..];
    }
    (language, label)
}

fn iso_language(bytes: &[u8]) -> Option<String> {
    (bytes.len() == 3 && bytes.iter().all(u8::is_ascii_alphabetic)).then(|| {
        bytes
            .iter()
            .map(|byte| char::from(byte.to_ascii_lowercase()))
            .collect()
    })
}

fn safe_label(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    if text.is_empty() || text.bytes().any(|byte| byte.is_ascii_control()) {
        return None;
    }
    let mut label = String::new();
    for character in text.chars() {
        if label.len() + character.len_utf8() > MAX_LABEL_BYTES {
            break;
        }
        label.push(character);
    }
    (!label.is_empty()).then_some(label)
}

struct PacketSelector {
    pmt_pid: u16,
    retained_pids: HashSet<u16>,
    pat: Vec<u8>,
    pmt: Vec<u8>,
    pat_continuity: u8,
    pmt_continuity: u8,
}

impl PacketSelector {
    fn new(
        programme: &Programme,
        selected_audio_pid: Option<u16>,
    ) -> Result<Self, TransportStreamError> {
        let mut retained_streams = programme
            .streams
            .iter()
            .filter(|stream| {
                matches!(stream.kind, StreamKind::Video)
                    || selected_audio_pid.is_some_and(|pid| stream.pid == pid)
            })
            .collect::<Vec<_>>();
        if let Some(pcr_stream) = programme
            .streams
            .iter()
            .find(|stream| stream.pid == programme.pcr_pid)
        {
            if matches!(pcr_stream.kind, StreamKind::Audio(_))
                && selected_audio_pid != Some(pcr_stream.pid)
            {
                return Err(TransportStreamError::UnsupportedProgramme);
            }
            if !retained_streams
                .iter()
                .any(|stream| stream.pid == pcr_stream.pid)
            {
                retained_streams.push(pcr_stream);
            }
        }
        let mut retained_pids = retained_streams
            .iter()
            .map(|stream| stream.pid)
            .collect::<HashSet<_>>();
        if programme.pcr_pid != 0x1fff {
            retained_pids.insert(programme.pcr_pid);
        }
        let pat = rewritten_pat(programme);
        let pmt = rewritten_pmt(programme, &retained_streams)?;
        Ok(Self {
            pmt_pid: programme.pmt_pid,
            retained_pids,
            pat,
            pmt,
            pat_continuity: 0,
            pmt_continuity: 0,
        })
    }

    fn filter(&mut self, packets: &[[u8; TS_PACKET_BYTES]]) -> Vec<u8> {
        let mut output = Vec::new();
        for packet in packets {
            let Ok(header) = PacketHeader::parse(packet) else {
                continue;
            };
            if header.pid == 0 {
                if header.payload_unit_start {
                    packetize_section(0, &self.pat, &mut self.pat_continuity, &mut output);
                }
            } else if header.pid == self.pmt_pid {
                if header.payload_unit_start {
                    packetize_section(
                        self.pmt_pid,
                        &self.pmt,
                        &mut self.pmt_continuity,
                        &mut output,
                    );
                }
            } else if self.retained_pids.contains(&header.pid) {
                output.extend_from_slice(packet);
            }
        }
        output
    }
}

fn rewritten_pat(programme: &Programme) -> Vec<u8> {
    let mut section = vec![
        0x00,
        0xb0,
        0x0d,
        (programme.transport_stream_id >> 8) as u8,
        programme.transport_stream_id as u8,
        0xc1 | (programme.pat_version << 1),
        0x00,
        0x00,
        (programme.program_number >> 8) as u8,
        programme.program_number as u8,
        0xe0 | ((programme.pmt_pid >> 8) as u8 & 0x1f),
        programme.pmt_pid as u8,
    ];
    append_crc(&mut section);
    section
}

fn rewritten_pmt(
    programme: &Programme,
    retained_streams: &[&ElementaryStream],
) -> Result<Vec<u8>, TransportStreamError> {
    let streams_length = retained_streams
        .iter()
        .try_fold(0_usize, |length, stream| {
            length.checked_add(5 + stream.descriptors.len())
        })
        .ok_or(TransportStreamError::UnsupportedProgramme)?;
    let section_length = 9_usize
        .checked_add(programme.program_descriptors.len())
        .and_then(|length| length.checked_add(streams_length))
        .and_then(|length| length.checked_add(4))
        .ok_or(TransportStreamError::UnsupportedProgramme)?;
    if section_length > 0x03ff {
        return Err(TransportStreamError::UnsupportedProgramme);
    }
    let mut section = Vec::with_capacity(3 + section_length);
    section.extend_from_slice(&[
        0x02,
        0xb0 | ((section_length >> 8) as u8 & 0x0f),
        section_length as u8,
        (programme.program_number >> 8) as u8,
        programme.program_number as u8,
        0xc1 | (programme.version << 1),
        0x00,
        0x00,
        0xe0 | ((programme.pcr_pid >> 8) as u8 & 0x1f),
        programme.pcr_pid as u8,
        0xf0 | ((programme.program_descriptors.len() >> 8) as u8 & 0x0f),
        programme.program_descriptors.len() as u8,
    ]);
    section.extend_from_slice(&programme.program_descriptors);
    for stream in retained_streams {
        section.extend_from_slice(&[
            stream.stream_type,
            0xe0 | ((stream.pid >> 8) as u8 & 0x1f),
            stream.pid as u8,
            0xf0 | ((stream.descriptors.len() >> 8) as u8 & 0x0f),
            stream.descriptors.len() as u8,
        ]);
        section.extend_from_slice(&stream.descriptors);
    }
    append_crc(&mut section);
    Ok(section)
}

fn packetize_section(pid: u16, section: &[u8], continuity: &mut u8, output: &mut Vec<u8>) {
    let mut remaining = section;
    let mut first = true;
    while !remaining.is_empty() {
        let mut packet = [0xff_u8; TS_PACKET_BYTES];
        packet[0] = SYNC_BYTE;
        packet[1] = ((pid >> 8) as u8 & 0x1f) | if first { 0x40 } else { 0x00 };
        packet[2] = pid as u8;
        packet[3] = 0x10 | *continuity;
        *continuity = (*continuity + 1) & 0x0f;
        let mut offset = 4;
        if first {
            packet[offset] = 0;
            offset += 1;
            first = false;
        }
        let take = remaining.len().min(TS_PACKET_BYTES - offset);
        packet[offset..offset + take].copy_from_slice(&remaining[..take]);
        remaining = &remaining[take..];
        output.extend_from_slice(&packet);
    }
}

fn append_crc(section: &mut Vec<u8>) {
    let checksum = mpeg_crc32(section);
    section.extend_from_slice(&checksum.to_be_bytes());
}

fn mpeg_crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures_util::stream;

    #[test]
    fn opaque_track_ids_are_stable_and_refined() {
        let first = AudioTrackId::generated(1, 0x102, 0x0f);
        assert_eq!(first, AudioTrackId::generated(1, 0x102, 0x0f));
        assert_ne!(first, AudioTrackId::generated(1, 0x103, 0x0f));
        assert!(AudioTrackId::parse(first.as_str().to_owned()).is_ok());
        assert!(AudioTrackId::parse("atrk1_private".to_owned()).is_err());
        assert!(!format!("{first:?}").contains(first.as_str()));
    }

    #[test]
    fn arbitrary_chunks_reassemble_only_after_three_sync_bytes() {
        let packet = fixture_packet(0x101, false, 0, &[1, 2, 3]);
        let mut bytes = vec![9, 8, 7];
        bytes.extend_from_slice(&packet);
        bytes.extend_from_slice(&packet);
        bytes.extend_from_slice(&packet);
        let mut framer = PacketFramer::default();
        assert!(framer.push(&bytes[..190]).is_empty());
        let framed = framer.push(&bytes[190..]);
        assert_eq!(framed, vec![packet, packet, packet]);
    }

    #[test]
    fn fixture_enumerates_metadata_codecs_and_visible_saved_fallback() {
        let programme = fixture_programme();
        let missing = AudioTrackId::generated(1, 0x1fe, 0x0f);
        let (selection, pid, tracks) = programme.select_audio(&SelectionRequest::Initial {
            saved: Some(missing),
        });
        assert_eq!(pid, Some(0x102));
        assert!(matches!(
            selection,
            AudioSelection::Fallback {
                missing: MissingAudioSelection::SavedPreference,
                ..
            }
        ));
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0].language.as_deref(), Some("eng"));
        assert_eq!(tracks[0].label.as_deref(), Some("Original"));
        assert_eq!(tracks[0].codec(), AudioCodec::AacAdts);
        assert_eq!(tracks[1].codec(), AudioCodec::Mpeg2Audio);
        assert_eq!(tracks[2].codec(), AudioCodec::Ac3);
        assert!(tracks[0].selected);
    }

    #[test]
    fn explicit_selection_rewrites_psi_and_drops_every_other_audio_pid() {
        let programme = fixture_programme();
        let selected = AudioTrackId::generated(1, 0x103, 0x04);
        let (_, pid, tracks) = programme.select_audio(&SelectionRequest::Requested(selected));
        assert_eq!(pid, Some(0x103));
        assert!(tracks[1].selected);

        let mut selector = PacketSelector::new(&programme, pid).expect("fixture programme filters");
        let packets = vec![
            fixture_packet(0, true, 0, &[0]),
            fixture_packet(0x100, true, 0, &[0]),
            fixture_packet(0x101, false, 0, &[1]),
            fixture_packet(0x102, false, 0, &[2]),
            fixture_packet(0x103, false, 0, &[3]),
            fixture_packet(0x104, false, 0, &[4]),
        ];
        let output = selector.filter(&packets);
        let pids = output
            .as_chunks::<TS_PACKET_BYTES>()
            .0
            .iter()
            .map(|packet| (u16::from(packet[1] & 0x1f) << 8) | u16::from(packet[2]))
            .collect::<Vec<_>>();
        assert_eq!(pids, vec![0, 0x100, 0x101, 0x103]);

        let pmt_packet = output
            .as_chunks::<TS_PACKET_BYTES>()
            .0
            .iter()
            .find(|packet| ((u16::from(packet[1] & 0x1f) << 8) | u16::from(packet[2])) == 0x100)
            .expect("rewritten PMT is present");
        let section_length = (usize::from(pmt_packet[6] & 0x0f) << 8) | usize::from(pmt_packet[7]);
        let pmt = &pmt_packet[5..5 + 3 + section_length];
        assert_eq!(mpeg_crc32(pmt), 0);
        let rewritten = parse_pmt(pmt, 0x100).expect("rewritten PMT remains valid");
        assert_eq!(rewritten.streams.len(), 2);
        assert!(rewritten.streams.iter().any(|stream| stream.pid == 0x101));
        assert!(rewritten.streams.iter().any(|stream| stream.pid == 0x103));
    }

    #[test]
    fn pcr_on_a_deselected_audio_track_is_rejected() {
        let mut programme = fixture_programme();
        programme.pcr_pid = 0x102;
        assert!(matches!(
            PacketSelector::new(&programme, Some(0x103)),
            Err(TransportStreamError::UnsupportedProgramme)
        ));
    }

    #[tokio::test]
    async fn fragmented_fixture_discovers_tracks_and_keeps_packets_after_the_pmt() {
        let programme = fixture_programme();
        let bytes = fixture_transport(&programme);
        let chunks = [
            Bytes::copy_from_slice(&bytes[..37]),
            Bytes::copy_from_slice(&bytes[37..91]),
            Bytes::copy_from_slice(&bytes[91..]),
        ];
        let body: PlaybackByteStream = Box::pin(stream::iter(
            chunks.into_iter().map(Ok::<_, PlaybackReadError>),
        ));

        let opened = SelectedTransportStream::open(body, SelectionRequest::Initial { saved: None })
            .await
            .expect("fixture programme is discovered");

        assert_eq!(opened.tracks.len(), 3);
        assert_eq!(opened.tracks[0].language.as_deref(), Some("eng"));
        assert_eq!(opened.tracks[0].label.as_deref(), Some("Original"));
        assert!(opened.tracks[0].selected);
        assert!(matches!(
            opened.selection,
            AudioSelection::Selected {
                reason: AudioSelectionReason::FirstAvailable,
                ..
            }
        ));

        let output = opened
            .stream
            .ready
            .front()
            .expect("discovery emits mandatory PSI and retained packets");
        let pids = packet_pids(output);
        assert_eq!(pids, vec![0, 0x100, 0x101, 0x102]);

        let pat_packet = output
            .as_chunks::<TS_PACKET_BYTES>()
            .0
            .iter()
            .find(|packet| packet_pid(packet) == 0)
            .expect("rewritten PAT exists");
        let pat = section_from_single_packet(pat_packet);
        let parsed_pat = parse_pat(pat).expect("rewritten PAT is valid");
        assert_eq!(parsed_pat.transport_stream_id, 0x1234);
        assert_eq!(parsed_pat.version, 2);
    }

    #[tokio::test]
    async fn requested_track_filters_to_exactly_one_audio_pid() {
        let programme = fixture_programme();
        let selected = AudioTrackId::generated(1, 0x103, 0x04);
        let body: PlaybackByteStream = Box::pin(stream::iter([Ok::<_, PlaybackReadError>(
            Bytes::from(fixture_transport(&programme)),
        )]));

        let opened =
            SelectedTransportStream::open(body, SelectionRequest::Requested(selected.clone()))
                .await
                .expect("requested track opens");

        assert!(matches!(
            opened.selection,
            AudioSelection::Selected {
                track_id,
                reason: AudioSelectionReason::Requested,
            } if track_id == selected
        ));
        assert_eq!(
            packet_pids(
                opened
                    .stream
                    .ready
                    .front()
                    .expect("selected output is ready")
            ),
            vec![0, 0x100, 0x101, 0x103]
        );
    }

    fn fixture_programme() -> Programme {
        Programme {
            transport_stream_id: 0x1234,
            pat_version: 2,
            program_number: 1,
            pmt_pid: 0x100,
            pcr_pid: 0x101,
            version: 3,
            program_descriptors: Vec::new(),
            streams: vec![
                ElementaryStream {
                    stream_type: 0x1b,
                    pid: 0x101,
                    descriptors: Vec::new(),
                    kind: StreamKind::Video,
                    language: None,
                    label: None,
                },
                ElementaryStream {
                    stream_type: 0x0f,
                    pid: 0x102,
                    descriptors: vec![
                        0x0a, 4, b'e', b'n', b'g', 0, 0x50, 14, 0, 0, 0, b'e', b'n', b'g', b'O',
                        b'r', b'i', b'g', b'i', b'n', b'a', b'l',
                    ],
                    kind: StreamKind::Audio(AudioCodec::AacAdts),
                    language: Some("eng".to_owned()),
                    label: Some("Original".to_owned()),
                },
                ElementaryStream {
                    stream_type: 0x04,
                    pid: 0x103,
                    descriptors: Vec::new(),
                    kind: StreamKind::Audio(AudioCodec::Mpeg2Audio),
                    language: None,
                    label: None,
                },
                ElementaryStream {
                    stream_type: 0x06,
                    pid: 0x104,
                    descriptors: vec![0x6a, 0],
                    kind: StreamKind::Audio(AudioCodec::Ac3),
                    language: None,
                    label: None,
                },
            ],
        }
    }

    fn fixture_transport(programme: &Programme) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut pat_continuity = 0;
        packetize_section(
            0,
            &rewritten_pat(programme),
            &mut pat_continuity,
            &mut bytes,
        );
        let streams = programme.streams.iter().collect::<Vec<_>>();
        let mut pmt_continuity = 0;
        packetize_section(
            programme.pmt_pid,
            &rewritten_pmt(programme, &streams).expect("fixture PMT rewrites"),
            &mut pmt_continuity,
            &mut bytes,
        );
        for (pid, marker) in [(0x101, 1), (0x102, 2), (0x103, 3), (0x104, 4)] {
            bytes.extend_from_slice(&fixture_packet(pid, false, 0, &[marker]));
        }
        bytes
    }

    fn packet_pids(bytes: &[u8]) -> Vec<u16> {
        bytes
            .as_chunks::<TS_PACKET_BYTES>()
            .0
            .iter()
            .map(packet_pid)
            .collect()
    }

    fn packet_pid(packet: &[u8; TS_PACKET_BYTES]) -> u16 {
        (u16::from(packet[1] & 0x1f) << 8) | u16::from(packet[2])
    }

    fn section_from_single_packet(packet: &[u8]) -> &[u8] {
        let section = &packet[5..];
        let length = 3 + ((usize::from(section[1] & 0x0f) << 8) | usize::from(section[2]));
        &section[..length]
    }

    fn fixture_packet(
        pid: u16,
        payload_unit_start: bool,
        continuity: u8,
        payload: &[u8],
    ) -> [u8; TS_PACKET_BYTES] {
        let mut packet = [0xff; TS_PACKET_BYTES];
        packet[0] = SYNC_BYTE;
        packet[1] = ((pid >> 8) as u8 & 0x1f) | if payload_unit_start { 0x40 } else { 0 };
        packet[2] = pid as u8;
        packet[3] = 0x10 | (continuity & 0x0f);
        let take = payload.len().min(TS_PACKET_BYTES - 4);
        packet[4..4 + take].copy_from_slice(&payload[..take]);
        packet
    }
}
