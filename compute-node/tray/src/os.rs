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

/// Brings the window with this title to the front, restored if it was minimised. Returns false when there is no such window or
/// the system has no way to do it, and the caller does something else (the details window is closed and opened again).
pub fn focus_window(title: &str) -> bool {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        extern "system" {
            fn FindWindowW(class: *const u16, title: *const u16) -> isize;
            fn ShowWindow(hwnd: isize, command: i32) -> i32;
            fn SetForegroundWindow(hwnd: isize) -> i32;
        }
        let wide: Vec<u16> = title.encode_utf16().chain([0]).collect();
        let hwnd = unsafe { FindWindowW(std::ptr::null(), wide.as_ptr()) };
        if hwnd == 0 {
            return false;
        }
        // SW_RESTORE, so a minimised window comes back as it was.
        unsafe {
            ShowWindow(hwnd, 9);
            SetForegroundWindow(hwnd);
        }
        return true;
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        false
    }
}

/// Asks Windows 11 to put its blurred material behind the window, so the sidebar can be see-through as a sidebar is. Returns Some(true)
/// when the material is on, Some(false) when this system has none, and None when the window does not exist yet (try again next frame).
pub fn apply_backdrop(title: &str) -> Option<bool> {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct Margins {
            left: i32,
            right: i32,
            top: i32,
            bottom: i32,
        }
        #[link(name = "user32")]
        extern "system" {
            fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        }
        #[link(name = "dwmapi")]
        extern "system" {
            fn DwmSetWindowAttribute(hwnd: isize, attribute: u32, value: *const core::ffi::c_void, size: u32) -> i32;
            fn DwmExtendFrameIntoClientArea(hwnd: isize, margins: *const Margins) -> i32;
        }
        let wide: Vec<u16> = title.encode_utf16().chain([0]).collect();
        let hwnd = unsafe { FindWindowW(std::ptr::null(), wide.as_ptr()) };
        if hwnd == 0 {
            return None;
        }
        let set = |attribute: u32, value: u32| unsafe { DwmSetWindowAttribute(hwnd, attribute, &value as *const u32 as *const core::ffi::c_void, 4) };
        // DWMWA_USE_IMMERSIVE_DARK_MODE, so the material and the title bar are the dark kind.
        set(20, 1);
        // DWMWA_WINDOW_CORNER_PREFERENCE round, and DWMWA_BORDER_COLOR a hairline: the window draws its own title bar, so these are its edge.
        set(33, 2);
        set(34, 0x0046_4242);
        // DWMWA_SYSTEMBACKDROP_TYPE with DWMSBT_TRANSIENTWINDOW (acrylic). Older Windows says no, and the sidebar stays solid.
        if set(38, 3) != 0 {
            return Some(false);
        }
        let all = Margins { left: -1, right: -1, top: -1, bottom: -1 };
        unsafe { DwmExtendFrameIntoClientArea(hwnd, &all) };
        return Some(true);
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        Some(false)
    }
}

/// Lets the program with this id take the keyboard focus even though it was started by one that is in the background (the tray
/// is, until its icon is clicked). Without it a menu window can open without focus and then not know when it is clicked away
/// from. Other systems have no such rule.
pub fn allow_foreground(pid: u32) {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        extern "system" {
            fn AllowSetForegroundWindow(process_id: u32) -> i32;
        }
        unsafe { AllowSetForegroundWindow(pid) };
    }
    #[cfg(not(windows))]
    let _ = pid;
}

/// Windows 11 draws a thin border round every window without a frame, which shows as a square box behind a card with rounded
/// corners. This turns it off for the window with this title (it is found by the title: the menu and the panel have their own).
/// Returns true when the window was found, so the caller can try again on the next frame until it is. Other systems have no such
/// border and nothing to do.
pub fn remove_window_border(title: &str) -> bool {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        extern "system" {
            fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        }
        #[link(name = "dwmapi")]
        extern "system" {
            fn DwmSetWindowAttribute(hwnd: isize, attribute: u32, value: *const core::ffi::c_void, size: u32) -> i32;
        }
        let wide: Vec<u16> = title.encode_utf16().chain([0]).collect();
        let hwnd = unsafe { FindWindowW(std::ptr::null(), wide.as_ptr()) };
        if hwnd == 0 {
            return false;
        }
        // DWMWA_BORDER_COLOR with DWMWA_COLOR_NONE. Windows 10 does not know it and says so, which is fine: it has no border.
        let none: u32 = 0xFFFF_FFFE;
        unsafe { DwmSetWindowAttribute(hwnd, 34, &none as *const u32 as *const core::ffi::c_void, 4) };
        // DWMWA_WINDOW_CORNER_PREFERENCE with DWMWCP_DONOTROUND: the card has its own corners and the frame has none.
        let square: u32 = 1;
        unsafe { DwmSetWindowAttribute(hwnd, 33, &square as *const u32 as *const core::ffi::c_void, 4) };
        // DWMWA_NCRENDERING_POLICY with DWMNCRP_DISABLED: no frame is drawn at all, which takes the hairline off the top edge.
        let disabled: u32 = 1;
        unsafe { DwmSetWindowAttribute(hwnd, 2, &disabled as *const u32 as *const core::ffi::c_void, 4) };
        // Windows still leaves a one pixel line along the top edge of such a window. A region that starts one row down cuts it off.
        #[link(name = "gdi32")]
        extern "system" {
            fn CreateRectRgn(left: i32, top: i32, right: i32, bottom: i32) -> isize;
        }
        #[link(name = "user32")]
        extern "system" {
            fn SetWindowRgn(hwnd: isize, region: isize, redraw: i32) -> i32;
        }
        // Larger than any window: the window's own edges clip it. The system owns the region once it is set.
        unsafe { SetWindowRgn(hwnd, CreateRectRgn(0, 1, 16384, 16384), 1) };
        return true;
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        true
    }
}

/// True when the person has asked for less motion: the menu then lands where it goes with no springs. On Windows that is
/// "Show animations in Windows" turned off. `MARGINALIA_REDUCE_MOTION=1` asks for it on any system. Linux can read GNOME's
/// `enable-animations` setting here too: nothing else in the menu needs to change for it.
pub fn reduced_motion() -> bool {
    if std::env::var_os("MARGINALIA_REDUCE_MOTION").is_some_and(|v| v != "0") {
        return true;
    }
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        extern "system" {
            fn SystemParametersInfoW(action: u32, param: u32, value: *mut core::ffi::c_void, win_ini: u32) -> i32;
        }
        // SPI_GETCLIENTAREAANIMATION: 0 when animations are off.
        let mut on: i32 = 1;
        let ok = unsafe { SystemParametersInfoW(0x1042, 0, &mut on as *mut i32 as *mut core::ffi::c_void, 0) };
        if ok != 0 && on == 0 {
            return true;
        }
    }
    false
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

