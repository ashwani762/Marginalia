//! Speech to text, by whichever engine this node was set to use. Whisper is the default and the one to beat on clear speech. LFM3-Omni
//! can do it too, with the chat model it already has loaded, so no second model sits on the graphics card.
//!
//! LFM3 hears a recording in pieces of at most 30 seconds. Measured on real recordings, it holds up in pieces and falls apart on some
//! when given several minutes at once. It gives no word times, so each piece is one segment with the times of that piece.

use super::llm::LlmHandle;
use super::whisper::{is_quiet, Job, Outcome, Segment, Whisper};
use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const RATE: usize = 16_000;
const PIECE_S: usize = 30;

#[derive(Clone)]
pub enum Transcriber {
    Whisper(Arc<Whisper>),
    Lfm3 { llm: LlmHandle, gpu: bool },
}

/// Where each piece of a recording of `samples` starts and ends. A last scrap under a second belongs to the piece before it.
pub fn pieces(samples: usize) -> Vec<(usize, usize)> {
    let size = PIECE_S * RATE;
    let mut out = Vec::new();
    let mut start = 0;
    while start < samples {
        let end = if samples - start < size + RATE { samples } else { start + size };
        out.push((start, end));
        start = end;
    }
    out
}

/// The instruction LFM3 expects. Without a known language it is left to work it out.
pub fn asr_instruction(language: Option<&str>) -> String {
    let name = match language.map(|l| l.split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase()).as_deref() {
        Some("en") => "English",
        Some("es") => "Spanish",
        Some("pt") => "Portuguese",
        Some("de") => "German",
        Some("fr") => "French",
        Some("it") => "Italian",
        Some("zh") => "Chinese",
        Some("ja") => "Japanese",
        Some("ko") => "Korean",
        Some("hi") => "Hindi",
        Some("ar") => "Arabic",
        Some("nl") => "Dutch",
        Some("ru") => "Russian",
        Some("pl") => "Polish",
        _ => return "Perform ASR.".to_string(),
    };
    format!("Perform ASR in {name}.")
}

impl Transcriber {
    pub fn on_gpu(&self) -> bool {
        match self {
            Transcriber::Whisper(w) => w.on_gpu(),
            Transcriber::Lfm3 { gpu, .. } => *gpu,
        }
    }

    /// As [`Whisper::transcribe`]: 16 kHz mono samples between -1 and 1, segments and percent done as they come.
    pub fn transcribe(&self, pcm: &[f32], job: Job<'_>, cancel: &AtomicBool, on_segment: &mut dyn FnMut(Segment), on_progress: &mut dyn FnMut(i32)) -> Result<Outcome> {
        let llm = match self {
            Transcriber::Whisper(w) => return w.transcribe(pcm, job, cancel, on_segment, on_progress),
            Transcriber::Lfm3 { llm, .. } => llm,
        };
        let instruction = asr_instruction(job.language);
        let list = pieces(pcm.len());
        let mut segments = 0;
        for (n, (start, end)) in list.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("cancelled"));
            }
            let piece = &pcm[*start..*end];
            // Silence is skipped rather than asked about, because a model asked to hear something will find something.
            if !is_quiet(piece) {
                let text = llm.hear(&instruction, piece.to_vec())?;
                let text = text.trim();
                if !text.is_empty() {
                    on_segment(Segment { t0_ms: (*start * 1000 / RATE) as u64, t1_ms: (*end * 1000 / RATE) as u64, text: text.to_string() });
                    segments += 1;
                }
            }
            on_progress(((n + 1) * 100 / list.len()) as i32);
        }
        // An empty language is what a phone reads as "not known". LFM3 does not report what it heard.
        Ok(Outcome { language: job.language.unwrap_or("").to_string(), segments })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_is_cut_in_pieces_that_cover_all_of_it() {
        assert!(pieces(0).is_empty());
        assert_eq!(pieces(RATE * 10), vec![(0, RATE * 10)]);
        assert_eq!(pieces(RATE * 30), vec![(0, RATE * 30)]);
        assert_eq!(pieces(RATE * 65), vec![(0, RATE * 30), (RATE * 30, RATE * 60), (RATE * 60, RATE * 65)]);
    }

    #[test]
    fn a_scrap_at_the_end_joins_the_piece_before_it() {
        assert_eq!(pieces(RATE * 30 + RATE / 2), vec![(0, RATE * 30 + RATE / 2)]);
    }

    #[test]
    fn the_instruction_names_the_language_when_it_is_known() {
        assert_eq!(asr_instruction(Some("en")), "Perform ASR in English.");
        assert_eq!(asr_instruction(Some("hi-IN")), "Perform ASR in Hindi.");
        assert_eq!(asr_instruction(Some("xx")), "Perform ASR.");
        assert_eq!(asr_instruction(None), "Perform ASR.");
    }
}
