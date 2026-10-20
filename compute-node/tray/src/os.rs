//! The few things that differ by system: showing a message, and starting the node.

use std::path::PathBuf;
use std::process::{Command, Stdio};

const NODE_EXE: &str = if cfg!(windows) { "marginalia-node.exe" } else { "marginalia-node" };

/// Looks for `name` in PATH.
fn in_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|d| d.join(name)).find(|p| p.is_file())
}

/// The node program: next to this one first, then PATH, then the Windows install folder, then `~/.local/bin` (a login
/// session often does not have it in PATH).
pub fn find_node() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(NODE_EXE))).filter(|p| p.is_file());
    beside.or_else(|| in_path(NODE_EXE)).or_else(|| {
        // Where setup.ps1 installs it on Windows.
        let installed = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("MarginaliaNode").join(NODE_EXE)).filter(|p| p.is_file());
        installed.or_else(|| {
            let home = std::env::var_os("HOME").map(PathBuf::from)?;
            Some(home.join(".local/bin").join(NODE_EXE)).filter(|p| p.is_file())
        })
    })
}

/// Waits for a child in the background so it does not stay a zombie.
fn reap(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Starts the node. Returns a short sentence on success and the reason on failure.
/// Where a node keeps its settings, its port and its name, so a node that is not the installed one comes back the same.
#[derive(Clone, Debug)]
pub struct NodeHome {
    pub dir: String,
    pub port: u16,
    pub name: Option<String>,
}

/// Starts the node. `home` is given for a node that is not the installed one.
pub fn start_node(admin_port: u16, home: Option<NodeHome>) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    if admin_port == DEFAULT_ADMIN_PORT {
        // The installed user service is the proper way to run it, when it exists.
        let has_unit = Command::new("systemctl").args(["--user", "cat", "marginalia-node.service"]).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        if has_unit {
            let ok = Command::new("systemctl").args(["--user", "start", "marginalia-node"]).status().map(|s| s.success()).unwrap_or(false);
            return if ok { Ok("Started the marginalia-node service".into()) } else { Err("systemctl could not start the marginalia-node service. Try `journalctl --user -u marginalia-node`.".into()) };
        }
    }
    let exe = find_node().ok_or_else(|| format!("Could not find {NODE_EXE}. Put it next to the tray program or in your PATH."))?;
    let mut cmd = Command::new(&exe);
    if let Some(h) = &home {
        cmd.arg("--state-dir").arg(&h.dir);
    }
    cmd.arg("run");
    if admin_port != DEFAULT_ADMIN_PORT {
        cmd.arg("--admin-port").arg(admin_port.to_string());
    }
    if let Some(h) = &home {
        cmd.arg("--port").arg(h.port.to_string());
        if let Some(n) = &h.name {
            cmd.arg("--name").arg(n);
        }
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so closing a terminal does not take the node with it.
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW
        cmd.creation_flags(0x0000_0200 | 0x0000_0008 | 0x0800_0000);
    }
    let child = cmd.spawn().map_err(|e| format!("Could not start {}: {e}", exe.display()))?;
    reap(child);
    Ok(format!("Started {}", exe.display()))
}

/// `marginalia-node models list --json` for this models folder.
pub fn models_list(state_dir: &str, models_dir: &str) -> Result<serde_json::Value, String> {
    let exe = find_node().ok_or_else(|| format!("Could not find {NODE_EXE}."))?;
    let mut cmd = Command::new(&exe);
    cmd.args(["--state-dir", state_dir, "models", "list", "--json", "--dir", models_dir]).stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("Could not run {}: {e}", exe.display()))?;
    serde_json::from_slice(&out.stdout).map_err(|_| "The node program did not list its models.".to_string())
}

/// Starts a model download as its own program, so it keeps going when the panel closes. It writes its progress to `progress`.
pub fn start_download(state_dir: &str, models_dir: &str, args: &[String], progress: &std::path::Path) -> Result<(), String> {
    let exe = find_node().ok_or_else(|| format!("Could not find {NODE_EXE}."))?;
    let mut cmd = Command::new(&exe);
    cmd.args(["--state-dir", state_dir]).args(args).args(["--dir", models_dir]).env("MARGINALIA_PROGRESS_FILE", progress);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0200 | 0x0000_0008 | 0x0800_0000);
    }
    let _ = std::fs::remove_file(progress);
    let child = cmd.spawn().map_err(|e| format!("Could not start the download: {e}"))?;
    reap(child);
    Ok(())
}

/// Runs the node program with `args` to save a setting, and waits for it.
pub fn node_says(args: &[&str]) -> Result<(), String> {
    let exe = find_node().ok_or_else(|| format!("Could not find {NODE_EXE}. Put it next to the tray program or in your PATH."))?;
    let mut cmd = Command::new(&exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err("The node could not save the choice.".into()),
        Err(e) => Err(format!("Could not run {}: {e}", exe.display())),
    }
}

pub const DEFAULT_ADMIN_PORT: u16 = 47616;

/// A short message that goes away by itself.
#[cfg(target_os = "linux")]
pub fn toast(title: &str, body: &str, ms: u32) {
    match Command::new("notify-send").args(["--app-name=Marginalia node", &format!("--expire-time={ms}"), title, body]).spawn() {
        Ok(c) => reap(c),
        Err(e) => eprintln!("{title}: {body} (notify-send is not available: {e})"),
    }
}

/// A message box through PowerShell, which every supported Windows has. The text travels in
/// environment variables so no quoting can go wrong.
#[cfg(windows)]
pub fn message_box(title: &str, body: &str) {
    use std::os::windows::process::CommandExt;
    let script = "Add-Type -AssemblyName PresentationFramework; [void][System.Windows.MessageBox]::Show($env:MN_BODY, $env:MN_TITLE)";
    let spawned = Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", script]).env("MN_TITLE", title).env("MN_BODY", body).creation_flags(0x0800_0000).spawn();
    match spawned {
        Ok(c) => reap(c),
        Err(e) => eprintln!("{title}: {body} ({e})"),
    }
}

#[cfg(windows)]
pub fn toast(title: &str, body: &str, _ms: u32) {
    message_box(title, body)
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn toast(title: &str, body: &str, _ms: u32) {
    eprintln!("{title}: {body}")
}

