use crate::{Error, ErrorCode};
use hound::{SampleFormat, WavReader, WavSpec};
use rubato::audioadapter_buffers::owned::InterleavedOwned;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use std::io::{Read, Seek};

pub const TARGET_SAMPLE_RATE: u32 = 16_000;
pub const MAX_FILE_BYTES: u64 = 25_000_000;
pub const MAX_AUDIO_SECONDS: f64 = 120.0;
pub const MIN_SAMPLE_RATE: u32 = 8_000;
pub const MAX_SAMPLE_RATE: u32 = 192_000;

#[derive(Debug)]
pub struct DecodedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

pub fn decode_wav<R: Read + Seek>(reader: R) -> Result<DecodedAudio, Error> {
    let wav = WavReader::new(reader).map_err(|e| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("failed to read WAV header: {e}"),
        )
    })?;
    let spec = wav.spec();

    validate_spec(&spec)?;

    let samples = read_samples(&spec, wav)?;
    let mono = downmix_to_mono(samples, spec.channels)?;

    Ok(DecodedAudio {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        samples: mono,
    })
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

fn validate_spec(spec: &WavSpec) -> Result<(), Error> {
    if spec.channels == 0 || spec.channels > 2 {
        return Err(Error::unsupported_format(format!(
            "unsupported channel count: {} (expected 1 or 2)",
            spec.channels
        )));
    }

    if spec.sample_rate < MIN_SAMPLE_RATE || spec.sample_rate > MAX_SAMPLE_RATE {
        return Err(Error::unsupported_format(format!(
            "unsupported sample rate: {} Hz (expected {}–{} Hz)",
            spec.sample_rate, MIN_SAMPLE_RATE, MAX_SAMPLE_RATE
        )));
    }

    match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Int, 8 | 16 | 24 | 32) | (SampleFormat::Float, 32) => Ok(()),
        _ => Err(Error::unsupported_format(format!(
            "unsupported WAV format: {:?} {}-bit",
            spec.sample_format, spec.bits_per_sample
        ))),
    }
}

fn read_samples<R: Read>(spec: &WavSpec, mut wav: WavReader<R>) -> Result<Vec<f32>, Error> {
    match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Int, 8) => wav
            .samples::<i8>()
            .map(|s| s.map(|v| (v as f32) / 128.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(sample_error),
        (SampleFormat::Int, 16) => wav
            .samples::<i16>()
            .map(|s| s.map(|v| (v as f32) / 32_768.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(sample_error),
        (SampleFormat::Int, 24) => wav
            .samples::<i32>()
            .map(|s| s.map(|v| (v as f32) / 8_388_608.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(sample_error),
        (SampleFormat::Int, 32) => wav
            .samples::<i32>()
            .map(|s| s.map(|v| (v as f32) / 2_147_483_648.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(sample_error),
        (SampleFormat::Float, 32) => wav
            .samples::<f32>()
            .map(|s| {
                s.and_then(|v| {
                    if v.is_finite() {
                        Ok(v.clamp(-1.0, 1.0))
                    } else {
                        Err(hound::Error::Unsupported)
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(sample_error),
        _ => Err(Error::unsupported_format("unsupported WAV sample format")),
    }
}

fn sample_error(e: hound::Error) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("failed to decode WAV samples: {e}"),
    )
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
    use hound::WavWriter;
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
        assert!((decoded.samples[0] - 127.0 / 128.0).abs() < 1e-6, "8-bit max scaled to {}", decoded.samples[0]);
        assert!((decoded.samples[1] - (-1.0)).abs() < 1e-6, "8-bit min scaled to {}", decoded.samples[1]);
    }

    #[test]
    fn decode_16bit_full_scale() {
        let wav = write_wav_i16(&[i16::MAX, i16::MIN], 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert!((decoded.samples[0] - 32_767.0 / 32_768.0).abs() < 1e-6, "16-bit max scaled to {}", decoded.samples[0]);
        assert!((decoded.samples[1] - (-1.0)).abs() < 1e-6, "16-bit min scaled to {}", decoded.samples[1]);
    }

    #[test]
    fn decode_24bit_full_scale() {
        let wav = write_wav_i24(&[8_388_607, -8_388_608], 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert!((decoded.samples[0] - 8_388_607.0 / 8_388_608.0).abs() < 1e-6, "24-bit max scaled to {}", decoded.samples[0]);
        assert!((decoded.samples[1] - (-1.0)).abs() < 1e-6, "24-bit min scaled to {}", decoded.samples[1]);
    }

    #[test]
    fn decode_32bit_full_scale() {
        let wav = write_wav_i32(&[i32::MAX, i32::MIN], 16_000, 1);
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        assert!((decoded.samples[0] - 2_147_483_647.0 / 2_147_483_648.0).abs() < 1e-7, "32-bit max scaled to {}", decoded.samples[0]);
        assert!((decoded.samples[1] - (-1.0)).abs() < 1e-7, "32-bit min scaled to {}", decoded.samples[1]);
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
        let decoded = decode_wav(Cursor::new(&wav)).unwrap();
        let resampled = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
        let err = normalize_and_validate(resampled).unwrap_err();
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
