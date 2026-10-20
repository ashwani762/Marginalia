//! Where the models run: the graphics card, the processor, or both (mixed).
//!
//! In mixed mode the chat model is split: part of its layers on the graphics card, the rest on the processor.
//! The split is either worked out by the node from the free graphics memory and the model sizes (automatic),
//! or set by hand together with where the search and speech models go. The choice is one line in the state
//! folder (`run-on`), for example `mixed` or `mixed chat=60 search=cpu speech=gpu`.

use crate::models_dl::RunOn;
use anyhow::{Context, Result};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting {
    pub mode: RunOn,
    /// Mixed only: percent of the chat model's layers on the graphics card (0 is the processor only, 100 the card only).
    /// None means the node works it out from the free graphics memory.
    pub chat: Option<u8>,
    /// Mixed only: the same for the search model. None means the node works it out.
    pub search: Option<u8>,
    /// Mixed only: the speech model on the graphics card (true) or the processor (false). None means the graphics card.
    /// whisper.cpp cannot split a model, so it is one or the other.
    pub speech_gpu: Option<bool>,
    /// Mixed only: reading photos (the vision projector) on the graphics card or the processor. None means the graphics card.
    pub photos_gpu: Option<bool>,
    /// Telling speakers apart (the helper program) on the graphics card or the processor. None means the processor.
    /// The graphics card needs a helper built with the GPU version of sherpa-onnx, otherwise it runs on the processor.
    pub speakers_gpu: Option<bool>,
}

impl Setting {
    pub fn plain(mode: RunOn) -> Setting {
        Setting { mode, chat: None, search: None, speech_gpu: None, photos_gpu: None, speakers_gpu: None }
    }
    pub fn line(&self) -> String {
        let mut s = self.mode.name().to_string();
        let gc = |g: bool| if g { "gpu" } else { "cpu" };
        if let Some(g) = self.speakers_gpu {
            s += &format!(" speakers={}", gc(g));
        }
        if self.mode == RunOn::Mixed {
            if let Some(c) = self.chat {
                s += &format!(" chat={c}");
            }
            if let Some(c) = self.search {
                s += &format!(" search={c}");
            }
            if let Some(g) = self.speech_gpu {
                s += &format!(" speech={}", gc(g));
            }
            if let Some(g) = self.photos_gpu {
                s += &format!(" photos={}", gc(g));
            }
        }
        s
    }
    pub fn parse(line: &str) -> Option<Setting> {
        let mut parts = line.split_whitespace();
        let mode = match parts.next()? {
            "auto" => RunOn::Auto,
            "gpu" => RunOn::Gpu,
            "cpu" => RunOn::Cpu,
            "mixed" => RunOn::Mixed,
            _ => return None,
        };
        let mut s = Setting::plain(mode);
        for p in parts {
            let (k, v) = p.split_once('=')?;
            match k {
                "chat" => s.chat = Some(share(v)?),
                "search" => s.search = Some(share(v)?),
                "speech" => s.speech_gpu = Some(place(v)?),
                "photos" => s.photos_gpu = Some(place(v)?),
                "speakers" => s.speakers_gpu = Some(place(v)?),
                _ => return None,
            }
        }
        Some(s)
    }
    /// How people read it.
    pub fn words(&self) -> String {
        let speakers = match self.speakers_gpu {
            Some(true) => ", speakers on the graphics card",
            _ => "",
        };
        match self.mode {
            RunOn::Auto | RunOn::Gpu | RunOn::Cpu => format!("{}{speakers}", self.mode.words()),
            RunOn::Mixed => {
                let split = |what: &str, v: Option<u8>| match v {
                    None => format!("{what} split automatically"),
                    Some(0) => format!("{what} on the processor"),
                    Some(100) => format!("{what} on the graphics card"),
                    Some(c) => format!("{what} {c} percent on the graphics card"),
                };
                let at = |what: &str, g: Option<bool>| format!("{what} on {}", if g.unwrap_or(true) { "the graphics card" } else { "the processor" });
                format!("both: {}, {}, {}, {}{speakers}", split("chat", self.chat), split("search", self.search), at("speech", self.speech_gpu), at("photos", self.photos_gpu))
            }
        }
    }
}

/// A share in percent. `gpu` and `cpu` are accepted as 100 and 0.
fn share(v: &str) -> Option<u8> {
    match v {
        "gpu" => Some(100),
        "cpu" => Some(0),
        _ => Some(v.parse::<u8>().ok()?.min(100)),
    }
}

fn place(v: &str) -> Option<bool> {
    match v {
        "gpu" => Some(true),
        "cpu" => Some(false),
        _ => None,
    }
}

pub fn saved(state: &Path) -> Option<Setting> {
    Setting::parse(std::fs::read_to_string(state.join("run-on")).ok()?.trim())
}

pub fn save(state: &Path, s: &Setting) -> Result<()> {
    std::fs::create_dir_all(state)?;
    std::fs::write(state.join("run-on"), s.line()).context("could not save where the models run")
}

const MB: u64 = 1024 * 1024;

/// The graphics memory a model of `bytes` needs: its size times `factor` plus `extra_mb`. Nothing when it is not there.
fn need(bytes: Option<u64>, factor: f64, extra_mb: u64) -> u64 {
    bytes.map(|b| (b as f64 * factor) as u64 + extra_mb * MB).unwrap_or(0)
}

/// Model file sizes in bytes, None when that model is not there.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sizes {
    pub chat: Option<u64>,
    pub search: Option<u64>,
    pub speech: Option<u64>,
    pub photos: Option<u64>,
}

/// The split the node picks: percent of the chat and search models on the graphics card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Split {
    pub chat: u8,
    pub search: u8,
}

/// Fills in what `s` leaves to the node, from the free graphics memory `free` (before the node loads anything)
/// and the model sizes. A little memory stays free for the system and other programs. The speech and photo
/// models are placed first (they cannot be split), then the search model (small, used often), then as much of
/// the chat model as fits, in steps of 5 percent. A share set by hand is kept as it is.
pub fn auto_split(free: u64, s: &Setting, z: &Sizes) -> Split {
    // The estimates come from models.toml ([memory]).
    let m = crate::catalog::get().memory;
    let speech = if s.speech_gpu.unwrap_or(true) { need(z.speech, m.speech_factor, m.speech_extra_mb) } else { 0 };
    let photos = if s.photos_gpu.unwrap_or(true) { need(z.photos, m.photos_factor, m.photos_extra_mb) } else { 0 };
    let search_need = need(z.search, m.search_factor, m.search_extra_mb);
    let chat_need = need(z.chat, m.chat_factor, m.chat_extra_mb);
    let mut budget = free.saturating_sub(m.keep_free_mb * MB + speech + photos);
    let fit = |budget: u64, need: u64| if need == 0 { 100 } else { ((budget as f64 / need as f64 * 100.0).clamp(0.0, 100.0) / 5.0).floor() as u8 * 5 };
    let search = s.search.unwrap_or_else(|| fit(budget, search_need));
    budget = budget.saturating_sub(search_need * search as u64 / 100);
    let chat = s.chat.unwrap_or_else(|| fit(budget, chat_need));
    Split { chat, search }
}

/// Auto mode: the speech and photo models go on the card when they fit (speech first, it is used the most),
/// then the search and chat models take what is left. Returns the filled-in setting.
pub fn auto_all(free: u64, z: &Sizes) -> Setting {
    let m = crate::catalog::get().memory;
    let mut left = free.saturating_sub(m.keep_free_mb * MB);
    let mut take = |n: u64| if n > 0 && n <= left { left -= n; true } else { false };
    let speech_gpu = take(need(z.speech, m.speech_factor, m.speech_extra_mb));
    let photos_gpu = take(need(z.photos, m.photos_factor, m.photos_extra_mb));
    let mut s = Setting { speech_gpu: Some(speech_gpu), photos_gpu: Some(photos_gpu), ..Setting::plain(RunOn::Mixed) };
    let sp = auto_split(free, &s, z);
    s.chat = Some(sp.chat);
    s.search = Some(sp.search);
    s
}

/// Free graphics memory in bytes, from `nvidia-smi`. None without an NVIDIA card or when it cannot be read.
pub fn free_vram() -> Option<u64> {
    let mut cmd = std::process::Command::new("nvidia-smi");
    cmd.args(["--query-gpu=memory.used,memory.total", "--format=csv,noheader,nounits"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok().filter(|o| o.status.success())?;
    let line = String::from_utf8_lossy(&out.stdout);
    let mut it = line.lines().next()?.split(',').map(|v| v.trim().parse::<u64>().ok());
    let (used, total) = (it.next()??, it.next()??);
    Some(total.saturating_sub(used) * MB)
}

/// The split the node would pick for mixed mode on this machine, with the reason, for `run-on` and the log.
pub fn suggest(free: Option<u64>, s: &Setting, z: &Sizes) -> (Split, String) {
    let Some(f) = free else {
        let sp = Split { chat: s.chat.unwrap_or(50), search: s.search.unwrap_or(100) };
        return (sp, format!("chat {} percent and search {} percent on the graphics card (the free graphics memory could not be read)", sp.chat, sp.search));
    };
    let sp = auto_split(f, s, z);
    let free_gb = format!("{:.1} GB of graphics memory is free", f as f64 / (1024.0 * MB as f64));
    let advice = if sp.chat == 100 && sp.search == 100 && s.speech_gpu.unwrap_or(true) && s.photos_gpu.unwrap_or(true) {
        "everything fits on the graphics card, so gpu would be the same and a little faster".to_string()
    } else {
        format!("chat {} percent and search {} percent on the graphics card", sp.chat, sp.search)
    };
    (sp, format!("{advice} ({free_gb})"))
}

#[cfg(test)]
mod tests {
    use super::*;
    const GB: u64 = 1024 * MB;

    #[test]
    fn lines_round_trip() {
        for l in ["auto", "gpu", "cpu", "mixed", "mixed chat=60", "mixed chat=40 search=100 speech=gpu photos=cpu", "mixed speech=cpu", "gpu speakers=gpu", "auto speakers=cpu"] {
            assert_eq!(Setting::parse(l).unwrap().line(), l);
        }
        assert_eq!(Setting::parse("mixed chat=250").unwrap().chat, Some(100));
        assert_eq!(Setting::parse("mixed search=cpu").unwrap().search, Some(0));
        assert!(Setting::parse("fast").is_none());
        assert!(Setting::parse("mixed chat=lots").is_none());
        assert!(Setting::parse("mixed colour=red").is_none());
    }

    fn sizes() -> Sizes {
        Sizes { chat: Some(3 * GB), search: Some(300 * MB), speech: Some(600 * MB), photos: Some(500 * MB) }
    }

    #[test]
    fn auto_split_follows_free_memory() {
        let auto = Setting::plain(RunOn::Mixed);
        assert_eq!(auto_split(16 * GB, &auto, &sizes()), Split { chat: 100, search: 100 });
        assert_eq!(auto_split(1 * GB, &auto, &sizes()).chat, 0);
        let mid = auto_split(5 * GB, &auto, &sizes());
        assert!(mid.chat > 0 && mid.chat < 100 && mid.chat % 5 == 0 && mid.search == 100, "{mid:?}");
        // Moving speech and photos to the processor leaves more room for the chat model.
        let freed = Setting { speech_gpu: Some(false), photos_gpu: Some(false), ..auto };
        assert!(auto_split(5 * GB, &freed, &sizes()).chat > mid.chat);
        // A share set by hand is kept, and the search model on the processor frees room for chat.
        let hand = Setting { chat: Some(30), ..auto };
        assert_eq!(auto_split(5 * GB, &hand, &sizes()).chat, 30);
        let no_search = Setting { search: Some(0), ..auto };
        assert!(auto_split(5 * GB, &no_search, &sizes()).chat >= mid.chat);
    }

    #[test]
    fn auto_puts_what_fits_on_the_card() {
        let all = auto_all(16 * GB, &sizes());
        assert_eq!((all.chat, all.search, all.speech_gpu, all.photos_gpu), (Some(100), Some(100), Some(true), Some(true)));
        let none = auto_all(512 * MB, &sizes());
        assert_eq!((none.chat, none.search, none.speech_gpu, none.photos_gpu), (Some(0), Some(0), Some(false), Some(false)));
        let some = auto_all(4 * GB, &sizes());
        assert_eq!(some.speech_gpu, Some(true));
        assert!(some.chat.unwrap() < 100, "{some:?}");
    }

    #[test]
    fn wording_is_plain() {
        let auto = Setting::plain(RunOn::Mixed);
        for f in [Some(16 * GB), Some(5 * GB), Some(GB), None] {
            let (_, t) = suggest(f, &auto, &sizes());
            assert!(!t.contains(';') && !t.contains('\u{2014}'), "{t}");
        }
        let w = Setting::parse("mixed chat=60 search=0 photos=cpu").unwrap().words();
        assert!(!w.contains(';') && w.contains("chat 60 percent") && w.contains("search on the processor") && w.contains("photos on the processor"), "{w}");
    }
}
