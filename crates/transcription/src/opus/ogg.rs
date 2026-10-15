//! Ogg/Opus: strict page validation before Symphonia demux (decision D2),
//! then the shared packet decoder. Vorbis and other Ogg codecs never reach
//! this module.

use super::{OpusHead, OpusPacket, decode_packets};
use crate::audio::{DecodedAudio, map_symphonia_error};
use crate::engine::CancelToken;
use crate::{Error, ErrorCode};
use std::io::Cursor;
use std::sync::OnceLock;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSourceStream, ReadOnlySource};

const HEADER_LEN: usize = 27;
const FLAG_CONTINUED: u8 = 0x01;
const FLAG_BOS: u8 = 0x02;
const FLAG_EOS: u8 = 0x04;

/// Peeks at the first packet of an Ogg stream: true only for Opus streams.
pub(crate) fn is_ogg_opus(bytes: &[u8]) -> bool {
    first_packet(bytes).is_some_and(|payload| payload.starts_with(b"OpusHead"))
}

/// First packet payload of the first page, or None if the header does not
/// parse. Packet length comes from the segment table (a lacing value below
/// 255 ends the packet).
fn first_packet(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < HEADER_LEN || &bytes[0..4] != b"OggS" {
        return None;
    }
    let segments = usize::from(bytes[26]);
    let table_end = HEADER_LEN.checked_add(segments)?;
    let body = bytes.get(table_end..)?;
    let mut packet_len = 0usize;
    for &lace in &bytes[HEADER_LEN..table_end] {
        packet_len = packet_len.checked_add(usize::from(lace))?;
        if lace < 255 {
            return body.get(..packet_len);
        }
    }
    Some(body)
}

/// Ogg CRC-32: polynomial 0x04C11DB7, init 0, MSB-first, no final xor.
fn crc_table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            let mut crc = (i as u32) << 24;
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04C1_1DB7
                } else {
                    crc << 1
                };
            }
            *entry = crc;
        }
        table
    })
}

fn crc32(bytes: &[u8]) -> u32 {
    let table = crc_table();
    let mut crc = 0u32;
    for &byte in bytes {
        crc = (crc << 8) ^ table[(((crc >> 24) as u8) ^ byte) as usize];
    }
    crc
}

struct PageValidation {
    /// Completed packets in the stream (header packets included).
    packets: usize,
    /// Granule position of the final page: decoded samples including the
    /// pre-skip, i.e. `output + pre_skip`.
    final_granule: i64,
}

/// Strict Ogg envelope validation: capture pattern, version, per-page CRC,
/// one serial, consecutive sequence numbers, continuation consistency, one
/// BOS/EOS pair, no chained or multiplexed streams, complete packets and
/// sane granule positions. Symphonia's Ogg reader has loss-recovery paths;
/// this pass guarantees they cannot trigger, so a packet silently skipped
/// by the demuxer is detected by the packet-count check in
/// [`decode_ogg_opus`].
fn validate_pages(bytes: &[u8]) -> Result<PageValidation, Error> {
    let mut offset = 0usize;
    let mut serial: Option<u32> = None;
    let mut expected_seq = 0u32;
    let mut seen_bos = false;
    let mut seen_eos = false;
    let mut prev_granule = 0i64;
    let mut packets = 0usize;
    let mut packet_open = false;

    while offset < bytes.len() {
        let available = bytes.len() - offset;
        if available < HEADER_LEN {
            return Err(Error::invalid_input("truncated ogg page header"));
        }
        let header = &bytes[offset..];
        if &header[0..4] != b"OggS" {
            return Err(Error::invalid_input("invalid ogg page capture pattern"));
        }
        if header[4] != 0 {
            return Err(Error::invalid_input("unsupported ogg page version"));
        }
        let flags = header[5];
        if flags & !(FLAG_CONTINUED | FLAG_BOS | FLAG_EOS) != 0 {
            return Err(Error::invalid_input("invalid ogg page flags"));
        }
        let granule = i64::from_le_bytes(header[6..14].try_into().unwrap());
        let page_serial = u32::from_le_bytes(header[14..18].try_into().unwrap());
        let sequence = u32::from_le_bytes(header[18..22].try_into().unwrap());
        let stored_crc = u32::from_le_bytes(header[22..26].try_into().unwrap());
        let segment_count = usize::from(header[26]);
        if available < HEADER_LEN + segment_count {
            return Err(Error::invalid_input("truncated ogg segment table"));
        }
        let lacing = &header[HEADER_LEN..HEADER_LEN + segment_count];
        let body_len: usize = lacing.iter().map(|&v| usize::from(v)).sum();
        let page_len = HEADER_LEN
            .checked_add(segment_count)
            .and_then(|v| v.checked_add(body_len))
            .ok_or_else(|| Error::invalid_input("ogg page length overflow"))?;
        if available < page_len {
            return Err(Error::invalid_input("truncated ogg page body"));
        }
        let page = &mut bytes[offset..offset + page_len].to_vec();
        page[22..26].fill(0);
        if crc32(page) != stored_crc {
            return Err(Error::invalid_input("ogg page checksum mismatch"));
        }

        match serial {
            Some(expected) if expected != page_serial => {
                return Err(Error::invalid_input("ogg stream changes serial number"));
            }
            Some(_) => {}
            None => {
                if sequence != 0 {
                    return Err(Error::invalid_input("ogg stream does not start at page zero"));
                }
                serial = Some(page_serial);
            }
        }
        if sequence != expected_seq {
            return Err(Error::invalid_input("ogg page sequence gap"));
        }
        expected_seq = expected_seq.wrapping_add(1);

        let continued = flags & FLAG_CONTINUED != 0;
        let bos = flags & FLAG_BOS != 0;
        let eos = flags & FLAG_EOS != 0;
        if bos {
            if seen_bos || expected_seq != 1 || continued {
                return Err(Error::unsupported_format(
                    "chained ogg streams are not supported",
                ));
            }
            seen_bos = true;
        }
        if seen_eos {
            return Err(Error::unsupported_format(
                "chained ogg streams are not supported",
            ));
        }
        if continued != packet_open {
            return Err(Error::invalid_input("ogg packet continuation mismatch"));
        }
        packet_open = lacing.last() == Some(&255);
        if eos {
            if packet_open {
                return Err(Error::invalid_input("ogg stream ends mid-packet"));
            }
            seen_eos = true;
        }

        let completes_packet = !packet_open;
        if completes_packet {
            packets = packets
                .checked_add(lacing.iter().filter(|&&v| v < 255).count())
                .ok_or_else(|| Error::invalid_input("ogg packet count overflow"))?;
        }
        if granule == -1 {
            if completes_packet {
                return Err(Error::invalid_input("ogg page is missing its granule position"));
            }
        } else {
            if granule < 0 {
                return Err(Error::invalid_input("invalid ogg granule position"));
            }
            if expected_seq == 1 && granule != 0 {
                return Err(Error::unsupported_format(
                    "cropped ogg streams are not supported",
                ));
            }
            if granule < prev_granule {
                return Err(Error::invalid_input("ogg granule position moved backwards"));
            }
            prev_granule = granule;
        }

        offset = offset
            .checked_add(page_len)
            .ok_or_else(|| Error::invalid_input("ogg page offset overflow"))?;
    }

    if !seen_bos || !seen_eos {
        return Err(Error::invalid_input("ogg stream is missing its end marker"));
    }
    Ok(PageValidation {
        packets,
        final_granule: prev_granule,
    })
}

/// Decodes an Ogg/Opus file already identified by [`is_ogg_opus`].
pub(crate) fn decode_ogg_opus(
    bytes: Vec<u8>,
    cancel: &CancelToken,
) -> Result<DecodedAudio, Error> {
    let validation = validate_pages(&bytes)?;

    let source = ReadOnlySource::new(Cursor::new(bytes));
    let mss = MediaSourceStream::new(Box::new(source), Default::default());
    let mut format = symphonia::default::formats::OggReader::try_new(mss, FormatOptions::default())
        .map_err(map_symphonia_error)?;

    let audio_tracks = format
        .tracks()
        .iter()
        .filter(|t| {
            t.codec_params
                .as_ref()
                .map(|p| p.is_audio())
                .unwrap_or(false)
        })
        .count();
    if audio_tracks != 1 {
        return Err(Error::unsupported_format(
            "audio file must contain exactly one audio track",
        ));
    }
    let track_id = format.tracks()[0].id;
    let extra_data = format.tracks()[0]
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .and_then(|p| p.extra_data.as_deref())
        .ok_or_else(|| Error::invalid_input("ogg opus stream has no opus header"))?;
    let head = OpusHead::parse(extra_data)?;

    let mut packets: Vec<OpusPacket> = Vec::new();
    loop {
        if cancel.is_cancelled() {
            return Err(Error::new(ErrorCode::Cancelled, "transcription was cancelled"));
        }
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(err) => return Err(map_symphonia_error(err)),
        };
        if packet.track_id != track_id {
            return Err(Error::unsupported_format(
                "audio file switched to a different track",
            ));
        }
        if packet.data.is_empty() {
            return Err(Error::invalid_input("ogg stream contains an empty packet"));
        }
        packets.push(OpusPacket {
            data: packet.data.into(),
            trim_start: 0,
            trim_end: packet.trim_end.get() as usize,
        });
    }

    // No partial results: the demuxer must yield exactly the audio packets
    // the strict page walk counted. RFC 7845 fixes the two header packets
    // (OpusHead, OpusTags), which Symphonia consumes before audio begins.
    let audio_packets = validation.packets.checked_sub(2).ok_or_else(|| {
        Error::invalid_input("ogg opus stream contains no audio packets")
    })?;
    if packets.len() != audio_packets {
        return Err(Error::invalid_input("ogg packet count mismatch (lost packets)"));
    }
    let decoded = decode_packets(&head, &packets, cancel)?;
    if decoded.samples.len() + head.pre_skip != validation.final_granule as usize {
        return Err(Error::invalid_input(
            "ogg stream length does not match its declared duration",
        ));
    }
    Ok(decoded)
}
