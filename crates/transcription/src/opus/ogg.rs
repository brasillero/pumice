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

#[derive(Debug)]
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

        let continued = flags & FLAG_CONTINUED != 0;
        let bos = flags & FLAG_BOS != 0;
        let eos = flags & FLAG_EOS != 0;
        // Chained streams are a format limitation (like the existing Vorbis
        // behavior), so this is checked before the structural serial and
        // sequence rules.
        if seen_eos || (bos && seen_bos) {
            return Err(Error::unsupported_format(
                "chained ogg streams are not supported",
            ));
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

        if bos {
            if continued {
                return Err(Error::invalid_input("ogg packet continuation mismatch"));
            }
            seen_bos = true;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opus::tests::fixture;

    fn opus_fixture() -> Vec<u8> {
        fixture("sample-pt-48k.opus")
    }

    /// Offset of each Ogg page start.
    fn page_offsets(bytes: &[u8]) -> Vec<usize> {
        let mut offsets = Vec::new();
        let mut offset = 0usize;
        while offset < bytes.len() {
            assert_eq!(&bytes[offset..offset + 4], b"OggS");
            let segments = usize::from(bytes[offset + 26]);
            let body_len: usize = bytes[offset + 27..offset + 27 + segments]
                .iter()
                .map(|&v| usize::from(v))
                .sum();
            offsets.push(offset);
            offset += 27 + segments + body_len;
        }
        offsets
    }

    /// Rewrites the stored CRC of every page after `from` so mutations in
    /// page bodies stay CRC-consistent.
    fn fix_crcs(bytes: &mut [u8], from: usize) {
        for &offset in &page_offsets(bytes) {
            if offset < from {
                continue;
            }
            let segments = usize::from(bytes[offset + 26]);
            let body_len: usize = bytes[offset + 27..offset + 27 + segments]
                .iter()
                .map(|&v| usize::from(v))
                .sum();
            let page_len = 27 + segments + body_len;
            bytes[offset + 22..offset + 26].fill(0);
            let crc = crc32(&bytes[offset..offset + page_len]);
            bytes[offset + 22..offset + 26].copy_from_slice(&crc.to_le_bytes());
        }
    }

    fn err_code(bytes: &[u8]) -> ErrorCode {
        validate_pages(bytes).unwrap_err().code
    }

    #[test]
    fn detects_ogg_opus_only() {
        assert!(is_ogg_opus(&opus_fixture()));
        assert!(!is_ogg_opus(&fixture("sample-pt.ogg")));
        assert!(!is_ogg_opus(&fixture("sample-pt.webm")));
        assert!(!is_ogg_opus(b""));
        assert!(!is_ogg_opus(b"OggS"));
        assert!(!is_ogg_opus(b"OggS\x00\x00\x00"));
    }

    #[test]
    fn crc32_matches_ogg_spec() {
        // Ogg CRC of "123456789" (poly 0x04C11DB7, init 0, no reflection,
        // no final xor).
        assert_eq!(crc32(b"123456789"), 0x89A1_897F);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn valid_fixture_passes_page_validation() {
        let bytes = opus_fixture();
        let validation = validate_pages(&bytes).unwrap();
        assert_eq!(validation.packets, 712);
        assert_eq!(validation.final_granule, 681_432);
    }

    #[test]
    fn rejects_corrupted_page_payload() {
        let mut bytes = opus_fixture();
        let second = page_offsets(&bytes)[2];
        bytes[second + 40] ^= 0x01;
        assert_eq!(err_code(&bytes), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_corrupted_page_header() {
        let mut bytes = opus_fixture();
        // Break the capture pattern; the CRC would still match because the
        // header bytes are covered... so also fix nothing: validation must
        // fail on the capture check before CRC.
        let third = page_offsets(&bytes)[3];
        bytes[third] = b'X';
        assert_eq!(err_code(&bytes), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_page_sequence_gap() {
        let mut bytes = opus_fixture();
        let offsets = page_offsets(&bytes);
        // Drop the third page entirely: the following page's sequence number
        // no longer matches.
        let start = offsets[2];
        let end = offsets[3];
        bytes.drain(start..end);
        assert_eq!(err_code(&bytes), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_truncated_final_page() {
        let mut bytes = opus_fixture();
        bytes.truncate(bytes.len() - 30);
        assert_eq!(err_code(&bytes), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_missing_eos_page() {
        let mut bytes = opus_fixture();
        let offsets = page_offsets(&bytes);
        bytes.truncate(offsets[offsets.len() - 1]);
        assert_eq!(err_code(&bytes), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_chained_stream() {
        let mut bytes = opus_fixture();
        bytes.extend(opus_fixture());
        assert_eq!(err_code(&bytes), ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn rejects_data_after_eos() {
        // A complete page (the original BOS page) appended after the EOS
        // page is a chained stream.
        let mut bytes = opus_fixture();
        let first_page_len = page_offsets(&bytes)[1];
        bytes.extend_from_slice(&opus_fixture()[..first_page_len]);
        assert_eq!(err_code(&bytes), ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn rejects_stream_ending_mid_packet() {
        let mut bytes = opus_fixture();
        let offsets = page_offsets(&bytes);
        // Replace the final page with one whose single lacing value (255)
        // leaves a packet open at the EOS flag.
        let last = *offsets.last().unwrap();
        bytes.truncate(last);
        let sequence = offsets.len() as u32;
        let mut page = Vec::new();
        page.extend_from_slice(b"OggS");
        page.push(0);
        page.push(FLAG_EOS);
        page.extend_from_slice(&(-1i64).to_le_bytes());
        page.extend_from_slice(&0u32.to_le_bytes()); // serial; fixed below
        page.extend_from_slice(&sequence.to_le_bytes());
        page.extend_from_slice(&0u32.to_le_bytes()); // crc placeholder
        page.push(1); // one segment
        page.push(255);
        page.extend_from_slice(&[0x55u8; 100]); // unfinished packet body
        let serial = u32::from_le_bytes(bytes[14..18].try_into().unwrap());
        page[14..18].copy_from_slice(&serial.to_le_bytes());
        let crc = crc32(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        bytes.extend_from_slice(&page);
        assert_eq!(err_code(&bytes), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_backwards_granule() {
        let mut bytes = opus_fixture();
        let offsets = page_offsets(&bytes);
        let last = *offsets.last().unwrap();
        let previous = offsets[offsets.len() - 2];
        let previous_granule =
            i64::from_le_bytes(bytes[previous + 6..previous + 14].try_into().unwrap());
        // Set the final page's granule below the previous page's.
        let stale = previous_granule - 1;
        bytes[last + 6..last + 14].copy_from_slice(&stale.to_le_bytes());
        fix_crcs(&mut bytes, last);
        assert_eq!(err_code(&bytes), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_cropped_initial_granule() {
        let mut bytes = opus_fixture();
        bytes[6..14].copy_from_slice(&3840i64.to_le_bytes());
        fix_crcs(&mut bytes, 0);
        assert_eq!(err_code(&bytes), ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn rejects_multiplexed_serial() {
        let mut bytes = opus_fixture();
        let offsets = page_offsets(&bytes);
        let third = offsets[2];
        bytes[third + 14..third + 18].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        fix_crcs(&mut bytes, third);
        assert_eq!(err_code(&bytes), ErrorCode::InvalidInput);
    }

    #[test]
    fn rejects_bad_version_and_flags() {
        let mut bytes = opus_fixture();
        bytes[4] = 1;
        assert_eq!(err_code(&bytes), ErrorCode::InvalidInput);

        let mut bytes = opus_fixture();
        bytes[5] = 0x08;
        fix_crcs(&mut bytes, 0);
        assert_eq!(err_code(&bytes), ErrorCode::InvalidInput);
    }
}
