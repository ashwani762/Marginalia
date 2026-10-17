//! marginalia-node-tray: a small status icon for a running marginalia-node.
//!
//! On Ubuntu it appears in the top bar (through the AppIndicator extension that Ubuntu ships), on
//! Windows in the system tray. It only talks to the node's local admin channel on 127.0.0.1 and
//! shows counts and names. It never sees what a phone asked.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod admin;
mod control;
mod icon;
mod model;
mod os;
mod panel;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

use clap::Parser;

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
    /// With `--panel`: do not close the panel when it loses focus. For taking screenshots.
    #[arg(long, hide = true)]
    keep_open: bool,
}

fn main() {
    let cli = Cli::parse();
    if cli.panel {
        panel::run(cli.admin_port, cli.keep_open);
        return;
    }
    let (ctl, rx) = control::Controller::new(cli.admin_port);
    if cli.print {
        let view = ctl.fetch();
        println!("{}", model::tooltip(&view));
        for e in model::menu(&view) {
            match e {
                model::Entry::Info(s) => println!("  {s}"),
                model::Entry::Separator => println!("  ----"),
                model::Entry::Item(s, _) => println!("[ {s} ]"),
            }
        }
        return;
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
