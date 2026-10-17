//! Platform neutral behaviour: polling the node, and doing what a menu click asks.

use crate::admin;
use crate::model::{self, Action, Poll, View};
use crate::os;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long "Start node" shows "starting" before it gives up and offers Start again.
const START_GRACE: Duration = Duration::from_secs(120);
/// The icon only needs its colour kept right. The open panel polls faster on its own.
pub const POLL_EVERY: Duration = Duration::from_secs(5);

/// Something to tell the person after an action that did not work.
#[derive(Debug, PartialEq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

impl Notice {
    fn show(self) {
        os::toast(&self.title, &self.body, 8000);
    }
}

/// A pairing code the node just opened.
#[derive(Debug, PartialEq)]
pub struct Pairing {
    pub code: String,
    pub minutes: u64,
    pub addresses: Vec<String>,
}

/// Opens the details panel as a separate process (`marginalia-node-tray --panel`), so the tray stays
/// small and the panel works the same on every system. A click on the icon toggles it.
pub struct PanelHost {
    port: u16,
    child: Arc<Mutex<Option<std::process::Child>>>,
    closed_at: Arc<Mutex<Option<Instant>>>,
}

/// A click this soon after the panel closed itself (it closes when it loses focus, and the click on
/// the icon is what took the focus) is the same click, so it must not open the panel again.
const REOPEN_GUARD: Duration = Duration::from_millis(600);

impl PanelHost {
    fn new(port: u16) -> PanelHost {
        PanelHost { port, child: Default::default(), closed_at: Default::default() }
    }

    fn is_open(&self) -> bool {
        self.child.lock().map(|c| c.is_some()).unwrap_or(false)
    }

    pub fn open(&self) {
        if self.is_open() {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--panel").arg("--admin-port").arg(self.port.to_string());
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let Ok(child) = cmd.spawn() else { return };
        if let Ok(mut c) = self.child.lock() {
            *c = Some(child);
        }
        // Notices when the panel ends, however it ends.
        let (child, closed_at) = (self.child.clone(), self.closed_at.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(100));
            let Ok(mut c) = child.lock() else { return };
            match c.as_mut().map(|ch| ch.try_wait()) {
                Some(Ok(None)) => {}
                _ => {
                    *c = None;
                    if let Ok(mut t) = closed_at.lock() {
                        *t = Some(Instant::now());
                    }
                    return;
                }
            }
        });
    }

    pub fn toggle(&self) {
        if self.is_open() {
            if let Ok(mut c) = self.child.lock() {
                if let Some(ch) = c.as_mut() {
                    let _ = ch.kill();
                }
            }
        } else if self.closed_at.lock().ok().and_then(|t| *t).map(|t| t.elapsed() < REOPEN_GUARD).unwrap_or(false) {
            // The click that just closed it.
        } else {
            self.open();
        }
    }
}

pub struct Controller {
    port: u16,
    panel: PanelHost,
    starting: Mutex<Option<Instant>>,
    refresh: Sender<()>,
}

impl Controller {
    pub fn new(port: u16) -> (Arc<Controller>, Receiver<()>) {
        let (refresh, rx) = channel();
        (Arc::new(Controller { port, panel: PanelHost::new(port), starting: Mutex::new(None), refresh }), rx)
    }

    fn starting(&self) -> bool {
        self.starting.lock().map(|s| s.map(|t| t.elapsed() < START_GRACE).unwrap_or(false)).unwrap_or(false)
    }

    /// Asks the node what it is doing. A node that does not answer is "not running".
    pub fn fetch(&self) -> View {
        match admin::ask(self.port, "status") {
            Ok(status) => {
                if let Ok(mut s) = self.starting.lock() {
                    *s = None;
                }
                let stats = admin::ask(self.port, "stats").ok();
                View { poll: Poll::Up(Box::new(model::parse(&status, stats.as_ref()))), starting: false }
            }
            Err(_) => View { poll: Poll::Down, starting: self.starting() },
        }
    }

    /// Opens or closes the details panel (a click on the icon).
    pub fn toggle_panel(&self) {
        self.panel.toggle();
    }

    /// Asks the node to open pairing and returns the code to show.
    pub fn pairing(&self) -> Result<Pairing, String> {
        let r = admin::ask(self.port, "pair")?;
        Ok(Pairing {
            code: r["code"].as_str().unwrap_or("").to_string(),
            minutes: r["minutes"].as_u64().unwrap_or(5),
            addresses: r["addresses"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(String::from)).collect(),
        })
    }

    /// `block`, `revoke`, `approve` or `deny` for one phone, by its id.
    pub fn phone(&self, cmd: &str, who: &str) -> Option<Notice> {
        let what = match cmd {
            "block" => "block that phone",
            "revoke" => "remove that phone",
            "approve" => "allow that phone",
            _ => "turn that phone down",
        };
        match admin::ask_with(self.port, &serde_json::json!({"cmd": cmd, "who": who})) {
            Ok(_) => None,
            Err(e) => Some(Notice { title: format!("Could not {what}"), body: e }),
        }
    }

    /// Does what a click on `action` asks. Returns what to tell the person if it did not work.
    /// `Quit` belongs to the interface and does nothing here.
    pub fn perform(&self, action: Action) -> Option<Notice> {
        let notice = |title: &str, body: String| Some(Notice { title: title.into(), body });
        let simple = |cmd: &str, what: &str| match admin::ask(self.port, cmd) {
            Ok(_) => None,
            Err(e) => notice(&format!("Could not {what}"), e),
        };
        match action {
            Action::Pause => simple("pause", "pause the node"),
            Action::Resume => simple("resume", "resume the node"),
            Action::Lock => simple("lock", "lock new phones"),
            Action::Unlock => simple("unlock", "unlock new phones"),
            Action::Stop => simple("shutdown", "stop the node"),
            Action::ShowDetails => {
                self.panel.open();
                None
            }
            Action::Start => {
                if let Ok(mut s) = self.starting.lock() {
                    *s = Some(Instant::now());
                }
                match os::start_node(self.port) {
                    Ok(_) => None,
                    Err(e) => {
                        if let Ok(mut s) = self.starting.lock() {
                            *s = None;
                        }
                        notice("Could not start the node", e)
                    }
                }
            }
            Action::ChatSmall | Action::ChatLarge => {
                let choice = if action == Action::ChatSmall { "e2b" } else { "e4b" };
                if let Err(e) = os::set_chat_model(choice) {
                    return notice("Could not change the chat model", e);
                }
                // The old node must be gone first so its model leaves the graphics card before the new one loads.
                if admin::ask(self.port, "shutdown").is_ok() {
                    let mut gone = false;
                    for _ in 0..60 {
                        if admin::ask(self.port, "status").is_err() {
                            gone = true;
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    if !gone {
                        return notice("Could not change the chat model", "The node did not stop in 30 seconds. The new choice is saved and is used the next time the node starts.".into());
                    }
                    // A moment for the graphics driver to take the memory back.
                    std::thread::sleep(Duration::from_secs(1));
                }
                self.perform(Action::Start)
            }
            Action::Quit => None,
        }
    }

    /// Runs a click on its own thread (it may talk to the node or start a program) and then asks
    /// the poller to look again right away.
    pub fn click(self: &Arc<Self>, action: Action) {
        let me = self.clone();
        std::thread::spawn(move || {
            if let Some(notice) = me.perform(action) {
                notice.show();
            }
            let _ = me.refresh.send(());
        });
    }
}

/// Polls every two seconds (or sooner after a click) and hands each result to `on_view`.
pub fn spawn_poller(ctl: Arc<Controller>, rx: Receiver<()>, mut on_view: impl FnMut(View) + Send + 'static) {
    std::thread::spawn(move || loop {
        on_view(ctl.fetch());
        let _ = rx.recv_timeout(POLL_EVERY);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::State;
    use serde_json::{json, Value};
    use std::io::{BufRead, BufReader, Write};
    use std::net::{Ipv4Addr, TcpListener};

    /// A fake admin server on a free port. Answers by command and remembers the commands it saw.
    fn fake_node(answer: fn(&str) -> Value) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(vec![]));
        let log = seen.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut s) = conn else { return };
                let mut line = String::new();
                if BufReader::new(s.try_clone().unwrap()).read_line(&mut line).is_err() {
                    continue;
                }
                let cmd = serde_json::from_str::<Value>(&line).map(|v| v["cmd"].as_str().unwrap_or("").to_string()).unwrap_or_default();
                let _ = s.write_all(format!("{}\n", answer(&cmd)).as_bytes());
                log.lock().unwrap().push(cmd);
            }
        });
        (port, seen)
    }

    fn node_answers(cmd: &str) -> Value {
        match cmd {
            "status" => json!({"name": "falcon", "paused": true, "served": 3, "clients": [], "addresses": ["192.168.1.2:47615"]}),
            "stats" => json!({"name": "falcon", "report": "the report", "chat": {"jobs": 3, "errors": 0}, "search": {"jobs": 0, "errors": 0}, "phones": []}),
            "pair" => json!({"code": "AAAA-BBBB", "minutes": 5, "name": "falcon", "addresses": ["192.168.1.2:47615"]}),
            "pause" | "resume" | "lock" | "unlock" | "shutdown" | "block" | "revoke" | "approve" | "deny" => json!({"ok": true}),
            _ => json!({"error": "unknown command"}),
        }
    }

    #[test]
    fn fetch_reads_a_paused_node() {
        let (port, seen) = fake_node(node_answers);
        let (ctl, _rx) = Controller::new(port);
        let v = ctl.fetch();
        assert_eq!(v.state(), State::Paused);
        assert_eq!(model::tooltip(&v), "falcon: paused, 3 jobs served, 0 phones");
        assert_eq!(*seen.lock().unwrap(), ["status", "stats"]);
    }

    #[test]
    fn fetch_with_no_node_is_down_and_never_panics() {
        // Bind and drop to find a port nothing listens on.
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
        let (ctl, _rx) = Controller::new(port);
        let v = ctl.fetch();
        assert_eq!(v.poll, Poll::Down);
        assert!(!v.starting);
        // After a Start click the view says starting, until the node answers.
        *ctl.starting.lock().unwrap() = Some(Instant::now());
        assert!(ctl.fetch().starting);
    }

    #[test]
    fn starting_clears_when_the_node_answers() {
        let (port, _seen) = fake_node(node_answers);
        let (ctl, _rx) = Controller::new(port);
        *ctl.starting.lock().unwrap() = Some(Instant::now());
        let v = ctl.fetch();
        assert!(!v.starting);
        assert!(!ctl.starting());
    }

    #[test]
    fn clicks_send_the_matching_commands() {
        let (port, seen) = fake_node(node_answers);
        let (ctl, _rx) = Controller::new(port);
        for a in [Action::Pause, Action::Resume, Action::Lock, Action::Unlock, Action::Stop] {
            assert_eq!(ctl.perform(a), None);
        }
        assert_eq!(ctl.perform(Action::Quit), None);
        assert_eq!(*seen.lock().unwrap(), ["pause", "resume", "lock", "unlock", "shutdown"]);
    }

    #[test]
    fn pairing_comes_back_with_the_code() {
        let (port, _seen) = fake_node(node_answers);
        let (ctl, _rx) = Controller::new(port);
        let p = ctl.pairing().unwrap();
        assert_eq!(p, Pairing { code: "AAAA-BBBB".into(), minutes: 5, addresses: vec!["192.168.1.2:47615".into()] });
    }

    #[test]
    fn phone_commands_carry_the_id() {
        let (port, seen) = fake_node(node_answers);
        let (ctl, _rx) = Controller::new(port);
        for c in ["block", "revoke", "approve", "deny"] {
            assert_eq!(ctl.phone(c, "aaaaaaaaaaaaaaaa"), None);
        }
        assert_eq!(*seen.lock().unwrap(), ["block", "revoke", "approve", "deny"]);
    }

    #[test]
    fn a_failed_click_explains_itself() {
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
        let (ctl, _rx) = Controller::new(port);
        match ctl.perform(Action::Pause) {
            Some(Notice { title, .. }) => assert_eq!(title, "Could not pause the node"),
            other => panic!("{other:?}"),
        }
    }
}
