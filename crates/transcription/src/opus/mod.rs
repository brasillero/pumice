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
pub(crate) struct OpusPacket {
    pub data: Box<[u8]>,
    pub trim_start: usize,
    pub trim_end: usize,
}

/// A validated `OpusHead` (RFC 7845 section 5).
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
