//! A picture of the machine while the node works: processor, memory, graphics card and network, once a second.
//! Only numbers are kept, in memory, for the last 15 minutes. The tray window draws them and a summary line goes to the log
//! every minute, so a slow or stuck node can be understood afterwards.

use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How many one second samples are kept (15 minutes).
const KEEP: usize = 900;
/// A graphics card reading older than this is not shown.
const GPU_STALE: Duration = Duration::from_secs(4);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    /// Milliseconds since 1970.
    pub at_ms: u64,
    /// Processor use of the whole machine, 0 to 100.
    pub cpu: f32,
    /// The part of that used by this node, 0 to 100 of all cores.
    pub node_cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    /// Memory held by this node.
    pub node_rss: u64,
    pub gpu: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub gpu_temp: Option<f32>,
    pub gpu_watts: Option<f32>,
    /// Bytes a second, all network cards of the machine except loopback.
    pub net_rx: u64,
    pub net_tx: u64,
    /// Jobs running or waiting on this node.
    pub jobs: u32,
}

impl Sample {
    pub fn to_json(&self) -> Value {
        json!({
            "t": self.at_ms, "cpu": round1(self.cpu), "node_cpu": round1(self.node_cpu),
            "mem_used": self.mem_used, "mem_total": self.mem_total, "node_rss": self.node_rss,
            "gpu": self.gpu.map(round1), "vram_used": self.vram_used, "vram_total": self.vram_total,
            "gpu_temp": self.gpu_temp.map(round1), "gpu_watts": self.gpu_watts.map(round1),
            "net_rx": self.net_rx, "net_tx": self.net_tx, "jobs": self.jobs,
        })
    }

    /// One line for the log, for example `cpu 12% (node 9%), memory 9.1 of 31.2 GB (node 4.8 GB), gpu 35% 3.4 of 8.0 GB 52C, network 12 KB/s in 3 KB/s out, 1 job`.
    pub fn summary(&self) -> String {
        let gb = |b: u64| format!("{:.1}", b as f64 / 1e9);
        let gpu = match (self.gpu, self.vram_used, self.vram_total) {
            (Some(g), Some(u), Some(t)) => format!("gpu {g:.0}% {} of {} GB{}", gb(u), gb(t), self.gpu_temp.map(|c| format!(" {c:.0}C")).unwrap_or_default()),
            _ => "no gpu reading".into(),
        };
        format!(
            "cpu {:.0}% (node {:.0}%), memory {} of {} GB (node {} GB), {gpu}, network {} in {} out, {} {}",
            self.cpu, self.node_cpu, gb(self.mem_used), gb(self.mem_total), gb(self.node_rss), rate(self.net_rx), rate(self.net_tx), self.jobs, if self.jobs == 1 { "job" } else { "jobs" }
        )
    }
}

fn round1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

/// Bytes a second as short text: "532 B/s", "12 KB/s", "3.4 MB/s".
pub fn rate(bytes: u64) -> String {
    let b = bytes as f64;
    if b < 1000.0 {
        format!("{bytes} B/s")
    } else if b < 1e6 {
        format!("{:.0} KB/s", b / 1e3)
    } else {
        format!("{:.1} MB/s", b / 1e6)
    }
}

#[derive(Default, Clone)]
struct GpuReading {
    name: String,
    util: f32,
    used: u64,
    total: u64,
    temp: Option<f32>,
    watts: Option<f32>,
}

pub struct Resources {
    ring: Mutex<VecDeque<Sample>>,
    gpu: Mutex<Option<(Instant, GpuReading)>>,
}

impl Resources {
    pub fn new() -> Arc<Resources> {
        Arc::new(Resources { ring: Mutex::new(VecDeque::with_capacity(KEEP)), gpu: Mutex::new(None) })
    }

    /// The newest `n` samples, oldest first.
    pub fn last(&self, n: usize) -> Vec<Sample> {
        let r = self.ring.lock();
        let skip = r.len().saturating_sub(n);
        r.iter().skip(skip).cloned().collect()
    }

    pub fn gpu_name(&self) -> Option<String> {
        self.gpu.lock().as_ref().map(|(_, g)| g.name.clone()).filter(|n| !n.is_empty())
    }

    fn push(&self, s: Sample) {
        let mut r = self.ring.lock();
        if r.len() == KEEP {
            r.pop_front();
        }
        r.push_back(s);
    }

    /// Starts reading once a second on a thread of its own. `jobs` says how many jobs the node has right now.
    /// A summary goes to the log every minute (and the first one after 5 seconds), and at once when the machine is nearly out of memory.
    pub fn start(self: &Arc<Self>, jobs: impl Fn() -> u32 + Send + 'static) {
        self.watch_gpu();
        let me = self.clone();
        let _ = std::thread::Builder::new().name("resources".into()).spawn(move || {
            use sysinfo::{Networks, Pid, ProcessesToUpdate, System};
            let mut sys = System::new();
            let mut nets = Networks::new_with_refreshed_list();
            let pid = Pid::from_u32(std::process::id());
            let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f32;
            // The first reading of the processor has nothing to compare with.
            sys.refresh_cpu_usage();
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]));
            std::thread::sleep(Duration::from_millis(500));
            let mut last = Instant::now();
            let mut ticks = 0u64;
            loop {
                std::thread::sleep(Duration::from_millis(1000));
                let dt = last.elapsed().as_secs_f64().max(0.2);
                last = Instant::now();
                sys.refresh_cpu_usage();
                sys.refresh_memory();
                sys.refresh_processes(ProcessesToUpdate::Some(&[pid]));
                nets.refresh();
                let (rx, tx) = nets.iter().filter(|(n, _)| n.as_str() != "lo" && !n.to_lowercase().contains("loopback")).fold((0u64, 0u64), |a, (_, d)| (a.0 + d.received(), a.1 + d.transmitted()));
                let proc_ = sys.process(pid);
                let gpu = me.gpu.lock().as_ref().filter(|(at, _)| at.elapsed() < GPU_STALE).map(|(_, g)| g.clone());
                let s = Sample {
                    at_ms: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
                    cpu: sys.global_cpu_usage().clamp(0.0, 100.0),
                    node_cpu: proc_.map(|p| p.cpu_usage() / cores).unwrap_or(0.0).clamp(0.0, 100.0),
                    mem_used: sys.used_memory(),
                    mem_total: sys.total_memory(),
                    node_rss: proc_.map(|p| p.memory()).unwrap_or(0),
                    gpu: gpu.as_ref().map(|g| g.util),
                    vram_used: gpu.as_ref().map(|g| g.used),
                    vram_total: gpu.as_ref().map(|g| g.total),
                    gpu_temp: gpu.as_ref().and_then(|g| g.temp),
                    gpu_watts: gpu.as_ref().and_then(|g| g.watts),
                    net_rx: (rx as f64 / dt) as u64,
                    net_tx: (tx as f64 / dt) as u64,
                    jobs: jobs(),
                };
                ticks += 1;
                let low_memory = s.mem_total > 0 && s.mem_used as f64 > s.mem_total as f64 * 0.95;
                if ticks == 5 || ticks % 60 == 0 || (low_memory && ticks % 10 == 0) {
                    if low_memory {
                        tracing::warn!("the machine is nearly out of memory: {}", s.summary());
                    } else {
                        tracing::info!("resources: {}", s.summary());
                    }
                }
                me.push(s);
            }
        });
    }

    /// Reads the graphics card from `nvidia-smi` (one program that keeps running and prints a line a second). Nothing happens
    /// when there is no NVIDIA card or the program is not installed, and the graph then has no graphics card line.
    fn watch_gpu(self: &Arc<Self>) {
        let me = self.clone();
        let _ = std::thread::Builder::new().name("gpu-reading".into()).spawn(move || {
            let mut cmd = Command::new("nvidia-smi");
            cmd.args(["--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw", "--format=csv,noheader,nounits", "-l", "1"]).stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // no console window
            }
            let Ok(mut child) = cmd.spawn() else { return };
            let Some(out) = child.stdout.take() else { return };
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(g) = parse_gpu_line(&line) {
                    *me.gpu.lock() = Some((Instant::now(), g));
                }
            }
            let _ = child.wait();
        });
    }
}

/// `NVIDIA GeForce RTX 4060, 35, 3456, 8188, 52, 41.20` (memory in MiB, temperature in C, power in W, any of the last two can be `[N/A]`).
fn parse_gpu_line(line: &str) -> Option<GpuReading> {
    let f: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
    if f.len() < 4 {
        return None;
    }
    let num = |s: &str| s.parse::<f32>().ok();
    Some(GpuReading {
        name: f[0].to_string(),
        util: num(f[1])?,
        used: (num(f[2])? as u64) * 1024 * 1024,
        total: (num(f[3])? as u64) * 1024 * 1024,
        temp: f.get(4).and_then(|s| num(s)),
        watts: f.get(5).and_then(|s| num(s)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_graphics_card_line_is_read_and_missing_values_are_left_out() {
        let g = parse_gpu_line("NVIDIA GeForce RTX 4060, 35, 3456, 8188, 52, 41.20").unwrap();
        assert_eq!((g.name.as_str(), g.util, g.used, g.total, g.temp, g.watts), ("NVIDIA GeForce RTX 4060", 35.0, 3456 * 1048576, 8188 * 1048576, Some(52.0), Some(41.2)));
        let g = parse_gpu_line("NVIDIA GeForce RTX 4060, 1, 3456, 8188, 45, [N/A]").unwrap();
        assert_eq!((g.temp, g.watts), (Some(45.0), None));
        assert!(parse_gpu_line("not a reading").is_none());
        assert!(parse_gpu_line("name, [N/A], 1, 2").is_none());
    }

    #[test]
    fn rates_are_short_text() {
        assert_eq!((rate(532), rate(12_400), rate(3_400_000)), ("532 B/s".to_string(), "12 KB/s".to_string(), "3.4 MB/s".to_string()));
    }

    #[test]
    fn the_newest_samples_are_kept_oldest_first_and_the_summary_is_one_plain_line() {
        let r = Resources::new();
        for i in 0..(KEEP as u64 + 10) {
            r.push(Sample { at_ms: i, ..Default::default() });
        }
        let last = r.last(3);
        assert_eq!(last.iter().map(|s| s.at_ms).collect::<Vec<_>>(), [KEEP as u64 + 7, KEEP as u64 + 8, KEEP as u64 + 9]);
        assert_eq!(r.last(10_000).len(), KEEP);
        let s = Sample { cpu: 12.4, node_cpu: 9.0, mem_used: 9_100_000_000, mem_total: 31_200_000_000, node_rss: 4_800_000_000, gpu: Some(35.0), vram_used: Some(3_400_000_000), vram_total: Some(8_000_000_000), gpu_temp: Some(52.0), net_rx: 12_000, net_tx: 3_000, jobs: 1, ..Default::default() };
        assert_eq!(s.summary(), "cpu 12% (node 9%), memory 9.1 of 31.2 GB (node 4.8 GB), gpu 35% 3.4 of 8.0 GB 52C, network 12 KB/s in 3 KB/s out, 1 job");
        assert!(!s.summary().contains(';') && !s.summary().contains('—'));
        assert!(Sample::default().summary().contains("no gpu reading"));
    }
}
