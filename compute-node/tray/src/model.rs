//! Everything the tray knows about the node, and the text it shows. No icons and no windows in here,
//! so it works the same on every system and can be tested without a screen.
//!
//! The node only ever reports counts, timings and names. Nothing from a job is available to show.

use serde_json::Value;

/// A click on a menu entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Pause,
    Resume,
    Lock,
    Unlock,
    /// Opens the details panel.
    ShowDetails,
    Stop,
    Start,
    /// Opens the details panel on its Settings page (chat model, where the models run, logs).
    ShowSettings,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Ready for jobs.
    Running,
    /// Working on a job right now.
    Busy,
    /// Paused by hand: new jobs are turned away.
    Paused,
    /// New phones must be allowed first.
    Locked,
    /// Not running, or not answering.
    Down,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Phone {
    /// The first 16 characters of the phone's fingerprint. What `block` and `revoke` take.
    pub id: String,
    pub name: String,
    pub chat_jobs: u64,
    pub search_jobs: u64,
    /// Seconds since 1970 when the phone last did a job, if it has.
    pub last_seen: Option<u64>,
}

/// Another compute node this node works together with (joined with `marginalia-node join`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Peer {
    pub name: String,
    pub addr: String,
    /// Answered the last time this node looked.
    pub reachable: bool,
    pub paused: bool,
    /// Jobs running or waiting on it.
    pub load: u64,
}

/// A compute node seen on the network that is not a peer yet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nearby {
    pub name: String,
    pub addr: Option<String>,
}

/// A phone that asked to join and has not been answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Waiting {
    pub id: String,
    pub name: String,
    pub waited_s: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub name: String,
    pub paused: bool,
    pub locked: bool,
    pub running_jobs: u64,
    pub waiting_jobs: u64,
    pub served: u64,
    pub uptime_s: u64,
    pub chat_model: Option<String>,
    pub search_model: Option<String>,
    pub chat_tok_s: f64,
    pub search_per_s: f64,
    /// The speech model, if this node transcribes recordings, and how many times faster than real time it is.
    pub speech_model: Option<String>,
    pub speech_x: f64,
    /// The vision projector, if this node reads photos for phones.
    pub vision_model: Option<String>,
    /// This node can tell speakers apart for phones that ask (the helper program and the speaker models are there).
    pub speakers: bool,
    pub chat_jobs: u64,
    pub search_jobs: u64,
    pub failed: u64,
    pub phones: Vec<Phone>,
    pub asking: Vec<Waiting>,
    pub peers: Vec<Peer>,
    pub nearby: Vec<Nearby>,
    pub addresses: Vec<String>,
    /// Where the node keeps its log files.
    pub log_dir: Option<String>,
    /// Where the models run: `auto`, `gpu`, `cpu` or `mixed`, the saved setting line, and the line the panel shows for it.
    pub run_mode: Option<String>,
    pub run_setting: Option<String>,
    pub runs_on: Option<String>,
    /// Mixed only: where each model ended up, for the second line of the panel row.
    pub runs_on_detail: Option<String>,
    /// Where the models are now, as By hand starts from it: percent of the chat and search models on the card,
    /// and whether speech and photos are on the card.
    pub now_chat: Option<u8>,
    pub now_search: Option<u8>,
    pub now_speech_gpu: Option<bool>,
    pub now_photos_gpu: Option<bool>,
    /// The node's state and models folders, for saving settings and downloading models.
    pub state_dir: Option<String>,
    pub models_dir: Option<String>,
}

/// What the last poll found.
#[derive(Clone, Debug, PartialEq)]
pub enum Poll {
    Down,
    Up(Box<Info>),
}

/// What the interface shows: the poll plus whether we just asked the node to start.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub poll: Poll,
    pub starting: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A line that is shown but cannot be clicked.
    Info(String),
    Separator,
    Item(String, Action),
}

impl View {
    pub fn state(&self) -> State {
        match &self.poll {
            Poll::Down => State::Down,
            Poll::Up(i) if i.paused => State::Paused,
            Poll::Up(i) if i.running_jobs > 0 => State::Busy,
            Poll::Up(i) if i.locked => State::Locked,
            Poll::Up(_) => State::Running,
        }
    }
}

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(String::from)
}

/// Builds the node's picture from the answers to `status` and (optionally) `stats`.
pub fn parse(status: &Value, stats: Option<&Value>) -> Info {
    let jobs = |kind: &str, field: &str| stats.map(|s| n(&s[kind][field])).unwrap_or(0);
    let per_phone = |id: &str| -> (u64, u64, Option<u64>) {
        stats
            .and_then(|s| s["phones"].as_array())
            .and_then(|a| a.iter().find(|p| p["id"].as_str() == Some(id)))
            .map(|p| (n(&p["chat_jobs"]), n(&p["search_jobs"]), p["last_seen"].as_u64().filter(|t| *t > 0)))
            .unwrap_or((0, 0, None))
    };
    let phones = status["clients"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let id = c["fp"].as_str().unwrap_or("").to_string();
            let (chat_jobs, search_jobs, last_seen) = per_phone(&id);
            Phone { id, name: c["name"].as_str().unwrap_or("A phone").to_string(), chat_jobs, search_jobs, last_seen }
        })
        .collect();
    let asking = status["waiting"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|w| Waiting { id: w["id"].as_str().unwrap_or("").to_string(), name: w["name"].as_str().unwrap_or("A phone").to_string(), waited_s: n(&w["waited_s"]) })
        .collect();
    let peers = status["peers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| Peer {
            name: text(&p["name"]).unwrap_or_else(|| "A compute node".into()),
            addr: p["addr"].as_str().unwrap_or("").to_string(),
            reachable: p["reachable"].as_bool().unwrap_or(false),
            paused: p["paused"].as_bool().unwrap_or(false),
            load: n(&p["load"]),
        })
        .collect();
    let nearby = status["nearby"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| Nearby { name: text(&p["name"]).unwrap_or_else(|| "A compute node".into()), addr: text(&p["addr"]) })
        .collect();
    Info {
        name: text(&status["name"]).unwrap_or_else(|| "node".into()),
        paused: status["paused"].as_bool().unwrap_or(false),
        locked: status["locked"].as_bool().unwrap_or(false),
        running_jobs: n(&status["running_jobs"]),
        waiting_jobs: n(&status["waiting_jobs"]),
        served: n(&status["served"]),
        uptime_s: n(&status["uptime_s"]),
        chat_model: text(&status["chat"]),
        search_model: text(&status["embed"]),
        chat_tok_s: status["chat_tok_s"].as_f64().unwrap_or(0.0),
        search_per_s: status["embed_per_s"].as_f64().unwrap_or(0.0),
        speech_model: text(&status["speech"]),
        speech_x: status["speech_x"].as_f64().unwrap_or(0.0),
        speakers: status["speakers"].as_bool() == Some(true),
        run_mode: text(&status["placement"]["mode"]),
        run_setting: text(&status["placement"]["setting"]),
        runs_on: runs_on_text(&status["placement"]),
        runs_on_detail: runs_on_detail(&status["placement"]),
        now_chat: share_now(&status["placement"]["chat"]),
        now_search: share_now(&status["placement"]["search"]),
        now_speech_gpu: status["placement"]["speech"]["on"].as_str().map(|o| o == "gpu"),
        now_photos_gpu: status["placement"]["photos"]["on"].as_str().map(|o| o == "gpu"),
        state_dir: text(&status["state_dir"]),
        models_dir: text(&status["models_dir"]),
        vision_model: if status["vision"].as_bool() == Some(true) { Some(text(&status["vision_model"]).unwrap_or_else(|| "vision".into())) } else { None },
        chat_jobs: jobs("chat", "jobs"),
        search_jobs: jobs("search", "jobs"),
        failed: jobs("chat", "errors") + jobs("search", "errors") + jobs("speech", "errors"),
        phones,
        asking,
        peers,
        nearby,
        addresses: status["addresses"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(String::from)).collect(),
        log_dir: text(&status["log_dir"]),
    }
}

fn plural(count: u64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The Runs on line of the panel: where the models run.
pub fn runs_on_text(p: &Value) -> Option<String> {
    Some(match p["mode"].as_str()? {
        "auto" => "Auto".into(),
        "gpu" => "Graphics card".into(),
        "cpu" => "Processor".into(),
        "mixed" if p["setting"].as_str().map(|s| s.trim() == "mixed").unwrap_or(true) => "Auto".into(),
        "mixed" => "Custom".into(),
        other => other.to_string(),
    })
}

/// The share of a model on the graphics card now, in steps of 5: 100 on the card, 0 on the processor.
pub fn share_now(m: &Value) -> Option<u8> {
    Some(match m["on"].as_str()? {
        "gpu" => 100,
        "cpu" => 0,
        "mixed" => {
            let p = m["gpu_layers"].as_u64()? * 100 / m["layers"].as_u64()?.max(1);
            ((p + 2) / 5 * 5).min(100) as u8
        }
        _ => return None,
    })
}

/// Mixed only: one short part per model, for example "chat 16 of 35 layers on the card, search card, speech card".
pub fn runs_on_detail(p: &Value) -> Option<String> {
    if !matches!(p["mode"].as_str()?, "mixed" | "auto") {
        return None;
    }
    let one = |name: &str| -> Option<String> {
        let m = &p[name];
        Some(match m["on"].as_str()? {
            "gpu" => format!("{name} card"),
            "cpu" => format!("{name} processor"),
            "mixed" => format!("{name} {} of {} layers on the card", m["gpu_layers"].as_u64()?, m["layers"].as_u64()?),
            _ => return None,
        })
    };
    let parts: Vec<String> = ["chat", "search", "speech", "photos"].iter().filter_map(|n| one(n)).collect();
    if parts.is_empty() { None } else { Some(parts.join(", ")) }
}

/// The Photos line of the panel: whether this node reads photos for phones.
pub fn photos_text(i: &Info) -> String {
    if i.vision_model.is_some() {
        "Reads photos for phones".into()
    } else {
        "Not set up, the vision file is missing".into()
    }
}

/// The Speakers line of the panel: whether this node tells speakers apart. It is opt-in, so "not set up" is a normal state.
pub fn speakers_text(i: &Info) -> String {
    if i.speakers {
        "Tells speakers apart for phones".into()
    } else {
        "Off, the speaker models are not downloaded".into()
    }
}

/// What a peer is doing, for the panel: "Not answering", "Paused", "Idle" or "Working on 2 jobs".
pub fn peer_line(p: &Peer) -> String {
    if !p.reachable {
        "Not answering".into()
    } else if p.paused {
        "Paused".into()
    } else if p.load == 0 {
        "Idle".into()
    } else {
        format!("Working on {}", plural(p.load, "job", "jobs"))
    }
}

/// "5 chat, 3 search, seen 3 min ago" for one phone in the panel. `now` is seconds since 1970.
pub fn phone_line(p: &Phone, now: u64) -> String {
    let seen = match p.last_seen {
        Some(t) => format!("seen {}", ago_text(now.saturating_sub(t))),
        None => "not used yet".into(),
    };
    format!("{} chat, {} search, {seen}", p.chat_jobs, p.search_jobs)
}

/// The line under the node's name in the panel: what the pause and lock settings mean right now.
pub fn status_line(i: &Info) -> String {
    match (i.paused, i.locked) {
        (true, true) => "New jobs are turned away, new phones must be allowed first".into(),
        (true, false) => "New jobs are turned away".into(),
        (false, true) => "New phones must be allowed first".into(),
        (false, false) => "Ready for jobs, new phones can join".into(),
    }
}

/// "Idle", "Working on 1 job" or "Working on 2 jobs, 1 waiting".
pub fn work_label(running: u64, waiting: u64) -> String {
    match (running, waiting) {
        (0, 0) => "Idle".into(),
        (r, 0) => format!("Working on {}", plural(r.max(1), "job", "jobs")),
        (r, w) => format!("Working on {}, {w} waiting", plural(r.max(1), "job", "jobs")),
    }
}

/// The measured speeds as short text. A speed of zero was never measured.
pub fn chat_speed_text(tok_s: f64) -> String {
    if tok_s > 0.0 { format!("{tok_s:.0} tokens/s") } else { "not measured".into() }
}

pub fn search_speed_text(per_s: f64) -> String {
    if per_s > 0.0 { format!("{per_s:.0} passages/s") } else { "not measured".into() }
}

/// How many times faster than the recording itself a node transcribes. Zero means no recording was transcribed yet.
pub fn speech_speed_text(x: f64) -> String {
    if x <= 0.0 {
        return "not measured".into();
    }
    if x < 1.0 {
        return "slower than the recording".into();
    }
    // "1 h in 1 min" (an hour of audio takes a minute) says more than "60x real time" and fits the panel row.
    let secs = 3600.0 / x;
    if secs < 60.0 { format!("1 h in {secs:.0} s") } else { format!("1 h in {:.0} min", secs / 60.0) }
}

#[cfg(test)]
mod speech_text_tests {
    use super::speech_speed_text;

    #[test]
    fn speech_speed_is_said_in_plain_terms() {
        assert_eq!(speech_speed_text(0.0), "not measured");
        assert_eq!(speech_speed_text(0.5), "slower than the recording");
        assert_eq!(speech_speed_text(60.0), "1 h in 1 min");
        assert_eq!(speech_speed_text(120.0), "1 h in 30 s");
    }
}

/// Short form for the big number in the panel: "45s", "12m", "1h 6m", "2d 5h".
pub fn uptime_short(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        s => format!("{}d {}h", s / 86400, s % 86400 / 3600),
    }
}

/// "just now", "5 min ago", "2 h ago", "3 d ago".
pub fn ago_text(secs: u64) -> String {
    match secs {
        s if s < 45 => "just now".into(),
        s if s < 3600 => format!("{} min ago", (s / 60).max(1)),
        s if s < 86400 => format!("{} h ago", s / 3600),
        s => format!("{} d ago", s / 86400),
    }
}

/// How full a speed bar is, from 0 to 1.
pub fn gauge(value: f64, max: f64) -> f32 {
    if max <= 0.0 || !value.is_finite() {
        return 0.0;
    }
    (value / max).clamp(0.0, 1.0) as f32
}

/// Sensible tops for the two speed bars. Faster than this just fills the bar.
pub const CHAT_TOK_S_MAX: f64 = 150.0;
pub const SEARCH_PER_S_MAX: f64 = 500.0;
pub const SPEECH_X_MAX: f64 = 200.0;

/// "4:32" for a countdown.
pub fn countdown(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// How much the activity strip remembers.
pub const ACTIVITY_WINDOW_S: f64 = 120.0;

/// The last two minutes of how busy the node was, kept in memory only. One sample per poll.
#[derive(Default)]
pub struct Activity {
    /// (seconds on any steady clock, jobs in flight, jobs served so far)
    samples: std::collections::VecDeque<(f64, u32, u64)>,
}

impl Activity {
    pub fn push(&mut self, t: f64, in_flight: u32, served: u64) {
        // A smaller counter means the node restarted, so the old history no longer belongs to it.
        if self.samples.back().map(|s| served < s.2).unwrap_or(false) {
            self.samples.clear();
        }
        self.samples.push_back((t, in_flight, served));
        while self.samples.front().map(|s| t - s.0 > ACTIVITY_WINDOW_S).unwrap_or(false) {
            self.samples.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Jobs finished within the window.
    pub fn finished(&self) -> u64 {
        match (self.samples.front(), self.samples.back()) {
            (Some(a), Some(b)) => b.2.saturating_sub(a.2),
            _ => 0,
        }
    }

    /// `columns` bars, oldest first, each 0 to 1: the busiest moment in that slice of the window,
    /// against the busiest moment overall (never less than one job). The newest slice is last.
    pub fn bars(&self, columns: usize, now: f64) -> Vec<f32> {
        let mut peak = vec![0u32; columns];
        for &(t, in_flight, _) in &self.samples {
            let age = (now - t).clamp(0.0, ACTIVITY_WINDOW_S - 1e-6);
            let col = columns - 1 - (age / ACTIVITY_WINDOW_S * columns as f64) as usize;
            peak[col] = peak[col].max(in_flight);
        }
        let top = peak.iter().copied().max().unwrap_or(0).max(1) as f32;
        peak.into_iter().map(|p| p as f32 / top).collect()
    }
}

fn state_word(i: &Info) -> &'static str {
    match (View { poll: Poll::Up(Box::new(i.clone())), starting: false }).state() {
        State::Paused => "paused",
        State::Busy => "busy",
        State::Locked => "running, locked",
        _ => "running",
    }
}

/// The hover text, for example "falcon: running, 12 jobs served, 2 phones".
pub fn tooltip(v: &View) -> String {
    match &v.poll {
        Poll::Down if v.starting => "Marginalia node: starting".into(),
        Poll::Down => "Marginalia node: not running".into(),
        Poll::Up(i) => {
            let peers = if i.peers.is_empty() { String::new() } else { format!(", {}", plural(i.peers.len() as u64, "peer", "peers")) };
            format!("{}: {}, {} served, {}{peers}", i.name, state_word(i), plural(i.served, "job", "jobs"), plural(i.phones.len() as u64, "phone", "phones"))
        }
    }
}

/// The right-click menu. It is short on purpose: the details live in the panel that opens on a click.
pub fn menu(v: &View) -> Vec<Entry> {
    use Entry::*;
    let mut m = vec![];
    match &v.poll {
        Poll::Down => {
            m.push(Info(if v.starting { "Marginalia node: starting, this can take a minute".into() } else { "Marginalia node: not running".into() }));
            m.push(Separator);
            if v.starting {
                m.push(Info("Starting node...".into()));
            } else {
                m.push(Item("Start node".into(), Action::Start));
            }
        }
        Poll::Up(i) => {
            m.push(Info(format!("{}: {}", i.name, state_word(i))));
            m.push(Separator);
            m.push(if i.paused { Item("Resume".into(), Action::Resume) } else { Item("Pause".into(), Action::Pause) });
            m.push(Item("Show details".into(), Action::ShowDetails));
            m.push(Separator);
            m.push(Item("Stop node".into(), Action::Stop));
        }
    }
    m.push(Item("Quit tray".into(), Action::Quit));
    m
}

/// The right-click menu as the icons show it: `menu` plus Settings under Show details while the node runs.
pub fn menu_with_settings(v: &View) -> Vec<Entry> {
    let mut m = menu(v);
    if let (Poll::Up(_), Some(at)) = (&v.poll, m.iter().position(|e| matches!(e, Entry::Item(_, Action::ShowDetails)))) {
        m.insert(at + 1, Entry::Item("Settings".into(), Action::ShowSettings));
    }
    m
}

/// Where the models run, as the Settings page edits it. Turns into `marginalia-node run-on ...` arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct RunDraft {
    /// `auto`, `gpu`, `cpu` or `custom` (every model set here).
    pub mode: String,
    pub chat: u8,
    pub search: u8,
    pub speech_gpu: bool,
    pub photos_gpu: bool,
    pub speakers_gpu: bool,
}

impl RunDraft {
    /// From the line the node saved, for example `mixed chat=60 search=0 speech=cpu`. A mixed line with shares
    /// given is Custom. A mixed line without them lets the node split, which is what Auto does, so it shows as Auto.
    pub fn from_setting(line: Option<&str>) -> RunDraft {
        let mut d = RunDraft { mode: "gpu".into(), chat: 50, search: 100, speech_gpu: true, photos_gpu: true, speakers_gpu: false };
        let Some(line) = line else { return d };
        let mut parts = line.split_whitespace();
        if let Some(m) = parts.next() {
            d.mode = m.to_string();
        }
        let mut given = 0;
        for p in parts {
            let Some((k, v)) = p.split_once('=') else { continue };
            let gpu = v == "gpu";
            match k {
                "chat" => { d.chat = v.parse().unwrap_or(d.chat); given += 1 }
                "search" => { d.search = v.parse().unwrap_or(d.search); given += 1 }
                "speech" => { d.speech_gpu = gpu; given += 1 }
                "photos" => { d.photos_gpu = gpu; given += 1 }
                "speakers" => d.speakers_gpu = gpu,
                _ => {}
            }
        }
        if d.mode == "mixed" {
            d.mode = if given > 0 { "custom" } else { "auto" }.into();
        }
        d
    }

    /// The arguments for `marginalia-node`.
    pub fn args(&self) -> Vec<String> {
        let gc = |g: bool| if g { "gpu" } else { "cpu" }.to_string();
        let mut a: Vec<String> = vec!["run-on".into()];
        match self.mode.as_str() {
            "custom" => {
                a.push("mixed".into());
                a.extend(["--chat".into(), self.chat.to_string(), "--search".into(), self.search.to_string(), "--speech".into(), gc(self.speech_gpu), "--photos".into(), gc(self.photos_gpu)]);
            }
            m => a.push(m.to_string()),
        }
        a.extend(["--speakers".into(), gc(self.speakers_gpu)]);
        a
    }

    /// The four choices the Settings page offers: the value, its name and one line on what it does.
    pub const MODES: [(&'static str, &'static str, &'static str); 4] = [
        ("auto", "Auto", "Fits the models to free card memory"),
        ("gpu", "Graphics card", "All on the card. Fastest"),
        ("cpu", "Processor", "All on the processor. Card stays free"),
        ("custom", "Custom", "Choose for each model"),
    ];

}

/// One downloadable item on the Settings page. Speakers is two files shown as one.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelItem {
    pub label: String,
    /// Its size on disk when it is there, else 0.
    pub bytes: u64,
    /// All of its files are on disk.
    pub ready: bool,
    /// The `marginalia-node models download` arguments that fetch it (without the folder).
    pub args: Vec<String>,
    /// The node loads it only when it starts, so it needs a restart after a download.
    pub needs_restart: bool,
}

/// The items for the Settings page from `marginalia-node models list --json`.
pub fn model_items(list: &Value) -> Vec<ModelItem> {
    let models = list["models"].as_array().cloned().unwrap_or_default();
    let find = |id: &str| models.iter().find(|m| m["id"].as_str() == Some(id));
    let one = |ids: &[&str], label: &str, args: &[&str], restart: bool| -> Option<ModelItem> {
        let ms: Vec<&Value> = ids.iter().filter_map(|i| find(i)).collect();
        if ms.is_empty() {
            return None;
        }
        Some(ModelItem {
            label: label.into(),
            bytes: ms.iter().map(|m| m["present"].as_u64().unwrap_or(0)).sum(),
            ready: ms.iter().all(|m| !m["present"].is_null()),
            args: ["models", "download"].iter().chain(args).map(|a| a.to_string()).collect(),
            needs_restart: restart,
        })
    };
    [
        one(&["chat-e2b"], "Chat E2B", &["--only", "chat", "--chat-model", "e2b"], true),
        one(&["chat-e4b"], "Chat E4B", &["--only", "chat", "--chat-model", "e4b"], true),
        one(&["search"], "Search", &["--only", "search"], true),
        one(&["speech"], "Speech", &["--only", "speech"], true),
        one(&["vision-e2b"], "Photos (E2B)", &["--only", "vision", "--chat-model", "e2b"], true),
        one(&["vision-e4b"], "Photos (E4B)", &["--only", "vision", "--chat-model", "e4b"], true),
        one(&["speakers-segmentation", "speakers-voices"], "Speakers", &["--only", "speakers"], false),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// What a running or finished download says in its progress file.
#[derive(Clone, Debug, PartialEq)]
pub enum Download {
    Running { file: String, done: u64, total: u64 },
    Done,
    Failed(String),
}

/// Reads the progress file. A "downloading" line that has not changed for `stale_s` seconds belongs to a download that died.
pub fn download_state(v: &Value, age_s: u64, stale_s: u64) -> Option<Download> {
    match v["state"].as_str()? {
        "downloading" if age_s <= stale_s => Some(Download::Running { file: v["file"].as_str().unwrap_or("").into(), done: v["done"].as_u64().unwrap_or(0), total: v["total"].as_u64().unwrap_or(0) }),
        "downloading" => Some(Download::Failed("The download stopped. Try again, it continues where it stopped.".into())),
        "done" => Some(Download::Done),
        "failed" => Some(Download::Failed(v["error"].as_str().unwrap_or("The download failed").chars().take(160).collect())),
        _ => None,
    }
}

/// "2.8 GB" or "560 MB".
pub fn size_text(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1.0 { format!("{gb:.1} GB") } else { format!("{} MB", (bytes as f64 / 1e6).round() as u64) }
}

/// The chat model the node runs now, `e2b` or `e4b`.
pub fn chat_choice(i: &Info) -> Option<&'static str> {
    match i.chat_model.as_deref() {
        Some(c) if c.contains("E4B") => Some("e4b"),
        Some(c) if c.contains("E2B") => Some("e2b"),
        _ => None,
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    fn view(chat: Option<&str>) -> View {
        let i = Info { name: "falcon".into(), chat_model: chat.map(String::from), ..Default::default() };
        View { poll: Poll::Up(Box::new(i)), starting: false }
    }

    #[test]
    fn menu_has_settings_after_show_details_only_while_running() {
        let m = menu_with_settings(&view(Some("gemma-4-E2B-it-Q4_0")));
        let at = m.iter().position(|e| matches!(e, Entry::Item(_, Action::ShowDetails))).unwrap();
        assert_eq!(m[at + 1], Entry::Item("Settings".into(), Action::ShowSettings));
        let down = View { poll: Poll::Down, starting: false };
        assert_eq!(menu_with_settings(&down), menu(&down));
    }

    #[test]
    fn drafts_read_the_saved_line_and_write_arguments() {
        let d = RunDraft::from_setting(Some("mixed chat=60 search=0 speech=cpu photos=gpu speakers=gpu"));
        assert_eq!((d.mode.as_str(), d.chat, d.search, d.speech_gpu, d.photos_gpu, d.speakers_gpu), ("custom", 60, 0, false, true, true));
        assert_eq!(d.args().join(" "), "run-on mixed --chat 60 --search 0 --speech cpu --photos gpu --speakers gpu");
        let auto = RunDraft::from_setting(Some("auto"));
        assert_eq!(auto.mode, "auto");
        assert_eq!(auto.args().join(" "), "run-on auto --speakers cpu");
        assert_eq!(RunDraft::from_setting(Some("mixed")).mode, "auto");
        assert_eq!(RunDraft::from_setting(None).mode, "gpu");
    }

    #[test]
    fn chat_choice_and_wording() {
        let up = |c: &str| Info { chat_model: Some(c.into()), ..Default::default() };
        assert_eq!(chat_choice(&up("gemma-4-E4B-it-Q4_0")), Some("e4b"));
        assert_eq!(chat_choice(&up("gemma-4-E2B-it-Q4_0")), Some("e2b"));
        for (_, name, line) in RunDraft::MODES {
            for t in [name, line] {
                assert!(!t.contains(';') && !t.contains('\u{2014}'), "{t}");
            }
        }
    }

    #[test]
    fn runs_on_lines() {
        let p = serde_json::json!({"mode": "mixed", "setting": "mixed", "chat": {"on": "mixed", "gpu_layers": 16, "layers": 35}, "search": {"on": "gpu"}, "speech": {"on": "cpu"}, "photos": null});
        assert_eq!(runs_on_text(&p).as_deref(), Some("Auto"));
        assert_eq!(runs_on_detail(&p).as_deref(), Some("chat 16 of 35 layers on the card, search card, speech processor"));
        let hand = serde_json::json!({"mode": "mixed", "setting": "mixed chat=30"});
        assert_eq!(runs_on_text(&hand).as_deref(), Some("Custom"));
        assert_eq!(runs_on_text(&serde_json::json!({"mode": "cpu"})).as_deref(), Some("Processor"));
        assert_eq!(runs_on_detail(&serde_json::json!({"mode": "gpu"})), None);
        assert_eq!(runs_on_text(&Value::Null), None);
        assert_eq!(share_now(&serde_json::json!({"on": "mixed", "gpu_layers": 14, "layers": 35})), Some(40));
        assert_eq!(share_now(&serde_json::json!({"on": "cpu"})), Some(0));
        assert_eq!(share_now(&Value::Null), None);
    }

    #[test]
    fn model_items_group_files_and_carry_download_arguments() {
        let list = serde_json::json!({"models": [
            {"id": "chat-e2b", "present": 2841481184u64},
            {"id": "chat-e4b", "present": null},
            {"id": "speakers-segmentation", "present": 5992913},
            {"id": "speakers-voices", "present": null}
        ]});
        let items = model_items(&list);
        assert_eq!(items.len(), 3);
        assert!(items[0].ready && !items[1].ready);
        assert_eq!(items[1].args.join(" "), "models download --only chat --chat-model e4b");
        let sp = items.iter().find(|i| i.label == "Speakers").unwrap();
        assert!(!sp.ready && !sp.needs_restart && sp.bytes == 5992913);
        assert_eq!(size_text(2841481184), "2.8 GB");
        assert_eq!(size_text(32523463), "33 MB");
    }

    #[test]
    fn download_progress_and_stale_downloads() {
        let running = serde_json::json!({"state": "downloading", "file": "a.gguf", "done": 5, "total": 10});
        assert_eq!(download_state(&running, 2, 90), Some(Download::Running { file: "a.gguf".into(), done: 5, total: 10 }));
        assert!(matches!(download_state(&running, 200, 90), Some(Download::Failed(_))));
        assert_eq!(download_state(&serde_json::json!({"state": "done"}), 0, 90), Some(Download::Done));
        assert_eq!(download_state(&Value::Null, 0, 90), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status() -> Value {
        json!({
            "name": "falcon", "paused": false, "locked": false, "running_jobs": 1, "waiting_jobs": 2, "served": 12, "uptime_s": 7500, "phones": 2,
            "chat": "gemma-4-E2B-it-Q4_0", "embed": "embeddinggemma-2-270m", "chat_tok_s": 41.4, "embed_per_s": 250.2,
            "addresses": ["192.168.1.20:47615", "10.0.0.5:47615"],
            "waiting": [{"id": "ffffffffffffffff", "name": "New phone", "waited_s": 3}],
            "clients": [{"name": "Pixel", "fp": "aaaaaaaaaaaaaaaa", "paired_at": 1}, {"name": "Tab_2", "fp": "bbbbbbbbbbbbbbbb", "paired_at": 2}],
        })
    }

    fn stats() -> Value {
        json!({
            "name": "falcon", "report": "falcon up for 2 h\n",
            "chat": {"jobs": 8, "errors": 1}, "search": {"jobs": 4, "errors": 0},
            "phones": [{"name": "Pixel", "id": "aaaaaaaaaaaaaaaa", "chat_jobs": 5, "search_jobs": 3, "last_seen": 1_700_000_000}],
        })
    }

    fn up() -> View {
        View { poll: Poll::Up(Box::new(parse(&status(), Some(&stats())))), starting: false }
    }

    fn lines(m: &[Entry]) -> Vec<String> {
        m.iter()
            .map(|e| match e {
                Entry::Info(s) => format!("- {s}"),
                Entry::Separator => "---".into(),
                Entry::Item(s, _) => format!("> {s}"),
            })
            .collect()
    }

    #[test]
    fn parses_status_and_stats() {
        let i = parse(&status(), Some(&stats()));
        assert_eq!((i.name.as_str(), i.served, i.running_jobs, i.waiting_jobs), ("falcon", 12, 1, 2));
        assert_eq!((i.chat_jobs, i.search_jobs, i.failed), (8, 4, 1));
        assert_eq!(
            i.phones,
            vec![
                Phone { id: "aaaaaaaaaaaaaaaa".into(), name: "Pixel".into(), chat_jobs: 5, search_jobs: 3, last_seen: Some(1_700_000_000) },
                Phone { id: "bbbbbbbbbbbbbbbb".into(), name: "Tab_2".into(), chat_jobs: 0, search_jobs: 0, last_seen: None },
            ]
        );
        assert_eq!(i.asking, vec![Waiting { id: "ffffffffffffffff".into(), name: "New phone".into(), waited_s: 3 }]);
        assert_eq!(i.addresses.len(), 2);
    }

    #[test]
    fn works_without_stats_and_with_an_old_node() {
        let i = parse(&json!({"name": "old"}), None);
        assert_eq!((i.name.as_str(), i.served, i.chat_jobs, i.phones.len(), i.paused), ("old", 0, 0, 0, false));
        assert_eq!(parse(&json!({}), None).name, "node");
    }

    #[test]
    fn state_mapping() {
        // The fixture has a job running.
        assert_eq!(up().state(), State::Busy);
        let mut idle = parse(&status(), None);
        idle.running_jobs = 0;
        assert_eq!((View { poll: Poll::Up(Box::new(idle)), starting: false }).state(), State::Running);
        let mut paused = parse(&status(), None);
        paused.paused = true;
        assert_eq!(View { poll: Poll::Up(Box::new(paused)), starting: false }.state(), State::Paused);
        assert_eq!(View { poll: Poll::Down, starting: false }.state(), State::Down);
    }

    #[test]
    fn peers_and_nearby_nodes_are_read_from_the_status() {
        let mut st = status();
        st["peers"] = json!([
            {"name": "Den", "id": "aaaaaaaaaaaaaaaa", "addr": "192.168.1.5:47615", "reachable": true, "load": 2, "paused": false},
            {"name": "Attic", "id": "bbbbbbbbbbbbbbbb", "addr": "192.168.1.6:47615", "reachable": false, "load": null, "paused": null},
            {"name": "Loft", "addr": "192.168.1.8:47615", "reachable": true, "load": 0, "paused": true},
        ]);
        st["nearby"] = json!([{"name": "Kitchen node", "id": "ca2d5dee00112233", "addr": "192.168.1.7:47621", "paused": false}, {"name": "", "addr": null}]);
        let i = parse(&st, None);
        assert_eq!(i.peers.len(), 3);
        assert_eq!(i.peers.iter().map(peer_line).collect::<Vec<_>>(), ["Working on 2 jobs", "Not answering", "Paused"]);
        assert_eq!(model_peer_idle(), "Idle");
        assert_eq!(i.nearby, vec![Nearby { name: "Kitchen node".into(), addr: Some("192.168.1.7:47621".into()) }, Nearby { name: "A compute node".into(), addr: None }]);
        let v = View { poll: Poll::Up(Box::new(i)), starting: false };
        assert_eq!(tooltip(&v), "falcon: busy, 12 jobs served, 2 phones, 3 peers");
        // An old node that says nothing about peers shows none.
        assert!(parse(&status(), None).peers.is_empty() && parse(&status(), None).nearby.is_empty());
    }

    fn model_peer_idle() -> String {
        peer_line(&Peer { name: "x".into(), addr: String::new(), reachable: true, paused: false, load: 0 })
    }

    #[test]
    fn the_vision_model_is_read_from_the_status() {
        let mut st = status();
        assert_eq!(parse(&st, None).vision_model, None);
        assert_eq!(photos_text(&parse(&st, None)), "Not set up, the vision file is missing");
        st["vision"] = json!(true);
        st["vision_model"] = json!("mmproj-gemma-4-E2B-it-Q8_0");
        let i = parse(&st, None);
        assert_eq!(i.vision_model.as_deref(), Some("mmproj-gemma-4-E2B-it-Q8_0"));
        assert_eq!(photos_text(&i), "Reads photos for phones");
        // A node that says it reads photos without naming the file still shows as ready.
        st["vision_model"] = json!(null);
        assert_eq!(parse(&st, None).vision_model.as_deref(), Some("vision"));
    }

    #[test]
    fn the_speakers_setting_is_read_from_the_status() {
        let mut st = status();
        assert!(!parse(&st, None).speakers);
        assert!(speakers_text(&parse(&st, None)).starts_with("Off, the speaker models"));
        st["speakers"] = json!(true);
        assert!(parse(&st, None).speakers);
        assert_eq!(speakers_text(&parse(&st, None)), "Tells speakers apart for phones");
        let t = speakers_text(&parse(&status(), None));
        assert!(!t.contains(';') && !t.contains('—'));
    }

    #[test]
    fn busy_and_locked_have_their_own_state_and_words() {
        let with = |f: &dyn Fn(&mut Info)| {
            let mut i = parse(&status(), None);
            i.paused = false;
            i.locked = false;
            i.running_jobs = 0;
            f(&mut i);
            View { poll: Poll::Up(Box::new(i)), starting: false }
        };
        let busy = with(&|i| i.running_jobs = 1);
        assert_eq!((busy.state(), tooltip(&busy).as_str()), (State::Busy, "falcon: busy, 12 jobs served, 2 phones"));
        let locked = with(&|i| i.locked = true);
        assert_eq!((locked.state(), tooltip(&locked).as_str()), (State::Locked, "falcon: running, locked, 12 jobs served, 2 phones"));
        // Paused wins over busy and locked, because it is the one that turns jobs away.
        let all = with(&|i| { i.paused = true; i.locked = true; i.running_jobs = 2 });
        assert_eq!(all.state(), State::Paused);
        let both = with(&|i| { i.locked = true; i.running_jobs = 1 });
        assert_eq!(both.state(), State::Busy);
    }

    #[test]
    fn tooltip_text() {
        assert_eq!(tooltip(&up()), "falcon: busy, 12 jobs served, 2 phones");
        let one = parse(&json!({"name": "pi", "paused": true, "served": 1, "clients": [{"name": "A", "fp": "x"}]}), None);
        assert_eq!(tooltip(&View { poll: Poll::Up(Box::new(one)), starting: false }), "pi: paused, 1 job served, 1 phone");
        assert_eq!(tooltip(&View { poll: Poll::Down, starting: false }), "Marginalia node: not running");
        assert_eq!(tooltip(&View { poll: Poll::Down, starting: true }), "Marginalia node: starting");
    }

    #[test]
    fn right_click_menu_is_short() {
        assert_eq!(lines(&menu(&up())), ["- falcon: busy", "---", "> Pause", "> Show details", "---", "> Stop node", "> Quit tray"]);
        let mut i = parse(&status(), None);
        i.paused = true;
        let m = lines(&menu(&View { poll: Poll::Up(Box::new(i)), starting: false }));
        assert_eq!(m[0], "- falcon: paused");
        assert_eq!(m[2], "> Resume");
    }

    #[test]
    fn menu_when_the_node_is_not_running() {
        assert_eq!(lines(&menu(&View { poll: Poll::Down, starting: false })), ["- Marginalia node: not running", "---", "> Start node", "> Quit tray"]);
        let m = menu(&View { poll: Poll::Down, starting: true });
        assert!(!m.iter().any(|e| matches!(e, Entry::Item(_, Action::Start))));
        assert!(m.iter().any(|e| matches!(e, Entry::Item(_, Action::Quit))));
    }

    #[test]
    fn plain_wording_without_semicolons_or_em_dashes() {
        let mut texts = lines(&menu(&up()));
        texts.extend(lines(&menu(&View { poll: Poll::Down, starting: true })));
        texts.extend(lines(&menu(&View { poll: Poll::Down, starting: false })));
        texts.push(tooltip(&up()));
        texts.extend([uptime_short(5), uptime_short(4000), ago_text(10), ago_text(4000), countdown(272), work_label(1, 2), status_line(&Info::default()), chat_speed_text(0.0), chat_speed_text(3.0), search_speed_text(3.0)]);
        for t in texts {
            assert!(!t.contains(';') && !t.contains('\u{2014}') && !t.contains('\u{2013}'), "bad wording: {t}");
        }
    }

    #[test]
    fn short_uptime_and_last_seen() {
        assert_eq!(uptime_short(5), "5s");
        assert_eq!(uptime_short(600), "10m");
        assert_eq!(uptime_short(3 * 3600 + 120), "3h 2m");
        assert_eq!(uptime_short(2 * 86400 + 5 * 3600), "2d 5h");
        assert_eq!(ago_text(3), "just now");
        assert_eq!(ago_text(60), "1 min ago");
        assert_eq!(ago_text(7200), "2 h ago");
        assert_eq!(ago_text(3 * 86400), "3 d ago");
    }

    #[test]
    fn panel_texts() {
        let p = Phone { id: "a".into(), name: "Pixel".into(), chat_jobs: 5, search_jobs: 3, last_seen: Some(1000) };
        assert_eq!(phone_line(&p, 1000 + 300), "5 chat, 3 search, seen 5 min ago");
        assert_eq!(phone_line(&Phone { last_seen: None, ..p.clone() }, 5), "5 chat, 3 search, not used yet");
        // A clock a little behind the node's must not underflow.
        assert_eq!(phone_line(&p, 900), "5 chat, 3 search, seen just now");
        let mut i = Info::default();
        assert_eq!(status_line(&i), "Ready for jobs, new phones can join");
        i.locked = true;
        assert_eq!(status_line(&i), "New phones must be allowed first");
        i.paused = true;
        assert_eq!(status_line(&i), "New jobs are turned away, new phones must be allowed first");
        i.locked = false;
        assert_eq!(status_line(&i), "New jobs are turned away");
        assert_eq!(work_label(0, 0), "Idle");
        assert_eq!(work_label(1, 0), "Working on 1 job");
        assert_eq!(work_label(1, 2), "Working on 1 job, 2 waiting");
        assert_eq!(chat_speed_text(41.4), "41 tokens/s");
        assert_eq!(chat_speed_text(0.0), "not measured");
        assert_eq!(search_speed_text(250.2), "250 passages/s");
    }

    #[test]
    fn gauges_and_countdown() {
        assert_eq!(gauge(75.0, 150.0), 0.5);
        assert_eq!(gauge(900.0, 500.0), 1.0);
        assert_eq!(gauge(-3.0, 500.0), 0.0);
        assert_eq!(gauge(f64::NAN, 500.0), 0.0);
        assert_eq!(gauge(5.0, 0.0), 0.0);
        assert_eq!(countdown(300), "5:00");
        assert_eq!(countdown(272), "4:32");
        assert_eq!(countdown(9), "0:09");
    }

    #[test]
    fn activity_keeps_two_minutes_and_counts_finished_jobs() {
        let mut a = Activity::default();
        a.push(0.0, 0, 10);
        a.push(10.0, 2, 10);
        a.push(20.0, 1, 12);
        assert_eq!(a.finished(), 2);
        // Old samples fall out of the window.
        a.push(200.0, 0, 15);
        assert_eq!(a.finished(), 0);
        a.push(210.0, 0, 16);
        assert_eq!(a.finished(), 1);
    }

    #[test]
    fn activity_resets_when_the_node_restarts() {
        let mut a = Activity::default();
        a.push(0.0, 1, 50);
        a.push(2.0, 1, 3);
        assert_eq!(a.finished(), 0);
        a.push(4.0, 0, 5);
        assert_eq!(a.finished(), 2);
    }

    #[test]
    fn activity_bars_are_oldest_first_and_scaled() {
        let mut a = Activity::default();
        assert_eq!(a.bars(4, 0.0), vec![0.0; 4]);
        a.push(0.0, 2, 0);
        a.push(100.0, 1, 0);
        a.push(119.0, 0, 0);
        let b = a.bars(4, 119.0);
        // Age 119 s is the oldest column, age 19 s and 0 s share the newest.
        assert_eq!(b, vec![1.0, 0.0, 0.0, 0.5]);
        assert!(a.bars(60, 119.0).iter().all(|v| (0.0..=1.0).contains(v)));
    }
}
