//! The processor, memory, graphics card and network readings the live graphs draw, as the node reports them once a second.

use serde_json::Value;
use std::collections::VecDeque;

/// One second of readings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// Milliseconds since 1970.
    pub at_ms: u64,
    pub cpu: f32,
    pub node_cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub node_rss: u64,
    pub gpu: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub gpu_temp: Option<f32>,
    pub net_rx: u64,
    pub net_tx: u64,
    pub jobs: u32,
}

impl Reading {
    pub fn from_json(v: &Value) -> Option<Reading> {
        let n = |k: &str| v[k].as_u64().unwrap_or(0);
        let f = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
        Some(Reading {
            at_ms: v["t"].as_u64()?,
            cpu: f("cpu"),
            node_cpu: f("node_cpu"),
            mem_used: n("mem_used"),
            mem_total: n("mem_total"),
            node_rss: n("node_rss"),
            gpu: v["gpu"].as_f64().map(|x| x as f32),
            vram_used: v["vram_used"].as_u64(),
            vram_total: v["vram_total"].as_u64(),
            gpu_temp: v["gpu_temp"].as_f64().map(|x| x as f32),
            net_rx: n("net_rx"),
            net_tx: n("net_tx"),
            jobs: n("jobs") as u32,
        })
    }

    /// Memory in use, 0 to 1.
    pub fn mem_fraction(&self) -> f32 {
        if self.mem_total == 0 { 0.0 } else { (self.mem_used as f64 / self.mem_total as f64) as f32 }
    }

    pub fn node_mem_fraction(&self) -> f32 {
        if self.mem_total == 0 { 0.0 } else { (self.node_rss as f64 / self.mem_total as f64) as f32 }
    }

    pub fn vram_fraction(&self) -> Option<f32> {
        match (self.vram_used, self.vram_total) {
            (Some(u), Some(t)) if t > 0 => Some((u as f64 / t as f64) as f32),
            _ => None,
        }
    }
}

/// The last few minutes of readings, oldest first.
#[derive(Default)]
pub struct History {
    pub readings: VecDeque<Reading>,
    pub gpu_name: Option<String>,
}

impl History {
    /// Readings kept (5 minutes).
    pub const KEEP: usize = 300;

    /// Adds the readings that are newer than the newest one held. A list that starts over (the node restarted) replaces everything.
    pub fn merge(&mut self, list: Vec<Reading>) {
        // A whole list that ends well before what is held (the clock was set back) replaces it. A list that merely starts
        // before it is the normal case, the node sends the last few readings each time.
        if let (Some(newest_new), Some(last)) = (list.last(), self.readings.back()) {
            if newest_new.at_ms + 5_000 < last.at_ms {
                self.readings.clear();
            }
        }
        let newest = self.readings.back().map(|r| r.at_ms).unwrap_or(0);
        for r in list.into_iter().filter(|r| r.at_ms > newest) {
            if self.readings.len() == Self::KEEP {
                self.readings.pop_front();
            }
            self.readings.push_back(r);
        }
    }

    pub fn clear(&mut self) {
        self.readings.clear();
        self.gpu_name = None;
    }

    pub fn latest(&self) -> Option<&Reading> {
        self.readings.back()
    }

    /// Whether there is a graphics card reading in the last readings.
    pub fn has_gpu(&self) -> bool {
        self.readings.iter().rev().take(5).any(|r| r.gpu.is_some())
    }

    /// One value per reading as `(time, value)`, leaving out the readings that have none.
    pub fn series(&self, f: impl Fn(&Reading) -> Option<f32>) -> Vec<(u64, f32)> {
        self.readings.iter().filter_map(|r| f(r).map(|v| (r.at_ms, v))).collect()
    }

    /// The times between which jobs were running, from the job counts.
    pub fn busy_spans(&self) -> Vec<(u64, u64)> {
        let mut out: Vec<(u64, u64)> = Vec::new();
        let mut open: Option<u64> = None;
        for r in &self.readings {
            match (r.jobs > 0, open) {
                (true, None) => open = Some(r.at_ms),
                (false, Some(from)) => {
                    out.push((from, r.at_ms));
                    open = None;
                }
                _ => {}
            }
        }
        if let (Some(from), Some(last)) = (open, self.readings.back()) {
            out.push((from, last.at_ms));
        }
        out
    }
}

/// The value of a series at a moment, between its two neighbours. None before the first and after the last.
pub fn value_at(series: &[(u64, f32)], t_ms: f64) -> Option<f32> {
    let (first, last) = (series.first()?, series.last()?);
    if t_ms < first.0 as f64 || t_ms > last.0 as f64 {
        return None;
    }
    let i = series.partition_point(|(t, _)| (*t as f64) <= t_ms);
    if i == 0 {
        return Some(first.1);
    }
    if i >= series.len() {
        return Some(last.1);
    }
    let (a, b) = (series[i - 1], series[i]);
    let span = (b.0 - a.0).max(1) as f64;
    Some(a.1 + (b.1 - a.1) * ((t_ms - a.0 as f64) / span) as f32)
}

/// The top of a scale that fits `peak`: 1, 2 or 5 times a power of ten, never below `floor`.
pub fn nice_max(peak: f32, floor: f32) -> f32 {
    let p = peak.max(floor).max(f32::MIN_POSITIVE);
    let mag = 10f32.powf(p.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if p <= m * mag {
            return m * mag;
        }
    }
    10.0 * mag
}

/// How loaded something is, for the colour of its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Load {
    Calm,
    Busy,
    Hot,
}

pub fn load_of(fraction: f32) -> Load {
    if fraction >= 0.9 {
        Load::Hot
    } else if fraction >= 0.7 {
        Load::Busy
    } else {
        Load::Calm
    }
}

/// "9.1 GB", "340 MB".
pub fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else {
        format!("{:.0} MB", b / 1e6)
    }
}

/// "532 B/s", "12 KB/s", "3.4 MB/s".
pub fn rate_text(bytes: f32) -> String {
    if bytes < 1000.0 {
        format!("{bytes:.0} B/s")
    } else if bytes < 1e6 {
        format!("{:.0} KB/s", bytes / 1e3)
    } else {
        format!("{:.1} MB/s", bytes / 1e6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn r(t: u64, jobs: u32) -> Reading {
        Reading { at_ms: t, jobs, ..Default::default() }
    }

    #[test]
    fn a_reading_is_read_from_the_node_and_missing_graphics_values_stay_missing() {
        let v = json!({"t": 1000, "cpu": 12.5, "node_cpu": 9.0, "mem_used": 9_100_000_000u64, "mem_total": 31_200_000_000u64, "node_rss": 4_800_000_000u64,
                       "gpu": null, "vram_used": null, "vram_total": null, "gpu_temp": null, "net_rx": 12_000, "net_tx": 3_000, "jobs": 2});
        let x = Reading::from_json(&v).unwrap();
        assert_eq!((x.at_ms, x.cpu, x.jobs, x.gpu, x.vram_fraction()), (1000, 12.5, 2, None, None));
        assert!((x.mem_fraction() - 0.2917).abs() < 0.001);
        assert!(Reading::from_json(&json!({"cpu": 1})).is_none(), "a reading without a time is not used");
        let g = Reading::from_json(&json!({"t": 1, "gpu": 35.0, "vram_used": 3_400_000_000u64, "vram_total": 8_000_000_000u64})).unwrap();
        assert_eq!((g.gpu, g.vram_fraction()), (Some(35.0), Some(0.425)));
    }

    #[test]
    fn new_readings_are_added_once_and_a_restart_starts_the_history_again() {
        let mut h = History::default();
        h.merge(vec![r(1000, 0), r(2000, 0), r(3000, 1)]);
        h.merge(vec![r(2000, 0), r(3000, 1), r(4000, 1)]);
        assert_eq!(h.readings.iter().map(|x| x.at_ms).collect::<Vec<_>>(), [1000, 2000, 3000, 4000]);
        // The clock was set back by more than 5 seconds: the whole list is older than what is held, so it replaces it.
        let mut late = History::default();
        late.merge(vec![r(100_000, 0), r(101_000, 0)]);
        late.merge(vec![r(50_000, 0), r(51_000, 0)]);
        assert_eq!(late.readings.iter().map(|x| x.at_ms).collect::<Vec<_>>(), [50_000, 51_000]);
        // The usual refresh: a few readings that start before the newest held one must add to the history, not wipe it.
        let mut h2 = History::default();
        h2.merge((0..120).map(|i| r(1_000_000 + i * 1000, 0)).collect());
        h2.merge((113..121).map(|i| r(1_000_000 + i * 1000, 0)).collect());
        assert_eq!(h2.readings.len(), 121);
        for i in 0..(History::KEEP as u64 + 20) {
            h.merge(vec![r(10_000 + i * 1000, 0)]);
        }
        assert_eq!(h.readings.len(), History::KEEP);
        h.clear();
        assert!(h.latest().is_none());
    }

    #[test]
    fn the_busy_stretches_are_found_from_the_job_counts() {
        let mut h = History::default();
        h.merge(vec![r(1000, 0), r(2000, 1), r(3000, 2), r(4000, 0), r(5000, 0), r(6000, 1)]);
        assert_eq!(h.busy_spans(), [(2000, 4000), (6000, 6000)]);
    }

    #[test]
    fn a_series_is_read_between_its_points() {
        let s = vec![(1000, 10.0), (2000, 30.0), (4000, 50.0)];
        assert_eq!(value_at(&s, 1500.0), Some(20.0));
        assert_eq!(value_at(&s, 3000.0), Some(40.0));
        assert_eq!(value_at(&s, 2000.0), Some(30.0));
        assert_eq!((value_at(&s, 500.0), value_at(&s, 4500.0), value_at(&[], 1.0)), (None, None, None));
    }

    #[test]
    fn scales_are_round_and_loads_have_levels() {
        assert_eq!((nice_max(730.0, 1.0), nice_max(0.0, 10_000.0), nice_max(12_400.0, 10_000.0), nice_max(3_400_000.0, 10_000.0)), (1000.0, 10_000.0, 20_000.0, 5_000_000.0));
        assert_eq!((load_of(0.2), load_of(0.7), load_of(0.95)), (Load::Calm, Load::Busy, Load::Hot));
        assert_eq!((size_text(9_100_000_000), size_text(340_000_000)), ("9.1 GB".to_string(), "340 MB".to_string()));
        assert_eq!((rate_text(532.0), rate_text(12_400.0), rate_text(3_400_000.0)), ("532 B/s".to_string(), "12 KB/s".to_string(), "3.4 MB/s".to_string()));
    }
}
