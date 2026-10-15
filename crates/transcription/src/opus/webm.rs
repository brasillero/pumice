//! WebM/Opus: bounded EBML pre-validation of the whole file, then demux with
//! webm-iterable (decision D2). The pre-validation catches truncation that
//! webm-iterable alone would auto-close over; webm-iterable then does the
//! typed demux on data already proven structurally complete.

use super::{OPUS_SAMPLE_RATE, OpusHead, OpusPacket, decode_packets, ns_to_samples, packet_frames};
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
const ID_TRACK_TIMESTAMP_SCALE: u64 = 0x23314F;
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
const ID_INFO_TIMESTAMP_SCALE: u64 = 0x2AD7B1;
const ID_AUDIO: u64 = 0xE1;
const ID_CHANNELS: u64 = 0x9F;
const ID_SAMPLING_FREQUENCY: u64 = 0xB5;
const ID_VOID: u64 = 0xEC;
const ID_CRC32: u64 = 0xBF;

/// Lacing mode bits in a block flags byte. All lacing is rejected for now.
const LACING_FLAGS: u8 = 0x06;

/// Matroska default TimestampScale: one timestamp tick is 1 ms.
const DEFAULT_TIMESTAMP_SCALE: u64 = 1_000_000;

/// Nanoseconds per second, for converting Opus durations to timestamp ticks.
const NS_PER_SECOND: i128 = 1_000_000_000;

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

/// EBML float element: 4- or 8-byte big-endian IEEE 754.
fn read_float(bytes: &[u8], element: &Element) -> Result<f64, Error> {
    let data = element_slice(bytes, element)?;
    match data.len() {
        4 => Ok(f32::from_be_bytes(data.try_into().unwrap()).into()),
        8 => Ok(f64::from_be_bytes(data.try_into().unwrap())),
        _ => Err(Error::invalid_input("invalid webm float element")),
    }
}

#[derive(Debug)]
struct WebmInfo {
    track_number: u64,
    codec_private: Vec<u8>,
    codec_delay_ns: Option<u64>,
    /// TimestampScale in nanoseconds per tick (Matroska default 1 ms).
    timestamp_scale: u64,
    /// Audio/Channels, when the track declares it.
    audio_channels: Option<u64>,
    /// Audio/SamplingFrequency, when the track declares it.
    sampling_frequency: Option<f64>,
    block_count: usize,
}

/// The codec timeline across blocks, checked as one stream-wide expectation
/// once every block is known: the position predicted from the first block's
/// timestamp plus the sum of all preceding packet durations (exact rational
/// arithmetic) must match each block's timestamp within a fixed allowance
/// that never resets at a cluster boundary.
#[derive(Debug)]
struct Timeline {
    tick_scale_ns: u64,
    /// Absolute timestamp (ticks) and validated 48 kHz frame count per block,
    /// in stream order.
    blocks: Vec<(i128, usize)>,
}

impl Timeline {
    fn new(tick_scale_ns: u64) -> Self {
        Self {
            tick_scale_ns,
            blocks: Vec::new(),
        }
    }

    /// Validates the packet duration and records the block; the stream-wide
    /// continuity check runs in [`Timeline::check`] after the walk, because
    /// the allowance depends on the shortest packet duration in the stream.
    fn note(&mut self, absolute: i128, payload: &[u8]) -> Result<(), Error> {
        let duration = packet_frames(payload)?;
        self.blocks.push((absolute, duration));
        Ok(())
    }

    /// Every block must lie within `bound` of the position predicted from the
    /// first block's timestamp plus the sum of all preceding packet durations.
    /// The allowance is the smaller of 6 ms plus one timestamp tick (real
    /// muxers timestamp blocks on a wall clock; measured Chrome
    /// MediaRecorder recordings stay within 7 ms at a 1 ms timebase, while
    /// the sum of absolute per-interval errors reaches seconds) and one unit
    /// below half the shortest packet in the stream. A lost, duplicated or
    /// duration-altered packet is rejected when it moves a later block's
    /// timestamp beyond that bound; the last packet has no later timestamp to
    /// witness it, and a packet shorter than the jitter allowance can go
    /// unnoticed. All arithmetic is checked integer math in units of 48 kHz
    /// frames * ns/s.
    fn check(&self) -> Result<(), Error> {
        let Some(&(first_timestamp, _)) = self.blocks.first() else {
            return Ok(());
        };
        let tick_ns = i128::from(self.tick_scale_ns);
        let jitter_bound = i128::from(6_000_000u64)
            .checked_add(tick_ns)
            .and_then(|ns| ns.checked_mul(i128::from(OPUS_SAMPLE_RATE)))
            .ok_or_else(|| Error::invalid_input("webm timestamp scale out of range"))?;
        let min_packet_frames = self
            .blocks
            .iter()
            .map(|&(_, frames)| frames as i128)
            .min()
            .expect("blocks is non-empty");
        let half_packet = min_packet_frames
            .checked_mul(NS_PER_SECOND)
            .and_then(|ns| ns.checked_div(2))
            .and_then(|ns| ns.checked_sub(1))
            .ok_or_else(|| Error::invalid_input("webm timestamps out of range"))?;
        let bound = jitter_bound.min(half_packet);
        let mut total_frames = 0i128;
        for &(timestamp, frames) in &self.blocks {
            let residual = timestamp
                .checked_sub(first_timestamp)
                .and_then(|ticks| ticks.checked_mul(tick_ns))
                .and_then(|scaled| scaled.checked_mul(i128::from(OPUS_SAMPLE_RATE)))
                .and_then(|scaled| scaled.checked_sub(total_frames.checked_mul(NS_PER_SECOND)?))
                .ok_or_else(|| Error::invalid_input("webm timestamps out of range"))?;
            if residual.abs() > bound {
                return Err(Error::invalid_input(
                    "webm block timeline does not match the packet durations",
                ));
            }
            total_frames = total_frames
                .checked_add(frames as i128)
                .ok_or_else(|| Error::invalid_input("webm timestamps out of range"))?;
        }
        Ok(())
    }
}

/// Validates a block element and returns its (cluster-relative) timestamp
/// and Opus payload.
fn block_parts<'a>(bytes: &'a [u8], element: &Element) -> Result<(i16, &'a [u8]), Error> {
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
    let timestamp = i16::from_be_bytes([data[track_len], data[track_len + 1]]);
    Ok((timestamp, &data[track_len + 3..]))
}

fn note_block(
    bytes: &[u8],
    element: &Element,
    cluster_ts: i128,
    timeline: &mut Timeline,
    block_count: &mut usize,
) -> Result<(), Error> {
    let (timestamp, payload) = block_parts(bytes, element)?;
    let absolute = cluster_ts + i128::from(timestamp);
    timeline.note(absolute, payload)?;
    *block_count += 1;
    Ok(())
}

fn walk_track_entry(bytes: &[u8], element: &Element, info: &mut WebmInfo) -> Result<(), Error> {
    let mut reader = Reader::new(bytes, element.data_start, element.data_end);
    let mut track_number: Option<u64> = None;
    let mut track_type: Option<u64> = None;
    let mut codec_id: Option<&[u8]> = None;
    let mut codec_private: Option<Vec<u8>> = None;
    let mut codec_delay: Option<u64> = None;
    let mut track_timestamp_scale: Option<f64> = None;
    let mut audio: Option<bool> = None;
    while let Some(child) = reader.next()? {
        match child.id {
            ID_TRACK_NUMBER => {
                if track_number.replace(read_uint(bytes, &child)?).is_some() {
                    return Err(Error::invalid_input("duplicate webm track number"));
                }
            }
            ID_TRACK_TYPE => {
                if track_type.replace(read_uint(bytes, &child)?).is_some() {
                    return Err(Error::invalid_input("duplicate webm track type"));
                }
            }
            ID_CODEC_ID => {
                if codec_id.replace(element_slice(bytes, &child)?).is_some() {
                    return Err(Error::invalid_input("duplicate webm codec id"));
                }
            }
            ID_CODEC_PRIVATE => {
                if codec_private
                    .replace(element_slice(bytes, &child)?.to_vec())
                    .is_some()
                {
                    return Err(Error::invalid_input("duplicate webm codec private"));
                }
            }
            ID_CODEC_DELAY => {
                if codec_delay.replace(read_uint(bytes, &child)?).is_some() {
                    return Err(Error::invalid_input("duplicate webm codec delay"));
                }
            }
            ID_TRACK_TIMESTAMP_SCALE => {
                // Block time is (ClusterTimestamp + block timestamp *
                // TrackTimestampScale) * TimestampScale; only the default
                // 1.0 leaves the timeline validation's timestamps correct,
                // so any other value (including NaN and infinities) is
                // rejected before the continuity check.
                let value = read_float(bytes, &child)?;
                if track_timestamp_scale.is_some() {
                    return Err(Error::invalid_input("duplicate webm track timestamp scale"));
                }
                track_timestamp_scale = Some(value);
                if value != 1.0 {
                    return Err(Error::unsupported_format(
                        "webm track timestamp scale is not supported",
                    ));
                }
            }
            ID_AUDIO => {
                // At most one Audio master; its channel and rate metadata is
                // checked against OpusHead once the header is parsed.
                if audio.replace(true).is_some() {
                    return Err(Error::invalid_input("duplicate webm audio element"));
                }
                let mut channels: Option<u64> = None;
                let mut sampling_frequency: Option<f64> = None;
                let mut audio_reader = Reader::new(bytes, child.data_start, child.data_end);
                while let Some(grandchild) = audio_reader.next()? {
                    match grandchild.id {
                        ID_CHANNELS => {
                            let value = read_uint(bytes, &grandchild)?;
                            if channels.is_some() {
                                return Err(Error::invalid_input(
                                    "duplicate webm audio channel count",
                                ));
                            }
                            channels = Some(value);
                        }
                        ID_SAMPLING_FREQUENCY => {
                            let value = read_float(bytes, &grandchild)?;
                            if sampling_frequency.is_some() {
                                return Err(Error::invalid_input(
                                    "duplicate webm audio sampling frequency",
                                ));
                            }
                            sampling_frequency = Some(value);
                        }
                        _ => {}
                    }
                    audio_reader.pos = grandchild.data_end;
                }
                info.audio_channels = channels;
                info.sampling_frequency = sampling_frequency;
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
    if track_type.ok_or_else(|| Error::invalid_input("webm track has no type"))? != 2 {
        return Err(Error::unsupported_format("webm track is not audio"));
    }
    if codec_id.ok_or_else(|| Error::invalid_input("webm track has no codec id"))? != b"A_OPUS" {
        return Err(Error::unsupported_format(
            "unsupported webm codec (expected A_OPUS)",
        ));
    }
    info.codec_private =
        codec_private.ok_or_else(|| Error::invalid_input("webm track has no opus header"))?;
    info.codec_delay_ns = codec_delay;
    info.track_number = track_number;
    Ok(())
}

fn walk_block_group(
    bytes: &[u8],
    element: &Element,
    cluster_ts: i128,
    block_count: &mut usize,
    timeline: &mut Timeline,
) -> Result<(), Error> {
    let mut reader = Reader::new(bytes, element.data_start, element.data_end);
    let mut blocks = 0usize;
    let mut discard_padding: Option<()> = None;
    while let Some(child) = reader.next()? {
        match child.id {
            ID_BLOCK => {
                note_block(bytes, &child, cluster_ts, timeline, &mut blocks)?;
            }
            ID_DISCARD_PADDING => {
                if discard_padding.replace(()).is_some() {
                    return Err(Error::invalid_input("duplicate webm discard padding"));
                }
            }
            ID_BLOCK_DURATION | ID_REFERENCE_BLOCK | ID_VOID | ID_CRC32 => {}
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
    timeline: &mut Timeline,
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
                note_block(bytes, &child, cluster_ts, timeline, block_count)?;
            }
            ID_BLOCK_GROUP => {
                walk_block_group(bytes, &child, cluster_ts, block_count, timeline)?;
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
        timestamp_scale: DEFAULT_TIMESTAMP_SCALE,
        audio_channels: None,
        sampling_frequency: None,
        block_count: 0,
    };
    let mut track_entries = 0usize;
    let mut timeline: Option<Timeline> = None;
    let mut segment_reader = Reader::new(bytes, segment_start, segment_end);
    while let Some(element) = segment_reader.next()? {
        match element.id {
            ID_SEEKHEAD => segment_reader.pos = element.data_end,
            ID_INFO => {
                // The TimestampScale here decides what one timestamp tick
                // means for the cluster timeline, so Info must precede the
                // clusters; every other Info child is informational.
                if timeline.is_some() {
                    return Err(Error::invalid_input("webm info appears after its clusters"));
                }
                let mut scale: Option<u64> = None;
                let mut info_reader = Reader::new(bytes, element.data_start, element.data_end);
                while let Some(child) = info_reader.next()? {
                    if child.id == ID_INFO_TIMESTAMP_SCALE {
                        let value = read_uint(bytes, &child)?;
                        if value == 0 {
                            return Err(Error::invalid_input(
                                "webm timestamp scale must not be zero",
                            ));
                        }
                        if scale.replace(value).is_some() {
                            return Err(Error::invalid_input("duplicate webm timestamp scale"));
                        }
                    }
                    info_reader.pos = child.data_end;
                }
                if let Some(value) = scale {
                    info.timestamp_scale = value;
                }
                segment_reader.pos = element.data_end;
            }
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
            ID_CLUSTER => {
                let timeline = timeline.get_or_insert_with(|| Timeline::new(info.timestamp_scale));
                walk_cluster(
                    bytes,
                    &element,
                    &mut segment_reader,
                    &mut info.block_count,
                    timeline,
                )?
            }
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
    if let Some(timeline) = &timeline {
        timeline.check()?;
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
    if info
        .audio_channels
        .is_some_and(|channels| channels != u64::from(head.channels))
    {
        return Err(Error::unsupported_format(
            "webm audio channel metadata disagrees with the opus header",
        ));
    }
    if info
        .sampling_frequency
        .is_some_and(|rate| !rate.is_finite() || rate <= 0.0)
    {
        // Per the Matroska A_OPUS mapping this is the original input rate
        // (OpusHead's input sample rate), not a decode rate; decoding always
        // happens at 48 kHz. Only reject metadata that is not a real rate.
        return Err(Error::invalid_input(
            "webm audio sampling frequency must be positive and finite",
        ));
    }
    if let Some(ns) = info.codec_delay_ns {
        let ns =
            i64::try_from(ns).map_err(|_| Error::invalid_input("webm codec delay out of range"))?;
        let delay = ns_to_samples(ns)?;
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
        if body.len() < 127 {
            out.push(0x80 | body.len() as u8);
        } else {
            // 127 is the all-ones (unknown-size) marker in a 1-byte vint.
            assert!(body.len() < 16_383);
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
        // Each 20 ms packet advances the timestamp by 20 ticks (1 ms each).
        for base in [0u64, 20, 40] {
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

    /// Offsets of every Cluster element, following the same unknown-size
    /// sibling-boundary walk as the validator.
    fn cluster_offsets(bytes: &[u8]) -> Vec<usize> {
        let mut reader = Reader::new(bytes, 0, bytes.len());
        let header = reader.next().unwrap().unwrap();
        reader.pos = header.data_end;
        let mut segment = None;
        while let Some(element) = reader.next().unwrap() {
            if element.id == ID_SEGMENT {
                segment = Some((element.data_start, element.data_end));
            }
            reader.pos = element.data_end;
        }
        let (start, end) = segment.unwrap();
        let mut offsets = Vec::new();
        let mut pos = start;
        while pos < end {
            let mut segment_reader = Reader::new(bytes, pos, end);
            let Some(element) = segment_reader.next().unwrap() else {
                break;
            };
            if element.id == ID_CLUSTER {
                offsets.push(element.start);
                if element.size.is_none() {
                    // The cluster ends at the first Segment-level sibling.
                    let mut inner = Reader::new(bytes, element.data_start, element.data_end);
                    while inner.pos < element.data_end {
                        let Some(child) = inner.next().unwrap() else {
                            break;
                        };
                        if is_segment_level(child.id) {
                            inner.pos = child.start;
                            break;
                        }
                        inner.pos = child.data_end;
                    }
                    pos = inner.pos;
                    continue;
                }
            }
            pos = element.data_end;
        }
        offsets
    }

    #[test]
    fn rejects_removed_middle_cluster_in_real_fixtures() {
        // Dropping a whole middle cluster leaves every later timestamp in
        // place; the timeline check must notice the gap. Review cases
        // chrome-missing-middle-cluster.webm and live-missing-middle-cluster
        // .webm, built here from the pristine fixtures.
        for name in ["chrome-mediarecorder.webm", "sample-pt-live.webm"] {
            let bytes = fixture(name);
            let offsets = cluster_offsets(&bytes);
            assert!(offsets.len() > 6, "{name}: {}", offsets.len());
            let mut mutated = bytes[..offsets[5]].to_vec();
            mutated.extend_from_slice(&bytes[offsets[6]..]);
            assert_eq!(
                decode_webm_opus(mutated, &CancelToken::new())
                    .unwrap_err()
                    .code,
                ErrorCode::InvalidInput,
                "{name}"
            );
        }
    }

    #[test]
    fn rejects_timestamp_gap_between_blocks() {
        // Two 20 ms packets whose timestamps are 1000 ms apart: the gap is
        // far beyond half a packet of tolerance, so the file cannot be
        // silently compressed. Review reproduction.
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &track_entry("A_OPUS", &opus_head_bytes())));
        let mut cluster = uint(TIMESTAMP, 0);
        cluster.extend(simple_block(0, 0, &opus_packet_bytes()));
        cluster.extend(simple_block(1000, 0, &opus_packet_bytes()));
        file.extend(el(CLUSTER, &cluster));
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_toc_mutations_with_shorter_packet_duration() {
        // Randomized review cases 1544 and 3176: flipping one Opus TOC byte
        // shortens that packet by 720 frames, so the following block
        // timestamp no longer matches the packet durations.
        let mut bytes = fixture("sample-pt-live.webm");
        assert_eq!(bytes[98_850], 0xf8);
        bytes[98_850] = 0xe8;
        assert_eq!(
            decode_webm_opus(bytes, &CancelToken::new())
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );

        let mut bytes = fixture("sample-pt.webm");
        assert_eq!(bytes[38_403], 0xf8);
        bytes[38_403] = 0xe8;
        assert_eq!(
            decode_webm_opus(bytes, &CancelToken::new())
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );
    }

    /// A complete TrackEntry: number, audio type, A_OPUS, CodecPrivate.
    fn full_track_entry() -> Vec<u8> {
        let mut entry = uint(TRACK_NUMBER, 1);
        entry.extend(uint(TRACK_TYPE, 2));
        entry.extend(str_el(CODEC_ID, "A_OPUS"));
        entry.extend(el(CODEC_PRIVATE, &opus_head_bytes()));
        entry
    }

    /// A file whose single track entry carries `extras` after the complete
    /// entry fields, with one 20 ms block so end-to-end decode works.
    fn file_with_track_extras(extras: &[u8]) -> Vec<u8> {
        let mut entry = full_track_entry();
        entry.extend_from_slice(extras);
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &el(TRACK_ENTRY, &entry)));
        let mut cluster = uint(TIMESTAMP, 0);
        cluster.extend(simple_block(0, 0, &opus_packet_bytes()));
        file.extend(el(CLUSTER, &cluster));
        file
    }

    #[test]
    fn rejects_missing_track_type_and_codec_id() {
        // Review cases missing-tracktype.webm and missing-codecid.webm.
        let mut entry = uint(TRACK_NUMBER, 1);
        entry.extend(str_el(CODEC_ID, "A_OPUS"));
        entry.extend(el(CODEC_PRIVATE, &opus_head_bytes()));
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &el(TRACK_ENTRY, &entry)));
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);

        let mut entry = uint(TRACK_NUMBER, 1);
        entry.extend(uint(TRACK_TYPE, 2));
        entry.extend(el(CODEC_PRIVATE, &opus_head_bytes()));
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &el(TRACK_ENTRY, &entry)));
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_duplicate_singular_track_fields() {
        // Review cases duplicate-codec.webm, duplicate-head.webm and a
        // duplicate CodecDelay (the complete entry has none, so add two).
        let mut delays = el(CODEC_DELAY, &6_500_000u64.to_be_bytes()[1..]);
        delays.extend(el(CODEC_DELAY, &6_500_000u64.to_be_bytes()[1..]));
        for extra in [
            str_el(CODEC_ID, "A_OPUS"),
            el(CODEC_PRIVATE, &opus_head_bytes()),
            delays,
        ] {
            let file = file_with_track_extras(&extra);
            assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
        }
        // A duplicate Audio master is rejected too.
        let audio = uint(&[0x9F], 1);
        let mut dup = el(&[0xE1], &audio);
        dup.extend(el(&[0xE1], &audio));
        let file = file_with_track_extras(&dup);
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_out_of_range_codec_delay() {
        // Review case delay-u64max.webm and the i64::MAX boundary: the
        // unsigned claim must not wrap into a tiny negative delay.
        for delay in [u64::MAX, i64::MAX as u64] {
            let file = file_with_track_extras(&el(CODEC_DELAY, &delay.to_be_bytes()));
            assert_eq!(
                decode_webm_opus(file, &CancelToken::new())
                    .unwrap_err()
                    .code,
                ErrorCode::InvalidInput,
                "delay {delay}"
            );
        }
    }

    #[test]
    fn checks_audio_metadata_against_the_opus_header() {
        // Review case track-channels3.webm: Audio/Channels declaring three
        // channels contradicts the mono OpusHead.
        let file = file_with_track_extras(&el(&[0xE1], &uint(&[0x9F], 3)));
        assert_eq!(
            decode_webm_opus(file, &CancelToken::new())
                .unwrap_err()
                .code,
            ErrorCode::UnsupportedFormat
        );

        // Consistent metadata (mono, 48 kHz) decodes fine.
        let file = file_with_track_extras(&el(&[0xE1], &uint(&[0x9F], 1)));
        let decoded = decode_webm_opus(file, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), 960 - 312);

        let mut audio = uint(&[0x9F], 1);
        audio.extend(el(&[0xB5], &48_000f32.to_be_bytes()));
        let file = file_with_track_extras(&el(&[0xE1], &audio));
        decode_webm_opus(file, &CancelToken::new()).unwrap();

        // The Matroska A_OPUS mapping declares the original input rate here
        // (OpusHead's input sample rate), not the decode rate: decoding
        // stays at 48 kHz, so any positive finite value is accepted.
        for rate in [16_000f32, 44_100.0, 48_000.0] {
            let mut audio = uint(&[0x9F], 1);
            audio.extend(el(&[0xB5], &rate.to_be_bytes()));
            let file = file_with_track_extras(&el(&[0xE1], &audio));
            let decoded = decode_webm_opus(file, &CancelToken::new()).unwrap();
            assert_eq!(decoded.samples.len(), 960 - 312, "rate {rate}");
        }

        // A rate that is not positive and finite is not a real input rate.
        for rate in [0.0f32, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut audio = uint(&[0x9F], 1);
            audio.extend(el(&[0xB5], &rate.to_be_bytes()));
            let file = file_with_track_extras(&el(&[0xE1], &audio));
            assert_eq!(
                decode_webm_opus(file, &CancelToken::new())
                    .unwrap_err()
                    .code,
                ErrorCode::InvalidInput,
                "rate {rate}"
            );
        }
    }

    #[test]
    fn accepts_original_input_rate_metadata_in_real_fixture() {
        // Review case real-input-rate16k.webm: the full ordinary WebM
        // fixture with only the original-input metadata changed to 16 kHz
        // (Audio/SamplingFrequency and, per the mapping, OpusHead's input
        // rate) must decode unchanged at 48 kHz.
        let mut bytes = fixture("sample-pt.webm");
        let pattern: Vec<u8> = [0xB5, 0x88]
            .into_iter()
            .chain(48_000f64.to_be_bytes())
            .collect();
        let at = bytes
            .windows(pattern.len())
            .position(|w| w == pattern)
            .unwrap();
        bytes[at + 2..at + 10].copy_from_slice(&16_000f64.to_be_bytes());
        let decoded = decode_webm_opus(bytes, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), 681_120);
    }

    #[test]
    fn rejects_duplicate_discard_padding() {
        // Review case duplicate-padding.webm: DiscardPadding is singular;
        // last-wins silently changed the trim.
        let mut group = el(BLOCK, &{
            let mut body = vec![0x81];
            body.extend(0i16.to_be_bytes());
            body.push(0);
            body.extend_from_slice(&opus_packet_bytes());
            body
        });
        group.extend(el(DISCARD_PADDING, &500_000u64.to_be_bytes()[2..]));
        group.extend(el(DISCARD_PADDING, &[0]));
        let mut cluster = uint(TIMESTAMP, 0);
        cluster.extend(el(BLOCK_GROUP, &group));
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &el(TRACK_ENTRY, &full_track_entry())));
        file.extend(el(CLUSTER, &cluster));
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    // --- Stream-wide cumulative timeline (review round 2) -------------------

    /// The first `count` real 20 ms audio packets from the Ogg fixture.
    fn real_packets(count: usize) -> Vec<Box<[u8]>> {
        let bytes = fixture("sample-pt-48k.opus");
        let packets = crate::opus::tests::ogg_packets(&bytes);
        packets[2..2 + count].to_vec()
    }

    /// A complete file holding one SimpleBlock per entry, in order:
    /// (relative timestamp in ticks, payload). Blocks are spread over
    /// clusters so the tiny test writer's size vints stay short; cluster
    /// boundaries do not reset the stream-wide timeline check.
    fn file_of_blocks(blocks: &[(i16, &[u8])]) -> Vec<u8> {
        let mut file = ebml_header("webm");
        file.extend_from_slice(SEGMENT);
        file.extend_from_slice(UNKNOWN_SIZE);
        file.extend(el(TRACKS, &track_entry("A_OPUS", &opus_head_bytes())));
        for chunk in blocks.chunks(32) {
            let mut cluster = uint(TIMESTAMP, 0);
            for &(timestamp, payload) in chunk {
                cluster.extend(simple_block(timestamp, 0, payload));
            }
            file.extend(el(CLUSTER, &cluster));
        }
        file
    }

    /// Review reproduction 20ms-jitter5-control: a uniform 20 ms stream whose
    /// timestamps deviate from the ideal grid by at most 5 ms stays well
    /// inside the cumulative 7 ms allowance and decodes.
    #[test]
    fn stream_wide_timeline_accepts_bounded_wall_clock_jitter() {
        let packets = real_packets(100);
        let jitter = |i: usize| match i {
            19 => 5,
            21 => -5,
            _ => 0,
        };
        let blocks: Vec<(i16, &[u8])> = packets
            .iter()
            .enumerate()
            .map(|(i, p)| (20 * i as i16 + jitter(i), &**p))
            .collect();
        let file = file_of_blocks(&blocks);
        let decoded = decode_webm_opus(file, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), 100 * 960 - 312);
    }

    /// Review reproduction 20ms-jitter5-dropped: removing one packet while
    /// keeping every surviving timestamp shifts the stream-wide residual by a
    /// whole 20 ms packet, which no per-block allowance can hide.
    #[test]
    fn stream_wide_timeline_rejects_dropped_packet_under_jitter() {
        let packets = real_packets(100);
        let jitter = |i: usize| match i {
            19 => 5,
            21 => -5,
            _ => 0,
        };
        let blocks: Vec<(i16, &[u8])> = packets
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 20)
            .map(|(i, p)| (20 * i as i16 + jitter(i), &**p))
            .collect();
        let file = file_of_blocks(&blocks);
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    /// Review reproductions drift-20ms-plus1ms/plus10ms: shifting every
    /// timestamp by a small amount that locally looks like jitter
    /// accumulates past the allowance; the cumulative residual rejects it.
    #[test]
    fn stream_wide_timeline_rejects_accumulated_drift() {
        for shift in [1i16, 10] {
            let packets = real_packets(100);
            let blocks: Vec<(i16, &[u8])> = packets
                .iter()
                .enumerate()
                .map(|(i, p)| ((20 + shift) * i as i16, &**p))
                .collect();
            let file = file_of_blocks(&blocks);
            assert_eq!(
                validate_code(&file),
                ErrorCode::InvalidInput,
                "shift {shift}"
            );
        }
    }

    /// Merges three consecutive same-configuration 20 ms packets into one
    /// 60 ms packet (Opus code 3, VBR) the way a muxer's repacketizer
    /// combines frames: one shared TOC, a 0x80-flagged frame count, lengths
    /// for all but the last frame, then the frame data without the per-
    /// packet TOC bytes.
    fn merged_60ms_packet(packets: &[Box<[u8]>]) -> Vec<u8> {
        let [a, b, c]: [&[u8]; 3] = packets
            .iter()
            .map(|p| &**p)
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let toc = a[0];
        assert_eq!(toc & 0x03, 0, "single-frame packets only");
        assert_eq!(toc >> 3, b[0] >> 3, "same configuration");
        assert_eq!(toc >> 3, c[0] >> 3, "same configuration");
        assert!(a.len() <= 255 && b.len() <= 255, "one-byte frame lengths");
        let mut merged = vec![
            (toc & 0xFC) | 0x03,
            0x80 | 3,
            (a.len() - 1) as u8,
            (b.len() - 1) as u8,
        ];
        merged.extend_from_slice(&a[1..]);
        merged.extend_from_slice(&b[1..]);
        merged.extend_from_slice(&c[1..]);
        merged
    }

    /// Review reproductions mixed-60-20-control/mixed-dropped-20-after60:
    /// with changing packet sizes, a per-block half-packet allowance keyed on
    /// the previous (60 ms) packet absorbed a dropped 20 ms packet; the
    /// stream-wide residual still jumps by the full 20 ms.
    #[test]
    fn stream_wide_timeline_rejects_dropped_packet_after_60ms_packet() {
        let packets = real_packets(20);
        let p60 = merged_60ms_packet(&packets[10..13]);
        let control: Vec<(i16, &[u8])> = vec![
            (0, &p60),
            (60, &packets[13]),
            (80, &packets[14]),
            (100, &packets[15]),
        ];
        let file = file_of_blocks(&control);
        let decoded = decode_webm_opus(file, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), 2_880 + 3 * 960 - 312);

        let dropped: Vec<(i16, &[u8])> = vec![(0, &p60), (80, &packets[14]), (100, &packets[15])];
        let file = file_of_blocks(&dropped);
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    /// Review reproduction toc-double20-to40: relabeling a real 20 ms packet
    /// as two equal-sized frames (code 1) doubles its duration; the old
    /// allowance used the corrupted duration itself, the cumulative residual
    /// exposes the mismatch immediately.
    #[test]
    fn stream_wide_timeline_rejects_packet_duration_extension() {
        let packets = real_packets(20);
        let at = packets
            .iter()
            .position(|p| p.len() % 2 == 1 && p[0] & 3 == 0)
            .unwrap();
        let p40: Vec<u8> = std::iter::once(packets[at][0] | 1)
            .chain(packets[at][1..].iter().copied())
            .collect();
        let blocks: Vec<(i16, &[u8])> =
            vec![(0, &p40), (20, &packets[at + 1]), (40, &packets[at + 2])];
        let file = file_of_blocks(&blocks);
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }

    /// Byte offsets of every block's two-byte relative timestamp field in a
    /// real fixture, following the same unknown-size sibling-boundary walk
    /// as the validator.
    fn block_timestamp_offsets(bytes: &[u8]) -> Vec<usize> {
        let mut reader = Reader::new(bytes, 0, bytes.len());
        let header = reader.next().unwrap().unwrap();
        reader.pos = header.data_end;
        let mut segment = None;
        while let Some(element) = reader.next().unwrap() {
            if element.id == ID_SEGMENT {
                segment = Some((element.data_start, element.data_end));
            }
            reader.pos = element.data_end;
        }
        let (segment_start, segment_end) = segment.unwrap();
        let mut offsets: Vec<usize> = Vec::new();
        let mut pos = segment_start;
        while pos < segment_end {
            let mut segment_reader = Reader::new(bytes, pos, segment_end);
            let Some(element) = segment_reader.next().unwrap() else {
                break;
            };
            if element.id == ID_CLUSTER {
                let mut cluster = Reader::new(bytes, element.data_start, element.data_end);
                while cluster.pos < element.data_end {
                    let Some(child) = cluster.next().unwrap() else {
                        break;
                    };
                    if element.size.is_none() && is_segment_level(child.id) {
                        cluster.pos = child.start;
                        break;
                    }
                    if child.id == ID_SIMPLE_BLOCK {
                        let track_len = bytes[child.data_start].leading_zeros() as usize + 1;
                        offsets.push(child.data_start + track_len);
                    }
                    cluster.pos = child.data_end;
                }
                if element.size.is_none() {
                    // The cluster ends at the first Segment-level sibling;
                    // resume the segment walk there.
                    pos = cluster.pos;
                    continue;
                }
            }
            pos = element.data_end;
        }
        offsets
    }

    /// Review reproduction chrome-middle-gap-20ms: shifting one middle block
    /// and every later block forward by 20 ms compresses an interior gap
    /// below half of the 60 ms packets; the cumulative residual jumps by the
    /// full 20 ms at the shifted block.
    #[test]
    fn rejects_shifted_block_tail_in_chrome_fixture() {
        let mut bytes = fixture("chrome-mediarecorder.webm");
        let offsets = block_timestamp_offsets(&bytes);
        assert!(offsets.len() > 100, "{}", offsets.len());
        for &at in &offsets[100..] {
            let relative = i16::from_be_bytes([bytes[at], bytes[at + 1]]);
            bytes[at..at + 2].copy_from_slice(&(relative + 20).to_be_bytes());
        }
        assert_eq!(
            decode_webm_opus(bytes, &CancelToken::new())
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );
    }

    // --- Track time transformations (review round 2) ------------------------

    const TRACK_TIMESTAMP_SCALE: &[u8] = &[0x23, 0x31, 0x4F];

    fn float_el(id: &[u8], value: f64) -> Vec<u8> {
        el(id, &value.to_be_bytes())
    }

    /// Review reproductions track-timescale2/track-timescale-nan: block time
    /// is (ClusterTimestamp + block timestamp * TrackTimestampScale) *
    /// TimestampScale, so a scale other than the default 1.0 (including NaN
    /// and infinities) changes the real timeline and must be rejected instead
    /// of validating the untransformed timestamps.
    #[test]
    fn rejects_track_timestamp_scale_transforms() {
        for value in [
            2.0f64,
            0.0,
            -1.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            let file = file_with_track_extras(&float_el(TRACK_TIMESTAMP_SCALE, value));
            assert_eq!(
                decode_webm_opus(file, &CancelToken::new())
                    .unwrap_err()
                    .code,
                ErrorCode::UnsupportedFormat,
                "scale {value}"
            );
        }

        // The default 1.0 is a no-op and stays accepted; duplicates reject.
        let file = file_with_track_extras(&float_el(TRACK_TIMESTAMP_SCALE, 1.0));
        decode_webm_opus(file, &CancelToken::new()).unwrap();

        let mut duplicate = float_el(TRACK_TIMESTAMP_SCALE, 1.0);
        duplicate.extend(float_el(TRACK_TIMESTAMP_SCALE, 1.0));
        let file = file_with_track_extras(&duplicate);
        assert_eq!(validate_code(&file), ErrorCode::InvalidInput);
    }
}
