//! In-process audio capture: microphone and system audio (PipeWire monitor),
//! resampled to 16 kHz mono, segmented by VAD and handed to whisper.

pub mod decode;
pub mod vad;

use crate::engine::{AudioSource, WhisperHandle};
use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use vad::{Segmenter, SAMPLE_RATE};

pub const WAVE_POINTS: usize = 128;
/// Milliseconds of audio per waveform point.
pub const WAVE_STEP_MS: u64 = 25;

/// Shared, lock-light telemetry for the UI (levels + waveform history).
#[derive(Default)]
pub struct AudioMeters {
    mic_level: AtomicU32,
    sys_level: AtomicU32,
    pub wave: Mutex<VecDeque<f32>>,
    /// Unix millis of the last waveform push, for smooth scrolling between frames.
    last_push_ms: std::sync::atomic::AtomicU64,
    /// Display-side eased levels (updated once per rendered frame).
    smooth_mic: AtomicU32,
    smooth_sys: AtomicU32,
}

impl AudioMeters {
    pub fn level(&self, src: AudioSource) -> f32 {
        let bits = match src {
            AudioSource::Microphone => self.mic_level.load(Ordering::Relaxed),
            AudioSource::System => self.sys_level.load(Ordering::Relaxed),
        };
        f32::from_bits(bits)
    }
    fn set_level(&self, src: AudioSource, v: f32) {
        let cell = match src {
            AudioSource::Microphone => &self.mic_level,
            AudioSource::System => &self.sys_level,
        };
        cell.store(v.to_bits(), Ordering::Relaxed);
    }
    /// Level eased toward the live value; call once per frame for a smooth meter.
    pub fn smoothed_level(&self, src: AudioSource) -> f32 {
        let target = self.level(src);
        let cell = match src {
            AudioSource::Microphone => &self.smooth_mic,
            AudioSource::System => &self.smooth_sys,
        };
        let prev = f32::from_bits(cell.load(Ordering::Relaxed));
        // Fast attack, slower release, like a VU meter.
        let k = if target > prev { 0.45 } else { 0.12 };
        let next = prev + (target - prev) * k;
        cell.store(next.to_bits(), Ordering::Relaxed);
        next
    }

    pub fn wave_snapshot(&self) -> Vec<f32> {
        self.wave.lock().iter().copied().collect()
    }
    /// Fraction [0,1) of the current step that has elapsed since the last push.
    pub fn scroll_phase(&self) -> f32 {
        let last = self.last_push_ms.load(Ordering::Relaxed);
        if last == 0 {
            return 0.0;
        }
        let now = now_ms();
        ((now.saturating_sub(last)) as f32 / WAVE_STEP_MS as f32).clamp(0.0, 1.0)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub struct CaptureConfig {
    pub mic_device: Option<String>,
    pub capture_system: bool,
}

pub struct AudioCapture {
    streams: Vec<cpal::Stream>,
    segmenters: Vec<Arc<Mutex<Segmenter>>>,
    pub meters: Arc<AudioMeters>,
    pub sources: Vec<AudioSource>,
    pub warnings: Vec<String>,
}

// cpal::Stream is !Send on some hosts; we only ever touch it from the UI thread.
impl AudioCapture {
    pub fn start(cfg: &CaptureConfig, whisper: WhisperHandle) -> Result<Self> {
        let host = cpal::default_host();
        let meters = Arc::new(AudioMeters::default());
        let mut streams = Vec::new();
        let mut segmenters = Vec::new();
        let mut sources = Vec::new();
        let mut warnings = Vec::new();

        // Microphone.
        let mic = pick_mic(&host, cfg.mic_device.as_deref());
        match mic {
            Some(dev) => match open_stream(&dev, AudioSource::Microphone, whisper.clone(), meters.clone()) {
                Ok((stream, seg)) => {
                    streams.push(stream);
                    segmenters.push(seg);
                    sources.push(AudioSource::Microphone);
                }
                Err(e) => warnings.push(format!("Microphone unavailable: {e:#}")),
            },
            None => warnings.push("No microphone found".into()),
        }

        // System audio via PipeWire's ALSA plugin: a capture stream flagged with
        // `stream.capture.sink=true` is linked to the default sink's monitor.
        if cfg.capture_system {
            match open_system_stream(&host, whisper.clone(), meters.clone()) {
                Ok((stream, seg)) => {
                    streams.push(stream);
                    segmenters.push(seg);
                    sources.push(AudioSource::System);
                }
                Err(e) => warnings.push(format!("System audio unavailable: {e:#}")),
            }
        }

        if streams.is_empty() {
            return Err(anyhow!("no audio inputs could be opened: {}", warnings.join("; ")));
        }
        for s in &streams {
            s.play().context("starting audio stream")?;
        }
        Ok(Self { streams, segmenters, meters, sources, warnings })
    }

    /// Stop capture and flush trailing speech to the transcriber.
    pub fn stop(self, whisper: &WhisperHandle) {
        for s in &self.streams {
            let _ = s.pause();
        }
        drop(self.streams);
        for seg in &self.segmenters {
            if let Some(s) = seg.lock().flush() {
                whisper.transcribe(s);
            }
        }
    }
}

/// Stable device identifier: the ALSA PCM id (e.g. `pipewire`, `default`)
/// when available, otherwise the human-readable name.
fn dev_name(d: &cpal::Device) -> Option<String> {
    let desc = d.description().ok()?;
    Some(desc.driver().unwrap_or(desc.name()).to_string())
}

/// Input devices as (stable id, friendly label). The PipeWire/ALSA default is
/// always first; raw ALSA plugin names and duplicates are hidden.
pub fn list_input_devices() -> Vec<(String, String)> {
    let host = cpal::default_host();
    let mut out: Vec<(String, String)> = Vec::new();
    let Ok(devs) = host.input_devices() else { return out };
    for d in devs {
        let Ok(desc) = d.description() else { continue };
        let id = desc.driver().unwrap_or(desc.name()).to_string();
        let lower = id.to_ascii_lowercase();
        // Skip virtual/duplicate plugin entries; the system default covers them.
        if matches!(lower.as_str(), "null" | "default" | "pipewire" | "pulse" | "jack" | "oss")
            || lower.starts_with("hw:")
            || lower.starts_with("plughw:")
            || lower.starts_with("front:")
            || lower.starts_with("surround")
            || lower.contains("iec958")
            || lower.contains("hdmi")
            || lower.contains("dmix")
            || lower.contains("dsnoop")
            || lower.starts_with("lavrate")
            || lower.starts_with("samplerate")
            || lower.starts_with("speexrate")
            || lower.starts_with("upmix")
            || lower.starts_with("vdownmix")
            || lower.starts_with("usbstream")
        {
            continue;
        }
        let mut label = desc.name().trim().to_string();
        if let Some(i) = label.find(", ") {
            // "INZONE H9 / INZONE H7, USB Audio" -> "INZONE H9 / INZONE H7"
            label.truncate(i);
        }
        if label.is_empty() || label == id {
            label = id.clone();
        }
        if out.iter().any(|(_, l)| *l == label) {
            continue;
        }
        out.push((id, label));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

fn pick_mic(host: &cpal::Host, wanted: Option<&str>) -> Option<cpal::Device> {
    if let Some(w) = wanted {
        if let Ok(devs) = host.input_devices() {
            for d in devs {
                if dev_name(&d).as_deref() == Some(w) {
                    return Some(d);
                }
            }
        }
    }
    // Prefer the PipeWire/Pulse routed device so the user's default mic is used.
    if let Ok(devs) = host.input_devices() {
        let devs: Vec<_> = devs.collect();
        for pref in ["pipewire", "pulse", "default"] {
            if let Some(d) = devs.iter().find(|d| dev_name(d).as_deref() == Some(pref)) {
                return Some(d.clone());
            }
        }
    }
    host.default_input_device()
}

fn open_system_stream(
    host: &cpal::Host,
    whisper: WhisperHandle,
    meters: Arc<AudioMeters>,
) -> Result<(cpal::Stream, Arc<Mutex<Segmenter>>)> {
    let devs: Vec<_> = host.input_devices().context("enumerating devices")?.collect();
    let dev = devs
        .iter()
        .find(|d| dev_name(d).as_deref() == Some("pipewire"))
        .or_else(|| devs.iter().find(|d| dev_name(d).as_deref() == Some("default")))
        .cloned()
        .ok_or_else(|| anyhow!("PipeWire ALSA device not found (install pipewire-alsa)"))?;
    // Scoped env: PipeWire reads PIPEWIRE_PROPS when the stream is created.
    std::env::set_var("PIPEWIRE_PROPS", "{ stream.capture.sink=true node.name=marginalia-system-capture }");
    let r = open_stream(&dev, AudioSource::System, whisper, meters);
    std::env::remove_var("PIPEWIRE_PROPS");
    r
}

fn open_stream(
    dev: &cpal::Device,
    source: AudioSource,
    whisper: WhisperHandle,
    meters: Arc<AudioMeters>,
) -> Result<(cpal::Stream, Arc<Mutex<Segmenter>>)> {
    let segmenter = Arc::new(Mutex::new(Segmenter::new(source)));
    let default = dev.default_input_config().context("default input config")?;
    // Try to let the device/PipeWire resample for us; otherwise do it ourselves.
    let preferred = cpal::StreamConfig {
        channels: 1,
        sample_rate: SAMPLE_RATE as u32,
        buffer_size: cpal::BufferSize::Default,
    };
    let supports_pref = dev
        .supported_input_configs()
        .map(|cfgs| {
            cfgs.into_iter().any(|c| {
                c.channels() == 1
                    && c.min_sample_rate() <= SAMPLE_RATE as u32
                    && c.max_sample_rate() >= SAMPLE_RATE as u32
                    && c.sample_format() == cpal::SampleFormat::F32
            })
        })
        .unwrap_or(false);
    let (config, format) = if supports_pref {
        (preferred, cpal::SampleFormat::F32)
    } else {
        (default.config(), default.sample_format())
    };
    let in_rate = config.sample_rate as usize;
    let channels = config.channels as usize;
    tracing::info!("audio {:?}: {} Hz x{} {:?}", source, in_rate, channels, format);

    let seg2 = segmenter.clone();
    let mut resampler = Resampler::new(in_rate, channels);
    let mut wave_acc = 0.0f32;
    let mut wave_n = 0usize;
    let err_cb = move |e: cpal::Error| tracing::warn!("audio stream error: {e}");
    let mut handle = move |mono16k: &[f32]| {
        let mut seg = seg2.lock();
        let segs = seg.push(mono16k);
        meters.set_level(source, seg.level);
        drop(seg);
        for s in mono16k {
            wave_acc += s * s;
            wave_n += 1;
            if wave_n >= SAMPLE_RATE * WAVE_STEP_MS as usize / 1000 {
                let rms = (wave_acc / wave_n as f32).sqrt();
                wave_acc = 0.0;
                wave_n = 0;
                let mut w = meters.wave.lock();
                // Combine sources by max so the waveform reflects whoever is loudest.
                if source == AudioSource::System {
                    if let Some(last) = w.back_mut() {
                        if *last < rms {
                            *last = rms;
                        }
                        continue;
                    }
                }
                w.push_back(rms);
                while w.len() > WAVE_POINTS {
                    w.pop_front();
                }
                meters.last_push_ms.store(now_ms(), Ordering::Relaxed);
            }
        }
        for s in segs {
            whisper.transcribe(s);
        }
    };
    let stream = match format {
        cpal::SampleFormat::F32 => dev.build_input_stream(
            config,
            move |data: &[f32], _| {
                let out = resampler.process_f32(data);
                handle(&out);
            },
            err_cb,
            None,
        )?,
        cpal::SampleFormat::I16 => dev.build_input_stream(
            config,
            move |data: &[i16], _| {
                let f: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                let out = resampler.process_f32(&f);
                handle(&out);
            },
            err_cb,
            None,
        )?,
        cpal::SampleFormat::U16 => dev.build_input_stream(
            config,
            move |data: &[u16], _| {
                let f: Vec<f32> = data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0).collect();
                let out = resampler.process_f32(&f);
                handle(&out);
            },
            err_cb,
            None,
        )?,
        other => return Err(anyhow!("unsupported sample format {other:?}")),
    };
    Ok((stream, segmenter))
}

/// Downmix to mono and resample to 16 kHz (linear interpolation after a light
/// box pre-filter when decimating). Adequate for speech recognition.
struct Resampler {
    in_rate: usize,
    channels: usize,
    phase: f64,
    last: f32,
}

impl Resampler {
    fn new(in_rate: usize, channels: usize) -> Self {
        Self { in_rate, channels: channels.max(1), phase: 0.0, last: 0.0 }
    }

    fn process_f32(&mut self, interleaved: &[f32]) -> Vec<f32> {
        let frames = interleaved.len() / self.channels;
        let mut mono: Vec<f32> = Vec::with_capacity(frames);
        for f in 0..frames {
            let base = f * self.channels;
            let mut acc = 0.0;
            for c in 0..self.channels {
                acc += interleaved[base + c];
            }
            mono.push(acc / self.channels as f32);
        }
        if self.in_rate == SAMPLE_RATE {
            return mono;
        }
        let ratio = self.in_rate as f64 / SAMPLE_RATE as f64;
        if ratio > 1.5 {
            // Box filter over `k` samples to suppress aliasing before decimation.
            let k = ratio.floor() as usize;
            let mut filtered = Vec::with_capacity(mono.len());
            let mut acc = 0.0f32;
            let mut win: VecDeque<f32> = VecDeque::with_capacity(k);
            for &s in &mono {
                win.push_back(s);
                acc += s;
                if win.len() > k {
                    acc -= win.pop_front().unwrap_or(0.0);
                }
                filtered.push(acc / win.len() as f32);
            }
            mono = filtered;
        }
        let mut out = Vec::with_capacity((mono.len() as f64 / ratio) as usize + 2);
        let mut pos = self.phase;
        while pos < mono.len() as f64 {
            let i = pos.floor() as usize;
            let frac = (pos - i as f64) as f32;
            let a = if i == 0 { self.last } else { mono[i - 1] };
            let b = mono[i];
            // interpolate between previous and current sample
            out.push(a + (b - a) * frac);
            pos += ratio;
        }
        self.phase = pos - mono.len() as f64;
        if let Some(&l) = mono.last() {
            self.last = l;
        }
        out
    }
}
