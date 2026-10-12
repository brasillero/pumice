use std::{error::Error, time::Instant};
use transcribe_cpp::{Backend, Model, ModelOptions, RunOptions, SessionOptions};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 {
        return Err("usage: stt-spike MODEL.gguf AUDIO.wav".into());
    }
    let mut wav = hound::WavReader::open(&args[2])?;
    let spec = wav.spec();
    if spec.sample_rate != 16000
        || spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err("expected 16 kHz mono signed 16-bit PCM WAV".into());
    }
    let pcm: Vec<f32> = wav
        .samples::<i16>()
        .map(|s| s.map(|v| v as f32 / 32768.0))
        .collect::<Result<_, _>>()?;
    let threads = std::thread::available_parallelism()?.get() as i32;
    println!(
        "audio_seconds={:.6}\nthreads={threads}",
        pcm.len() as f64 / 16000.0
    );
    let start = Instant::now();
    let model = Model::load_with(
        &args[1],
        &ModelOptions {
            backend: Backend::Cpu,
            ..Default::default()
        },
    )?;
    println!(
        "model_load_seconds={:.6}\nbackend={}",
        start.elapsed().as_secs_f64(),
        model.backend()
    );
    let start = Instant::now();
    let mut session = model.session_with(&SessionOptions {
        n_threads: threads,
        ..Default::default()
    })?;
    println!("session_init_seconds={:.6}", start.elapsed().as_secs_f64());
    let start = Instant::now();
    let result = session.run(&pcm, &RunOptions::default())?;
    if result.text.trim().is_empty() {
        return Err("transcription returned empty text".into());
    }
    println!(
        "transcription_seconds={:.6}\nlanguage={}\ntext={:?}",
        start.elapsed().as_secs_f64(),
        result.language.as_deref().unwrap_or("unknown"),
        result.text
    );
    Ok(())
}
