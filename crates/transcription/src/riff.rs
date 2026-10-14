use crate::Error;

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

// Speaker position bits defined by Microsoft (bits 0..=18).
const SPEAKER_MASK_ALL_DEFINED: u32 = 0x0007_FFFF;
// Bit 31 is reserved and must not be set.
const SPEAKER_RESERVED: u32 = 0x8000_0000;

const KSDATAFORMAT_SUBTYPE_PCM: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: [u8; 16] = [
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];

/// Strictly validated information extracted from a RIFF/WAVE header.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct WavFormatInfo {
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    /// Size of the `data` chunk body in bytes.
    pub data_bytes: u32,
    /// Valid bits per sample (EXTENSIBLE only).
    pub valid_bits: u16,
    /// Channel mask (EXTENSIBLE only).
    pub channel_mask: u32,
    /// Subformat GUID (EXTENSIBLE only).
    pub subformat: [u8; 16],
}

/// Validate a RIFF/WAVE file in memory before handing it to a decoder.
///
/// Checks the signature, enforces that every chunk fits inside the declared
/// RIFF size with correct odd-size padding, requires exactly one `fmt ` chunk
/// before exactly one `data` chunk, validates channel and block-align fields,
/// and computes the expected frame count from the `data` chunk size.
///
/// Returns the validated format info together with a canonical WAV built from
/// only the validated `fmt ` and `data` chunks. Decoders must use the canonical
/// bytes so that any trailing or embedded chunks cannot be reinterpreted as
/// audio data.
pub fn validate_wav(data: &[u8]) -> Result<(WavFormatInfo, Vec<u8>), Error> {
    if data.len() < 12 {
        return Err(Error::invalid_input("WAV file too small"));
    }
    if &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(Error::invalid_input("not a RIFF/WAVE file"));
    }

    let riff_size = read_u32(data, 4) as usize;
    let expected_len = riff_size
        .checked_add(8)
        .ok_or_else(|| Error::invalid_input("RIFF chunk size overflow"))?;
    if expected_len != data.len() {
        return Err(Error::invalid_input(format!(
            "RIFF chunk size {} does not match file size {}",
            riff_size,
            data.len()
        )));
    }
    if riff_size < 4 {
        return Err(Error::invalid_input("RIFF chunk size too small"));
    }

    let mut offset = 12usize;
    let mut remaining = riff_size - 4;
    let mut fmt_seen = false;
    let mut data_seen = false;
    let mut format_info: Option<WavFormatInfo> = None;
    let mut data_body: &[u8] = &[];

    while remaining >= 8 {
        let chunk_id = &data[offset..offset + 4];
        let chunk_size = read_u32(data, offset + 4) as usize;
        let padded_size = chunk_size
            .checked_add(chunk_size & 1)
            .ok_or_else(|| Error::invalid_input("chunk size overflow"))?;
        let total_chunk = 8usize
            .checked_add(padded_size)
            .ok_or_else(|| Error::invalid_input("chunk size overflow"))?;
        if total_chunk > remaining {
            return Err(Error::invalid_input(format!(
                "chunk '{}' size {} exceeds remaining RIFF space {}",
                String::from_utf8_lossy(chunk_id),
                chunk_size,
                remaining
            )));
        }

        let body_start = offset + 8;
        let body_end = body_start + chunk_size;

        if chunk_id == b"fmt " {
            if fmt_seen {
                return Err(Error::invalid_input("multiple fmt chunks"));
            }
            if data_seen {
                return Err(Error::invalid_input("fmt chunk appears after data chunk"));
            }
            format_info = Some(parse_fmt(&data[body_start..body_end])?);
            fmt_seen = true;
        } else if chunk_id == b"data" {
            if !fmt_seen {
                return Err(Error::invalid_input("data chunk appears before fmt chunk"));
            }
            if data_seen {
                return Err(Error::invalid_input("multiple data chunks"));
            }
            let info = format_info.unwrap();
            if !chunk_size.is_multiple_of(info.block_align as usize) {
                return Err(Error::invalid_input(format!(
                    "data chunk size {} is not a multiple of block align {}",
                    chunk_size, info.block_align
                )));
            }
            data_body = &data[body_start..body_end];
            format_info = Some(WavFormatInfo {
                data_bytes: chunk_size as u32,
                ..info
            });
            data_seen = true;
        }

        offset += total_chunk;
        remaining -= total_chunk;
    }

    if remaining != 0 {
        return Err(Error::invalid_input(format!(
            "incomplete chunk header at offset {} ({} bytes remaining)",
            offset, remaining
        )));
    }

    if !fmt_seen {
        return Err(Error::invalid_input("missing fmt chunk"));
    }
    if !data_seen {
        return Err(Error::invalid_input("missing data chunk"));
    }

    let info = format_info.unwrap();
    let canonical = build_canonical_wav(&info, data_body);
    Ok((info, canonical))
}

fn build_canonical_wav(info: &WavFormatInfo, data_body: &[u8]) -> Vec<u8> {
    let fmt_body = match info.format_tag {
        WAVE_FORMAT_PCM | WAVE_FORMAT_IEEE_FLOAT => {
            let mut fmt = Vec::with_capacity(16);
            fmt.extend_from_slice(&info.format_tag.to_le_bytes());
            fmt.extend_from_slice(&info.channels.to_le_bytes());
            fmt.extend_from_slice(&info.sample_rate.to_le_bytes());
            let avg_bytes_per_sec = info.sample_rate * u32::from(info.block_align);
            fmt.extend_from_slice(&avg_bytes_per_sec.to_le_bytes());
            fmt.extend_from_slice(&info.block_align.to_le_bytes());
            fmt.extend_from_slice(&info.bits_per_sample.to_le_bytes());
            fmt
        }
        WAVE_FORMAT_EXTENSIBLE => {
            let mut fmt = Vec::with_capacity(40);
            fmt.extend_from_slice(&info.format_tag.to_le_bytes());
            fmt.extend_from_slice(&info.channels.to_le_bytes());
            fmt.extend_from_slice(&info.sample_rate.to_le_bytes());
            let avg_bytes_per_sec = info.sample_rate * u32::from(info.block_align);
            fmt.extend_from_slice(&avg_bytes_per_sec.to_le_bytes());
            fmt.extend_from_slice(&info.block_align.to_le_bytes());
            fmt.extend_from_slice(&info.bits_per_sample.to_le_bytes());
            fmt.extend_from_slice(&22u16.to_le_bytes());
            fmt.extend_from_slice(&info.valid_bits.to_le_bytes());
            fmt.extend_from_slice(&info.channel_mask.to_le_bytes());
            fmt.extend_from_slice(&info.subformat);
            fmt
        }
        _ => unreachable!(),
    };

    let data_pad = if data_body.len() % 2 == 1 { 1 } else { 0 };
    let riff_size = 4 + 8 + fmt_body.len() + 8 + data_body.len() + data_pad;

    let mut out = Vec::with_capacity(8 + riff_size);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(riff_size as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&(fmt_body.len() as u32).to_le_bytes());
    out.extend_from_slice(&fmt_body);

    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_body.len() as u32).to_le_bytes());
    out.extend_from_slice(data_body);
    if data_pad == 1 {
        out.push(0);
    }

    out
}

fn parse_fmt(body: &[u8]) -> Result<WavFormatInfo, Error> {
    if body.len() < 16 {
        return Err(Error::invalid_input("fmt chunk body too small"));
    }

    let format_tag = read_u16(body, 0);
    let channels = read_u16(body, 2);
    let sample_rate = read_u32(body, 4);
    let avg_bytes_per_sec = read_u32(body, 8);
    let block_align = read_u16(body, 12);
    let bits_per_sample = read_u16(body, 14);

    // P1: reject obviously bogus channel counts before the decoder sees them.
    if channels == 0 || channels > 2 {
        return Err(Error::invalid_input(format!(
            "unsupported channel count: {} (expected 1 or 2)",
            channels
        )));
    }

    if !(crate::audio::MIN_SAMPLE_RATE..=crate::audio::MAX_SAMPLE_RATE).contains(&sample_rate) {
        return Err(Error::unsupported_format(format!(
            "unsupported sample rate: {} Hz (expected {}-{} Hz)",
            sample_rate,
            crate::audio::MIN_SAMPLE_RATE,
            crate::audio::MAX_SAMPLE_RATE
        )));
    }

    match format_tag {
        WAVE_FORMAT_PCM | WAVE_FORMAT_IEEE_FLOAT => {
            if body.len() != 16 && body.len() != 18 {
                return Err(Error::invalid_input(
                    "fmt chunk body must be 16 or 18 bytes for PCM/IEEE_FLOAT",
                ));
            }
            if body.len() == 18 && read_u16(body, 16) != 0 {
                return Err(Error::invalid_input(
                    "fmt chunk cbSize must be 0 for 18-byte PCM/IEEE_FLOAT body",
                ));
            }

            // P2: for PCM/IEEE_FLOAT block_align must match channels * ceil(bits/8).
            let bytes_per_sample = bits_per_sample.div_ceil(8);
            let expected_block_align = (channels as u32) * (bytes_per_sample as u32);
            if u32::from(block_align) != expected_block_align {
                return Err(Error::invalid_input(format!(
                    "block_align mismatch: expected {} for {} channels and {} bits, got {}",
                    expected_block_align, channels, bits_per_sample, block_align
                )));
            }
            if avg_bytes_per_sec != sample_rate * u32::from(block_align) {
                return Err(Error::invalid_input(format!(
                    "avg_bytes_per_sec mismatch: expected {}, got {}",
                    sample_rate * u32::from(block_align),
                    avg_bytes_per_sec
                )));
            }
        }
        WAVE_FORMAT_EXTENSIBLE => {
            if body.len() != 40 {
                return Err(Error::invalid_input(
                    "EXTENSIBLE fmt chunk body must be exactly 40 bytes",
                ));
            }
            let extension_size = read_u16(body, 16);
            if extension_size != 22 {
                return Err(Error::invalid_input(format!(
                    "EXTENSIBLE extra data size must be 22, got {}",
                    extension_size
                )));
            }
            let valid_bits = read_u16(body, 18);
            let channel_mask = read_u32(body, 20);
            let subformat = &body[24..40];

            // P2: container size must be a whole number of bytes.
            if !bits_per_sample.is_multiple_of(8) {
                return Err(Error::invalid_input(
                    "EXTENSIBLE container bits per sample must be a multiple of 8",
                ));
            }
            let container_bytes = bits_per_sample / 8;
            let expected_block_align = (channels as u32) * (container_bytes as u32);
            if u32::from(block_align) != expected_block_align {
                return Err(Error::invalid_input(format!(
                    "EXTENSIBLE block_align mismatch: expected {} for {} channels and {} container bytes, got {}",
                    expected_block_align, channels, container_bytes, block_align
                )));
            }
            if valid_bits > bits_per_sample {
                return Err(Error::invalid_input(format!(
                    "valid bits per sample ({}) exceeds container bits ({})",
                    valid_bits, bits_per_sample
                )));
            }
            if avg_bytes_per_sec != sample_rate * u32::from(block_align) {
                return Err(Error::invalid_input(format!(
                    "avg_bytes_per_sec mismatch: expected {}, got {}",
                    sample_rate * u32::from(block_align),
                    avg_bytes_per_sec
                )));
            }

            // P1: validate channel mask before the decoder sees it.
            if channel_mask & SPEAKER_RESERVED != 0 {
                return Err(Error::invalid_input(
                    "reserved speaker bit (0x80000000) set in channel mask",
                ));
            }
            if channel_mask != 0 {
                let popcount = channel_mask.count_ones();
                if popcount != channels as u32 {
                    return Err(Error::invalid_input(format!(
                        "channel mask popcount ({}) does not match channel count ({})",
                        popcount, channels
                    )));
                }
                if channel_mask & !SPEAKER_MASK_ALL_DEFINED != 0 {
                    return Err(Error::invalid_input(
                        "channel mask uses undefined speaker bits",
                    ));
                }
            }

            if subformat != KSDATAFORMAT_SUBTYPE_PCM && subformat != KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
            {
                return Err(Error::unsupported_format(
                    "unsupported EXTENSIBLE subformat",
                ));
            }

            return Ok(WavFormatInfo {
                format_tag,
                channels,
                sample_rate,
                block_align,
                bits_per_sample,
                data_bytes: 0,
                valid_bits,
                channel_mask,
                subformat: subformat.try_into().unwrap(),
            });
        }
        _ => {
            return Err(Error::unsupported_format(format!(
                "unsupported WAV format tag: {}",
                format_tag
            )));
        }
    }

    Ok(WavFormatInfo {
        format_tag,
        channels,
        sample_rate,
        block_align,
        bits_per_sample,
        data_bytes: 0,
        valid_bits: 0,
        channel_mask: 0,
        subformat: [0; 16],
    })
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}
