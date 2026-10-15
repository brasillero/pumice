//! Opus decoding (feature `opus`): validated `OpusHead`, Pumice-owned packets
//! and the reference libopus decoder, independent of Symphonia.
//!
//! Opus always decodes at 48 kHz. The shared pipeline then applies the
//! existing mono downmix and rubato resampling to 16 kHz, so this module
//! returns 48 kHz mono like any other decoder.

pub(crate) mod ogg;
pub(crate) mod webm;

use crate::audio::{DecodedAudio, MAX_AUDIO_SECONDS, downmix_to_mono};
use crate::engine::CancelToken;
use crate::{Error, ErrorCode};

pub(crate) const OPUS_SAMPLE_RATE: u32 = 48_000;
/// An Opus packet decodes to at most 120 ms, i.e. 5,760 frames per channel.
pub(crate) const MAX_PACKET_FRAMES: usize = 5_760;
/// Stereo-interleaved scratch size for one packet.
const MAX_PACKET_SAMPLES: usize = MAX_PACKET_FRAMES * 2;
/// Total output cap at 48 kHz (the shared 120 s limit), checked around every
/// packet with actual output counts.
const MAX_OUTPUT_FRAMES: usize = 5_760_000; // 120 s * 48 kHz

/// One Opus packet plus the 48 kHz samples to remove from its decoded output.
/// Trims come from the container (Ogg end granule, WebM DiscardPadding); the
/// stream-level pre-skip is applied separately, exactly once, in
/// [`decode_packets`].
#[derive(Clone)]
pub(crate) struct OpusPacket {
    pub data: Box<[u8]>,
    pub trim_start: usize,
    pub trim_end: usize,
}

/// A validated `OpusHead` (RFC 7845 section 5).
#[derive(Debug)]
pub(crate) struct OpusHead {
    channels: u16,
    pre_skip: usize,
    /// Signed output gain in Q8 dB units.
    gain_db_q8: i16,
}

impl OpusHead {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 19 || &bytes[0..8] != b"OpusHead" {
            return Err(Error::invalid_input("missing or malformed opus header"));
        }
        if bytes[8] != 1 {
            return Err(Error::invalid_input("unsupported opus header version"));
        }
        let channels = u16::from(bytes[9]);
        if !matches!(channels, 1 | 2) {
            return Err(Error::unsupported_format(format!(
                "cannot downmix {channels} opus channels to mono"
            )));
        }
        let pre_skip = usize::from(u16::from_le_bytes([bytes[10], bytes[11]]));
        // The original input sample rate (bytes 12..16) is informational.
        let gain_db_q8 = i16::from_le_bytes([bytes[16], bytes[17]]);
        match bytes[18] {
            0 => {
                if bytes.len() != 19 {
                    return Err(Error::invalid_input("malformed opus header"));
                }
            }
            1 => {
                // Channel mapping family 1 is only accepted in its canonical
                // mono/stereo form, which a plain libopus decoder handles
                // identically to family 0. Anything else needs a multistream
                // decoder and is rejected.
                let expected_len = 21 + usize::from(channels);
                if bytes.len() != expected_len {
                    return Err(Error::invalid_input("malformed opus header"));
                }
                let stream_count = usize::from(bytes[19]);
                let coupled_count = usize::from(bytes[20]);
                let canonical = match channels {
                    1 => stream_count == 1 && coupled_count == 0 && bytes[21] == 0,
                    _ => {
                        stream_count == 1
                            && coupled_count == 1
                            && bytes[21] == 0
                            && bytes[22] == 1
                    }
                };
                if !canonical {
                    return Err(Error::unsupported_format(
                        "non-canonical opus channel mapping is not supported",
                    ));
                }
            }
            _ => {
                return Err(Error::unsupported_format(
                    "unsupported opus channel mapping family",
                ));
            }
        }
        Ok(Self {
            channels,
            pre_skip,
            gain_db_q8,
        })
    }
}

/// Number of 48 kHz frames a packet decodes to, rejecting empty packets and
/// packets over the 120 ms Opus limit before any decode buffer is used.
fn packet_frames(data: &[u8]) -> Result<usize, Error> {
    if data.is_empty() {
        return Err(Error::invalid_input("opus stream contains an empty packet"));
    }
    let frames = opus::packet::get_nb_samples(data, OPUS_SAMPLE_RATE)
        .map_err(|_| Error::invalid_input("malformed opus packet"))?;
    if frames == 0 {
        return Err(Error::invalid_input("opus packet has no audio duration"));
    }
    if frames > MAX_PACKET_FRAMES {
        return Err(Error::invalid_input(
            "opus packet is longer than 120 ms (corrupt container duration)",
        ));
    }
    Ok(frames)
}

/// Converts a signed nanosecond container value (CodecDelay,
/// DiscardPadding) to whole 48 kHz samples, rounding half away from zero.
/// All arithmetic is checked: claimed values can never drive an allocation
/// or a wrap.
pub(crate) fn ns_to_samples(ns: i64) -> Result<i64, Error> {
    const NS_PER_SECOND: i128 = 1_000_000_000;
    let value = i128::from(ns) * 48_000;
    let (magnitude, sign) = if value < 0 { (-value, -1i128) } else { (value, 1) };
    let rounded = (magnitude + NS_PER_SECOND / 2) / NS_PER_SECOND;
    i64::try_from(sign * rounded).map_err(|_| Error::invalid_input("timestamp value out of range"))
}

/// Decodes validated packets to 48 kHz mono. No FEC, no PLC, no skipped
/// packets: any decode failure rejects the whole file. Cancellation is
/// checked between packets. Everything is buffered before returning, so a
/// failure never produces a partial result.
pub(crate) fn decode_packets(
    head: &OpusHead,
    packets: &[OpusPacket],
    cancel: &CancelToken,
) -> Result<DecodedAudio, Error> {
    if packets.is_empty() {
        return Err(Error::invalid_input("opus stream contains no audio packets"));
    }
    let channels = match head.channels {
        1 => opus::Channels::Mono,
        _ => opus::Channels::Stereo,
    };
    let mut decoder = opus::Decoder::new(OPUS_SAMPLE_RATE, channels)
        .map_err(|_| Error::internal("failed to initialize the opus decoder"))?;
    let mut scratch = [0.0f32; MAX_PACKET_SAMPLES];
    let mut interleaved: Vec<f32> = Vec::new();
    let mut pre_skip_left = head.pre_skip;
    let mut output_frames = 0usize;
    let channel_count = usize::from(head.channels);

    for packet in packets {
        if cancel.is_cancelled() {
            return Err(Error::cancelled());
        }
        let frames = packet_frames(&packet.data)?;
        let budget = MAX_OUTPUT_FRAMES - output_frames;
        if frames > budget {
            return Err(Error::audio_too_long(
                (output_frames + frames) as f64 / OPUS_SAMPLE_RATE as f64,
            ));
        }
        // Decode into a fixed buffer; `decode_float` returns whole frames and
        // never writes past the buffer. FEC is off and empty input never
        // reaches libopus, so no packet-loss concealment can run.
        let decoded = decoder
            .decode_float(&packet.data, &mut scratch, false)
            .map_err(|_| Error::invalid_input("failed to decode opus packet"))?;
        if decoded > MAX_PACKET_FRAMES {
            return Err(Error::invalid_input(
                "opus packet decoded longer than 120 ms (corrupt packet)",
            ));
        }

        // The stream pre-skip is consumed across as many packets as needed,
        // exactly once, before any container trim is applied.
        let skip = pre_skip_left.min(decoded);
        pre_skip_left -= skip;
        let start = skip
            .checked_add(packet.trim_start)
            .ok_or_else(|| Error::invalid_input("opus trim exceeds packet duration"))?;
        let end = packet.trim_end;
        if start + end > decoded {
            return Err(Error::invalid_input("opus trim exceeds packet duration"));
        }
        let keep = decoded - start - end;
        output_frames = output_frames
            .checked_add(keep)
            .ok_or_else(|| Error::audio_too_long(MAX_AUDIO_SECONDS + 1.0))?;
        interleaved.extend_from_slice(&scratch[start * channel_count..(decoded - end) * channel_count]);
    }

    if pre_skip_left > 0 {
        return Err(Error::invalid_input(
            "opus stream ended before the pre-skip elapsed (truncated stream)",
        ));
    }
    if output_frames == 0 {
        return Err(Error::invalid_input("audio contains no samples"));
    }
    let seconds = output_frames as f64 / OPUS_SAMPLE_RATE as f64;
    if seconds > MAX_AUDIO_SECONDS {
        return Err(Error::audio_too_long(seconds));
    }

    let mut mono = downmix_to_mono(interleaved, head.channels)?;
    // Mandatory signed Q8 output gain, as a linear factor.
    let gain = 10f64.powf(f64::from(head.gain_db_q8) / 256.0 / 20.0);
    for sample in &mut mono {
        *sample = (*sample as f64 * gain) as f32;
    }
    if mono.iter().any(|s| !s.is_finite()) {
        return Err(Error::new(ErrorCode::InvalidInput, "audio contains non-finite samples"));
    }
    for sample in &mut mono {
        *sample = sample.clamp(-1.0, 1.0);
    }

    Ok(DecodedAudio {
        samples: mono,
        sample_rate: OPUS_SAMPLE_RATE,
        channels: head.channels,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ErrorCode;

    /// Reads a fixture from the crate's fixture directory.
    pub(crate) fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    /// Extracts every Ogg packet payload (header packets included) by
    /// walking pages. Test-only; production demux goes through
    /// `ogg::decode_ogg_opus`.
    pub(crate) fn ogg_packets(bytes: &[u8]) -> Vec<Box<[u8]>> {
        let mut packets = Vec::new();
        let mut offset = 0usize;
        let mut current: Vec<u8> = Vec::new();
        while offset + 27 <= bytes.len() {
            assert_eq!(&bytes[offset..offset + 4], b"OggS");
            let segments = usize::from(bytes[offset + 26]);
            let table = &bytes[offset + 27..offset + 27 + segments];
            let body_len: usize = table.iter().map(|&v| usize::from(v)).sum();
            let body =
                &bytes[offset + 27 + segments..offset + 27 + segments + body_len];
            let mut pos = 0usize;
            for &lace in table {
                let take = usize::from(lace).min(body.len() - pos);
                current.extend_from_slice(&body[pos..pos + take]);
                pos += take;
                if lace < 255 {
                    packets.push(std::mem::take(&mut current).into_boxed_slice());
                }
            }
            offset += 27 + segments + body_len;
        }
        packets
    }

    /// A few real Opus audio packets (header packets skipped).
    pub(crate) fn audio_packets(count: usize) -> Vec<OpusPacket> {
        let bytes = fixture("sample-pt-48k.opus");
        let packets = ogg_packets(&bytes);
        packets[2..2 + count]
            .iter()
            .map(|data| OpusPacket {
                data: data.clone(),
                trim_start: 0,
                trim_end: 0,
            })
            .collect()
    }

    fn family0_head(pre_skip: usize, gain_db_q8: i16) -> OpusHead {
        OpusHead {
            channels: 1,
            pre_skip,
            gain_db_q8,
        }
    }

    fn rms(samples: &[f32]) -> f64 {
        (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()
            / samples.len() as f64)
            .sqrt()
    }

    #[test]
    fn opus_head_parses_family0() {
        let bytes = fixture("sample-pt.webm");
        let at = bytes.windows(8).position(|w| w == b"OpusHead").unwrap();
        let head = OpusHead::parse(&bytes[at..at + 19]).unwrap();
        assert_eq!(head.channels, 1);
        assert_eq!(head.pre_skip, 312);
        assert_eq!(head.gain_db_q8, 0);
    }

    #[test]
    fn opus_head_rejects_bad_inputs() {
        let bytes = fixture("sample-pt.webm");
        let at = bytes.windows(8).position(|w| w == b"OpusHead").unwrap();
        let valid = &bytes[at..at + 19];

        let mut wrong_version = valid.to_vec();
        wrong_version[8] = 2;
        assert_eq!(
            OpusHead::parse(&wrong_version).unwrap_err().code,
            ErrorCode::InvalidInput
        );

        let mut bad_magic = valid.to_vec();
        bad_magic[0] = b'X';
        assert_eq!(
            OpusHead::parse(&bad_magic).unwrap_err().code,
            ErrorCode::InvalidInput
        );

        for channels in [0u8, 3, 6] {
            let mut bad = valid.to_vec();
            bad[9] = channels;
            assert_eq!(
                OpusHead::parse(&bad).unwrap_err().code,
                ErrorCode::UnsupportedFormat
            );
        }

        let mut family2 = valid.to_vec();
        family2[18] = 2;
        assert_eq!(
            OpusHead::parse(&family2).unwrap_err().code,
            ErrorCode::UnsupportedFormat
        );

        // Family 0 with trailing bytes is malformed.
        let mut trailing = valid.to_vec();
        trailing.push(0);
        assert_eq!(
            OpusHead::parse(&trailing).unwrap_err().code,
            ErrorCode::InvalidInput
        );

        assert_eq!(
            OpusHead::parse(&valid[..10]).unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }

    #[test]
    fn opus_head_family1_canonical_mono_and_stereo() {
        // Base: 19-byte family 0 header fields, family patched per case.
        let mut base = b"OpusHead".to_vec();
        base.extend([1, 1, 0x38, 0x01]); // version 1, 1 ch, pre-skip 312
        base.extend([0x80, 0xBB, 0, 0]); // 48 kHz input rate
        base.extend([0, 0]); // gain 0
        base.push(1); // family 1

        // Mono: 1 stream, 0 coupled, mapping [0].
        let mut mono = base.clone();
        mono.extend([1, 0, 0]);
        let head = OpusHead::parse(&mono).unwrap();
        assert_eq!(head.channels, 1);

        // Stereo: 1 stream, 1 coupled, mapping [0, 1].
        let mut stereo = base;
        stereo[9] = 2;
        stereo.extend([1, 1, 0, 1]);
        let head = OpusHead::parse(&stereo).unwrap();
        assert_eq!(head.channels, 2);

        // Non-canonical stereo mapping (streams swapped) is rejected.
        let mut swapped = stereo.clone();
        let n = swapped.len();
        swapped[n - 2] = 1;
        swapped[n - 1] = 0;
        assert_eq!(
            OpusHead::parse(&swapped).unwrap_err().code,
            ErrorCode::UnsupportedFormat
        );

        // Family 1 claiming two streams needs a multistream decoder.
        let mut multi = stereo.clone();
        multi[19] = 2;
        assert_eq!(
            OpusHead::parse(&multi).unwrap_err().code,
            ErrorCode::UnsupportedFormat
        );
    }

    #[test]
    fn ns_to_samples_converts_and_rounds() {
        assert_eq!(ns_to_samples(6_500_000).unwrap(), 312);
        assert_eq!(ns_to_samples(3_500_000).unwrap(), 168);
        assert_eq!(ns_to_samples(-3_500_000).unwrap(), -168);
        assert_eq!(ns_to_samples(0).unwrap(), 0);
        assert_eq!(ns_to_samples(10_416_667).unwrap(), 500);
        assert_eq!(ns_to_samples(-10_416_667).unwrap(), -500);
    }

    #[test]
    fn decode_applies_pre_skip_across_packets_exactly_once() {
        let packets = audio_packets(4);
        let frames_per_packet = 960; // 20 ms at 48 kHz
        let total = frames_per_packet * packets.len();

        // Pre-skip smaller than one packet.
        let decoded = decode_packets(
            &family0_head(100, 0),
            &packets,
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(decoded.samples.len(), total - 100);

        // Pre-skip spanning three whole packets plus a remainder.
        let skip = frames_per_packet * 3 + 17;
        let decoded = decode_packets(
            &family0_head(skip, 0),
            &packets,
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(decoded.samples.len(), total - skip);

        // A pre-skip beyond the stream is truncated input.
        let err = decode_packets(
            &family0_head(total + 1, 0),
            &packets,
            &CancelToken::new(),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn decode_rejects_trim_exceeding_packet() {
        let packets = audio_packets(2);
        let mut bad = packets.clone();
        bad[0].trim_end = 961; // packet decodes to 960 frames
        let err = decode_packets(&family0_head(0, 0), &bad, &CancelToken::new()).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);

        let mut bad = packets;
        bad[0].trim_start = usize::MAX - 10;
        let err = decode_packets(&family0_head(0, 0), &bad, &CancelToken::new()).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn decode_enforces_120_second_cap_per_packet() {
        let one = audio_packets(1).pop().unwrap();
        let frames: usize = 960;
        // 120 s / 20 ms = 6,000 packets; the 6,001st must fail, checked
        // before it is decoded.
        let mut packets: Vec<OpusPacket> = std::iter::repeat_n(one.clone(), 6_001).collect();
        for packet in &mut packets {
            packet.trim_start = 0;
            packet.trim_end = 0;
        }
        let err = decode_packets(&family0_head(0, 0), &packets, &CancelToken::new()).unwrap_err();
        assert_eq!(err.code, ErrorCode::AudioTooLong);

        // Exactly 120 s still passes the cap (and then fails the strict
        // length check in normalize_and_validate, not here).
        let mut packets: Vec<OpusPacket> = std::iter::repeat_n(one, 6_000).collect();
        for packet in &mut packets {
            packet.trim_start = 0;
            packet.trim_end = 0;
        }
        let decoded = decode_packets(&family0_head(0, 0), &packets, &CancelToken::new()).unwrap();
        assert_eq!(decoded.samples.len(), frames * 6_000);
        let _ = frames;
    }

    #[test]
    fn decode_honours_cancellation_between_packets() {
        let one = audio_packets(1).pop().unwrap();
        let mut packets: Vec<OpusPacket> = std::iter::repeat_n(one, 16).collect();
        for packet in &mut packets {
            packet.trim_start = 0;
            packet.trim_end = 0;
        }
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = decode_packets(&family0_head(0, 0), &packets, &cancel).unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled);
    }

    #[test]
    fn decode_applies_output_gain() {
        let packets = audio_packets(8);
        let flat = family0_head(0, 0);
        let boosted = family0_head(0, 256); // +1 dB
        let plain = decode_packets(&flat, &packets, &CancelToken::new()).unwrap();
        let loud = decode_packets(&boosted, &packets, &CancelToken::new()).unwrap();
        let ratio = rms(&loud.samples) / rms(&plain.samples);
        assert!((ratio - 10f64.powf(1.0 / 20.0)).abs() < 1e-3, "ratio {ratio}");
    }

    #[test]
    fn decode_rejects_empty_packet_list_and_empty_packet() {
        let err = decode_packets(&family0_head(0, 0), &[], &CancelToken::new()).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);

        let mut packets = audio_packets(1);
        packets[0].data = Box::from([] as [u8; 0]);
        let err = decode_packets(&family0_head(0, 0), &packets, &CancelToken::new()).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }
}
