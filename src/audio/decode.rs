//! Decode audio files (wav, mp3, m4a/aac, flac, ogg…) to 16 kHz mono f32 in-process.

use anyhow::{anyhow, Context, Result};
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::sample::Sample;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

pub const TARGET_RATE: usize = 16_000;

pub fn supported_extension(ext: &str) -> bool {
    matches!(ext.to_ascii_lowercase().as_str(), "wav" | "mp3" | "m4a" | "aac" | "mp4" | "flac" | "ogg" | "oga" | "opus" | "aiff" | "aif" | "mkv" | "webm")
}

/// Returns (samples at 16 kHz mono, duration in seconds).
pub fn decode_to_16k_mono(path: &Path) -> Result<(Vec<f32>, f64)> {
    let file = Box::new(File::open(path).with_context(|| format!("opening {}", path.display()))?);
    let mss = MediaSourceStream::new(file, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .context("unrecognized audio container")?;
    let track = format.default_track(TrackType::Audio).ok_or_else(|| anyhow!("no audio track"))?;
    let codec_params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| anyhow!("no audio codec parameters"))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(codec_params, &AudioDecoderOptions::default())
        .context("unsupported audio codec")?;
    let track_id = track.id;

    let mut rate = 0usize;
    let mut channels = 1usize;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(Error::IoError(_)) => break,
            Err(e) => return Err(anyhow!("reading audio: {e}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                if rate == 0 {
                    rate = buf.spec().rate() as usize;
                    channels = buf.spec().channels().count().max(1);
                }
                scratch.resize(buf.samples_interleaved(), f32::MID);
                buf.copy_to_slice_interleaved(&mut scratch);
                interleaved.extend_from_slice(&scratch);
            }
            Err(Error::DecodeError(_)) => continue,
            Err(_) => break,
        }
    }
    if rate == 0 || interleaved.is_empty() {
        return Err(anyhow!("no decodable audio"));
    }
    // Downmix.
    let frames = interleaved.len() / channels;
    let mut mono: Vec<f32> = Vec::with_capacity(frames);
    for f in 0..frames {
        let base = f * channels;
        let mut acc = 0.0f32;
        for c in 0..channels {
            acc += interleaved[base + c];
        }
        mono.push(acc / channels as f32);
    }
    let duration = mono.len() as f64 / rate as f64;
    Ok((resample(&mono, rate, TARGET_RATE), duration))
}

/// Linear resampler with a box pre-filter when decimating.
pub fn resample(input: &[f32], from: usize, to: usize) -> Vec<f32> {
    if from == to {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let src: Vec<f32> = if ratio > 1.5 {
        let k = ratio.floor() as usize;
        let mut out = Vec::with_capacity(input.len());
        let mut acc = 0.0f32;
        let mut win = std::collections::VecDeque::with_capacity(k);
        for &s in input {
            win.push_back(s);
            acc += s;
            if win.len() > k {
                acc -= win.pop_front().unwrap_or(0.0);
            }
            out.push(acc / win.len() as f32);
        }
        out
    } else {
        input.to_vec()
    };
    let n = (src.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let pos = i as f64 * ratio;
        let j = pos.floor() as usize;
        let frac = (pos - j as f64) as f32;
        let a = src[j.min(src.len() - 1)];
        let b = src[(j + 1).min(src.len() - 1)];
        out.push(a + (b - a) * frac);
    }
    out
}
