//! Opus decode tests (feature `opus`): exact lengths, fidelity and rejection
//! paths for Ogg/Opus and WebM/Opus.

#![cfg(feature = "opus")]

use pumice_transcription::ErrorCode;
use pumice_transcription::audio::{decode_audio, resample_to_target};
use std::io::Cursor;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

#[test]
fn ogg_opus_decodes_to_exact_length() {
    let decoded = decode_audio(Cursor::new(fixture("sample-pt-48k.opus"))).unwrap();
    assert_eq!(decoded.sample_rate, 48_000);
    assert_eq!(decoded.channels, 1);
    assert_eq!(decoded.samples.len(), 681_120);
    let pcm = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
    assert_eq!(pcm.len(), 227_040);
}

#[test]
fn webm_opus_decodes_to_exact_length() {
    let decoded = decode_audio(Cursor::new(fixture("sample-pt.webm"))).unwrap();
    let pcm = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
    assert_eq!(pcm.len(), 227_040);
}

#[test]
fn live_webm_without_duration_decodes_to_exact_length() {
    let decoded = decode_audio(Cursor::new(fixture("sample-pt-live.webm"))).unwrap();
    let pcm = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
    assert_eq!(pcm.len(), 227_040);
}

#[test]
fn stereo_webm_decodes_to_exact_length() {
    let decoded = decode_audio(Cursor::new(fixture("sample-pt-stereo.webm"))).unwrap();
    assert_eq!(decoded.channels, 2);
    let pcm = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
    assert_eq!(pcm.len(), 227_040);
}

#[test]
fn chrome_mediarecorder_webm_decodes_to_expected_length() {
    let decoded = decode_audio(Cursor::new(fixture("chrome-mediarecorder.webm"))).unwrap();
    let pcm = resample_to_target(&decoded.samples, decoded.sample_rate).unwrap();
    assert_eq!(pcm.len(), 226_560);
}

#[test]
fn opus_rms_is_close_to_wav_source() {
    let source = decode_audio(Cursor::new(fixture("sample-pt.wav"))).unwrap();
    let opus = decode_audio(Cursor::new(fixture("sample-pt-48k.opus"))).unwrap();
    let source_rms = rms(&source.samples);
    let opus_rms = rms(&opus.samples);
    assert!(
        (opus_rms - source_rms).abs() / source_rms < 0.01,
        "source rms {source_rms}, opus rms {opus_rms}"
    );
}

#[test]
fn existing_ogg_opus_fixture_decodes() {
    // Lower-bitrate sibling of sample-pt-48k.opus used by the rejection tests
    // in builds without the feature.
    let decoded = decode_audio(Cursor::new(fixture("sample-pt.opus"))).unwrap();
    assert!((decoded.samples.len() as f64 / 48_000.0 - 14.19).abs() < 1.0);
}

#[test]
fn rejects_corrupted_ogg_page() {
    let mut bytes = fixture("sample-pt-48k.opus");
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x01;
    let err = decode_audio(Cursor::new(bytes)).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
}

#[test]
fn rejects_chained_ogg_opus() {
    let mut bytes = fixture("sample-pt-48k.opus");
    bytes.extend(fixture("sample-pt-48k.opus"));
    let err = decode_audio(Cursor::new(bytes)).unwrap_err();
    assert_eq!(err.code, ErrorCode::UnsupportedFormat);
}

#[test]
fn rejects_truncated_ogg_opus() {
    let bytes = fixture("sample-pt-48k.opus");
    let err = decode_audio(Cursor::new(&bytes[..bytes.len() - 30])).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
}

#[test]
fn rejects_truncated_webm() {
    for name in [
        "sample-pt.webm",
        "sample-pt-live.webm",
        "chrome-mediarecorder.webm",
    ] {
        let bytes = fixture(name);
        let err = decode_audio(Cursor::new(&bytes[..bytes.len() - 17])).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{name}");
    }
}

#[test]
fn rejects_mutated_webm_opus_head() {
    let mut bytes = fixture("sample-pt.webm");
    let at = bytes.windows(8).position(|w| w == b"OpusHead").unwrap();
    bytes[at + 9] = 8; // channel count
    let err = decode_audio(Cursor::new(bytes)).unwrap_err();
    assert_eq!(err.code, ErrorCode::UnsupportedFormat);
}

#[test]
fn vorbis_ogg_still_uses_the_existing_path() {
    let decoded = decode_audio(Cursor::new(fixture("sample-pt.ogg"))).unwrap();
    assert!((decoded.samples.len() as f64 / 16_000.0 - 14.19).abs() < 1.0);
}
