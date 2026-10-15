//! Ogg/Opus: strict page validation before Symphonia demux (decision D2),
//! then the shared packet decoder. Vorbis and other Ogg codecs never reach
//! this module.

use super::{OpusHead, OpusPacket, decode_packets, packet_frames};
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
    let mut final_granule = 0i64;
    let mut decoded_so_far = 0u64;
    let mut packets = 0usize;
    let mut packet_open = false;
    let mut current: Vec<u8> = Vec::new();

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
                    return Err(Error::invalid_input(
                        "ogg stream does not start at page zero",
                    ));
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
        // A page with no lacing values neither completes nor opens anything:
        // the open-packet state must survive it unchanged.
        if segment_count > 0 {
            packet_open = lacing.last() == Some(&255);
        }
        if eos {
            if packet_open {
                return Err(Error::invalid_input("ogg stream ends mid-packet"));
            }
            seen_eos = true;
        }

        // Assemble the packets this page completes. Every lacing value below
        // 255 closes a packet, regardless of whether the page ends with an
        // open one; a page may complete several packets and then begin a
        // continuation. RFC 7845 fixes the two header packets (OpusHead,
        // OpusTags), so everything after them is an audio packet whose Opus
        // duration feeds the granule checks below.
        let body = &header[HEADER_LEN + segment_count..page_len];
        let mut pos = 0usize;
        let mut completed_any = false;
        let mut audio_on_page = false;
        let mut decoded_on_page = 0u64;
        for &lace in lacing {
            let take = usize::from(lace).min(body.len() - pos);
            current.extend_from_slice(&body[pos..pos + take]);
            pos += take;
            if lace < 255 {
                completed_any = true;
                packets = packets
                    .checked_add(1)
                    .ok_or_else(|| Error::invalid_input("ogg packet count overflow"))?;
                if packets > 2 {
                    // An audio packet: its duration counts toward this page's
                    // granule position.
                    let duration = packet_frames(&current)?;
                    decoded_on_page = decoded_on_page
                        .checked_add(duration as u64)
                        .ok_or_else(|| Error::invalid_input("ogg granule position overflow"))?;
                    audio_on_page = true;
                }
                current.clear();
            }
        }

        // Granule semantics, per RFC 7845 section 4: the granule position of
        // an audio page is the number of decoded 48 kHz samples (the pre-skip
        // region is decoded, so it is included) cumulated over the packets
        // completed so far, minus any end trim, which appears only on the
        // final EOS page.
        if !completed_any {
            // The page completes no packet: it must not claim a position.
            if granule != -1 {
                return Err(Error::invalid_input(
                    "ogg page granule without a completed packet",
                ));
            }
        } else if !audio_on_page {
            // A header page (OpusHead or OpusTags): the granule must be 0.
            // A nonzero value describes a stream cropped before encoding.
            if granule != 0 {
                return Err(Error::unsupported_format(
                    "cropped ogg streams are not supported",
                ));
            }
        } else if granule == -1 {
            return Err(Error::invalid_input(
                "ogg audio page is missing its granule position",
            ));
        } else {
            let decoded_before = decoded_so_far;
            let page_end = decoded_before
                .checked_add(decoded_on_page)
                .ok_or_else(|| Error::invalid_input("ogg granule position overflow"))?;
            if eos {
                // End trim is allowed only on the EOS page, and it may not
                // reach back past the packets completed before this page.
                if granule < decoded_before as i64 || granule > page_end as i64 {
                    return Err(Error::invalid_input(
                        "ogg end trim exceeds the samples decoded on the final page",
                    ));
                }
            } else if granule != page_end as i64 {
                return Err(Error::invalid_input(
                    "ogg granule position does not match the completed packet durations",
                ));
            }
            decoded_so_far = page_end;
        }
        if granule != -1 {
            final_granule = granule;
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
        final_granule,
    })
}

/// Decodes an Ogg/Opus file already identified by [`is_ogg_opus`].
pub(crate) fn decode_ogg_opus(bytes: Vec<u8>, cancel: &CancelToken) -> Result<DecodedAudio, Error> {
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
            return Err(Error::new(
                ErrorCode::Cancelled,
                "transcription was cancelled",
            ));
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
            data: packet.data,
            trim_start: 0,
            trim_end: packet.trim_end.get() as usize,
        });
    }

    // No partial results: the demuxer must yield exactly the audio packets
    // the strict page walk counted. RFC 7845 fixes the two header packets
    // (OpusHead, OpusTags), which Symphonia consumes before audio begins.
    let audio_packets = validation
        .packets
        .checked_sub(2)
        .ok_or_else(|| Error::invalid_input("ogg opus stream contains no audio packets"))?;
    if packets.len() != audio_packets {
        return Err(Error::invalid_input(
            "ogg packet count mismatch (lost packets)",
        ));
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
    use crate::opus::tests::{fixture, ogg_packets};

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

    /// Builds one Ogg page with a valid CRC. `laces` is the segment table
    /// and `payload` the concatenated packet bytes it describes; granule -1
    /// means the page completes no packet.
    fn page(seq: u32, flags: u8, granule: i64, laces: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut page = Vec::new();
        page.extend_from_slice(b"OggS");
        page.push(0); // stream structure version
        page.push(flags);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&123_456u32.to_le_bytes());
        page.extend_from_slice(&seq.to_le_bytes());
        page.extend_from_slice(&0u32.to_le_bytes()); // CRC placeholder
        page.push(laces.len() as u8);
        page.extend_from_slice(laces);
        page.extend_from_slice(payload);
        let crc = crc32(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        page
    }

    /// Lacing values for whole packets: each packet is full 255-byte laces
    /// plus a final short lace.
    fn packet_laces(payloads: &[&[u8]]) -> Vec<u8> {
        let mut laces = Vec::new();
        for payload in payloads {
            laces.extend(std::iter::repeat_n(255, payload.len() / 255));
            laces.push((payload.len() % 255) as u8);
        }
        laces
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

    #[test]
    fn real_120_second_ogg_passes() {
        // Exactly 120.0 s after its pre-skip and end trim: the review round 1
        // reproduction that the old untrimmed budget rejected.
        let bytes = fixture("silence120.opus");
        let decoded = decode_ogg_opus(bytes, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), 5_760_000);
    }

    /// Builds an Ogg/Opus stream of `audio_count` identical 20 ms packets
    /// with a zero pre-skip header and exact granules.
    fn repeated_packet_ogg(audio_count: usize) -> Vec<u8> {
        let fixture_packets = crate::opus::tests::ogg_packets(&opus_fixture());
        let mut head = b"OpusHead".to_vec();
        head.extend([1, 1, 0, 0]); // version 1, mono, pre-skip 0
        head.extend([0x80, 0xBB, 0x00, 0x00]); // 48 kHz input rate
        head.extend([0, 0]); // gain 0
        head.push(0); // channel mapping family 0
        let tags = &fixture_packets[1];
        let packet = &fixture_packets[2];

        let mut bytes = page(0, FLAG_BOS, 0, &packet_laces(&[&head]), &head);
        bytes.extend(page(1, 0, 0, &packet_laces(&[tags]), tags));
        let per_page = 50usize;
        let mut seq = 2u32;
        let mut decoded = 0u64;
        let mut remaining = audio_count;
        while remaining > 0 {
            let count = per_page.min(remaining);
            remaining -= count;
            let complete = remaining == 0;
            let flags = if complete { FLAG_EOS } else { 0 };
            // Every page ends the packets it carries; the granule is the
            // cumulative decoded samples including this page.
            decoded += (count * 960) as u64;
            let payloads: Vec<&[u8]> = std::iter::repeat_n(packet.as_ref(), count).collect();
            let laces = packet_laces(&payloads);
            let body: Vec<u8> = payloads.concat();
            bytes.extend(page(seq, flags, decoded as i64, &laces, &body));
            seq += 1;
        }
        bytes
    }

    #[test]
    fn ogg_boundary_120_seconds_exact_and_plus_one_packet() {
        // 120 s / 20 ms = 6,000 packets; exactly that must pass...
        let bytes = repeated_packet_ogg(6_000);
        let decoded = decode_ogg_opus(bytes, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), 5_760_000);

        // ...and one packet more must fail as too long.
        let bytes = repeated_packet_ogg(6_001);
        let err = decode_ogg_opus(bytes, &CancelToken::new()).unwrap_err();
        assert_eq!(err.code, ErrorCode::AudioTooLong);
    }

    /// Header pages for the continuation tests: BOS page with OpusHead, then
    /// a tags page, matching RFC 7845 section 3.
    fn header_pages(packets: &[Box<[u8]>]) -> Vec<u8> {
        let mut bytes = page(0, FLAG_BOS, 0, &packet_laces(&[&packets[0]]), &packets[0]);
        bytes.extend(page(1, 0, 0, &packet_laces(&[&packets[1]]), &packets[1]));
        bytes
    }

    #[test]
    fn continued_audio_page_is_valid() {
        // A page completes one 20 ms packet and begins the next, which
        // completes on the EOS page. Review case ogg-continued-audio.opus.
        let packets = ogg_packets(&opus_fixture());
        let first = &packets[2];
        let big = packets[2..]
            .iter()
            .find(|p| p.len() > 255)
            .expect("fixture has an audio packet larger than 255 bytes");

        let mut bytes = header_pages(&packets);
        let mut payload = first.to_vec();
        payload.extend_from_slice(&big[..255]);
        bytes.extend(page(2, 0, 960, &[first.len() as u8, 255], &payload));
        bytes.extend(page(
            3,
            FLAG_CONTINUED | FLAG_EOS,
            1920,
            &[(big.len() - 255) as u8],
            &big[255..],
        ));
        let validation = validate_pages(&bytes).unwrap();
        assert_eq!(validation.packets, 4);
        let decoded = decode_ogg_opus(bytes, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), 1_608);
    }

    #[test]
    fn continued_tags_and_empty_page_are_valid() {
        // The tags packet spans two pages, and an empty page sits between
        // the two halves of a split audio packet; open-packet state must
        // survive both. Review case ogg-continued-tags.opus plus an
        // empty-page variant.
        let packets = ogg_packets(&opus_fixture());
        let first = &packets[2];
        let big = packets[2..]
            .iter()
            .find(|p| p.len() > 255)
            .expect("fixture has an audio packet larger than 255 bytes");
        let mut tags = packets[1].to_vec();
        tags.extend(std::iter::repeat_n(0, 300));

        let mut bytes = page(0, FLAG_BOS, 0, &packet_laces(&[&packets[0]]), &packets[0]);
        // Tags: opened on page 1, completed on page 2 (a header page, so its
        // granule is 0).
        bytes.extend(page(1, 0, -1, &[255], &tags[..255]));
        bytes.extend(page(
            2,
            FLAG_CONTINUED,
            0,
            &[(tags.len() - 255) as u8],
            &tags[255..],
        ));
        // Audio: page 3 completes one packet and opens the big one; page 4
        // is empty (no packet completes, granule -1, state unchanged); page 5
        // completes the big one.
        let mut payload = first.to_vec();
        payload.extend_from_slice(&big[..255]);
        bytes.extend(page(3, 0, 960, &[first.len() as u8, 255], &payload));
        bytes.extend(page(4, FLAG_CONTINUED, -1, &[], &[]));
        bytes.extend(page(
            5,
            FLAG_CONTINUED | FLAG_EOS,
            1920,
            &[(big.len() - 255) as u8],
            &big[255..],
        ));
        let decoded = decode_ogg_opus(bytes, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), 1_608);
    }

    #[test]
    fn rejects_crc_consistent_granule_mutations() {
        // The tags completion page must have granule 0 (a nonzero value is a
        // cropped stream). Review case granule-page1.opus.
        let mut bytes = opus_fixture();
        let offsets = page_offsets(&bytes);
        bytes[offsets[1] + 6..offsets[1] + 14].copy_from_slice(&10i64.to_le_bytes());
        fix_crcs(&mut bytes, offsets[1]);
        assert_eq!(err_code(&bytes), ErrorCode::UnsupportedFormat);

        // A first non-EOS audio page granule below, or above, the samples its
        // completed packets decode to is impossible. Review cases
        // granule-page2.opus (0), ogg-firstaudio-small.opus (1) and
        // ogg-firstaudio-plus.opus (50000).
        for granule in [0i64, 1, 50_000] {
            let mut bytes = opus_fixture();
            let offsets = page_offsets(&bytes);
            bytes[offsets[2] + 6..offsets[2] + 14].copy_from_slice(&granule.to_le_bytes());
            fix_crcs(&mut bytes, offsets[2]);
            assert_eq!(
                err_code(&bytes),
                ErrorCode::InvalidInput,
                "granule {granule}"
            );
        }

        // The same equality rule applies to every intermediate audio page:
        // setting page 4's granule back to page 3's value is impossible.
        let mut bytes = opus_fixture();
        let offsets = page_offsets(&bytes);
        bytes[offsets[3] + 6..offsets[3] + 14].copy_from_slice(&48_000i64.to_le_bytes());
        fix_crcs(&mut bytes, offsets[3]);
        assert_eq!(err_code(&bytes), ErrorCode::InvalidInput);
    }

    #[test]
    fn eos_end_trim_within_the_final_page_is_valid() {
        // End trim shorter than one packet, and an end trim reaching back a
        // whole packet: both are allowed on the EOS page. Review cases
        // ogg-short-tail.opus (488 output samples) and
        // ogg-eos-trim-whole-packet.opus (1188).
        let packets = ogg_packets(&opus_fixture());

        let mut bytes = header_pages(&packets);
        bytes.extend(page(
            2,
            FLAG_EOS,
            800,
            &packet_laces(&[&packets[2]]),
            &packets[2],
        ));
        let decoded = decode_ogg_opus(bytes, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), 488);

        let mut bytes = header_pages(&packets);
        let mut payload = packets[2].to_vec();
        payload.extend_from_slice(&packets[3]);
        bytes.extend(page(
            2,
            FLAG_EOS,
            1500,
            &packet_laces(&[&packets[2], &packets[3]]),
            &payload,
        ));
        let decoded = decode_ogg_opus(bytes, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), 1_188);
    }
}
