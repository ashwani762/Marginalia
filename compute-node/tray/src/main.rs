//! marginalia-node-tray: a small status icon for a running marginalia-node.
//!
//! On Ubuntu it appears in the top bar (through the AppIndicator extension that Ubuntu ships), on
//! Windows in the system tray. It only talks to the node's local admin channel on 127.0.0.1 and
//! shows counts and names. It never sees what a phone asked.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod admin;
mod autostart;
mod charts;
mod control;
mod icon;
mod usage;
mod model;
mod os;
mod panel;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

use clap::Parser;

#[cfg(test)]
mod single_tests {
    use super::single_instance;

    #[test]
    fn a_second_tray_for_the_same_node_does_not_start_and_the_port_is_free_again_afterwards() {
        let port = 47_981;
        let first = single_instance(port).unwrap();
        assert!(first.is_some());
        assert!(single_instance(port).is_err(), "the second one must be turned away");
        // A tray for another node is not affected.
        assert!(single_instance(port + 10).unwrap().is_some());
        drop(first);
        assert!(single_instance(port).unwrap().is_some(), "closing the first frees the place");
    }
}

#[derive(Parser)]
#[command(name = "marginalia-node-tray", version, about = "A tray icon to watch and control marginalia-node")]
struct Cli {
    /// The node's local control port (the same one as `marginalia-node --admin-port`).
    #[arg(long, default_value_t = os::DEFAULT_ADMIN_PORT)]
    admin_port: u16,
    /// Print the menu as text once and exit, without showing an icon. For checking the setup.
    #[arg(long)]
    print: bool,
    /// Show the details panel and nothing else. The tray starts this itself when the icon is clicked.
    #[arg(long)]
    panel: bool,
    /// With `--panel`: open on the Settings page.
    #[arg(long)]
    settings: bool,
    /// With `--panel`: do not close the panel when it loses focus. For taking screenshots.
    #[arg(long, hide = true)]
    keep_open: bool,
    /// Set by the sign in entry: start the node too when it is not running.
    #[arg(long = "at-login")]
    at_login: bool,
    /// Turn starting at sign in on or off, or show whether it is on, then exit.
    #[arg(long, value_name = "on|off|status")]
    startup: Option<String>,
}

/// Only one tray icon per node: the first one holds a port on this machine for as long as it runs (the node's control port plus one),
/// and a second one finds it taken and does not start. Nothing is held for the panel or `--print`. Any other problem with the
/// port is no reason to stop the tray, so it carries on.
fn single_instance(admin_port: u16) -> Result<Option<std::net::TcpListener>, ()> {
    match std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, admin_port.wrapping_add(1))) {
        Ok(l) => Ok(Some(l)),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Err(()),
        Err(_) => Ok(None),
    }
}

/// `--startup on|off|status`: for scripts and the installer. Prints one line and returns the exit code.
fn startup_command(what: &str) -> i32 {
    let result = match what {
        "on" => autostart::set(true),
        "off" => autostart::set(false),
        "status" => Ok(()),
        _ => {
            eprintln!("Say on, off or status.");
            return 2;
        }
    };
    match result {
        Ok(()) => {
            println!("Start at sign in is {}.", if autostart::enabled() { "on" } else { "off" });
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn main() {
    let cli = Cli::parse();
    if let Some(what) = &cli.startup {
        std::process::exit(startup_command(what));
    }
    // Held until the program ends.
    let _only_one = if cli.panel || cli.print {
        None
    } else {
        match single_instance(cli.admin_port) {
            Ok(l) => l,
            Err(()) => {
                eprintln!("The tray icon is already running.");
                return;
            }
        }
    };
    if cli.panel {
        panel::run(cli.admin_port, cli.keep_open, cli.settings);
        return;
    }
    let (ctl, rx) = control::Controller::new(cli.admin_port);
    if cli.print {
        let view = ctl.fetch();
        println!("{}", model::tooltip(&view));
        for e in model::menu_with_startup(&view, autostart::enabled()) {
            match e {
                model::Entry::Info(s) => println!("  {s}"),
                model::Entry::Separator => println!("  ----"),
                model::Entry::Item(s, _) => println!("[ {s} ]"),
            }
        }
        return;
    }
    if cli.at_login {
        // The system started the tray at sign in. The node comes up with it, unless it is already running (a login task
        // may have started it) or this person stopped it on purpose a moment ago.
        let ctl = ctl.clone();
        std::thread::spawn(move || ctl.start_node_if_down());
    }
    #[cfg(target_os = "linux")]
    linux::run(ctl, rx);
    #[cfg(windows)]
    windows::run(ctl, rx);
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (ctl, rx);
        eprintln!("The tray icon is built for Linux and Windows only.");
    }
}
