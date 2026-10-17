//! The Linux icon layer: a StatusNotifierItem over D-Bus (the `ksni` crate, no GTK). GNOME on Ubuntu
//! shows it in the top bar through the AppIndicator extension. The logic is in `model` and `control`.
//! A left click opens the details panel, a right click shows the short menu.

use crate::control::{spawn_poller, Controller};
use crate::icon;
use crate::model::{Action, Entry, View};
use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

struct NodeTray {
    view: View,
    ctl: Arc<Controller>,
}

/// A single underscore marks a keyboard shortcut in a menu label, so real ones are doubled.
fn label(s: &str) -> String {
    s.replace('_', "__")
}

fn pixmaps(view: &View) -> Vec<ksni::Icon> {
    [24, 48].into_iter().map(|size| ksni::Icon { width: size as i32, height: size as i32, data: icon::argb(view.state(), size) }).collect()
}

impl ksni::Tray for NodeTray {
    fn id(&self) -> String {
        "marginalia-node-tray".into()
    }

    fn title(&self) -> String {
        "Marginalia node".into()
    }

    /// A left click on the icon opens or closes the details panel.
    fn activate(&mut self, _x: i32, _y: i32) {
        self.ctl.toggle_panel();
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        pixmaps(&self.view)
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: crate::model::tooltip(&self.view), icon_pixmap: pixmaps(&self.view), ..Default::default() }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        crate::model::menu(&self.view)
            .into_iter()
            .map(|e| match e {
                Entry::Separator => MenuItem::Separator,
                Entry::Info(text) => StandardItem { label: label(&text), enabled: false, ..Default::default() }.into(),
                Entry::Item(text, action) => {
                    let ctl = self.ctl.clone();
                    StandardItem {
                        label: label(&text),
                        activate: Box::new(move |_| match action {
                            Action::Quit => std::process::exit(0),
                            a => ctl.click(a),
                        }),
                        ..Default::default()
                    }
                    .into()
                }
            })
            .collect()
    }
}

pub fn run(ctl: Arc<Controller>, rx: Receiver<()>) {
    let first = ctl.fetch();
    // `assume_sni_available` keeps going when the top bar extension is not up yet (at login, for
    // example): the icon appears as soon as it is.
    let handle = match (NodeTray { view: first.clone(), ctl: ctl.clone() }).assume_sni_available(true).spawn() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Could not show the tray icon: {e}. On Ubuntu check that the AppIndicator extension is on.");
            std::process::exit(1);
        }
    };
    let mut last = first;
    spawn_poller(ctl, rx, move |view| {
        if view != last {
            last = view.clone();
            let _ = handle.update(|t| t.view = view);
        }
    });
    loop {
        std::thread::park();
    }
}
