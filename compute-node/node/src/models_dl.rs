//! `marginalia-node models list` and `models download`: fetches the two model files the node serves
//! from Hugging Face, with resume, retries and a progress line. Works the same on every system.
//!
//! The Hugging Face token (environment variable `HF_TOKEN`) is only ever put in the Authorization
//! header. It is never printed, logged or put in an error message.

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One model file the node can use.
#[derive(Clone, Copy, Debug)]
pub struct ModelFile {
    /// `chat` or `search`, as shown by `models list`.
    pub role: &'static str,
    pub repo: &'static str,
    pub file: &'static str,
    /// Expected size in bytes, 0 when not known. Used to skip finished files.
    pub bytes: u64,
    /// Not needed for the node to start (only used with `--chat-model`).
    pub optional: bool,
}

pub const CHAT_E2B: ModelFile = ModelFile { role: "chat", repo: "ggml-org/gemma-4-E2B-it-GGUF", file: crate::host::CHAT_FILE, bytes: 2_841_481_184, optional: false };
pub const CHAT_E4B: ModelFile = ModelFile { role: "chat", repo: "ggml-org/gemma-4-E4B-it-GGUF", file: "gemma-4-E4B-it-Q4_0.gguf", bytes: 0, optional: true };
pub const SEARCH: ModelFile = ModelFile { role: "search", repo: "ggml-org/embeddinggemma-2-GGUF", file: crate::host::EMBED_FILE, bytes: 309_855_456, optional: false };

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Only {
    Chat,
    Search,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ChatChoice {
    E2b,
    E4b,
}

/// The same folder `run` looks in when `--models` is not given.
pub fn default_dir() -> PathBuf {
    dirs::data_dir().map(|d| d.join("marginalia").join("models")).unwrap_or_default()
}

/// The files `download` fetches for the given options.
pub fn wanted(only: Option<Only>, chat: ChatChoice) -> Vec<ModelFile> {
    let chat_file = match chat {
        ChatChoice::E2b => CHAT_E2B,
        ChatChoice::E4b => CHAT_E4B,
    };
    match only {
        Some(Only::Chat) => vec![chat_file],
        Some(Only::Search) => vec![SEARCH],
        None => vec![chat_file, SEARCH],
    }
}

// ---------------------------------------------------------------------------------------------
// Pure helpers (no network, no files): these have the unit tests.
// ---------------------------------------------------------------------------------------------

pub fn url_for(m: &ModelFile) -> String {
    format!("https://huggingface.co/{}/resolve/main/{}", m.repo, m.file)
}

/// The `.part` file next to the final file.
pub fn part_path(dir: &Path, m: &ModelFile) -> PathBuf {
    dir.join(format!("{}.part", m.file))
}

/// The Range header value for resuming after `have` bytes, none when starting from the top.
pub fn range_header(have: u64) -> Option<String> {
    (have > 0).then(|| format!("bytes={have}-"))
}

/// What to do before sending a request, given what is already on disk.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// The finished file is already there.
    Skip,
    /// The `.part` file already has every byte, only the rename is left.
    Finish,
    /// Continue the `.part` file after this many bytes.
    Resume(u64),
    /// Start from the beginning.
    Fresh,
}

pub fn plan(final_len: Option<u64>, part_len: Option<u64>, expected: u64) -> Plan {
    if let Some(f) = final_len {
        if f > 0 && (expected == 0 || f == expected) {
            return Plan::Skip;
        }
    }
    match part_len {
        Some(p) if p > 0 => {
            if expected > 0 && p == expected {
                Plan::Finish
            } else if expected > 0 && p > expected {
                Plan::Fresh
            } else {
                Plan::Resume(p)
            }
        }
        _ => Plan::Fresh,
    }
}

/// Parses `bytes 100-199/200` into (start, end, total). The total is None for `*`.
pub fn parse_content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes")?.trim();
    let (range, total) = rest.split_once('/')?;
    let (a, b) = range.split_once('-')?;
    let total = if total.trim() == "*" { None } else { Some(total.trim().parse().ok()?) };
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?, total))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Problem {
    /// 401 or 403: a gated repository or a wrong token.
    Denied,
    NotFound,
    /// The server could not honour the Range request: throw the part file away and start again.
    RangeBad,
    /// Worth trying again later.
    Transient(u16),
    Other(u16),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    /// The body continues the part file. `total` is the full file size when known.
    Resume { total: Option<u64> },
    /// The body is the whole file (the server ignored the Range header, or none was sent).
    Full { total: Option<u64> },
    Problem(Problem),
}

/// Decides what a response means given the bytes we already hold.
pub fn interpret(status: u16, content_range: Option<&str>, content_length: Option<u64>, have: u64) -> Reply {
    match status {
        206 => match content_range.and_then(parse_content_range) {
            Some((start, _, total)) if start == have => Reply::Resume { total: total.or(content_length.map(|l| l + have)) },
            _ => Reply::Problem(Problem::RangeBad),
        },
        200 => Reply::Full { total: content_length },
        416 => Reply::Problem(Problem::RangeBad),
        401 | 403 => Reply::Problem(Problem::Denied),
        404 => Reply::Problem(Problem::NotFound),
        408 | 429 | 500..=599 => Reply::Problem(Problem::Transient(status)),
        s => Reply::Problem(Problem::Other(s)),
    }
}

/// How long to wait before retry number `attempt` (1 is the first retry).
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.clamp(1, 6)).min(60))
}

/// Megabytes (decimal, like the Hugging Face page), one decimal.
pub fn mb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_000_000.0)
}

/// A size for people: `2.84 GB`, `309.9 MB`, `12 KB`.
pub fn human_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1e9)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else {
        format!("{} KB", bytes / 1000)
    }
}

pub fn format_eta(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// One progress line: `name  120.5 / 2841.5 MB   4%  11.2 MB/s  ETA 4m 03s`.
pub fn progress_line(name: &str, done: u64, total: u64, bytes_per_s: f64) -> String {
    let speed = if bytes_per_s >= 1.0 { format!("{:.1} MB/s", bytes_per_s / 1e6) } else { "-- MB/s".to_string() };
    if total == 0 {
        return format!("{name}  {} MB  {speed}", mb(done));
    }
    let pct = (done as f64 * 100.0 / total as f64).min(100.0) as u32;
    let eta = if bytes_per_s >= 1.0 && total > done { format_eta(((total - done) as f64 / bytes_per_s) as u64) } else { "--".to_string() };
    format!("{name}  {} / {} MB  {pct:>3}%  {speed}  ETA {eta}", mb(done), mb(total))
}

/// A smoothed speed from samples taken now and then.
pub struct SpeedMeter {
    last_t: f64,
    last_bytes: u64,
    speed: f64,
}

impl SpeedMeter {
    pub fn new(now_s: f64, bytes: u64) -> SpeedMeter {
        SpeedMeter { last_t: now_s, last_bytes: bytes, speed: 0.0 }
    }

    /// Feed the time (seconds) and the byte count so far. Returns the smoothed bytes per second.
    pub fn update(&mut self, now_s: f64, bytes: u64) -> f64 {
        let dt = now_s - self.last_t;
        if dt >= 0.5 {
            let inst = bytes.saturating_sub(self.last_bytes) as f64 / dt;
            self.speed = if self.speed == 0.0 { inst } else { self.speed * 0.7 + inst * 0.3 };
            self.last_t = now_s;
            self.last_bytes = bytes;
        }
        self.speed
    }
}

/// The token from the environment, trimmed, None when unset or empty.
pub fn token_from(v: Option<String>) -> Option<String> {
    v.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Which of the files `run` needs are missing, as (role, file name).
pub fn missing_for_run(dir: &Path, chat_given: bool, embed_given: bool) -> Vec<ModelFile> {
    let mut v = vec![];
    if !chat_given && !dir.join(CHAT_E2B.file).is_file() {
        v.push(CHAT_E2B);
    }
    if !embed_given && !dir.join(SEARCH.file).is_file() {
        v.push(SEARCH);
    }
    v
}

/// The message `run` prints when it has nothing to serve.
pub fn missing_message(dir: &Path) -> String {
    let custom = dir != default_dir();
    let dl = if custom { format!("marginalia-node models download --dir \"{}\"", dir.display()) } else { "marginalia-node models download".to_string() };
    format!(
        "No models found in {}.\n\nDownload them (about 3 GB, it can be stopped and continued) with:\n\n    {dl}\n\nTo see what is missing: marginalia-node models list\nTo use another folder: marginalia-node run --models DIR (or --chat-model FILE and --embed-model FILE)",
        dir.display()
    )
}

// ---------------------------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------------------------

pub fn list(dir: Option<PathBuf>) -> Result<()> {
    let dir = dir.unwrap_or_else(default_dir);
    println!("Models folder: {}\n", dir.display());
    let mut missing_needed = false;
    for m in [CHAT_E2B, SEARCH, CHAT_E4B] {
        let len = std::fs::metadata(dir.join(m.file)).ok().filter(|x| x.is_file()).map(|x| x.len());
        let part = std::fs::metadata(part_path(&dir, &m)).ok().map(|x| x.len()).filter(|l| *l > 0);
        let role = if m.optional { format!("{} (optional, pass --chat-model)", m.role) } else { m.role.to_string() };
        match len {
            Some(l) => {
                let note = if m.bytes > 0 && l != m.bytes { "  (size differs from the expected one, download again to repair)" } else { "" };
                println!("  present  {:<28} {:>9}  {role}{note}", m.file, human_size(l));
            }
            None => {
                if !m.optional {
                    missing_needed = true;
                }
                let size = if m.bytes > 0 { format!("about {}", human_size(m.bytes)) } else { "size unknown".to_string() };
                let partial = part.map(|p| format!(", {} already downloaded", human_size(p))).unwrap_or_default();
                println!("  MISSING  {:<28} {:>9}  {role}{partial}", m.file, size);
            }
        }
    }
    println!();
    if missing_needed {
        println!("Some models are missing. Download them with: marginalia-node models download");
    } else {
        println!("Everything the node needs is here.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// download
// ---------------------------------------------------------------------------------------------

enum Fail {
    /// Retrying will not help.
    Fatal(String),
    /// Network trouble: retry after a pause, resuming where it stopped.
    Transient(String),
}

const MAX_RETRIES: u32 = 8;

pub fn download(dir: Option<PathBuf>, only: Option<Only>, chat: ChatChoice) -> Result<()> {
    let dir = dir.unwrap_or_else(default_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create the models folder {}", dir.display()))?;
    let token = token_from(std::env::var("HF_TOKEN").ok());
    println!("Models folder: {}", dir.display());
    if token.is_some() {
        println!("Using the HF_TOKEN environment variable for access.");
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut failed: Vec<String> = vec![];
    for m in wanted(only, chat) {
        match fetch(&agent, &m, &dir, token.as_deref()) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("\n{}: {e:#}", m.file);
                failed.push(m.file.to_string());
            }
        }
    }
    if failed.is_empty() {
        println!("\nDone. Start the node with: marginalia-node run");
        Ok(())
    } else {
        bail!("{} file(s) not downloaded: {}. Run the same command again to continue where it stopped.", failed.len(), failed.join(", "))
    }
}

fn fetch(agent: &ureq::Agent, m: &ModelFile, dir: &Path, token: Option<&str>) -> Result<()> {
    let mut tries = 0u32;
    loop {
        let mut progressed = false;
        match attempt(agent, m, dir, token, &mut progressed) {
            Ok(()) => return Ok(()),
            Err(Fail::Fatal(msg)) => bail!("{msg}"),
            Err(Fail::Transient(msg)) => {
                if progressed {
                    tries = 0;
                }
                tries += 1;
                if tries > MAX_RETRIES {
                    bail!("{msg}. Gave up after {MAX_RETRIES} tries. Run the same command again to continue where it stopped.");
                }
                let wait = backoff(tries);
                eprintln!("\n  {msg}. Trying again in {}s ({tries} of {MAX_RETRIES}).", wait.as_secs());
                std::thread::sleep(wait);
            }
        }
    }
}

fn attempt(agent: &ureq::Agent, m: &ModelFile, dir: &Path, token: Option<&str>, progressed: &mut bool) -> Result<(), Fail> {
    let final_path = dir.join(m.file);
    let part = part_path(dir, m);
    let final_len = std::fs::metadata(&final_path).ok().filter(|x| x.is_file()).map(|x| x.len());
    let part_len = std::fs::metadata(&part).ok().map(|x| x.len());
    let have = match plan(final_len, part_len, m.bytes) {
        Plan::Skip => {
            println!("{}: already complete ({}), skipping", m.file, human_size(final_len.unwrap_or(0)));
            return Ok(());
        }
        Plan::Finish => {
            std::fs::rename(&part, &final_path).map_err(|e| Fail::Fatal(format!("cannot move {} into place: {e}", part.display())))?;
            println!("{}: finished from the earlier download", m.file);
            return Ok(());
        }
        Plan::Resume(n) => n,
        Plan::Fresh => {
            let _ = std::fs::remove_file(&part);
            0
        }
    };

    let url = url_for(m);
    let mut req = agent.get(&url).header("User-Agent", "marginalia-node (rust)").header("Accept-Encoding", "identity");
    if let Some(t) = token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    if let Some(r) = range_header(have) {
        req = req.header("Range", &r);
    }
    let mut resp = match req.call() {
        Ok(r) => r,
        Err(e) => return Err(Fail::Transient(describe_network_error(&e))),
    };
    let status = resp.status().as_u16();
    let content_range = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).map(String::from);
    let content_length = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    let (start, total) = match interpret(status, content_range.as_deref(), content_length, have) {
        Reply::Resume { total } => (have, total),
        Reply::Full { total } => (0, total),
        Reply::Problem(Problem::Denied) => {
            let hint = if token.is_some() {
                "The HF_TOKEN you set was refused. Check that it is a valid read token and that your account accepted the licence on the model page."
            } else {
                "This repository needs a Hugging Face account. Create a read token at https://huggingface.co/settings/tokens, accept the licence on the model page, then set the HF_TOKEN environment variable and run this again (PowerShell: $env:HF_TOKEN = \"hf_...\")."
            };
            return Err(Fail::Fatal(format!("Hugging Face refused access to {} (HTTP {status}). {hint}", m.repo)));
        }
        Reply::Problem(Problem::NotFound) => return Err(Fail::Fatal(format!("{url} was not found (HTTP 404). The file may have been renamed, please report this."))),
        Reply::Problem(Problem::RangeBad) => {
            // Not a failure of the network: the part file is not usable, so begin again at once.
            let _ = std::fs::remove_file(&part);
            return Err(Fail::Transient("The earlier partial download could not be continued, starting that file again".into()));
        }
        Reply::Problem(Problem::Transient(s)) => return Err(Fail::Transient(format!("Hugging Face answered HTTP {s}, it is busy or having trouble"))),
        Reply::Problem(Problem::Other(s)) => return Err(Fail::Fatal(format!("Unexpected answer from Hugging Face (HTTP {s}) for {url}"))),
    };
    if let (Some(t), true) = (total, m.bytes > 0) {
        if t != m.bytes {
            eprintln!("  Note: the server reports {t} bytes, this program expected {}. The repository may have been updated.", m.bytes);
        }
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(start > 0)
        .truncate(start == 0)
        .open(&part)
        .map_err(|e| Fail::Fatal(format!("cannot write {}: {e}", part.display())))?;

    let interactive = std::io::stderr().is_terminal();
    let t0 = Instant::now();
    let mut meter = SpeedMeter::new(0.0, start);
    let mut last_print = Instant::now() - Duration::from_secs(1);
    let mut last_pct_printed = u32::MAX;
    let mut done = start;
    let total_n = total.unwrap_or(0);
    if start > 0 {
        println!("{}: continuing from {} MB", m.file, mb(start));
    } else {
        println!("{}: downloading {}", m.file, if total_n > 0 { human_size(total_n) } else { "(size unknown)".into() });
    }
    let mut reader = resp.body_mut().as_reader();
    let mut buf = vec![0u8; 256 * 1024];
    let read_error = loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break None,
            Ok(n) => n,
            Err(e) => break Some(e),
        };
        file.write_all(&buf[..n]).map_err(|e| Fail::Fatal(format!("cannot write {} (is the disk full?): {e}", part.display())))?;
        done += n as u64;
        *progressed = true;
        let speed = meter.update(t0.elapsed().as_secs_f64(), done);
        if last_print.elapsed() >= Duration::from_millis(250) {
            last_print = Instant::now();
            let line = progress_line(m.file, done, total_n, speed);
            if interactive {
                eprint!("\r  {line}   ");
                let _ = std::io::stderr().flush();
            } else {
                let pct = if total_n > 0 { (done * 100 / total_n) as u32 / 5 * 5 } else { 0 };
                if pct != last_pct_printed {
                    last_pct_printed = pct;
                    eprintln!("  {line}");
                }
            }
        }
    };
    let _ = file.flush();
    drop(file);
    if interactive {
        eprintln!();
    }
    if let Some(e) = read_error {
        return Err(Fail::Transient(format!("The connection broke ({e})")));
    }
    if total_n > 0 && done < total_n {
        return Err(Fail::Transient(format!("The connection closed early at {} of {} MB", mb(done), mb(total_n))));
    }
    if total_n > 0 && done > total_n {
        let _ = std::fs::remove_file(&part);
        return Err(Fail::Transient("The file came out larger than announced, starting that file again".into()));
    }
    std::fs::rename(&part, &final_path).map_err(|e| Fail::Fatal(format!("cannot move {} into place: {e}", part.display())))?;
    println!("  OK {} ({})", m.file, human_size(done));
    Ok(())
}

/// A plain sentence for a failed connection. Never includes request headers.
fn describe_network_error(e: &ureq::Error) -> String {
    match e {
        ureq::Error::HostNotFound => "Cannot find huggingface.co. Check your internet connection and DNS".into(),
        ureq::Error::ConnectionFailed => "Cannot connect to huggingface.co. Check your internet connection, proxy, VPN or firewall".into(),
        ureq::Error::Timeout(_) => "Hugging Face did not answer in time. Check your internet connection".into(),
        other => format!("Network problem ({other})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(url_for(&CHAT_E2B), "https://huggingface.co/ggml-org/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q4_0.gguf");
        assert_eq!(url_for(&SEARCH), "https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/resolve/main/embeddinggemma-2-Q8_0.gguf");
        assert_eq!(part_path(Path::new("m"), &SEARCH), Path::new("m").join("embeddinggemma-2-Q8_0.gguf.part"));
    }

    #[test]
    fn range() {
        assert_eq!(range_header(0), None);
        assert_eq!(range_header(1234).as_deref(), Some("bytes=1234-"));
    }

    #[test]
    fn plans() {
        assert_eq!(plan(Some(100), None, 100), Plan::Skip);
        assert_eq!(plan(Some(100), None, 0), Plan::Skip);
        assert_eq!(plan(Some(90), None, 100), Plan::Fresh);
        assert_eq!(plan(Some(0), None, 100), Plan::Fresh);
        assert_eq!(plan(None, Some(40), 100), Plan::Resume(40));
        assert_eq!(plan(None, Some(100), 100), Plan::Finish);
        assert_eq!(plan(None, Some(150), 100), Plan::Fresh);
        assert_eq!(plan(None, Some(150), 0), Plan::Resume(150));
        assert_eq!(plan(None, Some(0), 100), Plan::Fresh);
        assert_eq!(plan(Some(90), Some(10), 100), Plan::Resume(10));
    }

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 100-199/200"), Some((100, 199, Some(200))));
        assert_eq!(parse_content_range("bytes 0-9/*"), Some((0, 9, None)));
        assert_eq!(parse_content_range("items 0-9/10"), None);
        assert_eq!(parse_content_range("bytes */200"), None);
    }

    #[test]
    fn replies() {
        assert_eq!(interpret(206, Some("bytes 100-199/200"), Some(100), 100), Reply::Resume { total: Some(200) });
        assert_eq!(interpret(206, None, Some(100), 100), Reply::Problem(Problem::RangeBad));
        assert_eq!(interpret(206, Some("bytes 50-199/200"), Some(150), 100), Reply::Problem(Problem::RangeBad));
        assert_eq!(interpret(200, None, Some(200), 100), Reply::Full { total: Some(200) });
        assert_eq!(interpret(416, None, None, 100), Reply::Problem(Problem::RangeBad));
        assert_eq!(interpret(401, None, None, 0), Reply::Problem(Problem::Denied));
        assert_eq!(interpret(403, None, None, 0), Reply::Problem(Problem::Denied));
        assert_eq!(interpret(404, None, None, 0), Reply::Problem(Problem::NotFound));
        assert_eq!(interpret(503, None, None, 0), Reply::Problem(Problem::Transient(503)));
        assert_eq!(interpret(429, None, None, 0), Reply::Problem(Problem::Transient(429)));
        assert_eq!(interpret(418, None, None, 0), Reply::Problem(Problem::Other(418)));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let s: Vec<u64> = (1..=9).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(s, vec![2, 4, 8, 16, 32, 60, 60, 60, 60]);
    }

    #[test]
    fn formatting() {
        assert_eq!(mb(2_841_481_184), "2841.5");
        assert_eq!(human_size(2_841_481_184), "2.84 GB");
        assert_eq!(human_size(309_855_456), "309.9 MB");
        assert_eq!(format_eta(45), "45s");
        assert_eq!(format_eta(125), "2m 05s");
        assert_eq!(format_eta(3720), "1h 02m");
        assert_eq!(progress_line("a.gguf", 500_000_000, 1_000_000_000, 10_000_000.0), "a.gguf  500.0 / 1000.0 MB   50%  10.0 MB/s  ETA 50s");
        assert_eq!(progress_line("a.gguf", 0, 1_000_000_000, 0.0), "a.gguf  0.0 / 1000.0 MB    0%  -- MB/s  ETA --");
        assert_eq!(progress_line("a.gguf", 5_000_000, 0, 1_000_000.0), "a.gguf  5.0 MB  1.0 MB/s");
    }

    #[test]
    fn speed_is_smoothed() {
        let mut m = SpeedMeter::new(0.0, 0);
        assert_eq!(m.update(0.2, 1000), 0.0);
        assert_eq!(m.update(1.0, 1_000_000), 1_000_000.0);
        let s = m.update(2.0, 3_000_000);
        assert!(s > 1_000_000.0 && s < 2_000_000.0);
    }

    #[test]
    fn token_is_trimmed() {
        assert_eq!(token_from(Some("  abc \n".into())).as_deref(), Some("abc"));
        assert_eq!(token_from(Some("   ".into())), None);
        assert_eq!(token_from(None), None);
    }

    #[test]
    fn message_names_the_command() {
        let m = missing_message(&default_dir());
        assert!(m.contains("marginalia-node models download"));
        assert!(!m.contains("--dir"));
        assert!(missing_message(Path::new("D:\\x")).contains("--dir"));
    }
}
