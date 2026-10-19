//! The node's log on disk. Everything the node says while it runs also goes to `logs/node.log` in its state folder, so a problem
//! can be looked at afterwards. The file is replaced by a fresh one when it reaches 5 MB, and the last 5 are kept, so it never
//! grows past about 25 MB. The log holds events and numbers only: never the text of a note, a question, a recording or a photo.

use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const FILE: &str = "node.log";
const MAX_BYTES: u64 = 5 * 1024 * 1024;
const KEEP: usize = 5;

/// The folder of the logs for a state folder.
pub fn dir_of(state: &Path) -> PathBuf {
    state.join("logs")
}

struct Inner {
    dir: PathBuf,
    file: File,
    size: u64,
    max: u64,
    keep: usize,
}

#[derive(Clone)]
pub struct LogFile(Arc<Mutex<Inner>>);

impl LogFile {
    pub fn open(dir: &Path) -> Result<LogFile> {
        Self::open_with(dir, MAX_BYTES, KEEP)
    }

    fn open_with(dir: &Path, max: u64, keep: usize) -> Result<LogFile> {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let path = dir.join(FILE);
        let file = OpenOptions::new().create(true).append(true).open(&path).with_context(|| format!("cannot open {}", path.display()))?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(LogFile(Arc::new(Mutex::new(Inner { dir: dir.to_path_buf(), file, size, max, keep }))))
    }
}

impl Inner {
    /// `node.log` becomes `node.log.1`, that one `node.log.2` and so on, and the oldest goes.
    fn rotate(&mut self) {
        let name = |n: usize| self.dir.join(if n == 0 { FILE.to_string() } else { format!("{FILE}.{n}") });
        let _ = std::fs::remove_file(name(self.keep));
        for n in (0..self.keep).rev() {
            let _ = std::fs::rename(name(n), name(n + 1));
        }
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(name(0)) {
            self.file = f;
            self.size = 0;
        }
    }
}

/// What the log layer writes to.
pub struct Handle(LogFile);

impl Write for Handle {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut g = (self.0).0.lock();
        if g.size > 0 && g.size + buf.len() as u64 > g.max {
            g.rotate();
        }
        let n = g.file.write(buf)?;
        g.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        (self.0).0.lock().file.flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogFile {
    type Writer = Handle;

    fn make_writer(&'a self) -> Handle {
        Handle(self.clone())
    }
}

/// The last `lines` lines of the current log, for `marginalia-node logs`.
pub fn tail(dir: &Path, lines: usize) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join(FILE)).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("marginalia-log-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn lines_are_appended_and_the_file_is_replaced_when_full_keeping_the_newest_few() {
        let d = temp("rotate");
        let log = LogFile::open_with(&d, 100, 2).unwrap();
        let mut w = Handle(log.clone());
        // The logger hands over one whole line at a time, so a line is never cut in two by the change of file.
        for i in 0..12 {
            w.write_all(format!("line number {i:02} of the log\n").as_bytes()).unwrap();
        }
        w.flush().unwrap();
        let names: Vec<String> = { let mut v: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect(); v.sort(); v };
        // The current file and two older ones, nothing more.
        assert_eq!(names, ["node.log", "node.log.1", "node.log.2"]);
        let now = std::fs::read_to_string(d.join("node.log")).unwrap();
        assert!(now.contains("line number 11"), "{now}");
        assert!(std::fs::metadata(d.join("node.log")).unwrap().len() <= 100);
        assert_eq!(tail(&d, 2), ["line number 10 of the log", "line number 11 of the log"]);
        // Opening again carries on in the same file.
        let again = LogFile::open_with(&d, 100, 2).unwrap();
        Handle(again).write_all(b"after a restart\n").unwrap();
        assert!(tail(&d, 1)[0].contains("after a restart"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
