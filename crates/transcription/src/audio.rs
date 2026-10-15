use crate::engine::CancelToken;
use crate::{Error, ErrorCode};
use rubato::audioadapter_buffers::owned::InterleavedOwned;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use std::io::Read;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSourceStream, ReadOnlySource};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioFormat {
    Wav,
    Mp3,
    Flac,
    Ogg,
    #[cfg(feature = "opus")]
    Webm,
    Unknown,
}

impl AudioFormat {
    /// Detect the container format by inspecting the first bytes of the file.
    /// File extensions are never consulted.
    fn detect(bytes: &[u8]) -> Self {
        if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
            return Self::Wav;
        }
        if bytes.starts_with(b"fLaC") {
            return Self::Flac;
        }
        if bytes.len() >= 4 && &bytes[0..4] == b"OggS" {
            return Self::Ogg;
        }
        // EBML magic: WebM/Matroska. Only routed to the Opus decoder in
        // `opus` builds; otherwise it falls through to Unknown, exactly as
        // before.
        #[cfg(feature = "opus")]
        if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
            return Self::Webm;
        }
        // MP3: an ID3 tag at the start, or an MPEG audio frame sync word (11 set bits).
        if bytes.starts_with(b"ID3") {
            return Self::Mp3;
        }
        if bytes.len() >= 2 && (u16::from_be_bytes([bytes[0], bytes[1]]) & 0xFFE0) == 0xFFE0 {
            return Self::Mp3;
        }
        Self::Unknown
    }
}

fn make_media_source(bytes: Vec<u8>) -> MediaSourceStream<'static> {
    let source = ReadOnlySource::new(std::io::Cursor::new(bytes));
    MediaSourceStream::new(Box::new(source), Default::default())
}

/// Decode an audio file using Symphonia.
///
/// The input is read through the provided reader (already bounded by our own
/// byte budget). Decoding proceeds one packet at a time; the duration limit is
/// enforced per-packet before any downmix or resample buffer is allocated, and
/// no up-front reservation is made from header-declared lengths.
///
/// Supported formats: RIFF/WAVE, MP3, FLAC and Ogg Vorbis; with the `opus`
/// feature also Ogg/Opus and WebM/Opus. Ogg/Opus is detected as an Ogg
/// container; without the feature it is rejected with `UnsupportedFormat`
/// because Symphonia has no Opus decoder.
pub fn decode_audio<R: Read + Send + Sync>(reader: R) -> Result<DecodedAudio, Error> {
    decode_audio_with_cancel(reader, &CancelToken::new())
}

/// [`decode_audio`] that stops between packets once `cancel` is set.
pub fn decode_audio_with_cancel<R: Read + Send + Sync>(
    mut reader: R,
    cancel: &CancelToken,
) -> Result<DecodedAudio, Error> {
    // Read the whole file into memory. The caller wraps the reader in a
    // BudgetReader so this is bounded by MAX_FILE_BYTES.
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|e| Error::invalid_input(format!("failed to read audio data: {e}")))?;

    match AudioFormat::detect(&bytes) {
        AudioFormat::Wav => decode_wav_bytes(bytes, cancel),
        AudioFormat::Mp3 => {
            let mss = make_media_source(bytes);
            let mut format =
                symphonia::default::formats::MpaReader::try_new(mss, FormatOptions::default())
                    .map_err(map_symphonia_error)?;
            decode_track(&mut format, None, cancel)
        }
        AudioFormat::Flac => {
            let mss = make_media_source(bytes);
            let mut format =
                symphonia::default::formats::FlacReader::try_new(mss, FormatOptions::default())
                    .map_err(map_symphonia_error)?;
            decode_track(&mut format, None, cancel)
        }
        AudioFormat::Ogg => {
            #[cfg(feature = "opus")]
            if crate::opus::ogg::is_ogg_opus(&bytes) {
                return crate::opus::ogg::decode_ogg_opus(bytes, cancel);
            }
            let mss = make_media_source(bytes);
            let mut format =
                symphonia::default::formats::OggReader::try_new(mss, FormatOptions::default())
                    .map_err(map_symphonia_error)?;
            decode_track(&mut format, None, cancel)
        }
        #[cfg(feature = "opus")]
        AudioFormat::Webm => crate::opus::webm::decode_webm_opus(bytes, cancel),
        AudioFormat::Unknown => Err(Error::unsupported_format(
            "unknown or unsupported audio format",
        )),
    }
}

/// Decode a RIFF/WAVE file that has already passed strict pre-validation.
fn decode_wav_bytes(bytes: Vec<u8>, cancel: &CancelToken) -> Result<DecodedAudio, Error> {
    // Strict pre-validation: catch malformed RIFF/WAVE headers before Symphonia
    // sees them, so invalid input always maps to a safe typed error instead of a
    // decoder panic. Use only the canonical WAV built from validated chunks so
    // that embedded or trailing chunks cannot be reinterpreted as audio data.
    let (format_info, canonical) = crate::riff::validate_wav(&bytes)?;

    let mss = make_media_source(canonical);
    let mut format = symphonia::default::formats::WavReader::try_new(mss, FormatOptions::default())
        .map_err(map_symphonia_error)?;

    let expected_frames = (format_info.data_bytes / u32::from(format_info.block_align)) as usize;
    decode_track(&mut format, Some(expected_frames), cancel)
}

/// Decode a single audio track from any Symphonia format reader.
///
/// `expected_frames` is supplied by strict WAV validation. Compressed streams
/// may also declare an exact frame count in their headers.
fn decode_track(
    format: &mut dyn FormatReader,
    expected_frames: Option<usize>,
    cancel: &CancelToken,
) -> Result<DecodedAudio, Error> {
    let audio_track_count = format
        .tracks()
        .iter()
        .filter(|t| {
            t.codec_params
                .as_ref()
                .map(|p| p.is_audio())
                .unwrap_or(false)
        })
        .count();
    if audio_track_count != 1 {
        return Err(Error::unsupported_format(
            "audio file must contain exactly one audio track",
        ));
    }

    let track = &format.tracks()[0];
    let track_id = track.id;
    let expected_frames = expected_frames.map(|n| n as u64).or(track.num_frames);
    let codec_params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .cloned()
        .ok_or_else(|| Error::invalid_input("audio track has no codec parameters"))?;

    let sample_rate = codec_params
        .sample_rate
        .ok_or_else(|| Error::invalid_input("audio file has no sample rate"))?;
    let channels = codec_params
        .channels
        .as_ref()
        .map(|c| c.count() as u16)
        .unwrap_or(0);

    if !matches!(channels, 1 | 2) {
        return Err(Error::unsupported_format(format!(
            "cannot downmix {} channels to mono",
            channels
        )));
    }

    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
        return Err(Error::unsupported_format(format!(
            "unsupported sample rate: {} Hz (expected {}-{} Hz)",
            sample_rate, MIN_SAMPLE_RATE, MAX_SAMPLE_RATE
        )));
    }

    let decoder_opts = AudioDecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&codec_params, &decoder_opts)
        .map_err(map_symphonia_error)?;

    let mut samples = Vec::new();
    let mut decoded_frames: usize = 0;

    loop {
        if cancel.is_cancelled() {
            return Err(Error::cancelled());
        }

        // Enforce the duration limit before decoding the next packet.
        let seconds = decoded_frames as f64 / sample_rate as f64;
        if seconds > MAX_AUDIO_SECONDS {
            return Err(Error::audio_too_long(seconds));
        }

        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => return Err(map_symphonia_error(e)),
        };

        if packet.track_id != track_id {
            return Err(Error::unsupported_format(
                "audio file switched to a different track",
            ));
        }

        // A packet may contain many decoded frames. Reject a declared duration
        // over the limit before the decoder allocates its output buffer. The
        // decoded frame count is checked below as well, since headers are untrusted.
        let declared_frames = packet.block_dur().get();
        if declared_frames
            > (MAX_AUDIO_SECONDS as u64 * u64::from(sample_rate))
                .saturating_sub(decoded_frames as u64)
        {
            return Err(Error::audio_too_long(
                (decoded_frames as u64).saturating_add(declared_frames) as f64 / sample_rate as f64,
            ));
        }

        let decoded = decoder.decode(&packet).map_err(map_symphonia_error)?;
        let frames = decoded.frames();
        decoded_frames = decoded_frames
            .checked_add(frames)
            .ok_or_else(|| Error::audio_too_long(MAX_AUDIO_SECONDS + 1.0))?;

        // Enforce again after this packet's frames are known.
        let seconds = decoded_frames as f64 / sample_rate as f64;
        if seconds > MAX_AUDIO_SECONDS {
            return Err(Error::audio_too_long(seconds));
        }

        let plane_count = decoded.spec().channels().count();
        if decoded.spec().rate() != sample_rate || plane_count != usize::from(channels) {
            return Err(Error::unsupported_format(
                "audio stream changed sample rate or channel layout",
            ));
        }
        let packet_samples_count = frames * plane_count;
        let mut packet_samples = Vec::with_capacity(packet_samples_count);
        decoded.copy_to_vec_interleaved::<f32>(&mut packet_samples);

        // Reject non-finite samples before clamping.
        if packet_samples.iter().any(|s| !s.is_finite()) {
            return Err(Error::invalid_input("audio contains non-finite samples"));
        }

        // Clamp float samples to [-1, 1]; integer conversions already produce
        // values in this range.
        for s in &mut packet_samples {
            *s = s.clamp(-1.0, 1.0);
        }

        samples.extend(packet_samples);
    }

    if let Some(expected) = expected_frames
        && decoded_frames as u64 != expected
    {
        return Err(Error::invalid_input(format!(
            "decoded {} frames, expected {} (truncated or partial audio data)",
            decoded_frames, expected
        )));
    }

    let mono = downmix_to_mono(samples, channels)?;

    Ok(DecodedAudio {
        sample_rate,
        channels,
        samples: mono,
    })
}

pub(crate) fn map_symphonia_error(err: SymphoniaError) -> Error {
    let msg = err.to_string();
    match err {
        SymphoniaError::Unsupported(_) => Error::unsupported_format(msg),
        SymphoniaError::IoError(_) if msg.contains("byte budget exceeded") => {
            Error::input_too_large(MAX_FILE_BYTES)
        }
        // Decode errors are mapped to invalid_input. We fail the file on the
        // first decode error instead of trying to skip packets; this keeps the
        // policy simple and prevents a corrupt stream from looping forever.
        SymphoniaError::IoError(_)
        | SymphoniaError::DecodeError(_)
        | SymphoniaError::LimitError(_) => Error::invalid_input(msg),
        SymphoniaError::ResetRequired => {
            Error::unsupported_format("chained audio streams are not supported")
        }
        _ => Error::internal(msg),
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

pub(crate) fn downmix_to_mono(samples: Vec<f32>, channels: u16) -> Result<Vec<f32>, Error> {
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
        fmt.extend_from_slice(&0x0001u16.to_le_bytes()); // WAVE_FORMAT_PCM
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
        fmt.extend_from_slice(&0xFFFEu16.to_le_bytes()); // WAVE_FORMAT_EXTENSIBLE
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&sample_rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        fmt.extend_from_slice(&22u16.to_le_bytes()); // extension size
        fmt.extend_from_slice(&16u16.to_le_bytes()); // valid bits
        fmt.extend_from_slice(&0u32.to_le_bytes()); // channel mask
        fmt.extend_from_slice(&[
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38,
            0x9b, 0x71,
        ]); // PCM subformat GUID

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
        fmt.extend_from_slice(&0xFFFEu16.to_le_bytes()); // WAVE_FORMAT_EXTENSIBLE
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&sample_rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&32u16.to_le_bytes());
        fmt.extend_from_slice(&22u16.to_le_bytes()); // extension size
        fmt.extend_from_slice(&32u16.to_le_bytes()); // valid bits
        fmt.extend_from_slice(&0u32.to_le_bytes()); // channel mask
        fmt.extend_from_slice(&[
            0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38,
            0x9b, 0x71,
        ]); // IEEE float subformat GUID

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

    fn write_wav_24bit_in_32bit_container(
        samples: &[i32],
        sample_rate: u32,
        channels: u16,
    ) -> Vec<u8> {
        // 24-bit samples stored in the most-significant bits of a 32-bit container
        // via WAVE_FORMAT_EXTENSIBLE (lower 8 bits are zero padding).
        let bytes_per_sample = 4usize;
        let block_align = (channels as usize * bytes_per_sample) as u16;
        let byte_rate = sample_rate * block_align as u32;
        let data_bytes = samples.len() * bytes_per_sample;

        let mut fmt = Vec::new();
        fmt.extend_from_slice(&0xFFFEu16.to_le_bytes()); // WAVE_FORMAT_EXTENSIBLE
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&sample_rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&32u16.to_le_bytes()); // bits per sample (container)
        fmt.extend_from_slice(&22u16.to_le_bytes()); // extension size
        fmt.extend_from_slice(&24u16.to_le_bytes()); // valid bits per sample
        fmt.extend_from_slice(&0u32.to_le_bytes()); // channel mask
        fmt.extend_from_slice(&[
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38,
            0x9b, 0x71,
        ]); // PCM subformat GUID

        let mut data = Vec::with_capacity(data_bytes);
        for &s in samples {
            // Shift 24-bit sample into the most-significant bits of the 32-bit container.
            let padded = s << 8;
            data.extend_from_slice(&padded.to_le_bytes());
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

    fn write_wav_wrong_subformat_guid(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        let bytes_per_sample = 2usize;
        let block_align = (channels as usize * bytes_per_sample) as u16;
        let byte_rate = sample_rate * block_align as u32;
        let data_bytes = samples.len() * bytes_per_sample;

        let mut fmt = Vec::new();
        fmt.extend_from_slice(&0xFFFEu16.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&sample_rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        fmt.extend_from_slice(&0u32.to_le_bytes());
        // Random GUID that is neither PCM nor IEEE float.
        fmt.extend_from_slice(&[0xde; 16]);

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

    fn write_wav_riff_size_too_small(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        let mut wav = write_wav_extensible_pcm(samples, sample_rate, channels);
        // RIFF size is at bytes 4..8. Make it smaller than the actual chunks.
        wav[4..8].copy_from_slice(&16u32.to_le_bytes());
        wav
    }

    fn silence(samples: usize) -> Vec<f32> {
        vec![0.0f32; samples]
    }

    #[test]
    fn decode_16bit_mono() {
        let samples: Vec<i16> = (0..16000).map(|i| (i as i16).wrapping_mul(2)).collect();
        let wav = write_wav_i16(&samples, 16_000, 1);
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples.len(), 16_000);
        assert!(decoded.samples.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn decode_8bit_full_scale() {
        let wav = write_wav_i8(&[i8::MAX, i8::MIN], 16_000, 1);
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
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
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
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
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
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
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
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
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
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
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.samples.len(), 16_000);
        // Downmix of [0, 1] repeated should average to 0.5.
        assert!((decoded.samples[100] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn decode_odd_sized_junk_chunk() {
        // Regression: odd-sized chunks must have their mandatory pad byte honored.
        let samples: Vec<i16> = (0..16000).map(|i| (i as i16).wrapping_mul(2)).collect();
        let wav = write_wav_with_odd_junk(&samples, 16_000, 1);
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples.len(), 16_000);
        assert!(decoded.samples.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn decode_extensible_pcm() {
        let wav = write_wav_extensible_pcm(&[i16::MAX, i16::MIN], 16_000, 1);
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
        assert!((decoded.samples[0] - 32_767.0 / 32_768.0).abs() < 1e-6);
        assert!((decoded.samples[1] - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn decode_extensible_float() {
        let wav = write_wav_extensible_float(&[1.0, -1.0], 16_000, 1);
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
        assert!((decoded.samples[0] - 1.0).abs() < 1e-6);
        assert!((decoded.samples[1] - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn decode_24bit_in_32bit_container() {
        // Samples are stored as 24-bit values in a 32-bit container.
        let wav = write_wav_24bit_in_32bit_container(&[8_388_607, -8_388_608, 0], 16_000, 1);
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
        assert_eq!(decoded.samples.len(), 3);
        assert!(
            (decoded.samples[0] - 8_388_607.0 / 8_388_608.0).abs() < 1e-6,
            "24-in-32 max scaled to {}",
            decoded.samples[0]
        );
        assert!(
            (decoded.samples[1] - (-1.0)).abs() < 1e-6,
            "24-in-32 min scaled to {}",
            decoded.samples[1]
        );
        assert!(decoded.samples[2].abs() < 1e-6);
    }

    #[test]
    fn wrong_subformat_guid_rejected() {
        let wav = write_wav_wrong_subformat_guid(&[0, 0], 16_000, 1);
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn riff_size_smaller_than_data_rejected() {
        let wav = write_wav_riff_size_too_small(&[0; 10], 16_000, 1);
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert!(matches!(
            err.code,
            ErrorCode::InvalidInput | ErrorCode::UnsupportedFormat
        ));
    }

    #[test]
    fn fmt_size_all_ones_rejected_cleanly() {
        // 20-byte file: RIFF header + WAVE form + fmt chunk header with size 0xffffffff.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&12u32.to_le_bytes()); // RIFF size: just enough for WAVE + fmt header
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&0xffffffffu32.to_le_bytes());

        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
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
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
        let resampled = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
        let expected_len =
            (samples.len() as f64 * TARGET_SAMPLE_RATE as f64 / sample_rate as f64).ceil() as usize;
        assert!((resampled.len() as isize - expected_len as isize).abs() <= 2);
    }

    #[test]
    fn resample_8k_to_16k_doubles_length() {
        let samples = silence(8_000);
        let wav = write_wav_f32(&samples, 8_000, 1);
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
        let resampled = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
        assert!((resampled.len() as isize - 16_000_isize).abs() <= 2);
    }

    #[test]
    fn resample_192k_to_16k_reduces_length() {
        let samples = silence(192_000);
        let wav = write_wav_f32(&samples, 192_000, 1);
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
        let resampled = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
        assert!((resampled.len() as isize - 16_000_isize).abs() <= 2);
    }

    #[test]
    fn audio_longer_than_120_seconds_rejected() {
        let samples = silence(16_000 * 121);
        let wav = write_wav_f32(&samples, 16_000, 1);
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::AudioTooLong);
    }

    #[test]
    fn unsupported_channel_count_rejected() {
        let samples = silence(16_000 * 4); // 4 channels, 1 second
        let wav = write_wav_f32(&samples, 16_000, 4);
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn unsupported_sample_rate_rejected() {
        let samples = silence(2_000);
        let wav = write_wav_f32(&samples, 2_000, 1);
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn empty_audio_rejected() {
        let wav = write_wav_f32(&[], 16_000, 1);
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
        let err = normalize_and_validate(decoded.samples).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn nan_float_rejected() {
        let mut samples = silence(16_000);
        samples[0] = f32::NAN;
        let wav = write_wav_f32(&samples, 16_000, 1);
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn positive_infinity_float_rejected() {
        let mut samples = silence(16_000);
        samples[0] = f32::INFINITY;
        let wav = write_wav_f32(&samples, 16_000, 1);
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn negative_infinity_float_rejected() {
        let mut samples = silence(16_000);
        samples[0] = f32::NEG_INFINITY;
        let wav = write_wav_f32(&samples, 16_000, 1);
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn no_resampling_for_16k_input() {
        let samples = silence(16_000);
        let wav = write_wav_f32(&samples, 16_000, 1);
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
        let resampled = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
        assert_eq!(resampled.len(), decoded.samples.len());
    }

    // --- Round 3 reproducer tests: strict RIFF/WAVE pre-validation ---

    const KSDATAFORMAT_SUBTYPE_PCM: [u8; 16] = [
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b,
        0x71,
    ];
    const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: [u8; 16] = [
        0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b,
        0x71,
    ];

    fn build_wav_raw(
        format_tag: u16,
        channels: u16,
        sample_rate: u32,
        block_align: u16,
        bits_per_sample: u16,
        extra_fmt: Option<&[u8]>,
        data: &[u8],
    ) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&format_tag.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&sample_rate.to_le_bytes());
        let avg_bytes_per_sec = sample_rate * u32::from(block_align);
        fmt.extend_from_slice(&avg_bytes_per_sec.to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&bits_per_sample.to_le_bytes());
        if let Some(extra) = extra_fmt {
            fmt.extend_from_slice(extra);
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
        out.extend_from_slice(data);
        out
    }

    fn build_extensible_wav(
        channels: u16,
        channel_mask: u32,
        bits_per_sample: u16,
        valid_bits: u16,
        float: bool,
        data: &[u8],
    ) -> Vec<u8> {
        let block_align = channels as u32 * (bits_per_sample as u32 / 8);
        let mut extra = Vec::new();
        extra.extend_from_slice(&22u16.to_le_bytes());
        extra.extend_from_slice(&valid_bits.to_le_bytes());
        extra.extend_from_slice(&channel_mask.to_le_bytes());
        extra.extend_from_slice(if float {
            &KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        } else {
            &KSDATAFORMAT_SUBTYPE_PCM
        });
        build_wav_raw(
            0xFFFE,
            channels,
            16_000,
            block_align as u16,
            bits_per_sample,
            Some(&extra),
            data,
        )
    }

    #[test]
    fn reject_extensible_channels_33_mask_zero() {
        // channels=33 with mask=0 must still be rejected (channels must be 1 or 2).
        let data = vec![0u8; 33 * 2 * 3]; // 33 channels * 2 bytes * 3 frames
        let wav = build_extensible_wav(33, 0, 16, 16, false, &data);
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn reject_extensible_reserved_channel_mask_bit() {
        // channels=2 with reserved bit 0x80000000 set in the mask.
        let data = vec![0u8; 2 * 2 * 3];
        let wav = build_extensible_wav(2, 0x8000_0000, 16, 16, false, &data);
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn reject_pcm_channels_65535() {
        let mut wav = build_wav_raw(1, 1, 16_000, 2, 16, None, &[0u8; 6]);
        wav[22..24].copy_from_slice(&65_535u16.to_le_bytes());
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn reject_riff_size_larger_than_physical_file() {
        let mut wav = build_wav_raw(1, 1, 16_000, 2, 16, None, &[0u8; 6]);
        let size = u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]);
        wav[4..8].copy_from_slice(&(size + 100).to_le_bytes());
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn reject_truncated_data_chunk_body() {
        let mut wav = build_wav_raw(1, 1, 16_000, 2, 16, None, &[0u8; 6]);
        // Declare data chunk size larger than the bytes that follow.
        wav[40..44].copy_from_slice(&100u32.to_le_bytes());
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn reject_missing_odd_chunk_pad_byte() {
        // Build a WAV with an odd-sized JUNK chunk but do not include its pad byte.
        let data = vec![0u8; 6];
        let fmt = {
            let mut f = Vec::new();
            f.extend_from_slice(&1u16.to_le_bytes());
            f.extend_from_slice(&1u16.to_le_bytes());
            f.extend_from_slice(&16_000u32.to_le_bytes());
            f.extend_from_slice(&(16_000u32 * 2).to_le_bytes());
            f.extend_from_slice(&2u16.to_le_bytes());
            f.extend_from_slice(&16u16.to_le_bytes());
            f
        };
        let junk = b"x"; // odd size, requires pad byte
        let riff_size = 4 + 8 + fmt.len() as u32 + 8 + junk.len() as u32 + 8 + data.len() as u32;

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"JUNK");
        out.extend_from_slice(&(junk.len() as u32).to_le_bytes());
        out.extend_from_slice(junk);
        // Missing pad byte: the odd-size pad is not present, so chunk sizes no
        // longer line up with RIFF size.
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);

        let err = decode_audio(Cursor::new(&out)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn reject_second_fmt_chunk() {
        let data = vec![0u8; 6];
        let mut wav = build_wav_raw(1, 1, 16_000, 2, 16, None, &data);
        let second_fmt = vec![0u8; 16];
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&(second_fmt.len() as u32).to_le_bytes());
        wav.extend_from_slice(&second_fmt);
        let size = u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]);
        wav[4..8].copy_from_slice(&(size + 8 + second_fmt.len() as u32).to_le_bytes());
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn reject_second_data_chunk() {
        let data = vec![0u8; 6];
        let mut wav = build_wav_raw(1, 1, 16_000, 2, 16, None, &data);
        let second_data = vec![0u8; 4];
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(second_data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&second_data);
        let size = u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]);
        wav[4..8].copy_from_slice(&(size + 8 + second_data.len() as u32).to_le_bytes());
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn reject_data_size_not_multiple_of_block_align() {
        let mut wav = build_wav_raw(1, 1, 16_000, 2, 16, None, &[0u8; 5]);
        wav[40..44].copy_from_slice(&5u32.to_le_bytes());
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn reject_truncated_audio_data() {
        // Data chunk declares 6 bytes (3 frames) but only 4 bytes are present.
        // RIFF size is kept consistent with the actual bytes.
        let mut wav = build_wav_raw(1, 1, 16_000, 2, 16, None, &[0u8; 4]);
        wav[40..44].copy_from_slice(&6u32.to_le_bytes());
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn reject_pcm_24bit_block_align_4() {
        // Classic malformed 24-bit PCM with block_align=4 instead of 3.
        let wav = build_wav_raw(1, 1, 16_000, 4, 24, None, &[0u8; 12]);
        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn extensible_24_in_32_container_still_works() {
        let samples: Vec<i32> = vec![8_388_607, -8_388_608, 0];
        let mut data = Vec::new();
        for &s in &samples {
            data.extend_from_slice(&(s << 8).to_le_bytes());
        }
        let wav = build_extensible_wav(1, 0, 32, 24, false, &data);
        let decoded = decode_audio(Cursor::new(&wav)).unwrap();
        assert_eq!(decoded.samples.len(), 3);
        assert!((decoded.samples[0] - 8_388_607.0 / 8_388_608.0).abs() < 1e-6);
        assert!((decoded.samples[1] - (-1.0)).abs() < 1e-6);
        assert!(decoded.samples[2].abs() < 1e-6);
    }

    #[test]
    fn reject_extensible_fmt_with_embedded_data_chunk() {
        // Regression: a malformed EXTENSIBLE `fmt ` body longer than 40 bytes
        // can contain an embedded `data` chunk header. Symphonia reads only 40
        // bytes of the `fmt ` body and may parse the remainder as chunks,
        // decoding the embedded audio instead of the real top-level `data`
        // chunk. We require exactly 40-byte EXTENSIBLE `fmt ` bodies so this
        // file is rejected instead of decoding the hidden audio.
        let hidden_audio = vec![0xabu8; 16]; // would decode to non-silence if used
        let real_audio = vec![0u8; 16]; // real top-level data is silence

        let block_align = 2; // mono 16-bit
        let mut fmt_body = Vec::new();
        fmt_body.extend_from_slice(&0xFFFEu16.to_le_bytes());
        fmt_body.extend_from_slice(&1u16.to_le_bytes());
        fmt_body.extend_from_slice(&16_000u32.to_le_bytes());
        fmt_body.extend_from_slice(&(16_000u32 * block_align as u32).to_le_bytes());
        fmt_body.extend_from_slice(&(block_align as u16).to_le_bytes());
        fmt_body.extend_from_slice(&16u16.to_le_bytes());
        fmt_body.extend_from_slice(&22u16.to_le_bytes());
        fmt_body.extend_from_slice(&16u16.to_le_bytes());
        fmt_body.extend_from_slice(&0u32.to_le_bytes());
        fmt_body.extend_from_slice(&KSDATAFORMAT_SUBTYPE_PCM);
        assert_eq!(fmt_body.len(), 40);

        // Append an embedded `data` chunk header + hidden audio inside the fmt body.
        fmt_body.extend_from_slice(b"data");
        fmt_body.extend_from_slice(&(hidden_audio.len() as u32).to_le_bytes());
        fmt_body.extend_from_slice(&hidden_audio);

        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        let riff_size = 4 + 8 + fmt_body.len() as u32 + 8 + real_audio.len() as u32;
        wav.extend_from_slice(&riff_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");

        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&(fmt_body.len() as u32).to_le_bytes());
        wav.extend_from_slice(&fmt_body);

        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(real_audio.len() as u32).to_le_bytes());
        wav.extend_from_slice(&real_audio);

        let err = decode_audio(Cursor::new(&wav)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn deterministic_fuzz_does_not_panic() {
        // Generate a small valid WAV and deterministically mutate it. Every
        // outcome must be either success or a clean error; panics fail the test.
        let base = build_wav_raw(1, 1, 16_000, 2, 16, None, &[0u8; 6 * 2]);
        let mut rng = DeterministicRng::new(3811);

        for i in 0..500 {
            let mut wav = base.clone();
            let rounds = 1 + (rng.next() % 8);
            for _ in 0..rounds {
                if wav.is_empty() {
                    break;
                }
                let pos = rng.next() as usize % wav.len();
                let mode = rng.next() % 4;
                match mode {
                    0 => {
                        let bit = rng.next() % 8;
                        wav[pos] ^= 1 << bit;
                    }
                    1 => {
                        let len = (rng.next() as usize % 4).min(wav.len() - pos);
                        for j in 0..len {
                            wav[pos + j] = rng.next() as u8;
                        }
                    }
                    2 => {
                        wav.truncate(pos);
                    }
                    _ => {
                        let insert_len = rng.next() % 8;
                        for _ in 0..insert_len {
                            wav.insert(pos, rng.next() as u8);
                        }
                    }
                }
            }

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = decode_audio(Cursor::new(&wav));
            }));
            assert!(
                result.is_ok(),
                "panic on iteration {} with mutated wav {:?}",
                i,
                wav
            );
        }
    }

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures")).join(name)
    }

    fn read_fixture(name: &str) -> Vec<u8> {
        std::fs::read(fixture_path(name)).unwrap()
    }

    #[test]
    fn detect_wav_by_content() {
        assert_eq!(AudioFormat::detect(b"RIFF\0\0\0\0WAVE"), AudioFormat::Wav);
    }

    #[test]
    fn detect_mp3_by_content() {
        assert_eq!(AudioFormat::detect(b"ID3"), AudioFormat::Mp3);
        assert_eq!(AudioFormat::detect(&[0xFF, 0xFB]), AudioFormat::Mp3);
    }

    #[test]
    fn detect_flac_by_content() {
        assert_eq!(AudioFormat::detect(b"fLaC"), AudioFormat::Flac);
    }

    #[test]
    fn detect_ogg_by_content() {
        assert_eq!(AudioFormat::detect(b"OggS"), AudioFormat::Ogg);
    }

    #[test]
    fn detect_unknown_content() {
        assert_eq!(AudioFormat::detect(b"not audio"), AudioFormat::Unknown);
        assert_eq!(AudioFormat::detect(b""), AudioFormat::Unknown);
    }

    #[test]
    fn decode_fixture_mp3() {
        let decoded = decode_audio(std::io::Cursor::new(read_fixture("sample-pt.mp3"))).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        // Duration is ~14.2 s; allow a wide tolerance for encoder delay/padding.
        assert!((decoded.samples.len() as f64 / 16_000.0 - 14.19).abs() < 1.0);
    }

    #[test]
    fn decode_fixture_flac() {
        let decoded = decode_audio(std::io::Cursor::new(read_fixture("sample-pt.flac"))).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert!((decoded.samples.len() as f64 / 16_000.0 - 14.19).abs() < 0.5);
    }

    #[test]
    fn decode_fixture_ogg_vorbis() {
        let decoded = decode_audio(std::io::Cursor::new(read_fixture("sample-pt.ogg"))).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert!((decoded.samples.len() as f64 / 16_000.0 - 14.19).abs() < 1.0);
    }

    #[test]
    fn decode_fixture_wav_ignores_extension() {
        // Decoding is by content; the file extension is irrelevant.
        let bytes = read_fixture("sample-pt.wav");
        let decoded = decode_audio(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert!((decoded.samples.len() as f64 / 16_000.0 - 14.19).abs() < 0.5);
    }

    #[test]
    fn ogg_opus_rejected_as_unsupported() {
        let err = decode_audio(std::io::Cursor::new(read_fixture("sample-pt.opus"))).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn chained_ogg_streams_rejected_as_unsupported() {
        let mut bytes = read_fixture("sample-pt.ogg");
        bytes.extend(read_fixture("sample-pt.ogg"));
        let err = decode_audio(std::io::Cursor::new(bytes)).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn multichannel_flac_rejected() {
        let err =
            decode_audio(std::io::Cursor::new(read_fixture("three-channel.flac"))).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedFormat);
        assert!(err.message.contains("3 channels"));
    }

    #[test]
    fn mp3_magic_without_valid_frames_returns_typed_error() {
        for bytes in [b"ID3garbage".as_slice(), &[0xff, 0xfb, 0, 0][..]] {
            let err = decode_audio(std::io::Cursor::new(bytes)).unwrap_err();
            assert!(matches!(
                err.code,
                ErrorCode::InvalidInput | ErrorCode::UnsupportedFormat
            ));
        }
    }

    #[test]
    fn mp3_with_declared_frame_count_rejects_truncation() {
        let bytes = read_fixture("sample-pt.mp3");
        let err = decode_audio(std::io::Cursor::new(&bytes[..bytes.len() / 2])).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn empty_file_rejected_as_unsupported() {
        let err = decode_audio(std::io::Cursor::new(&[])).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn random_bytes_rejected_as_unsupported() {
        let mut rng = DeterministicRng::new(7723);
        let mut bytes = vec![0u8; 2048];
        for b in &mut bytes {
            *b = rng.next() as u8;
        }
        let err = decode_audio(std::io::Cursor::new(bytes)).unwrap_err();
        assert!(matches!(
            err.code,
            ErrorCode::UnsupportedFormat | ErrorCode::InvalidInput
        ));
    }

    #[test]
    fn truncated_files_rejected_cleanly() {
        for name in [
            "sample-pt.wav",
            "sample-pt.mp3",
            "sample-pt.flac",
            "sample-pt.ogg",
        ] {
            let bytes = read_fixture(name);
            let truncated = &bytes[..64.min(bytes.len())];
            let err = decode_audio(std::io::Cursor::new(truncated)).unwrap_err();
            assert!(
                matches!(
                    err.code,
                    ErrorCode::InvalidInput | ErrorCode::UnsupportedFormat
                ),
                "{name} truncated file should error, got {:?}",
                err.code
            );
        }
    }

    #[test]
    fn long_compressed_audio_rejected_as_too_long() {
        let err = decode_audio(std::io::Cursor::new(read_fixture("long-silence.ogg"))).unwrap_err();
        assert_eq!(err.code, ErrorCode::AudioTooLong);
    }

    struct DeterministicRng {
        state: u64,
    }

    impl DeterministicRng {
        fn new(seed: u64) -> Self {
            Self { state: seed }
        }

        fn next(&mut self) -> u64 {
            // xorshift64*: simple deterministic PRNG.
            self.state ^= self.state >> 12;
            self.state ^= self.state << 25;
            self.state ^= self.state >> 27;
            self.state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
    }
}
