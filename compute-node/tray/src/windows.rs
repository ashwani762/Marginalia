//! The Windows icon layer: a system tray icon (the `tray-icon` crate) driven by a `winit` event loop,
//! which supplies the message loop Windows needs. The logic is in `model` and `control`.
//!
//! A right click opens the menu window (`popover.rs`) beside the icon. With `--native-menu` the system's own menu is used instead.

use crate::control::{spawn_poller, Controller};
use crate::glyph;
use crate::menuspec;
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

/// The icon drawn at the size the screen wants: 16 pixels at 100 percent, more on a sharper screen, so it is never stretched.
fn icon_for(view: &View, scale: f64) -> Option<Icon> {
    let size = (16.0 * scale).round().clamp(16.0, 64.0) as u32;
    Icon::from_rgba(glyph::rgba(view.state(), size), size, size).ok()
}

struct App {
    ctl: Arc<Controller>,
    views: Receiver<View>,
    tray: Option<TrayIcon>,
    shown: Option<View>,
    /// Whether the menu on show was built with starting at sign in on, so a click on that entry rebuilds it.
    shown_startup: bool,
    /// The screen's scale (1.0 at 100 percent), as the icon's size follows it.
    scale: f64,
    /// Use the system's own menu instead of the menu window.
    native: bool,
    actions: HashMap<String, Action>,
}

impl App {
    /// Builds the menu for `view` and returns what each clickable entry does, by menu id.
    fn menu(view: &View) -> (Menu, HashMap<String, Action>) {
        let menu = Menu::new();
        let mut actions = HashMap::new();
        for (n, e) in crate::model::menu_with_startup(view, crate::autostart::enabled()).into_iter().enumerate() {
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

    /// Opens the menu window beside the icon, on the screen the icon is on, on the side of it that has room.
    fn open_menu(&mut self, event_loop: &ActiveEventLoop, icon: tray_icon::Rect) {
        let Some(view) = self.shown.clone() else { return };
        let mut spec = self.ctl.menu_spec(&view);
        let icon_px = menuspec::Rect { x: icon.position.x as f32, y: icon.position.y as f32, w: icon.size.width as f32, h: icon.size.height as f32 };
        let (cx, cy) = (icon_px.x + icon_px.w / 2.0, icon_px.y + icon_px.h / 2.0);
        // The screen that holds the icon, else the main one.
        let monitor = event_loop
            .available_monitors()
            .find(|m| {
                let (p, s) = (m.position(), m.size());
                cx >= p.x as f32 && cx < (p.x + s.width as i32) as f32 && cy >= p.y as f32 && cy < (p.y + s.height as i32) as f32
            })
            .or_else(|| event_loop.primary_monitor());
        let (screen, scale) = match monitor {
            Some(m) => (menuspec::Rect { x: m.position().x as f32, y: m.position().y as f32, w: m.size().width as f32, h: m.size().height as f32 }, m.scale_factor() as f32),
            None => (menuspec::Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }, self.scale as f32),
        };
        let at = menuspec::place(icon_px, screen, menuspec::window_size(&spec), scale);
        spec.rise = at.rise;
        if let Err(e) = self.ctl.toggle_menu(&spec, (at.x, at.y)) {
            crate::os::toast("Could not open the menu", &e, 6000);
        }
    }

    fn show(&mut self, view: View) {
        let startup = crate::autostart::enabled();
        if self.shown.as_ref() == Some(&view) && self.shown_startup == startup {
            return;
        }
        self.shown_startup = startup;
        let (menu, actions) = Self::menu(&view);
        self.actions = actions;
        match &self.tray {
            Some(t) => {
                let _ = t.set_icon(icon_for(&view, self.scale));
                let _ = t.set_tooltip(Some(crate::model::tooltip(&view)));
                if self.native {
                    t.set_menu(Some(Box::new(menu)));
                }
            }
            None => {
                let mut b = TrayIconBuilder::new().with_menu_on_left_click(false).with_tooltip(crate::model::tooltip(&view));
                if self.native {
                    b = b.with_menu(Box::new(menu));
                }
                if let Some(i) = icon_for(&view, self.scale) {
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
        if let Some(m) = event_loop.primary_monitor() {
            let scale = m.scale_factor();
            if (scale - self.scale).abs() > 0.01 {
                // The screen's scale changed (or is known now): draw the icon again at the right size.
                self.scale = scale;
                self.shown = None;
            }
        }
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
            match ev {
                TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => self.ctl.toggle_panel(),
                TrayIconEvent::Click { button: MouseButton::Right, button_state: MouseButtonState::Up, rect, .. } if !self.native => self.open_menu(event_loop, rect),
                _ => {}
            }
        }
        if self.ctl.quit_requested() {
            self.tray = None;
            event_loop.exit();
            return;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(250)));
    }
}

pub fn run(ctl: Arc<Controller>, rx: Receiver<()>, native: bool) {
    let (tx, views) = channel();
    spawn_poller(ctl.clone(), rx, move |view| {
        let _ = tx.send(view);
    });
    let Ok(event_loop) = EventLoop::new() else { return };
    let mut app = App { ctl, views, tray: None, shown: None, shown_startup: false, scale: 1.0, native, actions: HashMap::new() };
    let _ = event_loop.run_app(&mut app);
}
