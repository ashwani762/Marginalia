//! Tells speakers apart on a recording. `marginalia-node` starts this program for each job and reads its answer, and it is optional:
//! a node without it (or without the two speaker models) does not offer the work and the phone does it itself.
//!
//! It is a separate program on purpose. The sherpa-onnx library brings its own ONNX Runtime, built with the other C++ string layout than
//! llama.cpp, and linking both into one program corrupts memory the moment ONNX Runtime starts (`free(): invalid pointer`). Apart,
//! each is fine. It also means the speaker models are in memory only while a job runs, and a crash here cannot take the node down.
//!
//! Usage: `marginalia-node-speakers --segmentation FILE --voices FILE --audio FILE [--people N] [--threshold T] [--threads N] [--provider cpu|cuda]`
//! where the audio is raw PCM, 16 bit little endian, mono, 16 kHz. It prints one JSON line:
//! `{"voices": 2, "spans": [{"start": 0.32, "end": 6.83, "speaker": 0}, ...]}` (seconds), or an error on stderr and exit code 1.

use sherpa_onnx::{FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig, OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig, SpeakerEmbeddingExtractorConfig};
use std::process::ExitCode;

const SAMPLE_RATE: i32 = 16_000;

struct Args {
    segmentation: String,
    voices: String,
    audio: String,
    people: i32,
    threshold: f32,
    threads: i32,
    /// How far the segmentation window moves each step, as a share of the window (0 means the library default, 0.1).
    shift: f32,
    /// `cpu` (the default) or `cuda`. `cuda` needs the GPU build of sherpa-onnx, otherwise ONNX Runtime falls back to the processor.
    provider: String,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args { segmentation: String::new(), voices: String::new(), audio: String::new(), people: 0, threshold: 0.0, threads: 0, shift: 0.0, provider: "cpu".into() };
    let mut it = args.iter();
    while let Some(k) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(|| format!("{k} needs a value"));
        match k.as_str() {
            "--segmentation" => a.segmentation = value()?,
            "--voices" => a.voices = value()?,
            "--audio" => a.audio = value()?,
            "--people" => a.people = value()?.parse().map_err(|_| "--people must be a number".to_string())?,
            "--threshold" => a.threshold = value()?.parse().map_err(|_| "--threshold must be a number".to_string())?,
            "--shift" => a.shift = value()?.parse().map_err(|_| "--shift must be a number".to_string())?,
            "--threads" => a.threads = value()?.parse().map_err(|_| "--threads must be a number".to_string())?,
            "--provider" => {
                a.provider = value()?;
                if !matches!(a.provider.as_str(), "cpu" | "cuda") {
                    return Err("--provider must be cpu or cuda".into());
                }
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    if a.segmentation.is_empty() || a.voices.is_empty() || a.audio.is_empty() {
        return Err("needs --segmentation, --voices and --audio".into());
    }
    Ok(a)
}

/// Raw 16 bit little endian PCM as samples from -1 to 1.
fn samples_from(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect()
}

fn run(a: &Args) -> Result<serde_json::Value, String> {
    let bytes = std::fs::read(&a.audio).map_err(|e| format!("cannot read {}: {e}", a.audio))?;
    let samples = samples_from(&bytes);
    drop(bytes);
    if samples.len() < SAMPLE_RATE as usize {
        return Ok(serde_json::json!({"voices": 0, "spans": []}));
    }
    let threads = if a.threads > 0 { a.threads } else { (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2) / 2).clamp(2, 8) as i32 };
    let config = OfflineSpeakerDiarizationConfig {
        segmentation: OfflineSpeakerSegmentationModelConfig {
            pyannote: OfflineSpeakerSegmentationPyannoteModelConfig { model: Some(a.segmentation.clone()), window_shift_ratio: if a.shift > 0.0 { a.shift } else { 0.1 } },
            num_threads: threads,
            provider: Some(a.provider.clone()),
            ..Default::default()
        },
        embedding: SpeakerEmbeddingExtractorConfig { model: Some(a.voices.clone()), num_threads: threads, provider: Some(a.provider.clone()), ..Default::default() },
        clustering: FastClusteringConfig { num_clusters: if a.people > 0 { a.people } else { -1 }, threshold: if a.threshold > 0.0 { a.threshold } else { 0.5 }, ..Default::default() },
        min_duration_on: 0.3,
        min_duration_off: 0.5,
    };
    let engine = OfflineSpeakerDiarization::create(&config).ok_or("the speaker models could not be loaded")?;
    if engine.sample_rate() != SAMPLE_RATE {
        return Err(format!("the speaker model wants {} Hz audio", engine.sample_rate()));
    }
    let result = engine.process(&samples).ok_or("telling the speakers apart failed")?;
    let spans: Vec<serde_json::Value> = result
        .sort_by_start_time()
        .iter()
        .filter(|s| s.end > s.start && s.speaker >= 0)
        .map(|s| serde_json::json!({"start": s.start, "end": s.end, "speaker": s.speaker}))
        .collect();
    Ok(serde_json::json!({"voices": result.num_speakers(), "spans": spans}))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args).and_then(|a| run(&a)) {
        Ok(v) => {
            println!("{v}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options_are_read_and_the_three_files_are_required() {
        let a = parse(&args(&["--segmentation", "s.onnx", "--voices", "v.onnx", "--audio", "a.pcm", "--people", "3", "--threshold", "0.45"])).unwrap();
        assert_eq!((a.segmentation.as_str(), a.people, a.threshold, a.threads), ("s.onnx", 3, 0.45, 0));
        assert!(parse(&args(&["--voices", "v", "--audio", "a"])).is_err());
        assert!(parse(&args(&["--segmentation", "s", "--voices", "v", "--audio", "a", "--bogus"])).is_err());
        assert!(parse(&args(&["--segmentation", "s", "--voices", "v", "--audio"])).is_err());
        assert!(parse(&args(&["--segmentation", "s", "--voices", "v", "--audio", "a", "--people", "x"])).is_err());
    }

    #[test]
    fn pcm_bytes_become_samples() {
        assert_eq!(samples_from(&[0, 0, 0xff, 0x7f, 0x00, 0x80, 0x01]), vec![0.0, 32767.0 / 32768.0, -1.0]);
    }

    #[test]
    fn a_recording_shorter_than_a_second_has_no_speakers_and_needs_no_models() {
        let d = std::env::temp_dir().join(format!("marginalia-speakers-helper-{}.pcm", std::process::id()));
        std::fs::write(&d, vec![0u8; 3200]).unwrap();
        let a = Args { segmentation: "missing".into(), voices: "missing".into(), audio: d.to_string_lossy().to_string(), people: 0, threshold: 0.0, threads: 0, shift: 0.0, provider: "cpu".into() };
        assert_eq!(run(&a).unwrap(), serde_json::json!({"voices": 0, "spans": []}));
        let _ = std::fs::remove_file(&d);
        assert!(run(&Args { audio: "/nonexistent/x.pcm".into(), ..a }).is_err());
    }
}
