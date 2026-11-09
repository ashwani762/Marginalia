//! Starting the tray when someone signs in. The tray then starts the node too, when it is not running (`--at-login`).
//!
//! One small interface for every system: `enabled` and `set`. Windows keeps an entry in the current user's Run key (no
//! administrator needed), Linux a desktop entry in `~/.config/autostart`. What goes into either is built by plain functions
//! that the tests check, so nothing here needs a desktop to be tried.

/// The flag the entry passes, so the tray knows it was started by the system and not by hand.
pub const AT_LOGIN: &str = "--at-login";

/// The name the entry is kept under.
const NAME: &str = "Marginalia Node";

/// The command that starts the tray at sign in: the program in quotes, then the flag.
pub fn command_line(exe: &std::path::Path) -> String {
    format!("\"{}\" {AT_LOGIN}", exe.display())
}

/// The Linux desktop entry. A `.desktop` file does not expand `~` and wants a space in the path escaped by quotes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn desktop_entry(exe: &std::path::Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={NAME}\nComment=Shows the compute node in the top bar and starts it\nExec={}\nTerminal=false\nIcon=network-server\nCategories=Utility;\nX-GNOME-Autostart-enabled=true\nX-GNOME-Autostart-Delay=3\n",
        command_line(exe)
    )
}

/// True when the entry exists and starts this very program. An entry that starts another copy (a build folder,
/// an older install) does not count: the switch would show on while this program is not what starts.
pub fn points_at(entry: &str, exe: &std::path::Path) -> bool {
    let want = exe.display().to_string().to_lowercase();
    entry.to_lowercase().contains(&want)
}

#[cfg(windows)]
mod system {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    pub fn read() -> Option<String> {
        let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN, KEY_READ).ok()?;
        key.get_value::<String, _>(super::NAME).ok()
    }

    pub fn write(exe: &std::path::Path) -> Result<(), String> {
        let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN, KEY_WRITE).map_err(|e| format!("Could not open the Windows startup list: {e}"))?;
        key.set_value(super::NAME, &super::command_line(exe)).map_err(|e| format!("Could not add the startup entry: {e}"))
    }

    pub fn remove() -> Result<(), String> {
        let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN, KEY_WRITE).map_err(|e| format!("Could not open the Windows startup list: {e}"))?;
        match key.delete_value(super::NAME) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Could not remove the startup entry: {e}")),
        }
    }
}

#[cfg(not(windows))]
mod system {
    use std::path::PathBuf;

    fn file() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("autostart").join("marginalia-node-tray.desktop"))
    }

    pub fn read() -> Option<String> {
        std::fs::read_to_string(file()?).ok()
    }

    pub fn write(exe: &std::path::Path) -> Result<(), String> {
        let path = file().ok_or("Could not find your settings folder.")?;
        std::fs::create_dir_all(path.parent().unwrap_or(&path)).map_err(|e| format!("Could not make the startup folder: {e}"))?;
        std::fs::write(&path, super::desktop_entry(exe)).map_err(|e| format!("Could not write the startup entry: {e}"))
    }

    pub fn remove() -> Result<(), String> {
        let Some(path) = file() else { return Ok(()) };
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Could not remove the startup entry: {e}")),
        }
    }
}

/// Whether the tray starts when this person signs in.
pub fn enabled() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    system::read().is_some_and(|entry| points_at(&entry, &exe))
}

/// Turns starting at sign in on or off. Turning it on points the entry at this program.
pub fn set(on: bool) -> Result<(), String> {
    if !on {
        return system::remove();
    }
    let exe = std::env::current_exe().map_err(|e| format!("Could not find this program: {e}"))?;
    system::write(&exe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_command_quotes_the_program_and_adds_the_flag() {
        let c = command_line(Path::new(r"C:\Users\Some One\AppData\Local\MarginaliaNode\marginalia-node-tray.exe"));
        assert_eq!(c, r#""C:\Users\Some One\AppData\Local\MarginaliaNode\marginalia-node-tray.exe" --at-login"#);
    }

    #[test]
    fn the_linux_entry_runs_the_same_command_and_has_no_stray_text() {
        let e = desktop_entry(Path::new("/home/a b/.local/bin/marginalia-node-tray"));
        assert!(e.starts_with("[Desktop Entry]\nType=Application\n"));
        assert!(e.contains("Exec=\"/home/a b/.local/bin/marginalia-node-tray\" --at-login\n"));
        assert!(!e.contains(';') || e.contains("Categories=Utility;"), "only the category list may hold a semicolon");
    }

    #[test]
    fn an_entry_for_another_copy_does_not_count() {
        let here = Path::new(r"C:\Apps\MarginaliaNode\marginalia-node-tray.exe");
        assert!(points_at(&command_line(here), here));
        assert!(points_at(&command_line(Path::new(r"c:\apps\marginalianode\MARGINALIA-NODE-TRAY.EXE")), here), "case does not matter on Windows");
        assert!(!points_at(&command_line(Path::new(r"D:\build\target\release\marginalia-node-tray.exe")), here));
        assert!(!points_at("", here));
    }
}

/// Linux only: the real file, written into a folder of its own so nothing of the person's is touched.
#[cfg(all(test, target_os = "linux"))]
mod linux_file_tests {
    #[test]
    fn writing_then_reading_then_removing() {
        let dir = std::env::temp_dir().join(format!("mn-autostart-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let exe = std::env::current_exe().unwrap();
        assert!(!super::enabled());
        super::set(true).unwrap();
        assert!(dir.join("autostart/marginalia-node-tray.desktop").is_file());
        assert!(super::enabled(), "an entry for this program counts");
        super::set(false).unwrap();
        assert!(!super::enabled());
        super::set(false).unwrap();
        let _ = exe;
        std::fs::remove_dir_all(&dir).ok();
    }
}
