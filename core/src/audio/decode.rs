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

/// Length of an audio file in seconds, read from the container without decoding it. `None` when the file
/// does not say.
pub fn probe_duration_secs(path: &Path) -> Option<f64> {
    let file = Box::new(File::open(path).ok()?);
    let mss = MediaSourceStream::new(file, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let format = symphonia::default::get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default()).ok()?;
    let track = format.default_track(TrackType::Audio)?;
    let rate = track.codec_params.as_ref().and_then(|p| p.audio()).and_then(|a| a.sample_rate)?;
    let frames = track.num_frames?;
    (rate > 0 && frames > 0).then(|| frames as f64 / rate as f64)
}

/// Decodes a file piece by piece to 16 kHz mono and hands every piece to `on_chunk`, so a long recording is
/// never held in memory whole. `on_chunk` returns false to stop early. Returns how many samples were passed on.
pub fn decode_stream_16k_mono(path: &Path, on_chunk: &mut dyn FnMut(&[f32]) -> bool) -> Result<u64> {
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
    let codec_params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or_else(|| anyhow!("no audio codec parameters"))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(codec_params, &AudioDecoderOptions::default())
        .context("unsupported audio codec")?;
    let track_id = track.id;
    let mut resampler: Option<StreamResampler> = None;
    let mut scratch: Vec<f32> = Vec::new();
    let mut mono: Vec<f32> = Vec::new();
    let mut sent = 0u64;
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
                let rate = buf.spec().rate() as usize;
                let channels = buf.spec().channels().count().max(1);
                let rs = resampler.get_or_insert_with(|| StreamResampler::new(rate, TARGET_RATE));
                scratch.resize(buf.samples_interleaved(), f32::MID);
                buf.copy_to_slice_interleaved(&mut scratch);
                mono.clear();
                mono.extend(scratch.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32));
                let out = rs.push(&mono);
                if !out.is_empty() {
                    sent += out.len() as u64;
                    if !on_chunk(&out) {
                        return Ok(sent);
                    }
                }
            }
            Err(Error::DecodeError(_)) => continue,
            Err(_) => break,
        }
    }
    let Some(mut rs) = resampler else { return Err(anyhow!("no decodable audio")) };
    let tail = rs.finish();
    if !tail.is_empty() {
        sent += tail.len() as u64;
        on_chunk(&tail);
    }
    Ok(sent)
}

/// [`resample`] for audio that arrives in pieces: the same box pre-filter and linear steps, with the
/// state carried from one piece to the next so the joins are seamless.
pub struct StreamResampler {
    ratio: f64,
    /// Width of the box filter (1 when not decimating).
    k: usize,
    win: std::collections::VecDeque<f32>,
    acc: f32,
    /// Filtered samples from global index `base` on, not yet used up.
    buf: Vec<f32>,
    base: u64,
    /// Number of output samples made so far.
    made: u64,
}

impl StreamResampler {
    pub fn new(from: usize, to: usize) -> StreamResampler {
        let ratio = from as f64 / to as f64;
        let k = if from != to && ratio > 1.5 { ratio.floor() as usize } else { 1 };
        StreamResampler { ratio, k, win: std::collections::VecDeque::with_capacity(k + 1), acc: 0.0, buf: Vec::new(), base: 0, made: 0 }
    }

    pub fn push(&mut self, input: &[f32]) -> Vec<f32> {
        if (self.ratio - 1.0).abs() < f64::EPSILON {
            self.made += input.len() as u64;
            return input.to_vec();
        }
        if self.k > 1 {
            for &s in input {
                self.win.push_back(s);
                self.acc += s;
                if self.win.len() > self.k {
                    self.acc -= self.win.pop_front().unwrap_or(0.0);
                }
                self.buf.push(self.acc / self.win.len() as f32);
            }
        } else {
            self.buf.extend_from_slice(input);
        }
        let mut out = Vec::new();
        loop {
            let pos = self.made as f64 * self.ratio;
            let j = pos.floor() as u64;
            if j + 1 >= self.base + self.buf.len() as u64 {
                break;
            }
            let frac = (pos - j as f64) as f32;
            let a = self.buf[(j - self.base) as usize];
            let b = self.buf[(j + 1 - self.base) as usize];
            out.push(a + (b - a) * frac);
            self.made += 1;
        }
        // Samples before the next position are no longer needed.
        let keep_from = ((self.made as f64 * self.ratio).floor() as u64).min(self.base + self.buf.len() as u64);
        let drop = (keep_from - self.base) as usize;
        if drop > 0 {
            self.buf.drain(..drop);
            self.base = keep_from;
        }
        out
    }

    /// The last output samples, which need nothing after them.
    pub fn finish(&mut self) -> Vec<f32> {
        if (self.ratio - 1.0).abs() < f64::EPSILON || self.buf.is_empty() {
            return Vec::new();
        }
        let end = self.base + self.buf.len() as u64;
        let mut out = Vec::new();
        loop {
            let pos = self.made as f64 * self.ratio;
            let j = pos.floor() as u64;
            if j >= end {
                break;
            }
            let frac = (pos - j as f64) as f32;
            let a = self.buf[(j - self.base) as usize];
            let b = self.buf[((j + 1).min(end - 1) - self.base) as usize];
            out.push(a + (b - a) * frac);
            self.made += 1;
        }
        self.buf.clear();
        out
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(n: usize, rate: usize) -> Vec<f32> {
        (0..n).map(|i| ((i as f32 / rate as f32) * 440.0 * std::f32::consts::TAU).sin() * 0.5).collect()
    }

    fn streamed(input: &[f32], from: usize, piece: usize) -> Vec<f32> {
        let mut rs = StreamResampler::new(from, TARGET_RATE);
        let mut out = Vec::new();
        for c in input.chunks(piece) {
            out.extend(rs.push(c));
        }
        out.extend(rs.finish());
        out
    }

    #[test]
    fn pieces_join_up_like_the_whole() {
        for from in [44_100usize, 48_000, 22_050, 16_000] {
            let input = wave(from * 3 + 123, from);
            let whole = resample(&input, from, TARGET_RATE);
            for piece in [1024usize, 777, 5000] {
                let out = streamed(&input, from, piece);
                assert!((out.len() as i64 - whole.len() as i64).abs() <= 1, "{from} {piece}: {} vs {}", out.len(), whole.len());
                let n = out.len().min(whole.len());
                let worst = (0..n).map(|i| (out[i] - whole[i]).abs()).fold(0.0f32, f32::max);
                assert!(worst < 1e-3, "{from} {piece}: worst difference {worst}");
            }
        }
    }

    #[test]
    fn nothing_is_lost_when_pieces_are_tiny() {
        let input = wave(48_000, 48_000);
        let out = streamed(&input, 48_000, 1);
        assert!((out.len() as i64 - 16_000).abs() <= 1);
    }
}
