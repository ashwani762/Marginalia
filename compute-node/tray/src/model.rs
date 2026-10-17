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
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Paused,
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
    pub chat_jobs: u64,
    pub search_jobs: u64,
    pub failed: u64,
    pub phones: Vec<Phone>,
    pub asking: Vec<Waiting>,
    pub addresses: Vec<String>,
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
        chat_jobs: jobs("chat", "jobs"),
        search_jobs: jobs("search", "jobs"),
        failed: jobs("chat", "errors") + jobs("search", "errors") + jobs("speech", "errors"),
        phones,
        asking,
        addresses: status["addresses"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(String::from)).collect(),
    }
}

fn plural(count: u64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
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
    if i.paused {
        "paused"
    } else {
        "running"
    }
}

/// The hover text, for example "falcon: running, 12 jobs served, 2 phones".
pub fn tooltip(v: &View) -> String {
    match &v.poll {
        Poll::Down if v.starting => "Marginalia node: starting".into(),
        Poll::Down => "Marginalia node: not running".into(),
        Poll::Up(i) => format!("{}: {}, {} served, {}", i.name, state_word(i), plural(i.served, "job", "jobs"), plural(i.phones.len() as u64, "phone", "phones")),
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
        assert_eq!(up().state(), State::Running);
        let mut paused = parse(&status(), None);
        paused.paused = true;
        assert_eq!(View { poll: Poll::Up(Box::new(paused)), starting: false }.state(), State::Paused);
        assert_eq!(View { poll: Poll::Down, starting: false }.state(), State::Down);
    }

    #[test]
    fn tooltip_text() {
        assert_eq!(tooltip(&up()), "falcon: running, 12 jobs served, 2 phones");
        let one = parse(&json!({"name": "pi", "paused": true, "served": 1, "clients": [{"name": "A", "fp": "x"}]}), None);
        assert_eq!(tooltip(&View { poll: Poll::Up(Box::new(one)), starting: false }), "pi: paused, 1 job served, 1 phone");
        assert_eq!(tooltip(&View { poll: Poll::Down, starting: false }), "Marginalia node: not running");
        assert_eq!(tooltip(&View { poll: Poll::Down, starting: true }), "Marginalia node: starting");
    }

    #[test]
    fn right_click_menu_is_short() {
        assert_eq!(lines(&menu(&up())), ["- falcon: running", "---", "> Pause", "> Show details", "---", "> Stop node", "> Quit tray"]);
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
