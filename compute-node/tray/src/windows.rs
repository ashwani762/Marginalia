//! The Windows icon layer: a system tray icon (the `tray-icon` crate) driven by a `winit` event loop,
//! which supplies the message loop Windows needs. The logic is in `model` and `control`.
//!
//! NOTE: this file could not be compiled where it was written (no Windows target installed).

use crate::control::{spawn_poller, Controller};
use crate::icon;
use crate::model::{Action, Entry, View};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

/// A single ampersand marks a keyboard shortcut in a menu label, so real ones are doubled.
fn label(s: &str) -> String {
    s.replace('&', "&&")
}

fn icon_for(view: &View) -> Option<Icon> {
    Icon::from_rgba(icon::rgba(view.state(), 32), 32, 32).ok()
}

struct App {
    ctl: Arc<Controller>,
    views: Receiver<View>,
    tray: Option<TrayIcon>,
    shown: Option<View>,
    actions: HashMap<String, Action>,
}

impl App {
    /// Builds the menu for `view` and returns what each clickable entry does, by menu id.
    fn menu(view: &View) -> (Menu, HashMap<String, Action>) {
        let menu = Menu::new();
        let mut actions = HashMap::new();
        for (n, e) in crate::model::menu_with_chat_switch(view).into_iter().enumerate() {
            let _ = match e {
                Entry::Separator => menu.append(&PredefinedMenuItem::separator()),
                Entry::Info(text) => menu.append(&MenuItem::new(label(&text), false, None)),
                Entry::Item(text, action) => {
                    let id = format!("item-{n}");
                    actions.insert(id.clone(), action);
                    menu.append(&MenuItem::with_id(id, label(&text), true, None))
                }
            };
        }
        (menu, actions)
    }

    fn show(&mut self, view: View) {
        if self.shown.as_ref() == Some(&view) {
            return;
        }
        let (menu, actions) = Self::menu(&view);
        self.actions = actions;
        match &self.tray {
            Some(t) => {
                let _ = t.set_icon(icon_for(&view));
                let _ = t.set_tooltip(Some(crate::model::tooltip(&view)));
                t.set_menu(Some(Box::new(menu)));
            }
            None => {
                let mut b = TrayIconBuilder::new().with_menu(Box::new(menu)).with_menu_on_left_click(false).with_tooltip(crate::model::tooltip(&view));
                if let Some(i) = icon_for(&view) {
                    b = b.with_icon(i);
                }
                self.tray = b.build().ok();
            }
        }
        self.shown = Some(view);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        while let Ok(view) = self.views.try_recv() {
            self.show(view);
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            match self.actions.get(&ev.id().0).copied() {
                Some(Action::Quit) => {
                    self.tray = None;
                    event_loop.exit();
                    return;
                }
                Some(a) => self.ctl.click(a),
                None => {}
            }
        }
        // A left click on the icon opens or closes the details panel. The right click shows the menu.
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                self.ctl.toggle_panel();
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(250)));
    }
}

pub fn run(ctl: Arc<Controller>, rx: Receiver<()>) {
    let (tx, views) = channel();
    spawn_poller(ctl.clone(), rx, move |view| {
        let _ = tx.send(view);
    });
    let Ok(event_loop) = EventLoop::new() else { return };
    let mut app = App { ctl, views, tray: None, shown: None, actions: HashMap::new() };
    let _ = event_loop.run_app(&mut app);
}
