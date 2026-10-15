//! Opus decode tests (feature `opus`): exact lengths, fidelity and rejection
//! paths for Ogg/Opus and WebM/Opus.

#![cfg(feature = "opus")]

use pumice_transcription::audio::{decode_audio, resample_to_target};
use pumice_transcription::ErrorCode;
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
