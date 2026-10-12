use crate::{Error, ErrorCode};
use rubato::audioadapter_buffers::owned::InterleavedOwned;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use std::io::Read;

pub const TARGET_SAMPLE_RATE: u32 = 16_000;
pub const MAX_FILE_BYTES: u64 = 25_000_000;
pub const MAX_AUDIO_SECONDS: f64 = 120.0;
pub const MIN_SAMPLE_RATE: u32 = 8_000;
pub const MAX_SAMPLE_RATE: u32 = 192_000;

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const KSDATAFORMAT_SUBTYPE_PCM: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: [u8; 16] = [
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];

#[derive(Debug)]
pub struct DecodedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Minimal bounded RIFF/WAVE parser.
///
/// Reads only the `fmt ` and `data` chunks, skipping all others and honoring the
/// pad byte after odd-sized chunks. Supports PCM, IEEE float and WAVE_FORMAT_EXTENSIBLE
/// with PCM/float subformats. hound 3.5.1 mishandles odd-sized chunks (e.g. a 1-byte
/// JUNK chunk with its pad byte), so we decode ourselves instead of relying on it.
pub fn decode_wav<R: Read>(mut reader: R) -> Result<DecodedAudio, Error> {
    let mut header = [0u8; 12];
    reader.read_exact(&mut header).map_err(|e| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("failed to read WAV header: {e}"),
        )
    })?;

    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(Error::invalid_input("not a RIFF/WAVE file"));
    }

    let riff_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u64;
    // The RIFF size covers everything after the size field itself.
    let budget = riff_size.saturating_add(4).min(MAX_FILE_BYTES);

    let mut fmt: Option<WavFmt> = None;
    let mut data: Option<Vec<u8>> = None;
    let mut bytes_read: u64 = 0;

    loop {
        if data.is_some() {
            // We have everything we need; stop reading.
            break;
        }

        let mut id = [0u8; 4];
        match reader.read_exact(&mut id) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                return Err(Error::invalid_input(format!(
                    "failed to read chunk id: {e}"
                )));
            }
        }
        bytes_read += 4;

        let size = read_u32(&mut reader, &mut bytes_read)?;

        if &id == b"fmt " {
            let mut buf = vec![0u8; size as usize];
            reader
                .read_exact(&mut buf)
                .map_err(|e| Error::invalid_input(format!("failed to read fmt chunk: {e}")))?;
            bytes_read += size as u64;
            fmt = Some(parse_fmt(&buf)?);
        } else if &id == b"data" {
            if size as u64 > budget.saturating_sub(bytes_read) {
                return Err(Error::input_too_large(size as u64 + bytes_read));
            }
            let mut buf = vec![0u8; size as usize];
            reader
                .read_exact(&mut buf)
                .map_err(|e| Error::invalid_input(format!("failed to read data chunk: {e}")))?;
            bytes_read += size as u64;
            data = Some(buf);
        } else {
            // Skip unknown chunk body; pad byte is handled below.
            let to_skip = (size as u64).min(budget.saturating_sub(bytes_read));
            if to_skip > 0 {
                let skipped =
                    std::io::copy(&mut reader.by_ref().take(to_skip), &mut std::io::sink())
                        .map_err(|e| Error::invalid_input(format!("failed to skip chunk: {e}")))?;
                bytes_read += skipped;
            }
        }

        // RIFF requires a pad byte after any odd-sized chunk.
        if size % 2 == 1 && bytes_read < budget {
            let mut pad = [0u8; 1];
            match reader.read_exact(&mut pad) {
                Ok(()) => bytes_read += 1,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {}
                Err(e) => {
                    return Err(Error::invalid_input(format!(
                        "failed to read pad byte: {e}"
                    )));
                }
            }
        }
    }

    let fmt = fmt.ok_or_else(|| Error::invalid_input("WAV file has no fmt chunk"))?;
    let data = data.ok_or_else(|| Error::invalid_input("WAV file has no data chunk"))?;

    validate_format(&fmt)?;

    let interleaved = decode_samples(&fmt, &data)?;
    let mono = downmix_to_mono(interleaved, fmt.channels)?;

    Ok(DecodedAudio {
        sample_rate: fmt.sample_rate,
        channels: fmt.channels,
        samples: mono,
    })
}

#[derive(Debug, Clone, Copy)]
struct WavFmt {
    format_tag: u16,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
}

fn read_u32<R: Read>(reader: &mut R, counter: &mut u64) -> Result<u32, Error> {
    let mut buf = [0u8; 4];
    reader
        .read_exact(&mut buf)
        .map_err(|e| Error::invalid_input(format!("failed to read chunk size: {e}")))?;
    *counter += 4;
    Ok(u32::from_le_bytes(buf))
}

fn parse_fmt(buf: &[u8]) -> Result<WavFmt, Error> {
    if buf.len() < 16 {
        return Err(Error::unsupported_format("fmt chunk too short"));
    }

    let format_tag = u16::from_le_bytes([buf[0], buf[1]]);
    let channels = u16::from_le_bytes([buf[2], buf[3]]);
    let sample_rate = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let bits_per_sample = u16::from_le_bytes([buf[14], buf[15]]);

    let mut resolved_tag = format_tag;

    if format_tag == WAVE_FORMAT_EXTENSIBLE {
        if buf.len() < 40 {
            return Err(Error::unsupported_format("extensible fmt chunk too short"));
        }
        // buf[16..18] = extension size, buf[18..20] = valid bits, buf[20..24] = channel mask
        let subformat = &buf[24..40];
        if subformat == KSDATAFORMAT_SUBTYPE_PCM {
            resolved_tag = WAVE_FORMAT_PCM;
        } else if subformat == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
            resolved_tag = WAVE_FORMAT_IEEE_FLOAT;
        } else {
            return Err(Error::unsupported_format(
                "unsupported extensible subformat",
            ));
        }
    }

    Ok(WavFmt {
        format_tag: resolved_tag,
        channels,
        sample_rate,
        bits_per_sample,
    })
}

fn validate_format(fmt: &WavFmt) -> Result<(), Error> {
    if fmt.channels == 0 || fmt.channels > 2 {
        return Err(Error::unsupported_format(format!(
            "unsupported channel count: {} (expected 1 or 2)",
            fmt.channels
        )));
    }

    if fmt.sample_rate < MIN_SAMPLE_RATE || fmt.sample_rate > MAX_SAMPLE_RATE {
        return Err(Error::unsupported_format(format!(
            "unsupported sample rate: {} Hz (expected {}-{} Hz)",
            fmt.sample_rate, MIN_SAMPLE_RATE, MAX_SAMPLE_RATE
        )));
    }

    match (fmt.format_tag, fmt.bits_per_sample) {
        (WAVE_FORMAT_PCM, 8 | 16 | 24 | 32) | (WAVE_FORMAT_IEEE_FLOAT, 32) => Ok(()),
        _ => Err(Error::unsupported_format(format!(
            "unsupported WAV format: format_tag={:#06x} {}-bit",
            fmt.format_tag, fmt.bits_per_sample
        ))),
    }
}

fn decode_samples(fmt: &WavFmt, data: &[u8]) -> Result<Vec<f32>, Error> {
    let bytes_per_sample = fmt.bits_per_sample as usize / 8;
    let bytes_per_frame = bytes_per_sample * fmt.channels as usize;
    if !data.len().is_multiple_of(bytes_per_frame) {
        return Err(Error::invalid_input(
            "data chunk size is not aligned to frame size",
        ));
    }

    let frame_count = data.len() / bytes_per_frame;
    let mut out = Vec::with_capacity(frame_count * fmt.channels as usize);

    for (frame_idx, frame_bytes) in data.chunks_exact(bytes_per_frame).enumerate() {
        // Enforce the duration limit while decoding so a long file is rejected
        // before any downmix or resample buffer is allocated.
        let seconds = (frame_idx + 1) as f64 / fmt.sample_rate as f64;
        if seconds > MAX_AUDIO_SECONDS {
            return Err(Error::audio_too_long(seconds));
        }

        for ch in 0..fmt.channels as usize {
            let start = ch * bytes_per_sample;
            let sample = decode_sample(fmt, &frame_bytes[start..start + bytes_per_sample])?;
            out.push(sample);
        }
    }

    Ok(out)
}

fn decode_sample(fmt: &WavFmt, bytes: &[u8]) -> Result<f32, Error> {
    match (fmt.format_tag, fmt.bits_per_sample) {
        (WAVE_FORMAT_PCM, 8) => Ok((bytes[0] as f32 - 128.0) / 128.0),
        (WAVE_FORMAT_PCM, 16) => {
            let v = i16::from_le_bytes([bytes[0], bytes[1]]);
            Ok((v as f32) / 32_768.0)
        }
        (WAVE_FORMAT_PCM, 24) => {
            let v = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]) << 8 >> 8;
            Ok((v as f32) / 8_388_608.0)
        }
        (WAVE_FORMAT_PCM, 32) => {
            let v = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            Ok((v as f32) / 2_147_483_648.0)
        }
        (WAVE_FORMAT_IEEE_FLOAT, 32) => {
            let v = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if !v.is_finite() {
                return Err(Error::invalid_input("audio contains non-finite samples"));
            }
            Ok(v.clamp(-1.0, 1.0))
        }
        _ => Err(Error::unsupported_format("unsupported WAV sample format")),
    }
}

pub fn resample_to_target(input: &[f32], input_rate: u32) -> Result<Vec<f32>, Error> {
    if input_rate == TARGET_SAMPLE_RATE {
        return Ok(input.to_vec());
    }

    let ratio = TARGET_SAMPLE_RATE as f64 / input_rate as f64;
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: Some(0.95),
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let chunk_size = 4096;

    let mut resampler =
        Async::<f32>::new_sinc(ratio, 1.0, &params, chunk_size, 1, FixedAsync::Input).map_err(
            |e| {
                Error::new(
                    ErrorCode::Internal,
                    format!("failed to create resampler: {e}"),
                )
            },
        )?;

    let input_adapter =
        InterleavedOwned::new_from(input.to_vec(), 1, input.len()).map_err(|e| {
            Error::new(
                ErrorCode::Internal,
                format!("failed to create input adapter: {e}"),
            )
        })?;

    let output = resampler
        .process_all(&input_adapter, input.len(), None)
        .map_err(|e| Error::new(ErrorCode::Internal, format!("resampling failed: {e}")))?;

    Ok(output.take_data())
}

pub fn normalize_and_validate(pcm: Vec<f32>) -> Result<Vec<f32>, Error> {
    if pcm.is_empty() {
        return Err(Error::invalid_input("audio contains no samples"));
    }

    let seconds = pcm.len() as f64 / TARGET_SAMPLE_RATE as f64;
    if seconds > MAX_AUDIO_SECONDS {
        return Err(Error::audio_too_long(seconds));
    }

    // Reject non-finite samples defensively; normalized integer input is always finite.
    if pcm.iter().any(|s| !s.is_finite()) {
        return Err(Error::invalid_input("audio contains non-finite samples"));
    }

    Ok(pcm)
}

fn downmix_to_mono(samples: Vec<f32>, channels: u16) -> Result<Vec<f32>, Error> {
    match channels {
        1 => Ok(samples),
        2 => Ok(samples.chunks(2).map(|c| (c[0] + c[1]) / 2.0).collect()),
        _ => Err(Error::unsupported_format(format!(
            "cannot downmix {} channels to mono",
            channels
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};
    use std::io::Cursor;

    fn write_wav_f32(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let mut writer = WavWriter::new(&mut buf, spec).unwrap();
        for &s in samples {
            writer.write_sample(s).unwrap();
        }
        writer.finalize().unwrap();
        buf.into_inner()
    }

    fn write_wav_i8(samples: &[i8], sample_rate: u32, channels: u16) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 8,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::new(&mut buf, spec).unwrap();
        for &s in samples {
            writer.write_sample(s).unwrap();
        }
        writer.finalize().unwrap();
        buf.into_inner()
    }

    fn write_wav_i16(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::new(&mut buf, spec).unwrap();
        for &s in samples {
            writer.write_sample(s).unwrap();
        }
        writer.finalize().unwrap();
        buf.into_inner()
    }

    fn write_wav_i24(samples: &[i32], sample_rate: u32, channels: u16) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 24,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::new(&mut buf, spec).unwrap();
        for &s in samples {
            writer.write_sample(s).unwrap();
        }
        writer.finalize().unwrap();
        buf.into_inner()
    }

    fn write_wav_i32(samples: &[i32], sample_rate: u32, channels: u16) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::new(&mut buf, spec).unwrap();
        for &s in samples {
            writer.write_sample(s).unwrap();
        }
        writer.finalize().unwrap();
        buf.into_inner()
    }

    fn write_wav_with_odd_junk(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        // hound does not emit JUNK chunks, so build a minimal WAV by hand with a
        // 1-byte JUNK chunk (plus its mandatory pad byte) before the data chunk.
        let bytes_per_sample = 2usize;
        let block_align = (channels as usize * bytes_per_sample) as u16;
        let byte_rate = sample_rate * block_align as u32;
        let data_bytes = samples.len() * bytes_per_sample;

        let mut fmt = Vec::new();
        fmt.extend_from_slice(&WAVE_FORMAT_PCM.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&sample_rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());

        let mut data = Vec::with_capacity(data_bytes);
        for &s in samples {
            data.extend_from_slice(&s.to_le_bytes());
        }

        let junk = b"x"; // 1 byte -> odd size, requires pad byte

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        // RIFF size = 4 (WAVE) + 8 + fmt.len() + 8 + 1 + 1 (junk + pad) + 8 + data.len()
        let riff_size = 4 + 8 + fmt.len() as u32 + 8 + 2 + 8 + data.len() as u32;
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WAVE");

        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);

        out.extend_from_slice(b"JUNK");
        out.extend_from_slice(&(junk.len() as u32).to_le_bytes());
        out.extend_from_slice(junk);
        out.push(0); // pad byte

        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);

        out
    }

    fn write_wav_extensible_pcm(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        let bytes_per_sample = 2usize;
        let block_align = (channels as usize * bytes_per_sample) as u16;
        let byte_rate = sample_rate * block_align as u32;
        let data_bytes = samples.len() * bytes_per_sample;

        let mut fmt = Vec::new();
        fmt.extend_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&sample_rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        fmt.extend_from_slice(&22u16.to_le_bytes()); // extension size
        fmt.extend_from_slice(&16u16.to_le_bytes()); // valid bits
        fmt.extend_from_slice(&0u32.to_le_bytes()); // channel mask
        fmt.extend_from_slice(&KSDATAFORMAT_SUBTYPE_PCM);

        let mut data = Vec::with_capacity(data_bytes);
        for &s in samples {
            data.extend_from_slice(&s.to_le_bytes());
        }

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        let riff_size = 4 + 8 + fmt.len() as u32 + 8 + data.len() as u32;
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WAVE");

        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);

        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);

        out
    }

    fn write_wav_extensible_float(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
        let bytes_per_sample = 4usize;
        let block_align = (channels as usize * bytes_per_sample) as u16;
        let byte_rate = sample_rate * block_align as u32;
        let data_bytes = samples.len() * bytes_per_sample;

        let mut fmt = Vec::new();
        fmt.extend_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&sample_rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&32u16.to_le_bytes());
        fmt.extend_from_slice(&22u16.to_le_bytes()); // extension size
        fmt.extend_from_slice(&32u16.to_le_bytes()); // valid bits
        fmt.extend_from_slice(&0u32.to_le_bytes()); // channel mask
        fmt.extend_from_slice(&KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);

        let mut data = Vec::with_capacity(data_bytes);
        for &s in samples {
            data.extend_from_slice(&s.to_le_bytes());
        }

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        let riff_size = 4 + 8 + fmt.len() as u32 + 8 + data.len() as u32;
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WAVE");

        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);

        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);

        out
    }

    fn silence(samples: usize) -> Vec<f32> {
        vec![0.0f32; samples]
    }

    #[test]
    fn decode_16bit_mono() {
        let samples: Vec<i16> = (0..16000).map(|i| (i as i16).wrapping_mul(2)).collect();
        let wav = write_wav_i16(&samples, 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples.len(), 16_000);
        assert!(decoded.samples.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn decode_8bit_full_scale() {
        let wav = write_wav_i8(&[i8::MAX, i8::MIN], 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert!(
            (decoded.samples[0] - 127.0 / 128.0).abs() < 1e-6,
            "8-bit max scaled to {}",
            decoded.samples[0]
        );
        assert!(
            (decoded.samples[1] - (-1.0)).abs() < 1e-6,
            "8-bit min scaled to {}",
            decoded.samples[1]
        );
    }

    #[test]
    fn decode_16bit_full_scale() {
        let wav = write_wav_i16(&[i16::MAX, i16::MIN], 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert!(
            (decoded.samples[0] - 32_767.0 / 32_768.0).abs() < 1e-6,
            "16-bit max scaled to {}",
            decoded.samples[0]
        );
        assert!(
            (decoded.samples[1] - (-1.0)).abs() < 1e-6,
            "16-bit min scaled to {}",
            decoded.samples[1]
        );
    }

    #[test]
    fn decode_24bit_full_scale() {
        let wav = write_wav_i24(&[8_388_607, -8_388_608], 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert!(
            (decoded.samples[0] - 8_388_607.0 / 8_388_608.0).abs() < 1e-6,
            "24-bit max scaled to {}",
            decoded.samples[0]
        );
        assert!(
            (decoded.samples[1] - (-1.0)).abs() < 1e-6,
            "24-bit min scaled to {}",
            decoded.samples[1]
        );
    }

    #[test]
    fn decode_32bit_full_scale() {
        let wav = write_wav_i32(&[i32::MAX, i32::MIN], 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert!(
            (decoded.samples[0] - 2_147_483_647.0 / 2_147_483_648.0).abs() < 1e-7,
            "32-bit max scaled to {}",
            decoded.samples[0]
        );
        assert!(
            (decoded.samples[1] - (-1.0)).abs() < 1e-7,
            "32-bit min scaled to {}",
            decoded.samples[1]
        );
    }

    #[test]
    fn decode_float_full_scale() {
        let wav = write_wav_f32(&[1.0, -1.0], 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert!((decoded.samples[0] - 1.0).abs() < 1e-6);
        assert!((decoded.samples[1] - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn decode_float_stereo_downmix() {
        let left = silence(16_000);
        let right: Vec<f32> = (0..16_000).map(|_| 1.0).collect();
        let interleaved: Vec<f32> = left
            .iter()
            .zip(&right)
            .flat_map(|(&l, &r)| [l, r])
            .collect();
        let wav = write_wav_f32(&interleaved, 16_000, 2);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.samples.len(), 16_000);
        // Downmix of [0, 1] repeated should average to 0.5.
        assert!((decoded.samples[100] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn decode_odd_sized_junk_chunk() {
        // Regression: hound 3.5.1 fails to parse WAVs with odd-sized chunks
        // because it does not honor the mandatory pad byte.
        let samples: Vec<i16> = (0..16000).map(|i| (i as i16).wrapping_mul(2)).collect();
        let wav = write_wav_with_odd_junk(&samples, 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples.len(), 16_000);
        assert!(decoded.samples.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn decode_extensible_pcm() {
        let wav = write_wav_extensible_pcm(&[i16::MAX, i16::MIN], 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert!((decoded.samples[0] - 32_767.0 / 32_768.0).abs() < 1e-6);
        assert!((decoded.samples[1] - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn decode_extensible_float() {
        let wav = write_wav_extensible_float(&[1.0, -1.0], 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert!((decoded.samples[0] - 1.0).abs() < 1e-6);
        assert!((decoded.samples[1] - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn resample_preserves_length_within_tolerance() {
        let sample_rate = 48_000;
        let duration_secs = 1.0;
        let freq = 1000.0;
        let samples: Vec<f32> = (0..(sample_rate * duration_secs as u32))
            .map(|i| {
                (2.0 * std::f64::consts::PI * freq * i as f64 / sample_rate as f64).sin() as f32
            })
            .collect();
        let wav = write_wav_f32(&samples, sample_rate, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        let resampled = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
        let expected_len =
            (samples.len() as f64 * TARGET_SAMPLE_RATE as f64 / sample_rate as f64).ceil() as usize;
        assert!((resampled.len() as isize - expected_len as isize).abs() <= 2);
    }

    #[test]
    fn resample_8k_to_16k_doubles_length() {
        let samples = silence(8_000);
        let wav = write_wav_f32(&samples, 8_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        let resampled = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
        assert!((resampled.len() as isize - 16_000_isize).abs() <= 2);
    }

    #[test]
    fn resample_192k_to_16k_reduces_length() {
        let samples = silence(192_000);
        let wav = write_wav_f32(&samples, 192_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        let resampled = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
        assert!((resampled.len() as isize - 16_000_isize).abs() <= 2);
    }

    #[test]
    fn audio_longer_than_120_seconds_rejected() {
        let samples = silence(16_000 * 121);
        let wav = write_wav_f32(&samples, 16_000, 1);
        let err = decode_wav(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::AudioTooLong);
    }

    #[test]
    fn unsupported_channel_count_rejected() {
        let samples = silence(16_000 * 4); // 4 channels, 1 second
        let wav = write_wav_f32(&samples, 16_000, 4);
        let err = decode_wav(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn unsupported_sample_rate_rejected() {
        let samples = silence(2_000);
        let wav = write_wav_f32(&samples, 2_000, 1);
        let err = decode_wav(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn empty_audio_rejected() {
        let wav = write_wav_f32(&[], 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        let err = normalize_and_validate(decoded.samples).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn nan_float_rejected() {
        let mut samples = silence(16_000);
        samples[0] = f32::NAN;
        let wav = write_wav_f32(&samples, 16_000, 1);
        let err = decode_wav(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn positive_infinity_float_rejected() {
        let mut samples = silence(16_000);
        samples[0] = f32::INFINITY;
        let wav = write_wav_f32(&samples, 16_000, 1);
        let err = decode_wav(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn negative_infinity_float_rejected() {
        let mut samples = silence(16_000);
        samples[0] = f32::NEG_INFINITY;
        let wav = write_wav_f32(&samples, 16_000, 1);
        let err = decode_wav(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn no_resampling_for_16k_input() {
        let samples = silence(16_000);
        let wav = write_wav_f32(&samples, 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        let resampled = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
        assert_eq!(resampled.len(), decoded.samples.len());
    }
}
