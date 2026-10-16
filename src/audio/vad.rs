//! Energy-based voice activity segmentation with adaptive noise floor.

use crate::engine::{AudioSegment, AudioSource};

pub const SAMPLE_RATE: usize = 16_000;
const FRAME: usize = SAMPLE_RATE / 50; // 20 ms
const PRE_ROLL_FRAMES: usize = 15; // 300 ms kept before speech onset
const START_FRAMES: usize = 2; // 40 ms of speech to start
const END_SILENCE_FRAMES: usize = 30; // 0.6 s of silence to end
const MAX_SEGMENT_SECS: f64 = 10.0;
const PARTIAL_EVERY_SECS: f64 = 0.7;
const PARTIAL_WINDOW_SECS: f64 = 8.0;
const MIN_SEGMENT_SECS: f64 = 0.8;

pub struct Segmenter {
    source: AudioSource,
    pending: Vec<f32>,
    pre_roll: std::collections::VecDeque<Vec<f32>>,
    current: Vec<f32>,
    in_speech: bool,
    speech_run: usize,
    silence_run: usize,
    noise_floor: f32,
    samples_seen: u64,
    seg_start_sample: u64,
    partial_emitted_len: usize,
    pub level: f32,
}

impl Segmenter {
    pub fn new(source: AudioSource) -> Self {
        Self {
            source,
            pending: Vec::with_capacity(FRAME * 4),
            pre_roll: Default::default(),
            current: Vec::new(),
            in_speech: false,
            speech_run: 0,
            silence_run: 0,
            noise_floor: 0.004,
            samples_seen: 0,
            seg_start_sample: 0,
            partial_emitted_len: 0,
            level: 0.0,
        }
    }

    /// Feed 16 kHz mono samples; returns completed segments.
    pub fn push(&mut self, samples: &[f32]) -> Vec<AudioSegment> {
        let mut out = Vec::new();
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= FRAME {
            let frame: Vec<f32> = self.pending.drain(..FRAME).collect();
            if let Some(seg) = self.process_frame(frame) {
                out.push(seg);
            }
        }
        out
    }

    /// Flush whatever speech is buffered (end of session).
    pub fn flush(&mut self) -> Option<AudioSegment> {
        if self.in_speech {
            self.finish_segment()
        } else {
            None
        }
    }

    fn process_frame(&mut self, frame: Vec<f32>) -> Option<AudioSegment> {
        let rms = (frame.iter().map(|x| x * x).sum::<f32>() / frame.len() as f32).sqrt();
        self.level = self.level * 0.6 + rms * 0.4;
        let threshold = (self.noise_floor * 3.2).max(0.012);
        let is_speech = rms > threshold;
        if !is_speech {
            // Track noise floor slowly while quiet.
            self.noise_floor = self.noise_floor * 0.98 + rms * 0.02;
        } else {
            self.noise_floor = self.noise_floor * 0.999 + rms * 0.001;
        }
        self.samples_seen += FRAME as u64;

        if self.in_speech {
            self.current.extend_from_slice(&frame);
            if is_speech {
                self.silence_run = 0;
            } else {
                self.silence_run += 1;
            }
            let dur = self.current.len() as f64 / SAMPLE_RATE as f64;
            if self.silence_run >= END_SILENCE_FRAMES || dur >= MAX_SEGMENT_SECS {
                return self.finish_segment();
            }
            // Provisional transcription of the speech so far.
            let next_at = if self.partial_emitted_len == 0 { (2.0 * PARTIAL_EVERY_SECS * SAMPLE_RATE as f64) as usize } else { self.partial_emitted_len + (PARTIAL_EVERY_SECS * SAMPLE_RATE as f64) as usize };
            if self.current.len() >= next_at {
                self.partial_emitted_len = self.current.len();
                let window = (PARTIAL_WINDOW_SECS * SAMPLE_RATE as f64) as usize;
                let start = self.current.len().saturating_sub(window);
                let t0 = (self.seg_start_sample + start as u64) as f64 / SAMPLE_RATE as f64;
                let t1 = (self.seg_start_sample + self.current.len() as u64) as f64 / SAMPLE_RATE as f64;
                return Some(AudioSegment { source: self.source, pcm: self.current[start..].to_vec(), t0, t1, partial: true });
            }
            None
        } else {
            self.pre_roll.push_back(frame);
            while self.pre_roll.len() > PRE_ROLL_FRAMES {
                self.pre_roll.pop_front();
            }
            if is_speech {
                self.speech_run += 1;
            } else {
                self.speech_run = 0;
            }
            if self.speech_run >= START_FRAMES {
                self.in_speech = true;
                self.silence_run = 0;
                self.speech_run = 0;
                let pre: usize = self.pre_roll.iter().map(|f| f.len()).sum();
                self.seg_start_sample = self.samples_seen.saturating_sub(pre as u64);
                self.current = self.pre_roll.drain(..).flatten().collect();
                self.partial_emitted_len = 0;
            }
            None
        }
    }

    fn finish_segment(&mut self) -> Option<AudioSegment> {
        self.in_speech = false;
        self.silence_run = 0;
        let pcm = std::mem::take(&mut self.current);
        let t0 = self.seg_start_sample as f64 / SAMPLE_RATE as f64;
        let t1 = t0 + pcm.len() as f64 / SAMPLE_RATE as f64;
        self.partial_emitted_len = 0;
        if t1 - t0 < MIN_SEGMENT_SECS {
            // Still clear any provisional line for this source.
            return Some(AudioSegment { source: self.source, pcm: Vec::new(), t0, t1, partial: false });
        }
        Some(AudioSegment { source: self.source, pcm, t0, t1, partial: false })
    }
}
