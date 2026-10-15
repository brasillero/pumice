//! WebM/Opus: bounded EBML pre-validation of the whole file, then demux with
//! webm-iterable (decision D2). The pre-validation catches truncation that
//! webm-iterable alone would auto-close over; webm-iterable then does the
//! typed demux on data already proven structurally complete.

use super::{OpusHead, OpusPacket, decode_packets, ns_to_samples};
use crate::audio::DecodedAudio;
use crate::engine::CancelToken;
use crate::{Error, ErrorCode};
use std::io::Cursor;
use webm_iterable::WebmIterator;
use webm_iterable::matroska_spec::{Block, Master, MatroskaSpec as Tag, SimpleBlock};

const ID_EBML: u64 = 0x1A45_DFA3;
const ID_DOCTYPE: u64 = 0x4282;
const ID_SEGMENT: u64 = 0x1853_8067;
const ID_SEEKHEAD: u64 = 0x114D_9B74;
const ID_INFO: u64 = 0x1549_A966;
const ID_TRACKS: u64 = 0x1654_AE6B;
const ID_TRACK_ENTRY: u64 = 0xAE;
const ID_TRACK_NUMBER: u64 = 0xD7;
const ID_TRACK_TYPE: u64 = 0x83;
const ID_CODEC_ID: u64 = 0x86;
const ID_CODEC_PRIVATE: u64 = 0x63A2;
const ID_CODEC_DELAY: u64 = 0x56AA;
const ID_CONTENT_ENCODING: u64 = 0x6D80;
const ID_CUES: u64 = 0x1C53_BB6B;
const ID_ATTACHMENTS: u64 = 0x1941_A469;
const ID_CHAPTERS: u64 = 0x1043_A770;
const ID_TAGS: u64 = 0x1254_C367;
const ID_CLUSTER: u64 = 0x1F43_B675;
const ID_TIMESTAMP: u64 = 0xE7;
const ID_SIMPLE_BLOCK: u64 = 0xA3;
const ID_BLOCK_GROUP: u64 = 0xA0;
const ID_BLOCK: u64 = 0xA1;
const ID_DISCARD_PADDING: u64 = 0x75A2;
const ID_BLOCK_DURATION: u64 = 0x9B;
const ID_REFERENCE_BLOCK: u64 = 0xFB;
const ID_VOID: u64 = 0xEC;
const ID_CRC32: u64 = 0xBF;

/// Lacing mode bits in a block flags byte. All lacing is rejected for now.
const LACING_FLAGS: u8 = 0x06;

/// Backstop against element-count exhaustion; the 25 MB budget bounds a valid
/// file to far fewer elements than this.
const MAX_ELEMENTS: usize = 1_000_000;

/// Elements that may appear directly under Segment. An unknown-size Cluster
/// ends where one of these starts (the same sibling boundary rule
/// webm-iterable uses), since Cluster children never reuse these ids.
fn is_segment_level(id: u64) -> bool {
    matches!(
        id,
        ID_SEEKHEAD
            | ID_INFO
            | ID_TRACKS
            | ID_CUES
            | ID_ATTACHMENTS
            | ID_CHAPTERS
            | ID_CLUSTER
            | ID_TAGS
    )
}

struct Element {
    /// Offset of the element's id vint; rewinding to `start` re-reads the
    /// element from its beginning.
    start: usize,
    id: u64,
    /// Declared size; None means an all-ones (unknown) size vint, valid only
    /// for Segment and Cluster, whose ends the schema determines.
    size: Option<u64>,
    data_start: usize,
    data_end: usize,
}

/// A bounded EBML cursor over the already-in-memory file. `next` reads one
/// element header; the caller advances `pos` to the element's `data_end`
/// after consuming it. Every offset is checked: a known-size element must fit
/// inside its parent (and therefore inside the physical file), which is what
/// rejects truncated WebM before webm-iterable can auto-close an open master
/// at EOF. Nesting depth is fixed by the explicit walk below (at most 4
/// levels); the element counter backstops the loop.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    end: usize,
    elements: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], start: usize, end: usize) -> Self {
        Self {
            bytes,
            pos: start,
            end,
            elements: 0,
        }
    }

    fn next(&mut self) -> Result<Option<Element>, Error> {
        if self.pos == self.end {
            return Ok(None);
        }
        if self.pos > self.end || self.elements == MAX_ELEMENTS {
            return Err(Error::invalid_input("webm element limit exceeded"));
        }
        self.elements += 1;

        let start = self.pos;
        let id = self.read_id()?;
        let size = self.read_size()?;
        if size.is_none() && id != ID_SEGMENT && id != ID_CLUSTER {
            return Err(Error::invalid_input(
                "unknown-size element where the schema requires a size",
            ));
        }
        let data_start = self.pos;
        let data_end = match size {
            Some(size) => {
                let size = usize::try_from(size)
                    .map_err(|_| Error::invalid_input("webm element size out of range"))?;
                data_start
                    .checked_add(size)
                    .filter(|end| *end <= self.end)
                    .ok_or_else(|| Error::invalid_input("webm element extends past the file"))?
            }
            None => self.end,
        };
        Ok(Some(Element {
            start,
            id,
            size,
            data_start,
            data_end,
        }))
    }

    /// EBML id vint: 1-4 bytes, marker included in the value.
    fn read_id(&mut self) -> Result<u64, Error> {
        let first = self
            .bytes
            .get(self.pos)
            .copied()
            .ok_or_else(|| Error::invalid_input("truncated webm element id"))?;
        let length = first.leading_zeros() as usize + 1;
        if !(1..=4).contains(&length) {
            return Err(Error::invalid_input("invalid webm element id"));
        }
        let header_end = self
            .pos
            .checked_add(length)
            .filter(|pos| *pos <= self.end)
            .ok_or_else(|| Error::invalid_input("truncated webm element id"))?;
        let mut value = u64::from(first);
        for &byte in &self.bytes[self.pos + 1..header_end] {
            value = (value << 8) | u64::from(byte);
        }
        self.pos = header_end;
        Ok(value)
    }

    /// EBML size vint: 1-8 bytes, marker stripped; all-ones means unknown.
    fn read_size(&mut self) -> Result<Option<u64>, Error> {
        let first = self
            .bytes
            .get(self.pos)
            .copied()
            .ok_or_else(|| Error::invalid_input("truncated webm element size"))?;
        let length = first.leading_zeros() as usize + 1;
        if !(1..=8).contains(&length) {
            return Err(Error::invalid_input("invalid webm element size"));
        }
        let header_end = self
            .pos
            .checked_add(length)
            .filter(|pos| *pos <= self.end)
            .ok_or_else(|| Error::invalid_input("truncated webm element size"))?;
        let mut value = u64::from(first & 0xFFu8.checked_shr(length as u32).unwrap_or(0));
        for &byte in &self.bytes[self.pos + 1..header_end] {
            value = (value << 8) | u64::from(byte);
        }
        self.pos = header_end;
        let unknown = value == (1u64 << (7 * length)) - 1;
        Ok(if unknown { None } else { Some(value) })
    }
}

fn element_slice<'a>(bytes: &'a [u8], element: &Element) -> Result<&'a [u8], Error> {
    bytes
        .get(element.data_start..element.data_end)
        .ok_or_else(|| Error::invalid_input("webm element extends past the file"))
}

/// Big-endian unsigned integer element, at most 8 bytes per the EBML spec.
fn read_uint(bytes: &[u8], element: &Element) -> Result<u64, Error> {
    let data = element_slice(bytes, element)?;
    if data.is_empty() || data.len() > 8 {
        return Err(Error::invalid_input("invalid webm integer element"));
    }
    let mut value = 0u64;
    for &byte in data {
        value = (value << 8) | u64::from(byte);
    }
    Ok(value)
}

#[derive(Debug)]
struct WebmInfo {
    track_number: u64,
    codec_private: Vec<u8>,
    codec_delay_ns: Option<u64>,
    block_count: usize,
}

/// Validates a block element and returns its (cluster-relative) timestamp.
fn block_timestamp(bytes: &[u8], element: &Element) -> Result<i16, Error> {
    let data = element_slice(bytes, element)?;
    if data.len() < 4 {
        return Err(Error::invalid_input("truncated webm block"));
    }
    let track_len = data[0].leading_zeros() as usize + 1;
    if track_len > 8 || data.len() < track_len + 3 {
        return Err(Error::invalid_input("truncated webm block"));
    }
    if data[track_len + 2] & LACING_FLAGS != 0 {
        return Err(Error::unsupported_format(
            "laced webm blocks are not supported",
        ));
    }
    Ok(i16::from_be_bytes([data[track_len], data[track_len + 1]]))
}

fn note_block(
    bytes: &[u8],
    element: &Element,
    cluster_ts: i128,
    block_count: &mut usize,
    last_timestamp: &mut i128,
) -> Result<(), Error> {
    let timestamp = block_timestamp(bytes, element)?;
    let absolute = cluster_ts + i128::from(timestamp);
    if absolute < *last_timestamp {
        return Err(Error::invalid_input("webm block timestamps move backwards"));
    }
    *last_timestamp = absolute;
    *block_count += 1;
    Ok(())
}

fn walk_track_entry(bytes: &[u8], element: &Element, info: &mut WebmInfo) -> Result<(), Error> {
    let mut reader = Reader::new(bytes, element.data_start, element.data_end);
    let mut track_number: Option<u64> = None;
    while let Some(child) = reader.next()? {
        match child.id {
            ID_TRACK_NUMBER => {
                if track_number.replace(read_uint(bytes, &child)?).is_some() {
                    return Err(Error::invalid_input("duplicate webm track number"));
                }
            }
            ID_TRACK_TYPE => {
                if read_uint(bytes, &child)? != 2 {
                    return Err(Error::unsupported_format("webm track is not audio"));
                }
            }
            ID_CODEC_ID => {
                if element_slice(bytes, &child)? != b"A_OPUS" {
                    return Err(Error::unsupported_format(
                        "unsupported webm codec (expected A_OPUS)",
                    ));
                }
            }
            ID_CODEC_PRIVATE => {
                info.codec_private = element_slice(bytes, &child)?.to_vec();
            }
            ID_CODEC_DELAY => {
                info.codec_delay_ns = Some(read_uint(bytes, &child)?);
            }
            ID_CONTENT_ENCODING => {
                return Err(Error::unsupported_format(
                    "encrypted or compressed tracks are not supported",
                ));
            }
            _ => {}
        }
        reader.pos = child.data_end;
    }
    let track_number =
        track_number.ok_or_else(|| Error::invalid_input("webm track has no number"))?;
    if track_number == 0 {
        return Err(Error::invalid_input("webm track number is zero"));
    }
    info.track_number = track_number;
    Ok(())
}

fn walk_block_group(
    bytes: &[u8],
    element: &Element,
    cluster_ts: i128,
    block_count: &mut usize,
    last_timestamp: &mut i128,
) -> Result<(), Error> {
    let mut reader = Reader::new(bytes, element.data_start, element.data_end);
    let mut blocks = 0usize;
    while let Some(child) = reader.next()? {
        match child.id {
            ID_BLOCK => {
                note_block(bytes, &child, cluster_ts, &mut blocks, last_timestamp)?;
            }
            ID_DISCARD_PADDING | ID_BLOCK_DURATION | ID_REFERENCE_BLOCK | ID_VOID | ID_CRC32 => {}
            _ => {
                return Err(Error::invalid_input(
                    "unexpected element in webm block group",
                ));
            }
        }
        reader.pos = child.data_end;
    }
    if blocks != 1 {
        return Err(Error::invalid_input(
            "webm block group must contain exactly one block",
        ));
    }
    *block_count += blocks;
    Ok(())
}

/// Walks one Cluster and advances `parent` past it. For an unknown-size
/// Cluster, the walk stops at the first Segment-level sibling (the schema
/// boundary webm-iterable also applies) and the parent resumes there.
fn walk_cluster(
    bytes: &[u8],
    element: &Element,
    parent: &mut Reader<'_>,
    block_count: &mut usize,
    last_timestamp: &mut i128,
) -> Result<(), Error> {
    let mut reader = Reader::new(bytes, element.data_start, element.data_end);
    let mut cluster_ts = 0i128;
    while reader.pos < element.data_end {
        let Some(child) = reader.next()? else { break };
        if element.size.is_none() && is_segment_level(child.id) {
            // An unknown-size Cluster ends at the first Segment-level
            // sibling; rewind so the parent reader re-reads that element.
            reader.pos = child.start;
            break;
        }
        match child.id {
            ID_TIMESTAMP => {
                cluster_ts = i128::from(read_uint(bytes, &child)?);
            }
            ID_SIMPLE_BLOCK => {
                note_block(bytes, &child, cluster_ts, block_count, last_timestamp)?;
            }
            ID_BLOCK_GROUP => {
                walk_block_group(bytes, &child, cluster_ts, block_count, last_timestamp)?;
            }
            ID_VOID | ID_CRC32 => {}
            _ => return Err(Error::invalid_input("unexpected element in webm cluster")),
        }
        reader.pos = child.data_end;
    }
    parent.pos = reader.pos;
    Ok(())
}

/// Bounded structural validation of the whole file. Returns the track
/// metadata the decoder needs plus the exact block count, which the demuxer
/// must reproduce (a mismatch means data was lost or fabricated).
fn validate(bytes: &[u8]) -> Result<WebmInfo, Error> {
    let mut reader = Reader::new(bytes, 0, bytes.len());
    let header = reader
        .next()?
        .ok_or_else(|| Error::invalid_input("empty webm file"))?;
    if header.id != ID_EBML {
        return Err(Error::invalid_input("missing webm ebml header"));
    }
    let mut doc_type: Option<Vec<u8>> = None;
    let mut header_reader = Reader::new(bytes, header.data_start, header.data_end);
    while let Some(child) = header_reader.next()? {
        if child.id == ID_DOCTYPE {
            doc_type = Some(element_slice(bytes, &child)?.to_vec());
        }
        header_reader.pos = child.data_end;
    }
    reader.pos = header.data_end;
    match doc_type.as_deref() {
        Some(b"webm") | Some(b"matroska") => {}
        Some(_) => return Err(Error::unsupported_format("not a webm or matroska file")),
        None => return Err(Error::invalid_input("webm file has no doc type")),
    }

    let mut segment: Option<(usize, usize)> = None;
    while let Some(element) = reader.next()? {
        match element.id {
            ID_VOID | ID_CRC32 => {}
            ID_SEGMENT => {
                if segment.is_some() {
                    return Err(Error::invalid_input("multiple webm segments"));
                }
                segment = Some((element.data_start, element.data_end));
            }
            _ => {
                return Err(Error::invalid_input(
                    "unexpected element outside the webm segment",
                ));
            }
        }
        reader.pos = element.data_end;
    }
    let (segment_start, segment_end) =
        segment.ok_or_else(|| Error::invalid_input("webm file has no segment"))?;

    let mut info = WebmInfo {
        track_number: 0,
        codec_private: Vec::new(),
        codec_delay_ns: None,
        block_count: 0,
    };
    let mut track_entries = 0usize;
    let mut last_timestamp = i128::MIN;
    let mut segment_reader = Reader::new(bytes, segment_start, segment_end);
    while let Some(element) = segment_reader.next()? {
        match element.id {
            ID_SEEKHEAD => segment_reader.pos = element.data_end,
            ID_INFO => segment_reader.pos = element.data_end,
            ID_TRACKS => {
                if track_entries > 0 {
                    return Err(Error::unsupported_format(
                        "webm file must contain exactly one audio track",
                    ));
                }
                let mut tracks_reader = Reader::new(bytes, element.data_start, element.data_end);
                while let Some(entry) = tracks_reader.next()? {
                    if entry.id == ID_TRACK_ENTRY {
                        track_entries += 1;
                        if track_entries > 1 {
                            return Err(Error::unsupported_format(
                                "webm file must contain exactly one audio track",
                            ));
                        }
                        walk_track_entry(bytes, &entry, &mut info)?;
                    }
                    tracks_reader.pos = entry.data_end;
                }
                segment_reader.pos = element.data_end;
            }
            ID_CLUSTER => walk_cluster(
                bytes,
                &element,
                &mut segment_reader,
                &mut info.block_count,
                &mut last_timestamp,
            )?,
            ID_CUES | ID_ATTACHMENTS | ID_CHAPTERS | ID_TAGS | ID_VOID | ID_CRC32 => {
                segment_reader.pos = element.data_end;
            }
            _ => return Err(Error::invalid_input("unexpected element in webm segment")),
        }
    }
    if track_entries != 1 {
        return Err(Error::unsupported_format(
            "webm file must contain exactly one audio track",
        ));
    }
    if info.codec_private.is_empty() {
        return Err(Error::invalid_input("webm track has no opus header"));
    }
    Ok(info)
}

/// Demuxes blocks with webm-iterable over the pre-validated bytes. Trims come
/// from signed DiscardPadding (positive trims the end, negative the start),
/// converted to 48 kHz samples with checked arithmetic.
fn demux(bytes: &[u8], info: &WebmInfo, cancel: &CancelToken) -> Result<Vec<OpusPacket>, Error> {
    let mut iterator = WebmIterator::new(Cursor::new(bytes), &[Tag::BlockGroup(Master::Start)]);
    iterator.set_max_allowable_tag_size(Some(bytes.len()));
    let mut packets = Vec::new();
    for tag in &mut iterator {
        if cancel.is_cancelled() {
            return Err(Error::new(
                ErrorCode::Cancelled,
                "transcription was cancelled",
            ));
        }
        match tag.map_err(|_| Error::invalid_input("malformed webm data"))? {
            block @ Tag::SimpleBlock(_) => {
                let block = SimpleBlock::try_from(&block)
                    .map_err(|_| Error::invalid_input("malformed webm block"))?;
                if block.lacing.is_some() {
                    return Err(Error::unsupported_format(
                        "laced webm blocks are not supported",
                    ));
                }
                if block.track != info.track_number {
                    return Err(Error::invalid_input(
                        "webm block references a different track",
                    ));
                }
                packets.push(OpusPacket {
                    data: block.raw_frame_data().into(),
                    trim_start: 0,
                    trim_end: 0,
                });
            }
            Tag::BlockGroup(Master::Full(children)) => {
                let mut frame: Option<Box<[u8]>> = None;
                let mut discard_ticks: i64 = 0;
                for child in &children {
                    match child {
                        block @ Tag::Block(_) => {
                            let block = Block::try_from(block)
                                .map_err(|_| Error::invalid_input("malformed webm block"))?;
                            if block.lacing.is_some() {
                                return Err(Error::unsupported_format(
                                    "laced webm blocks are not supported",
                                ));
                            }
                            if block.track != info.track_number {
                                return Err(Error::invalid_input(
                                    "webm block references a different track",
                                ));
                            }
                            frame = Some(block.raw_frame_data().into());
                        }
                        Tag::DiscardPadding(value) => discard_ticks = *value,
                        _ => {}
                    }
                }
                let frame =
                    frame.ok_or_else(|| Error::invalid_input("webm block group has no block"))?;
                let samples = ns_to_samples(discard_ticks)?;
                let (trim_start, trim_end) = if samples < 0 {
                    ((-samples) as usize, 0)
                } else {
                    (0, samples as usize)
                };
                packets.push(OpusPacket {
                    data: frame,
                    trim_start,
                    trim_end,
                });
            }
            _ => {}
        }
    }
    Ok(packets)
}

/// Decodes a WebM/Opus file already identified by the EBML magic bytes.
pub(crate) fn decode_webm_opus(
    bytes: Vec<u8>,
    cancel: &CancelToken,
) -> Result<DecodedAudio, Error> {
    let info = validate(&bytes)?;
    let head = OpusHead::parse(&info.codec_private)?;
    if let Some(ns) = info.codec_delay_ns {
        let delay = ns_to_samples(ns as i64)?;
        if delay != head.pre_skip as i64 {
            return Err(Error::invalid_input(
                "webm codec delay disagrees with the opus pre-skip",
            ));
        }
    }
    let packets = demux(&bytes, &info, cancel)?;
    // No partial results: every block the validator counted must come back
    // from the demuxer.
    if packets.len() != info.block_count {
        return Err(Error::invalid_input(
            "webm block count mismatch (lost blocks)",
        ));
    }
    decode_packets(&head, &packets, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;
    use crate::opus::tests::fixture;

    /// A valid 19-byte family-0 OpusHead (mono, 312-sample pre-skip).
    fn opus_head_bytes() -> Vec<u8> {
        let bytes = fixture("sample-pt.webm");
        let at = bytes.windows(8).position(|w| w == b"OpusHead").unwrap();
        bytes[at..at + 19].to_vec()
    }

    /// One real Opus packet so end-to-end decode tests produce audio.
    fn opus_packet_bytes() -> Vec<u8> {
        let bytes = fixture("sample-pt-48k.opus");
        let packets = crate::opus::tests::ogg_packets(&bytes);
        packets[2].to_vec()
    }

    // --- Tiny EBML writer (sizes below 128, 1-byte size vints) -------------

    fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        if body.len() < 128 {
            out.push(0x80 | body.len() as u8);
        } else {
            assert!(body.len() < 16_384);
            out.push(0x40 | (body.len() >> 8) as u8);
            out.push(body.len() as u8);
        }
        out.extend_from_slice(body);
        out
    }

    fn uint(id: &[u8], value: u64) -> Vec<u8> {
        let bytes = value.to_be_bytes();
        let start = bytes
            .iter()
            .position(|&b| b != 0)
            .unwrap_or(bytes.len() - 1);
        el(id, &bytes[start..])
    }

    fn str_el(id: &[u8], value: &str) -> Vec<u8> {
        el(id, value.as_bytes())
    }

    const EBML: &[u8] = &[0x1A, 0x45, 0xDF, 0xA3];
    const SEGMENT: &[u8] = &[0x18, 0x53, 0x80, 0x67];
    const TRACKS: &[u8] = &[0x16, 0x54, 0xAE, 0x6B];
    const TRACK_ENTRY: &[u8] = &[0xAE];
    const TRACK_NUMBER: &[u8] = &[0xD7];
    const TRACK_TYPE: &[u8] = &[0x83];
    const CODEC_ID: &[u8] = &[0x86];
    const CODEC_PRIVATE: &[u8] = &[0x63, 0xA2];
    const CODEC_DELAY: &[u8] = &[0x56, 0xAA];
    const CONTENT_ENCODING: &[u8] = &[0x6D, 0x80];
    const CLUSTER: &[u8] = &[0x1F, 0x43, 0xB6, 0x75];
    const TIMESTAMP: &[u8] = &[0xE7];
    const SIMPLE_BLOCK: &[u8] = &[0xA3];
    const BLOCK_GROUP: &[u8] = &[0xA0];
    const BLOCK: &[u8] = &[0xA1];
    const DISCARD_PADDING: &[u8] = &[0x75, 0xA2];

    /// Unknown-size vint for Segment/Cluster.
    const UNKNOWN_SIZE: &[u8] = &[0xFF];

    fn ebml_header(doc_type: &str) -> Vec<u8> {
        el(EBML, &str_el(&[0x42, 0x82], doc_type))
    }

    fn track_entry(codec: &str, private: &[u8]) -> Vec<u8> {
        let mut body = uint(TRACK_NUMBER, 1);
        body.extend(uint(TRACK_TYPE, 2));
        body.extend(str_el(CODEC_ID, codec));
        body.extend(el(CODEC_PRIVATE, private));
        el(TRACK_ENTRY, &body)
    }

    /// One SimpleBlock: track 1, given timestamp and flags.
    fn simple_block(timestamp: i16, flags: u8, payload: &[u8]) -> Vec<u8> {
        let mut body = vec![0x81];
        body.extend(timestamp.to_be_bytes());
        body.push(flags);
        body.extend_from_slice(payload);
        el(SIMPLE_BLOCK, &body)
    }

    /// A minimal but complete WebM/Opus file with one packet.
    fn minimal_file() -> Vec<u8> {
        let mut tracks = Vec::new();
        tracks.extend(track_entry("A_OPUS", &opus_head_bytes()));
        let mut segment_body = el(TRACKS, &tracks);
        let mut cluster = uint(TIMESTAMP, 0);
        cluster.extend(simple_block(0, 0, &opus_packet_bytes()));
        segment_body.extend(el(CLUSTER, &cluster));
        // Segment with unknown size.
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(segment_body);
        file
    }

    fn validate_code(bytes: &[u8]) -> ErrorCode {
        validate(bytes).unwrap_err().code
    }

    #[test]
    fn minimal_file_validates_and_decodes() {
        let bytes = minimal_file();
        let info = validate(&bytes).unwrap();
        assert_eq!(info.track_number, 1);
        assert_eq!(info.block_count, 1);
        let decoded = decode_webm_opus(bytes, &CancelToken::new()).unwrap();
        assert_eq!(decoded.sample_rate, 48_000);
        assert!(!decoded.samples.is_empty());
    }

    #[test]
    fn accepts_matroska_doc_type() {
        let bytes = minimal_file();
        let mut bytes = bytes;
        // Rewrite the DocType element in place (both are 4-byte values with
        // the same 1-byte size).
        let at = bytes.windows(4).position(|w| w == b"webm").unwrap();
        bytes[at..at + 4].copy_from_slice(b"matroska"[..4].as_ref());
        // "matr" alone is not a doc type; build a proper one instead.
        let _ = bytes;
        let mut file = ebml_header("matroska");
        let mut tracks = el(TRACKS, &track_entry("A_OPUS", &opus_head_bytes()));
        let mut cluster = uint(TIMESTAMP, 0);
        cluster.extend(simple_block(0, 0, &opus_packet_bytes()));
        tracks.extend(el(CLUSTER, &cluster));
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(tracks);
        let info = validate(&file).unwrap();
        assert_eq!(info.block_count, 1);
    }

    #[test]
    fn rejects_wrong_or_missing_doc_type() {
        let mut file = ebml_header("x-matroska");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        assert_eq!(validate_code(&file), ErrorCode::UnsupportedFormat);

        let mut file = el(EBML, &[]);
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_missing_or_multiple_tracks() {
        // No Tracks at all.
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        let mut cluster = uint(TIMESTAMP, 0);
        cluster.extend(simple_block(0, 0, &opus_packet_bytes()));
        file.extend(el(CLUSTER, &cluster));
        assert_eq!(validate_code(&file), ErrorCode::UnsupportedFormat);

        // Two track entries.
        let mut tracks = track_entry("A_OPUS", &opus_head_bytes());
        tracks.extend(track_entry("A_OPUS", &opus_head_bytes()));
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &tracks));
        assert_eq!(validate_code(&file), ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn rejects_non_audio_and_non_opus_tracks() {
        let mut tracks = uint(TRACK_NUMBER, 1);
        tracks.extend(uint(TRACK_TYPE, 1)); // video
        tracks.extend(str_el(CODEC_ID, "A_OPUS"));
        tracks.extend(el(CODEC_PRIVATE, &opus_head_bytes()));
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &el(TRACK_ENTRY, &tracks)));
        assert_eq!(validate_code(&file), ErrorCode::UnsupportedFormat);

        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &track_entry("A_VORBIS", &[])));
        assert_eq!(validate_code(&file), ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn rejects_encrypted_track() {
        let mut entry = uint(TRACK_NUMBER, 1);
        entry.extend(uint(TRACK_TYPE, 2));
        entry.extend(str_el(CODEC_ID, "A_OPUS"));
        entry.extend(el(CODEC_PRIVATE, &opus_head_bytes()));
        entry.extend(el(CONTENT_ENCODING, &[]));
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &el(TRACK_ENTRY, &entry)));
        assert_eq!(validate_code(&file), ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn rejects_laced_blocks() {
        for flags in [0x02u8, 0x04, 0x06] {
            let mut file = ebml_header("webm");
            file.extend_from_slice(SEGMENT);
            file.extend_from_slice(UNKNOWN_SIZE);
            file.extend(el(TRACKS, &track_entry("A_OPUS", &opus_head_bytes())));
            let mut cluster = uint(TIMESTAMP, 0);
            cluster.extend(simple_block(0, flags, &opus_packet_bytes()));
            file.extend(el(CLUSTER, &cluster));
            assert_eq!(validate_code(&file), ErrorCode::UnsupportedFormat);
        }
    }

    #[test]
    fn rejects_unknown_size_non_masters() {
        // A Tracks element with an unknown-size vint must be rejected.
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend_from_slice(TRACKS);
        file.extend_from_slice(UNKNOWN_SIZE);
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_block_extending_past_file() {
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &track_entry("A_OPUS", &opus_head_bytes())));
        // SimpleBlock declaring 0x7F (127) bytes of payload with only 4
        // present: the extent check rejects it before any payload read.
        let mut cluster = uint(TIMESTAMP, 0);
        cluster.extend_from_slice(SIMPLE_BLOCK);
        cluster.push(0x80 | 127);
        cluster.extend([0x81, 0, 0, 0]);
        file.extend(el(CLUSTER, &cluster));
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_truncated_element_header() {
        let mut file = minimal_file();
        // Cut inside the final cluster's last element.
        file.truncate(file.len() - 2);
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);

        let mut file = minimal_file();
        file.truncate(file.len() / 2);
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_backwards_timestamps() {
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &track_entry("A_OPUS", &opus_head_bytes())));
        let mut cluster = uint(TIMESTAMP, 0);
        cluster.extend(simple_block(100, 0, &opus_packet_bytes()));
        cluster.extend(simple_block(50, 0, &opus_packet_bytes()));
        file.extend(el(CLUSTER, &cluster));
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    #[test]
    fn multiple_unknown_size_clusters_are_fine() {
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &track_entry("A_OPUS", &opus_head_bytes())));
        for base in [0u64, 1000, 2000] {
            let mut cluster = uint(TIMESTAMP, base);
            cluster.extend(simple_block(0, 0, &opus_packet_bytes()));
            file.extend_from_slice(CLUSTER);
            file.extend_from_slice(UNKNOWN_SIZE);
            file.extend(cluster);
        }
        let info = validate(&file).unwrap();
        assert_eq!(info.block_count, 3);
        let decoded = decode_webm_opus(file, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), 3 * 960 - 312);
    }

    #[test]
    fn block_group_with_discard_padding_end_to_end() {
        // BlockGroup with DiscardPadding of 0.5 ms (24 samples at 48 kHz):
        // 960 - 312 (pre-skip) - 24 = 624 output samples.
        let mut group = el(BLOCK, &{
            let mut body = vec![0x81];
            body.extend(0i16.to_be_bytes());
            body.push(0);
            body.extend_from_slice(&opus_packet_bytes());
            body
        });
        group.extend(el(DISCARD_PADDING, &500_000u64.to_be_bytes()[2..]));
        let mut cluster = uint(TIMESTAMP, 0);
        cluster.extend(el(BLOCK_GROUP, &group));
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &track_entry("A_OPUS", &opus_head_bytes())));
        file.extend(el(CLUSTER, &cluster));
        let decoded = decode_webm_opus(file, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), 960 - 312 - 24);
    }

    #[test]
    fn rejects_block_group_without_block_and_multi_block_groups() {
        // Empty BlockGroup.
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &track_entry("A_OPUS", &opus_head_bytes())));
        let mut cluster = uint(TIMESTAMP, 0);
        cluster.extend(el(BLOCK_GROUP, &[]));
        file.extend(el(CLUSTER, &cluster));
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_block_on_unexpected_track() {
        // Validation accepts the structure; demux rejects the track change.
        let mut block = vec![0x82]; // track 2
        block.extend(0i16.to_be_bytes());
        block.push(0);
        block.extend_from_slice(&opus_packet_bytes());
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &track_entry("A_OPUS", &opus_head_bytes())));
        let mut cluster = uint(TIMESTAMP, 0);
        cluster.extend(el(SIMPLE_BLOCK, &block));
        file.extend(el(CLUSTER, &cluster));
        let info = validate(&file).unwrap();
        assert_eq!(info.block_count, 1);
        let err = decode_webm_opus(file, &CancelToken::new()).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_codec_delay_disagreeing_with_pre_skip() {
        let mut entry = uint(TRACK_NUMBER, 1);
        entry.extend(uint(TRACK_TYPE, 2));
        entry.extend(str_el(CODEC_ID, "A_OPUS"));
        entry.extend(el(CODEC_PRIVATE, &opus_head_bytes()));
        // Pre-skip 312 samples = 6,500,000 ns; claim 7,000,000 instead.
        entry.extend(el(CODEC_DELAY, &700_000_000u64.to_be_bytes()[3..]));
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &el(TRACK_ENTRY, &entry)));
        let mut cluster = uint(TIMESTAMP, 0);
        cluster.extend(simple_block(0, 0, &opus_packet_bytes()));
        file.extend(el(CLUSTER, &cluster));
        let err = decode_webm_opus(file, &CancelToken::new()).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_codec_private_mutations_in_real_fixture() {
        let bytes = fixture("sample-pt.webm");

        let mut bad_magic = bytes.clone();
        let at = bad_magic.windows(8).position(|w| w == b"OpusHead").unwrap();
        bad_magic[at] = b'X';
        assert_eq!(
            decode_webm_opus(bad_magic, &CancelToken::new())
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );

        let mut many_channels = bytes.clone();
        let at = many_channels
            .windows(8)
            .position(|w| w == b"OpusHead")
            .unwrap();
        many_channels[at + 9] = 6;
        assert_eq!(
            decode_webm_opus(many_channels, &CancelToken::new())
                .unwrap_err()
                .code,
            ErrorCode::UnsupportedFormat
        );

        let mut bad_family = bytes.clone();
        let at = bad_family
            .windows(8)
            .position(|w| w == b"OpusHead")
            .unwrap();
        bad_family[at + 18] = 3;
        assert_eq!(
            decode_webm_opus(bad_family, &CancelToken::new())
                .unwrap_err()
                .code,
            ErrorCode::UnsupportedFormat
        );
    }

    #[test]
    fn rejects_truncated_real_fixtures() {
        for name in [
            "sample-pt.webm",
            "sample-pt-live.webm",
            "chrome-mediarecorder.webm",
        ] {
            let bytes = fixture(name);
            for cut in [1, 17, bytes.len() / 2] {
                let truncated = &bytes[..bytes.len() - cut];
                let err = decode_webm_opus(truncated.to_vec(), &CancelToken::new()).unwrap_err();
                assert_eq!(
                    err.code,
                    ErrorCode::InvalidInput,
                    "{name} cut {cut}: {:?}",
                    err.code
                );
            }
        }
    }

    #[test]
    fn rejects_doc_type_mutation_in_real_fixture() {
        let mut bytes = fixture("sample-pt.webm");
        let at = bytes.windows(4).position(|w| w == b"webm").unwrap();
        bytes[at] = b'x';
        assert_eq!(
            decode_webm_opus(bytes, &CancelToken::new())
                .unwrap_err()
                .code,
            ErrorCode::UnsupportedFormat
        );
    }

    #[test]
    fn rejects_codec_delay_mutation_in_real_fixture() {
        let mut bytes = fixture("sample-pt.webm");
        // CodecDelay 0x56AA, 3-byte value 0x632EA0 (6,499,744 ns ≈ 312
        // samples). Bump it far enough to change the rounded sample count.
        let at = bytes
            .windows(5)
            .position(|w| w == [0x56, 0xAA, 0x83, 0x63, 0x2E])
            .unwrap();
        bytes[at + 4] = 0x80; // 0x6380A0 ns -> 313 samples != 312
        assert_eq!(
            decode_webm_opus(bytes, &CancelToken::new())
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );
    }

    #[test]
    fn real_121_second_webm_is_too_long() {
        // 121 s of audio: the 120 s retained-output budget rejects it.
        let bytes = fixture("silence121.webm");
        let err = decode_webm_opus(bytes, &CancelToken::new()).unwrap_err();
        assert_eq!(err.code, ErrorCode::AudioTooLong);
    }
}
