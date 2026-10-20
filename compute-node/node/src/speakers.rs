//! Telling speakers apart for phones that ask. The work is done by `marginalia-node-speakers` (sherpa-onnx), a separate program that
//! sits next to the node and is started for each job (see its `main.rs` for why it is not part of the node). It is opt-in twice: the
//! helper program must be installed and the two model files must have been downloaded (`marginalia-node models download --only speakers`).
//! Without either the node does not offer it, and the phone does the work itself. The models are only in memory while a job runs and
//! nothing is kept of the audio.

use anyhow::{anyhow, bail, Context, Result};
use marginalia_proto::SpeakerSpan;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
const HELPER: &str = "marginalia-node-speakers.exe";
#[cfg(not(windows))]
const HELPER: &str = "marginalia-node-speakers";

/// Removes audio and photo files that an earlier run left in the temporary folder, for example when the node was killed or the machine lost power in the middle
/// of a job. Only this node's own names are touched, and only files older than a minute, so a job that is running on another node is left alone.
/// Returns how many were removed.
pub fn clear_leftovers(dir: &Path) -> usize {
    let mut n = 0;
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let ours = (name.starts_with("marginalia-speakers-") && name.ends_with(".pcm")) || (name.starts_with("marginalia-photo-") && name.ends_with(".jpg"));
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|a| a > Duration::from_secs(60)).unwrap_or(false);
        if ours && old && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

/// The helper program next to this one, when it is there.
pub fn helper_path() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.parent()?.join(HELPER);
    p.is_file().then_some(p)
}

/// The two model files in the models folder, when both are there.
pub fn model_files(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let seg = dir.join(crate::catalog::speakers_segmentation().file);
    let voices = dir.join(crate::catalog::speakers_voices().file);
    (seg.is_file() && voices.is_file()).then_some((seg, voices))
}

pub struct Speakers {
    dir: PathBuf,
    helper: Option<PathBuf>,
    /// Ask the helper to use the graphics card.
    want_gpu: bool,
    /// Where the last job really ran: 0 not run yet, 1 the processor, 2 the graphics card.
    last_on: std::sync::atomic::AtomicU8,
}

impl Speakers {
    pub fn new(models_dir: PathBuf, want_gpu: bool) -> Speakers {
        Speakers { dir: models_dir, helper: helper_path(), want_gpu, last_on: Default::default() }
    }

    /// Where telling speakers apart runs, for the status: what was asked for, and where the last job really ran.
    pub fn placement(&self) -> serde_json::Value {
        let on = match self.last_on.load(std::sync::atomic::Ordering::Relaxed) {
            2 => "gpu",
            1 => "cpu",
            _ if self.want_gpu => "gpu",
            _ => "cpu",
        };
        let mut v = serde_json::json!({"on": on, "asked": if self.want_gpu { "gpu" } else { "cpu" }});
        if self.want_gpu && on == "cpu" {
            v["note"] = "this helper build has no graphics card support, so it runs on the processor".into();
        }
        v
    }

    /// The helper is installed and the models are downloaded. The models are checked each time, so downloading them is enough, no restart.
    pub fn available(&self) -> bool {
        self.helper.is_some() && model_files(&self.dir).is_some()
    }

    /// Why it is not offered, for the log when a node starts.
    pub fn why_not(&self) -> Option<&'static str> {
        match (&self.helper, model_files(&self.dir)) {
            (Some(_), Some(_)) => None,
            (None, _) => Some("the helper program marginalia-node-speakers is not next to the node"),
            (_, None) => Some("the speaker models are not downloaded (marginalia-node models download --only speakers)"),
        }
    }

    /// Who spoke when. `pcm` is 16 kHz mono. `people` is how many spoke, or 0 to let the clustering decide with `threshold` (0 for the
    /// default). Blocks until the helper is done, so call it from a thread meant for that.
    pub fn run(&self, pcm: &[i16], people: u32, threshold: f32) -> Result<Vec<SpeakerSpan>> {
        let helper = self.helper.as_ref().ok_or_else(|| anyhow!("the helper program marginalia-node-speakers is not installed"))?;
        let (seg, voices) = model_files(&self.dir).ok_or_else(|| anyhow!("the speaker models are not downloaded: marginalia-node models download --only speakers"))?;
        let audio_s = pcm.len() as f32 / 16_000.0;
        // The audio goes to a file the helper reads and that is removed afterwards.
        let file = std::env::temp_dir().join(format!("marginalia-speakers-{}-{}.pcm", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0)));
        let mut bytes = Vec::with_capacity(pcm.len() * 2);
        for s in pcm {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(&file, &bytes).with_context(|| "cannot write the audio for the speaker helper")?;
        drop(bytes);
        let out = run_helper(helper, &seg, &voices, &file, people, threshold, self.want_gpu, Duration::from_secs_f32(300.0 + audio_s * 1.5));
        let _ = std::fs::remove_file(&file);
        let (stdout, stderr) = out?;
        if self.want_gpu {
            // ONNX Runtime says so when it cannot use the card and falls back to the processor.
            let fell_back = falls_back_to_cpu(&stderr);
            self.last_on.store(if fell_back { 1 } else { 2 }, std::sync::atomic::Ordering::Relaxed);
            if fell_back {
                tracing::warn!("the speaker helper could not use the graphics card and ran on the processor (it needs the GPU build of sherpa-onnx and cuDNN)");
            }
        } else {
            self.last_on.store(1, std::sync::atomic::Ordering::Relaxed);
        }
        parse_answer(&stdout)
    }
}

/// Starts the helper and waits for its one line of output, stopping it if it takes longer than `limit`.
/// Whether the helper's messages say the graphics card could not be used.
pub fn falls_back_to_cpu(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("fallback to cpu") || s.contains("fall back to cpu") || s.contains("falling back to cpu")
}

fn run_helper(helper: &Path, seg: &Path, voices: &Path, audio: &Path, people: u32, threshold: f32, gpu: bool, limit: Duration) -> Result<(String, String)> {
    let mut cmd = Command::new(helper);
    // The segmentation window moves half its length each step. The default of a tenth is 5 times slower for the same result on speech
    // (measured: 93% agreement with the slow setting, 3.4 s instead of 59 s for 4 minutes of audio).
    cmd.arg("--segmentation").arg(seg).arg("--voices").arg(voices).arg("--audio").arg(audio).arg("--shift").arg("0.5");
    if gpu {
        cmd.arg("--provider").arg("cuda");
    }
    if people > 0 {
        cmd.arg("--people").arg(people.to_string());
    }
    if threshold > 0.0 {
        cmd.arg("--threshold").arg(threshold.to_string());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window
    }
    let mut child = cmd.spawn().with_context(|| format!("cannot start {}", helper.display()))?;
    // Both pipes are read on their own threads, or a full pipe would stop the helper.
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            bail!("the speaker helper took longer than {} minutes and was stopped", limit.as_secs() / 60);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let (stdout, stderr) = (out_t.join().unwrap_or_default(), err_t.join().unwrap_or_default());
    if !status.success() {
        let why = stderr.lines().last().filter(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()).unwrap_or_else(|| format!("it stopped with {status}"));
        bail!("the speaker helper failed: {why}");
    }
    Ok((stdout, stderr))
}

/// The helper's JSON line as spans in milliseconds, in time order.
pub fn parse_answer(text: &str) -> Result<Vec<SpeakerSpan>> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).map_err(|_| anyhow!("the speaker helper answered with something unreadable"))?;
    let list = v["spans"].as_array().ok_or_else(|| anyhow!("the speaker helper's answer has no spans"))?;
    Ok(spans_from(list.iter().filter_map(|s| Some((s["start"].as_f64()? as f32, s["end"].as_f64()? as f32, s["speaker"].as_i64()? as i32)))))
}

/// Spans in milliseconds from segments in seconds, in time order. Segments with no length or a negative speaker are left out.
pub fn spans_from(segments: impl Iterator<Item = (f32, f32, i32)>) -> Vec<SpeakerSpan> {
    let mut v: Vec<SpeakerSpan> = segments
        .filter(|(a, b, s)| b > a && *s >= 0)
        .map(|(a, b, s)| SpeakerSpan { start_ms: (a.max(0.0) * 1000.0).round() as u64, end_ms: (b * 1000.0).round() as u64, speaker: s as u32 })
        .collect();
    v.sort_by_key(|s| (s.start_ms, s.end_ms));
    v
}

#[cfg(test)]
mod tests {

    #[test]
    fn notices_the_fallback_to_the_processor() {
        assert!(falls_back_to_cpu("Available providers: CPUExecutionProvider. Fallback to cpu!"));
        assert!(!falls_back_to_cpu("loaded the models"));
        let s = Speakers { dir: PathBuf::from("."), helper: None, want_gpu: true, last_on: Default::default() };
        assert_eq!(s.placement()["on"], "gpu");
        s.last_on.store(1, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(s.placement()["on"], "cpu");
        assert!(s.placement()["note"].as_str().unwrap().contains("no graphics card support"));
    }

    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("marginalia-speakers-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn segments_become_spans_in_time_order_without_empty_or_unlabelled_ones() {
        let spans = spans_from([(4.5, 9.1, 1), (0.0, 4.2, 0), (5.0, 5.0, 2), (6.0, 7.0, -1), (-0.2, 1.0, 0)].into_iter());
        assert_eq!(spans, vec![SpeakerSpan { start_ms: 0, end_ms: 1000, speaker: 0 }, SpeakerSpan { start_ms: 0, end_ms: 4200, speaker: 0 }, SpeakerSpan { start_ms: 4500, end_ms: 9100, speaker: 1 }]);
    }

    #[test]
    fn leftovers_of_an_earlier_run_are_cleared_but_nothing_else_and_nothing_fresh() {
        let d = temp("sweep");
        for f in ["marginalia-speakers-1-2.pcm", "marginalia-photo-1-2.jpg", "notes.pcm", "marginalia-speakers-9-9.txt", "fresh.pcm"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let old = std::time::SystemTime::now() - Duration::from_secs(3600);
        for f in ["marginalia-speakers-1-2.pcm", "marginalia-photo-1-2.jpg", "notes.pcm", "marginalia-speakers-9-9.txt"] {
            std::fs::File::options().write(true).open(d.join(f)).unwrap().set_modified(old).unwrap();
        }
        // A job that is running right now (a fresh file with our name) is left alone.
        std::fs::write(d.join("marginalia-speakers-5-5.pcm"), b"x").unwrap();
        assert_eq!(clear_leftovers(&d), 2);
        let mut left: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        left.sort();
        assert_eq!(left, ["fresh.pcm", "marginalia-speakers-5-5.pcm", "marginalia-speakers-9-9.txt", "notes.pcm"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_answer_line_is_read_and_a_bad_one_is_an_error() {
        let spans = parse_answer(r#"{"voices": 2, "spans": [{"start": 6.83, "end": 10.75, "speaker": 1}, {"start": 0.32, "end": 6.83, "speaker": 0}]}"#).unwrap();
        assert_eq!(spans, vec![SpeakerSpan { start_ms: 320, end_ms: 6830, speaker: 0 }, SpeakerSpan { start_ms: 6830, end_ms: 10750, speaker: 1 }]);
        assert!(parse_answer(r#"{"voices": 0, "spans": []}"#).unwrap().is_empty());
        assert!(parse_answer("not json").is_err() && parse_answer(r#"{"voices": 1}"#).is_err());
    }

    #[test]
    fn the_node_offers_it_only_with_the_helper_and_both_models() {
        let d = temp("offer");
        let none = Speakers { dir: d.clone(), helper: None, want_gpu: false, last_on: Default::default() };
        assert!(!none.available());
        std::fs::write(d.join(crate::catalog::speakers_segmentation().file), b"x").unwrap();
        let with_helper = Speakers { dir: d.clone(), helper: Some(PathBuf::from("helper")), want_gpu: false, last_on: Default::default() };
        assert!(!with_helper.available(), "one model is not enough");
        assert!(with_helper.why_not().unwrap().contains("not downloaded"));
        std::fs::write(d.join(crate::catalog::speakers_voices().file), b"x").unwrap();
        assert!(with_helper.available() && with_helper.why_not().is_none());
        assert!(!none.available(), "the models alone are not enough");
        assert!(none.why_not().unwrap().contains("helper"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A stand in for the helper: a shell script that prints a fixed answer, or fails, or hangs.
    #[cfg(unix)]
    fn fake(d: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = d.join("fake-helper");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn the_helper_is_run_with_the_audio_and_models_and_its_answer_is_used_or_its_failure_reported() {
        let d = temp("run");
        std::fs::write(d.join(crate::catalog::speakers_segmentation().file), b"x").unwrap();
        std::fs::write(d.join(crate::catalog::speakers_voices().file), b"x").unwrap();
        let pcm = vec![1i16; 32_000];
        // It checks that it was given the audio file with the right size and the number of people, then answers.
        let ok = fake(&d, r#"audio=""; people=""; while [ $# -gt 0 ]; do case "$1" in --audio) audio="$2";; --people) people="$2";; esac; shift; done
[ "$(wc -c < "$audio")" = "64000" ] && [ "$people" = "3" ] && echo '{"voices": 2, "spans": [{"start": 0.0, "end": 1.5, "speaker": 0}, {"start": 1.5, "end": 2.0, "speaker": 1}]}'"#);
        let s = Speakers { dir: d.clone(), helper: Some(ok), want_gpu: false, last_on: Default::default() };
        assert_eq!(s.run(&pcm, 3, 0.0).unwrap(), vec![SpeakerSpan { start_ms: 0, end_ms: 1500, speaker: 0 }, SpeakerSpan { start_ms: 1500, end_ms: 2000, speaker: 1 }]);
        // The audio file does not stay behind.
        assert!(std::fs::read_dir(std::env::temp_dir()).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(&format!("marginalia-speakers-{}-", std::process::id()))));
        // A helper that fails says why.
        let bad = fake(&d, "echo 'the speaker models could not be loaded' >&2; exit 1");
        let e = Speakers { dir: d.clone(), helper: Some(bad), want_gpu: false, last_on: Default::default() }.run(&pcm, 0, 0.0).unwrap_err().to_string();
        assert!(e.contains("could not be loaded"), "{e}");
        // One that hangs is stopped.
        let slow = fake(&d, "sleep 30");
        let t = Instant::now();
        let e = run_helper(&slow, Path::new("s"), Path::new("v"), Path::new("a"), 0, 0.0, false, Duration::from_millis(400)).unwrap_err().to_string();
        assert!(e.contains("was stopped") && t.elapsed() < Duration::from_secs(5), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
