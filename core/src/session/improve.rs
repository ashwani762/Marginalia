//! Plain helpers for listening to a kept recording again on a compute node: laying the audio files on one time
//! line, cutting the audio into pieces, tidying what comes back, and judging whether it is good enough to replace
//! the live transcript.

use crate::audio::decode;
use crate::remote::PcmFrames;
use std::path::Path;

/// 10 seconds of 16 kHz 16 bit mono audio per piece sent to a node.
pub(super) const FRAME_BYTES: usize = 16_000 * 2 * 10;
/// A silence between two kept files up to this long is sent as silence, so the time line stays true.
const MAX_SILENCE_SECS: f64 = 20.0;
/// A longer silence (the phone was off for a while) is squeezed to this much.
const SQUEEZED_SILENCE_SECS: f64 = 1.0;
/// Gaps smaller than this are ignored.
const IGNORED_GAP_SECS: f64 = 0.25;

/// One kept audio file on the time line of the audio that is sent.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PlannedFile {
    pub path: String,
    /// How long it is expected to run, in seconds.
    pub secs: f64,
    /// Silence to send before it, in seconds.
    pub pad_before: f64,
}

/// The kept audio of a meeting laid out as one stretch of sound. Time in that stretch ("stream time") starts at 0
/// and is turned back into meeting time with [`AudioPlan::to_meeting`].
#[derive(Clone, Debug, PartialEq)]
pub(super) struct AudioPlan {
    pub files: Vec<PlannedFile>,
    /// (stream seconds, meeting seconds) at the start of each unbroken piece.
    pieces: Vec<(f64, f64)>,
    pub total_secs: f64,
}

impl AudioPlan {
    /// Lays out files given as (path, seconds into the meeting where it starts, seconds long), in time order.
    pub fn new(files: &[(String, f64, f64)]) -> AudioPlan {
        let mut out: Vec<PlannedFile> = Vec::new();
        let mut pieces: Vec<(f64, f64)> = Vec::new();
        let mut stream = 0.0f64;
        let mut meeting_end = 0.0f64;
        for (i, (path, offset, secs)) in files.iter().enumerate() {
            let secs = secs.max(0.0);
            let mut pad = 0.0;
            if i == 0 {
                pieces.push((0.0, offset.max(0.0)));
            } else {
                let gap = offset - meeting_end;
                if gap > MAX_SILENCE_SECS {
                    pad = SQUEEZED_SILENCE_SECS;
                    pieces.push((stream + pad, *offset));
                } else if gap > IGNORED_GAP_SECS {
                    pad = gap;
                }
            }
            stream += pad + secs;
            meeting_end = offset + secs;
            out.push(PlannedFile { path: path.clone(), secs, pad_before: pad });
        }
        AudioPlan { files: out, pieces, total_secs: stream }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty() || self.total_secs <= 0.0
    }

    pub fn total_ms(&self) -> u64 {
        (self.total_secs * 1000.0).round() as u64
    }

    /// Meeting seconds for a time in the sent audio.
    pub fn to_meeting(&self, stream_secs: f64) -> f64 {
        let (s0, m0) = self.pieces.iter().rev().find(|p| p.0 <= stream_secs).copied().unwrap_or((0.0, 0.0));
        m0 + (stream_secs - s0)
    }

    /// The meeting time where the audio starts and where it ends.
    pub fn meeting_span(&self) -> (f64, f64) {
        (self.to_meeting(0.0), self.to_meeting(self.total_secs))
    }
}

/// Cuts a stream of samples into pieces of [`FRAME_BYTES`], skipping the first `skip` samples (when the audio
/// is resumed in the middle).
pub(super) struct Framer {
    skip: u64,
    buf: Vec<u8>,
    frame_bytes: usize,
}

impl Framer {
    pub fn new(skip_samples: u64, frame_bytes: usize) -> Framer {
        Framer { skip: skip_samples, buf: Vec::with_capacity(frame_bytes), frame_bytes: frame_bytes.max(2) & !1 }
    }

    /// Takes samples and returns the pieces that are full.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<u8>> {
        let mut rest = samples;
        if self.skip > 0 {
            let n = (self.skip as usize).min(rest.len());
            self.skip -= n as u64;
            rest = &rest[n..];
        }
        let mut full = Vec::new();
        for &s in rest {
            let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            self.buf.extend_from_slice(&v.to_le_bytes());
            if self.buf.len() >= self.frame_bytes {
                full.push(std::mem::replace(&mut self.buf, Vec::with_capacity(self.frame_bytes)));
            }
        }
        full
    }

    /// Silence of this many samples.
    pub fn push_silence(&mut self, samples: u64) -> Vec<Vec<u8>> {
        let mut full = Vec::new();
        let mut left = samples;
        while left > 0 {
            let n = left.min(16_000) as usize;
            full.extend(self.push(&vec![0.0f32; n]));
            left -= n as u64;
        }
        full
    }

    pub fn finish(&mut self) -> Option<Vec<u8>> {
        (!self.buf.is_empty()).then(|| std::mem::take(&mut self.buf))
    }
}

fn samples_for(secs: f64) -> u64 {
    (secs.max(0.0) * decode::TARGET_RATE as f64).round() as u64
}

/// The audio of the plan as pieces for a node, starting `start_ms` into it. Decoding runs on its own thread, a few
/// pieces ahead, and stops when the pieces are no longer wanted. A file that cannot be read, or ends early, is
/// replaced by silence of the planned length so the times of everything after it stay right.
pub(super) fn open_frames(plan: &AudioPlan, start_ms: u64) -> PcmFrames {
    let plan = plan.clone();
    let (tx, rx) = crossbeam_channel::bounded::<Vec<u8>>(2);
    std::thread::Builder::new()
        .name("audio-out".into())
        .spawn(move || {
            let mut framer = Framer::new(start_ms * (decode::TARGET_RATE as u64 / 1000), FRAME_BYTES);
            let mut alive = true;
            let send = |frames: Vec<Vec<u8>>| -> bool {
                for f in frames {
                    if tx.send(f).is_err() {
                        return false;
                    }
                }
                true
            };
            for file in &plan.files {
                if !alive {
                    break;
                }
                alive = send(framer.push_silence(samples_for(file.pad_before)));
                if !alive {
                    break;
                }
                let planned = samples_for(file.secs);
                let mut got = 0u64;
                let r = decode::decode_stream_16k_mono(Path::new(&file.path), &mut |chunk| {
                    // Never more than the planned length, so a long tail cannot move what follows.
                    let room = planned.saturating_sub(got) as usize;
                    let chunk = &chunk[..chunk.len().min(room)];
                    got += chunk.len() as u64;
                    alive = send(framer.push(chunk));
                    alive && got < planned
                });
                if let Err(e) = r {
                    tracing::warn!("could not read {}: {e:#}", file.path);
                }
                if alive && got < planned {
                    alive = send(framer.push_silence(planned - got));
                }
            }
            if alive {
                if let Some(last) = framer.finish() {
                    let _ = tx.send(last);
                }
            }
        })
        .expect("spawn audio-out");
    Box::new(std::iter::from_fn(move || rx.recv().ok()))
}

/// A node's text and times, tidied for storing: hallucinated filler removed, endless repeats dropped, and short
/// neighbouring pieces joined into stretches of speech about as long as the live ones are.
pub(super) fn tidy_segments(segments: Vec<(f64, f64, String)>, clean: impl Fn(&str) -> String) -> Vec<(f64, f64, String)> {
    let mut kept: Vec<(f64, f64, String)> = Vec::new();
    let mut same_run = 0usize;
    for (t0, t1, text) in segments {
        let text = clean(&text);
        if text.is_empty() {
            continue;
        }
        let key = |t: &str| t.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        // The same line said three times in a row is a loop in the model, not a speaker.
        if kept.last().map(|l| key(&l.2) == key(&text)).unwrap_or(false) {
            same_run += 1;
            if same_run >= 2 {
                continue;
            }
        } else {
            same_run = 0;
        }
        kept.push((t0, t1.max(t0), text));
    }
    let words = |t: &str| t.split_whitespace().count();
    let mut out: Vec<(f64, f64, String)> = Vec::new();
    for (t0, t1, text) in kept {
        if let Some(last) = out.last_mut() {
            let gap = t0 - last.1;
            let span = t1 - last.0;
            let tiny = words(&text) <= 3 || words(&last.2) <= 3;
            if (gap <= 0.6 && span <= 12.0) || (tiny && gap <= 1.5 && span <= 20.0) {
                last.2.push(' ');
                last.2.push_str(&text);
                last.1 = t1;
                continue;
            }
        }
        out.push((t0, t1, text));
    }
    out
}

/// Whether a node's transcript may take the place of the live one.
#[derive(Debug, PartialEq)]
pub(super) enum Verdict {
    /// Swap what lies between these meeting times (seconds) and keep the live text outside them.
    Replace { from: f64, to: f64 },
    Reject(&'static str),
}

fn word_count(segs: impl Iterator<Item = String>) -> usize {
    segs.map(|t| t.split_whitespace().count()).sum()
}

/// `new` is what the node heard, `old` the live transcript, `audio` the stretch of the meeting the audio covers
/// (meeting seconds). `complete` says the node reached the end. The live transcript is only replaced when the new
/// one has real text, reaches most of the way through what the live one covers, and is not much shorter than it.
/// When the node stopped early, the part it did cover can still be swapped.
pub(super) fn judge(new: &[(f64, f64, String)], old: &[(f64, f64, String)], audio: (f64, f64), complete: bool) -> Verdict {
    let new_words = word_count(new.iter().map(|s| s.2.clone()));
    if new_words == 0 {
        return Verdict::Reject("the compute node heard no words");
    }
    let new_end = new.iter().map(|s| s.1).fold(audio.0, f64::max);
    let to = if complete { audio.1 + 1.5 } else { new_end };
    let from = audio.0;
    let inside = |s: &&(f64, f64, String)| {
        let mid = (s.0 + s.1) / 2.0;
        mid >= from && mid < to
    };
    let old_in: Vec<&(f64, f64, String)> = old.iter().filter(inside).collect();
    let old_words = word_count(old_in.iter().map(|s| s.2.clone()));
    if old_words > 0 && new_words * 10 < old_words * 4 {
        return Verdict::Reject("the new transcript is much shorter than the live one");
    }
    if complete {
        let old_end = old_in.iter().map(|s| s.1).fold(from, f64::max);
        if old_words > 0 && new_end - from < 0.8 * (old_end - from) {
            return Verdict::Reject("the new transcript stops well before the live one does");
        }
    } else if new_words < 10 {
        return Verdict::Reject("too little came back");
    }
    Verdict::Replace { from, to }
}

/// The live transcript outside `from..to` kept, and the new one in between. Live segments are kept or dropped by their
/// middle, so a stretch is never counted twice.
pub(super) fn compose(old: &[(f64, f64, String)], new: &[(f64, f64, String)], from: f64, to: f64) -> Vec<(f64, f64, String)> {
    let mut out: Vec<(f64, f64, String)> = old
        .iter()
        .filter(|s| {
            let mid = (s.0 + s.1) / 2.0;
            mid < from || mid >= to
        })
        .cloned()
        .collect();
    out.extend(new.iter().cloned());
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// A language code worth passing on to a node: two or three letters.
pub(super) fn language_for_node(stored: Option<String>, setting: &str) -> Option<String> {
    let ok = |l: &str| (2..=3).contains(&l.len()) && l.chars().all(|c| c.is_ascii_alphabetic());
    stored.map(|l| l.trim().to_ascii_lowercase()).filter(|l| ok(l)).or_else(|| {
        let s = setting.trim().to_ascii_lowercase();
        (s != "auto" && ok(&s)).then_some(s)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(t0: f64, t1: f64, text: &str) -> (f64, f64, String) {
        (t0, t1, text.to_string())
    }

    fn words(n: usize) -> String {
        vec!["word"; n].join(" ")
    }

    #[test]
    fn one_file_maps_straight_onto_the_meeting() {
        let plan = AudioPlan::new(&[("a.m4a".into(), 0.0, 600.0)]);
        assert_eq!(plan.total_ms(), 600_000);
        assert_eq!(plan.to_meeting(125.5), 125.5);
        assert_eq!(plan.meeting_span(), (0.0, 600.0));
    }

    #[test]
    fn a_resumed_meeting_keeps_its_time_line() {
        // 10 minutes, a 4 second gap, then 5 more minutes: the gap is sent as silence.
        let plan = AudioPlan::new(&[("a".into(), 0.0, 600.0), ("b".into(), 604.0, 300.0)]);
        assert_eq!(plan.files[1].pad_before, 4.0);
        assert_eq!(plan.total_secs, 904.0);
        assert_eq!(plan.to_meeting(700.0), 700.0);
        assert_eq!(plan.meeting_span(), (0.0, 904.0));
    }

    #[test]
    fn a_long_gap_is_squeezed_and_times_still_come_out_right() {
        // The second file starts an hour in: only a second of silence is sent.
        let plan = AudioPlan::new(&[("a".into(), 0.0, 300.0), ("b".into(), 3600.0, 120.0)]);
        assert_eq!(plan.files[1].pad_before, 1.0);
        assert_eq!(plan.total_secs, 421.0);
        assert_eq!(plan.to_meeting(100.0), 100.0);
        assert_eq!(plan.to_meeting(301.0 + 10.0), 3610.0);
        assert_eq!(plan.meeting_span(), (0.0, 3720.0));
    }

    #[test]
    fn audio_that_starts_late_is_not_padded_in_front() {
        let plan = AudioPlan::new(&[("a".into(), 1800.0, 60.0)]);
        assert_eq!(plan.total_secs, 60.0);
        assert_eq!(plan.to_meeting(30.0), 1830.0);
        assert_eq!(plan.meeting_span(), (1800.0, 1860.0));
    }

    #[test]
    fn pieces_are_full_sized_and_the_start_can_be_skipped() {
        let mut f = Framer::new(0, 8);
        let got = f.push(&[0.0, 1.0, -1.0, 0.5, 0.25]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], [0, 0, 0xff, 0x7f, 0x01, 0x80, 0x00, 0x40]);
        assert_eq!(f.finish().unwrap().len(), 2);
        assert!(f.finish().is_none());

        // Skipping 3 samples across two pushes leaves 2 of the 5.
        let mut f = Framer::new(3, 1000);
        assert!(f.push(&[0.1, 0.1]).is_empty());
        assert!(f.push(&[0.1, 0.5, 0.5]).is_empty());
        assert_eq!(f.finish().unwrap().len(), 4);

        let mut f = Framer::new(0, 6400);
        let got = f.push_silence(40_000);
        assert_eq!(got.len(), 12);
        assert_eq!(f.finish().unwrap().len(), 80_000 - 12 * 6400);
    }

    #[test]
    fn filler_loops_and_short_pieces_are_tidied() {
        let clean = |t: &str| crate::engine::whisper::clean_transcript(t);
        let segs = vec![
            seg(0.0, 2.0, "Hello everyone"),
            seg(2.2, 5.0, "and welcome to the meeting."),
            seg(5.5, 6.0, "[BLANK_AUDIO]"),
            seg(30.0, 33.0, "Thank you."),
            seg(40.0, 44.0, "We ship on Friday."),
            seg(50.0, 52.0, "Okay then."),
            seg(52.0, 54.0, "Okay then."),
            seg(54.0, 56.0, "Okay then."),
            seg(56.0, 58.0, "okay then"),
        ];
        let out = tidy_segments(segs, clean);
        assert_eq!(out[0], seg(0.0, 5.0, "Hello everyone and welcome to the meeting."));
        assert!(out.iter().all(|s| !s.2.contains("BLANK") && s.2 != "Thank you."));
        assert_eq!(out.iter().filter(|s| s.2.to_lowercase().contains("okay then")).count(), 1, "three in a row is a loop, and short repeats join");
        assert!(out.iter().any(|s| s.2.starts_with("We ship on Friday.")));
    }

    #[test]
    fn long_stretches_stay_apart() {
        let long = words(30);
        let out = tidy_segments(vec![seg(0.0, 9.0, &long), seg(9.5, 18.0, &long)], |t| t.to_string());
        assert_eq!(out.len(), 2, "joining would pass 12 seconds");
    }

    #[test]
    fn a_good_transcript_replaces_the_live_one() {
        let old = vec![seg(0.0, 30.0, &words(50)), seg(30.0, 60.0, &words(50))];
        let new = vec![seg(0.0, 29.0, &words(55)), seg(30.0, 59.0, &words(52))];
        assert_eq!(judge(&new, &old, (0.0, 60.0), true), Verdict::Replace { from: 0.0, to: 61.5 });
    }

    #[test]
    fn an_empty_or_much_shorter_transcript_never_replaces() {
        let old = vec![seg(0.0, 30.0, &words(100))];
        assert!(matches!(judge(&[], &old, (0.0, 30.0), true), Verdict::Reject(_)));
        assert!(matches!(judge(&[seg(0.0, 3.0, "  ")], &old, (0.0, 30.0), true), Verdict::Reject(_)));
        assert!(matches!(judge(&[seg(0.0, 30.0, &words(20))], &old, (0.0, 30.0), true), Verdict::Reject(_)), "under 40 percent of the words");
        // Right amount of words but it stops half way.
        assert!(matches!(judge(&[seg(0.0, 15.0, &words(60))], &old, (0.0, 30.0), true), Verdict::Reject(_)), "does not cover most of it");
    }

    #[test]
    fn a_silent_live_transcript_accepts_whatever_was_heard() {
        assert_eq!(judge(&[seg(2.0, 5.0, "hello there")], &[], (0.0, 60.0), true), Verdict::Replace { from: 0.0, to: 61.5 });
    }

    #[test]
    fn an_early_stop_replaces_only_what_was_covered() {
        let old = vec![seg(0.0, 60.0, &words(100)), seg(60.0, 120.0, &words(100)), seg(120.0, 180.0, &words(100))];
        let new = vec![seg(0.0, 59.0, &words(110)), seg(60.0, 119.0, &words(105))];
        let v = judge(&new, &old, (0.0, 180.0), false);
        assert_eq!(v, Verdict::Replace { from: 0.0, to: 119.0 });
        let Verdict::Replace { from, to } = v else { unreachable!() };
        let merged = compose(&old, &new, from, to);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].0, 0.0);
        assert_eq!(merged[2], old[2], "the live text is kept for the rest");
        // A trickle is not enough.
        assert!(matches!(judge(&[seg(0.0, 5.0, "a few words")], &old, (0.0, 180.0), false), Verdict::Reject(_)));
    }

    #[test]
    fn audio_that_covers_only_part_of_the_meeting_leaves_the_rest_alone() {
        // The meeting ran 2 minutes before the kept recording starts.
        let old = vec![seg(0.0, 60.0, "early live text here"), seg(60.0, 120.0, "more early live text"), seg(120.0, 150.0, &words(40))];
        let new = vec![seg(121.0, 149.0, &words(44))];
        let Verdict::Replace { from, to } = judge(&new, &old, (120.0, 150.0), true) else { panic!("should be replaced") };
        let merged = compose(&old, &new, from, to);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0], old[0]);
        assert_eq!(merged[1], old[1]);
        assert_eq!(merged[2].0, 121.0);
    }

    #[test]
    fn the_language_given_to_a_node() {
        assert_eq!(language_for_node(Some("hi".into()), "auto").as_deref(), Some("hi"));
        assert_eq!(language_for_node(None, "EN").as_deref(), Some("en"));
        assert_eq!(language_for_node(None, "auto"), None);
        assert_eq!(language_for_node(Some("auto".into()), "auto"), None);
        assert_eq!(language_for_node(Some("hi".into()), "en").as_deref(), Some("hi"), "what the meeting really was beats the setting");
    }

    fn write_wav(path: &std::path::Path, rate: u32, secs: u32) {
        let n = rate * secs;
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + n * 2).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(n * 2).to_le_bytes());
        for i in 0..n {
            let v = ((i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 8000.0) as i16;
            b.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn a_kept_recording_becomes_one_stream_of_the_planned_length() {
        let dir = std::env::temp_dir().join(format!("marginalia-improve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.wav");
        write_wav(&a, 44_100, 2);
        // A second file that cannot be read still holds its place: 3 s of gap, then 1 s.
        let plan = AudioPlan::new(&[(a.to_string_lossy().to_string(), 0.0, 2.0), (dir.join("gone.m4a").to_string_lossy().to_string(), 5.0, 1.0)]);
        assert_eq!(plan.total_ms(), 6000);
        let all: Vec<u8> = open_frames(&plan, 0).flatten().collect();
        assert_eq!(all.len(), 6 * 32_000);
        // The first two seconds are the tone, the rest is silence.
        let loud = |bytes: &[u8]| bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs() as u32).max().unwrap_or(0);
        assert!(loud(&all[..2 * 32_000]) > 5000);
        assert_eq!(loud(&all[2 * 32_000 + 64..]), 0);
        // Carrying on from 1.5 s sends only what comes after.
        let tail: Vec<u8> = open_frames(&plan, 1500).flatten().collect();
        assert_eq!(tail.len(), 4500 * 32);
        assert_eq!(&tail[..64], &all[48_000..48_064]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
