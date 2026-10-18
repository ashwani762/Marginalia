//! The details panel: a small frameless window that opens when the tray icon is clicked. It is its
//! own process (`marginalia-node-tray --panel`), started and stopped by the tray. It polls the node's
//! local admin channel every 1.5 seconds while it is open and shows counts, names and timings only.
//! Everything it can do goes through the same admin commands as the command line.

use crate::control::{Controller, Pairing};
use crate::model::{self, Action, Activity, Info, Phone, Poll, State, View, Waiting};
use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const WIDTH: f32 = 340.0;
const HEIGHT: f32 = 540.0;
const POLL: Duration = Duration::from_millis(1500);
const SPARK_COLUMNS: usize = 60;

const BG: Color32 = Color32::from_rgb(22, 24, 29);
const CARD: Color32 = Color32::from_rgb(32, 35, 42);
const TRACK: Color32 = Color32::from_rgb(52, 56, 66);
const TEXT: Color32 = Color32::from_rgb(232, 234, 238);
const DIM: Color32 = Color32::from_rgb(142, 148, 160);
const GREEN: Color32 = Color32::from_rgb(70, 200, 105);
const AMBER: Color32 = Color32::from_rgb(240, 165, 30);
const BLUE: Color32 = Color32::from_rgb(96, 156, 255);
const VIOLET: Color32 = Color32::from_rgb(170, 125, 230);
const GREY: Color32 = Color32::from_rgb(130, 134, 142);
const RED: Color32 = Color32::from_rgb(240, 100, 95);
const GREEN_BTN: Color32 = Color32::from_rgb(38, 120, 66);
const RED_BTN: Color32 = Color32::from_rgb(110, 42, 44);

fn state_colour(s: State) -> Color32 {
    match s {
        State::Running => GREEN,
        State::Busy => BLUE,
        State::Paused => AMBER,
        State::Locked => VIOLET,
        State::Down => RED,
    }
}

/// What the interface asks the worker thread to do. The worker talks to the node, the interface never blocks.
enum Cmd {
    Click(Action),
    /// `block`, `revoke`, `approve` or `deny` for the phone with this id.
    Phone(&'static str, String),
    Pair,
}

enum Msg {
    View(View),
    Pairing(Pairing),
    Notice(String),
}

fn worker(port: u16, ctx: egui::Context, tx: Sender<Msg>, rx: Receiver<Cmd>) {
    let (ctl, _refresh) = Controller::new(port);
    loop {
        if tx.send(Msg::View(ctl.fetch())).is_err() {
            return;
        }
        ctx.request_repaint();
        match rx.recv_timeout(POLL) {
            Ok(cmd) => {
                let msg = match cmd {
                    Cmd::Click(a) => ctl.perform(a).map(|n| Msg::Notice(format!("{}: {}", n.title, n.body))),
                    Cmd::Phone(c, who) => ctl.phone(c, &who).map(|n| Msg::Notice(format!("{}: {}", n.title, n.body))),
                    Cmd::Pair => Some(match ctl.pairing() {
                        Ok(p) => Msg::Pairing(p),
                        Err(e) => Msg::Notice(format!("Could not get a pairing code: {e}")),
                    }),
                };
                if let Some(m) = msg {
                    if tx.send(m).is_err() {
                        return;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    Block,
    Remove,
}

struct Panel {
    rx: Receiver<Msg>,
    cmds: Sender<Cmd>,
    view: Option<View>,
    activity: Activity,
    started: Instant,
    /// The code on show and when it stops being valid.
    pairing: Option<(Pairing, Instant)>,
    confirm: Option<(String, Confirm)>,
    /// "Stop node" asks once more before it does anything.
    confirm_stop: bool,
    notice: Option<(String, Instant)>,
    /// Closing on focus loss only makes sense once the window has had focus.
    was_focused: bool,
    placed: bool,
    /// Do not close when focus is lost (for taking screenshots).
    keep_open: bool,
}

impl Panel {
    fn new(port: u16, keep_open: bool, ctx: egui::Context) -> Panel {
        let (tx, rx) = channel();
        let (cmds, cmd_rx) = channel();
        std::thread::spawn(move || worker(port, ctx, tx, cmd_rx));
        Panel { rx, cmds, view: None, activity: Activity::default(), started: Instant::now(), pairing: None, confirm: None, confirm_stop: false, notice: None, was_focused: false, placed: false, keep_open }
    }

    fn send(&self, c: Cmd) {
        let _ = self.cmds.send(c);
    }

    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::View(v) => {
                    match &v.poll {
                        Poll::Up(i) => self.activity.push(self.started.elapsed().as_secs_f64(), (i.running_jobs + i.waiting_jobs) as u32, i.served),
                        Poll::Down => self.activity.clear(),
                    }
                    self.view = Some(v);
                }
                Msg::Pairing(p) => {
                    let until = Instant::now() + Duration::from_secs(p.minutes * 60);
                    self.pairing = Some((p, until));
                }
                Msg::Notice(n) => self.notice = Some((n, Instant::now())),
            }
        }
    }

    /// Escape, or losing focus, closes the panel. The first frames place it top right where the system lets us.
    fn window_behaviour(&mut self, ctx: &egui::Context) {
        let (esc, focused, monitor) = ctx.input(|i| (i.key_pressed(egui::Key::Escape), i.viewport().focused, i.viewport().monitor_size));
        if focused == Some(true) {
            self.was_focused = true;
        }
        if esc || (self.was_focused && !self.keep_open && focused == Some(false)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let (false, Some(m)) = (self.placed, monitor) {
            self.placed = true;
            // Top right under the top bar, or bottom right above the taskbar on Windows. Wayland ignores this.
            let y = if cfg!(windows) { m.y - HEIGHT - 56.0 } else { 36.0 };
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2((m.x - WIDTH - 12.0).max(0.0), y.max(0.0))));
        }
    }
}

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    card_with(ui, Stroke::NONE, add)
}

/// A rounded block of related things. `stroke` is for the ones that ask for attention.
fn card_with(ui: &mut Ui, stroke: Stroke, add: impl FnOnce(&mut Ui)) {
    Frame::NONE.fill(CARD).stroke(stroke).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(10.5).color(DIM));
}

fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(12.0).color(DIM)
}

/// Every button shows a hand when the pointer is over it, and says what it does when it rests there.
fn finish(r: egui::Response, hint: &str) -> bool {
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
    let r = if hint.is_empty() { r } else { r.on_hover_text(hint) };
    r.clicked()
}

fn small_button(ui: &mut Ui, text: &str, hint: &str) -> bool {
    finish(ui.add(egui::Button::new(RichText::new(text).size(12.0)).min_size(Vec2::new(0.0, 22.0))), hint)
}

/// A small button with a colour that says what kind of action it is.
fn tinted_button(ui: &mut Ui, text: &str, fill: Color32, hint: &str) -> bool {
    finish(ui.add(egui::Button::new(RichText::new(text).size(12.0).color(TEXT)).fill(fill).min_size(Vec2::new(0.0, 22.0))), hint)
}

fn big_button(ui: &mut Ui, text: &str, fill: Option<Color32>, hint: &str) -> bool {
    let mut b = egui::Button::new(RichText::new(text).size(12.5).color(TEXT));
    if let Some(f) = fill {
        b = b.fill(f);
    }
    finish(ui.add_sized([ui.available_width(), 28.0], b), hint)
}

fn dot(ui: &mut Ui, colour: Color32, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(radius * 2.0 + 2.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), radius, colour);
}

fn bar(ui: &mut Ui, width: f32, fraction: f32, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width.max(10.0), 8.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(4), TRACK);
    if fraction > 0.0 {
        let mut fill = rect;
        fill.set_width((rect.width() * fraction).max(8.0));
        ui.painter().rect_filled(fill, CornerRadius::same(4), colour);
    }
}

fn speed_row(ui: &mut Ui, label: &str, fraction: f32, text: &str, colour: Color32) {
    ui.horizontal(|ui| {
        ui.add_sized([52.0, 14.0], egui::Label::new(dim(label)));
        bar(ui, ui.available_width() - 112.0, fraction, colour);
        ui.label(RichText::new(text).size(12.0).color(TEXT));
    });
}

fn truncated(ui: &mut Ui, text: RichText) {
    ui.add(egui::Label::new(text).truncate());
}

/// A small cross that is drawn rather than typed, so it looks the same on every system.
fn close_button(ui: &mut Ui) {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Close (Esc)");
    if r.hovered() {
        ui.painter().circle_filled(rect.center(), 10.0, TRACK);
    }
    let c = rect.center();
    let stroke = Stroke::new(1.6, if r.hovered() { TEXT } else { DIM });
    ui.painter().line_segment([c + Vec2::new(-4.0, -4.0), c + Vec2::new(4.0, 4.0)], stroke);
    ui.painter().line_segment([c + Vec2::new(-4.0, 4.0), c + Vec2::new(4.0, -4.0)], stroke);
    if r.clicked() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn now_epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Panel {
    fn header(&mut self, ui: &mut Ui, view: &View) {
        let colour = state_colour(view.state());
        let (name, state, sub) = match &view.poll {
            Poll::Up(i) => (i.name.clone(), if i.paused { "Paused" } else { "Running" }, Some(model::status_line(i))),
            Poll::Down => ("Marginalia node".to_string(), if view.starting { "Starting" } else { "Not running" }, None),
        };
        ui.horizontal(|ui| {
            dot(ui, colour, 6.0);
            truncated(ui, RichText::new(name).size(17.0).strong().color(TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close_button(ui);
                Frame::NONE.fill(colour.gamma_multiply(0.18)).corner_radius(CornerRadius::same(9)).inner_margin(Margin::symmetric(8, 2)).show(ui, |ui| {
                    ui.label(RichText::new(state).size(11.5).strong().color(colour));
                });
            });
        });
        if let Some(sub) = sub {
            ui.label(dim(sub));
        }
    }

    fn numbers(&self, ui: &mut Ui, i: &Info) {
        card(ui, |ui| {
            ui.columns(3, |cols| {
                for (col, (value, caption)) in cols.iter_mut().zip([(i.served.to_string(), if i.served == 1 { "job served" } else { "jobs served" }), (i.phones.len().to_string(), if i.phones.len() == 1 { "phone" } else { "phones" }), (model::uptime_short(i.uptime_s), "up for")]) {
                    col.vertical_centered(|ui| {
                        ui.label(RichText::new(value).size(20.0).strong().color(TEXT));
                        ui.label(dim(caption));
                    });
                }
            });
        });
    }

    fn speed(&self, ui: &mut Ui, i: &Info) {
        card(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let mut rows = vec![
                ("Chat", model::gauge(i.chat_tok_s, model::CHAT_TOK_S_MAX), model::chat_speed_text(i.chat_tok_s), BLUE, &i.chat_model),
                ("Search", model::gauge(i.search_per_s, model::SEARCH_PER_S_MAX), model::search_speed_text(i.search_per_s), GREEN, &i.search_model),
            ];
            if i.speech_model.is_some() {
                rows.push(("Speech", model::gauge(i.speech_x, model::SPEECH_X_MAX), model::speech_speed_text(i.speech_x), AMBER, &i.speech_model));
            }
            for (label, fraction, text, colour, model) in rows {
                speed_row(ui, label, fraction, &text, colour);
                ui.horizontal(|ui| {
                    ui.add_space(58.0);
                    truncated(ui, RichText::new(model.clone().unwrap_or_else(|| "not served".into())).size(11.0).color(DIM));
                });
            }
        });
    }

    fn activity(&self, ui: &mut Ui, i: &Info) {
        let in_flight = (i.running_jobs + i.waiting_jobs) as usize;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                let t = self.started.elapsed().as_secs_f32();
                for k in 0..in_flight.min(8) {
                    // Each running or waiting job is a dot that pulses.
                    let pulse = 0.55 + 0.45 * (t * 5.0 + k as f32 * 0.8).sin();
                    dot(ui, GREEN.gamma_multiply(pulse), 4.0);
                }
                ui.label(RichText::new(model::work_label(i.running_jobs, i.waiting_jobs)).size(12.0).color(TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(format!("{} failed", i.failed)).size(12.0).color(if i.failed > 0 { RED } else { DIM }));
                });
            });
            let now = self.started.elapsed().as_secs_f64();
            let bars = self.activity.bars(SPARK_COLUMNS, now);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
            let step = rect.width() / SPARK_COLUMNS as f32;
            for (k, f) in bars.iter().enumerate() {
                let h = if *f > 0.0 { (rect.height() * f).max(3.0) } else { 1.5 };
                let x = rect.left() + k as f32 * step;
                let r = egui::Rect::from_min_max(egui::pos2(x + 0.5, rect.bottom() - h), egui::pos2(x + step - 0.5, rect.bottom()));
                ui.painter().rect_filled(r, CornerRadius::same(1), if *f > 0.0 { BLUE } else { TRACK });
            }
            let n = self.activity.finished();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{n} {} finished in the last 2 min", if n == 1 { "job" } else { "jobs" })).size(11.0).color(DIM));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("now").size(11.0).color(DIM));
                });
            });
        });
        if in_flight > 0 {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
    }

    fn asking(&mut self, ui: &mut Ui, asking: &[Waiting]) {
        if asking.is_empty() {
            return;
        }
        card_with(ui, Stroke::new(1.0, AMBER.gamma_multiply(0.7)), |ui| {
            ui.label(RichText::new(if asking.len() == 1 { "A PHONE WANTS TO JOIN" } else { "PHONES WANT TO JOIN" }).size(10.5).color(AMBER));
            for w in asking {
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if small_button(ui, "Deny", "Turn this phone down") {
                            self.send(Cmd::Phone("deny", w.id.clone()));
                        }
                        if tinted_button(ui, "Allow", GREEN_BTN, "Let this phone use the node") {
                            self.send(Cmd::Phone("approve", w.id.clone()));
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| truncated(ui, RichText::new(&w.name).size(13.0).strong().color(TEXT)));
                    });
                });
            }
        });
    }

    fn phones(&mut self, ui: &mut Ui, phones: &[Phone]) {
        card(ui, |ui| {
            heading(ui, &format!("PHONES ({})", phones.len()));
            if phones.is_empty() {
                ui.label(dim("No phones yet. Open the app on a phone, Settings, Compute nodes, or use Show pairing code."));
            }
            let now = now_epoch();
            for p in phones {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if small_button(ui, "Remove", "Forget this phone. It can pair again later.") {
                            self.confirm = Some((p.id.clone(), Confirm::Remove));
                        }
                        if finish(ui.add(egui::Button::new(RichText::new("Block").size(12.0).color(RED)).min_size(Vec2::new(0.0, 22.0))), "Turn this phone away for good, even when new phones can join") {
                            self.confirm = Some((p.id.clone(), Confirm::Block));
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| truncated(ui, RichText::new(&p.name).size(13.0).strong().color(TEXT)));
                    });
                });
                match &self.confirm {
                    Some((id, what)) if *id == p.id => {
                        let what = *what;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(if what == Confirm::Block { "Block this phone for good?" } else { "Forget this phone?" }).size(12.0).color(AMBER));
                            if tinted_button(ui, "Yes", RED_BTN, "") {
                                self.send(Cmd::Phone(if what == Confirm::Block { "block" } else { "revoke" }, p.id.clone()));
                                self.confirm = None;
                            }
                            if small_button(ui, "No", "") {
                                self.confirm = None;
                            }
                        });
                    }
                    _ => {
                        ui.label(dim(model::phone_line(p, now)));
                    }
                }
            }
        });
    }

    /// Other compute nodes this node works together with, and the ones seen on the network that it has not joined.
    /// Nothing is shown when there are none, so a node that works alone stays as quiet as before.
    fn peers(&self, ui: &mut Ui, i: &Info) {
        if i.peers.is_empty() && i.nearby.is_empty() {
            return;
        }
        card(ui, |ui| {
            if !i.peers.is_empty() {
                heading(ui, &format!("PEERS ({})", i.peers.len()));
                for p in &i.peers {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        dot(ui, if !p.reachable { RED } else if p.paused { AMBER } else { GREEN }, 4.0);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(dim(model::peer_line(p)));
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| truncated(ui, RichText::new(&p.name).size(13.0).strong().color(TEXT)));
                        });
                    });
                    if !p.addr.is_empty() {
                        ui.label(RichText::new(&p.addr).font(FontId::monospace(11.0)).color(DIM));
                    }
                }
            }
            if !i.nearby.is_empty() {
                if !i.peers.is_empty() {
                    ui.add_space(6.0);
                }
                heading(ui, &format!("SEEN ON THIS NETWORK ({})", i.nearby.len()));
                for n in &i.nearby {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        dot(ui, GREY, 4.0);
                        truncated(ui, RichText::new(&n.name).size(13.0).strong().color(TEXT));
                    });
                    if let Some(a) = &n.addr {
                        ui.label(RichText::new(a).font(FontId::monospace(11.0)).color(DIM));
                    }
                }
                ui.add_space(2.0);
                ui.label(dim("Not joined yet. Run marginalia-node pair on one of them, then marginalia-node join NAME CODE here."));
            }
        });
    }

    fn addresses(&self, ui: &mut Ui, i: &Info) {
        card(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            if i.addresses.is_empty() {
                ui.label(dim("No network address found"));
            }
            for a in &i.addresses {
                ui.horizontal(|ui| {
                    ui.add_sized([58.0, 14.0], egui::Label::new(dim("Address")));
                    ui.label(RichText::new(a).font(FontId::monospace(12.0)).color(TEXT));
                });
            }
        });
    }

    fn pairing_card(&mut self, ui: &mut Ui) {
        let Some((p, until)) = &self.pairing else { return };
        let left = until.saturating_duration_since(Instant::now()).as_secs();
        if left == 0 {
            self.pairing = None;
            return;
        }
        let (code, addresses) = (p.code.clone(), p.addresses.clone());
        let mut hide = false;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                heading(ui, "PAIRING CODE");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if small_button(ui, "Hide", "Hide the code") {
                        hide = true;
                    }
                    ui.label(RichText::new(format!("valid for {}", model::countdown(left))).size(12.0).color(AMBER));
                });
            });
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(code).font(FontId::monospace(30.0)).strong().color(TEXT));
            });
            ui.label(dim("On the phone open Settings, Compute nodes, Add a compute node, then type this code."));
            if let Some(a) = addresses.first() {
                ui.label(dim(format!("Or add it by address: {a}")));
            }
        });
        if hide {
            self.pairing = None;
        }
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }

    fn footer(&mut self, ui: &mut Ui, i: &Info) {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.columns(2, |cols| {
            let (label, hint, action) = if i.paused {
                ("Resume", "Take new jobs again", Action::Resume)
            } else {
                ("Pause", "Turn new jobs away. Jobs already running finish.", Action::Pause)
            };
            if big_button(&mut cols[0], label, None, hint) {
                self.send(Cmd::Click(action));
            }
            if big_button(&mut cols[1], "Show pairing code", None, "Show a code to type on a new phone") {
                self.send(Cmd::Pair);
            }
        });
        if self.confirm_stop {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Stop the node? Phones lose it until it is started.").size(11.5).color(AMBER));
            });
            ui.columns(2, |cols| {
                if big_button(&mut cols[0], "Yes, stop it", Some(RED_BTN), "") {
                    self.send(Cmd::Click(Action::Stop));
                    self.confirm_stop = false;
                }
                if big_button(&mut cols[1], "Cancel", None, "") {
                    self.confirm_stop = false;
                }
            });
        } else {
            ui.columns(2, |cols| {
                let (label, hint, action) = if i.locked {
                    ("Unlock", "Let any phone on the network join again", Action::Unlock)
                } else {
                    ("Lock new phones", "New phones must be allowed before they can join", Action::Lock)
                };
                if big_button(&mut cols[0], label, None, hint) {
                    self.send(Cmd::Click(action));
                }
                if big_button(&mut cols[1], "Stop node", Some(RED_BTN), "Finish running jobs and shut the node down") {
                    self.confirm_stop = true;
                }
            });
        }
    }

    fn empty_state(&mut self, ui: &mut Ui, starting: bool) {
        ui.add_space(70.0);
        ui.vertical_centered(|ui| {
            dot(ui, RED, 16.0);
            ui.add_space(8.0);
            ui.label(RichText::new(if starting { "Starting the node" } else { "The node is not running" }).size(17.0).strong().color(TEXT));
            ui.add_space(4.0);
            ui.label(dim(if starting { "It is loading its models. This can take a minute." } else { "Your phone uses its own speed until it is back." }));
            ui.add_space(14.0);
            if !starting && finish(ui.add(egui::Button::new(RichText::new("Start node").size(14.0).color(TEXT)).fill(GREEN_BTN).min_size(Vec2::new(150.0, 34.0))), "Start marginalia-node on this machine") {
                self.send(Cmd::Click(Action::Start));
            }
        });
    }
}

impl eframe::App for Panel {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain();
        self.window_behaviour(&ctx);
        if self.notice.as_ref().map(|(_, t)| t.elapsed() > Duration::from_secs(8)).unwrap_or(false) {
            self.notice = None;
        }
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, CornerRadius::same(14), BG);
        ui.painter().rect_stroke(rect.shrink(0.5), CornerRadius::same(14), Stroke::new(1.0, TRACK), StrokeKind::Inside);

        let view = self.view.clone();
        let info = match &view {
            Some(View { poll: Poll::Up(i), .. }) => Some((**i).clone()),
            _ => None,
        };
        let edge = Margin { left: 12, right: 12, top: 4, bottom: 10 };
        if let Some(i) = &info {
            egui::Panel::bottom("footer").frame(Frame::NONE.inner_margin(edge)).show(ui, |ui| {
                if let Some((n, _)) = &self.notice {
                    ui.label(RichText::new(n).size(12.0).color(RED));
                }
                self.footer(ui, i);
            });
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            Frame::NONE.inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 2 }).show(ui, |ui| {
                match &view {
                    None => {
                        ui.label(dim("Looking for the node..."));
                    }
                    Some(v) => self.header(ui, v),
                }
            });
            match (&view, &info) {
                (Some(v), None) => {
                    Frame::NONE.inner_margin(Margin::symmetric(12, 0)).show(ui, |ui| {
                        self.empty_state(ui, v.starting);
                        if let Some((n, _)) = &self.notice {
                            ui.add_space(12.0);
                            ui.label(RichText::new(n).size(12.0).color(RED));
                        }
                    });
                }
                (_, Some(i)) => {
                    let out = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        Frame::NONE.inner_margin(Margin::symmetric(12, 4)).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 6.0;
                            self.pairing_card(ui);
                            self.asking(ui, &i.asking);
                            self.numbers(ui, i);
                            self.speed(ui, i);
                            self.activity(ui, i);
                            self.phones(ui, &i.phones);
                            self.peers(ui, i);
                            self.addresses(ui, i);
                        });
                    });
                    // A soft fade at the bottom says there is more to scroll to.
                    let more = out.state.clamped_offset().y + out.inner_rect.height() < out.content_size.y - 2.0;
                    if more {
                        let r = out.inner_rect;
                        for k in 0..10 {
                            let strip = egui::Rect::from_min_max(egui::pos2(r.left(), r.bottom() - (10 - k) as f32 * 2.0), egui::pos2(r.right(), r.bottom() - (9 - k) as f32 * 2.0));
                            ui.painter().rect_filled(strip, 0.0, BG.gamma_multiply((k + 1) as f32 / 12.0));
                        }
                    }
                }
                (None, None) => {}
            };
        });
    }
}

pub fn run(port: u16, keep_open: bool) {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Marginalia node")
        .with_app_id("marginalia-node-tray")
        .with_inner_size([WIDTH, HEIGHT])
        .with_resizable(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_taskbar(false);
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let result = eframe::run_native(
        "Marginalia node",
        options,
        Box::new(move |cc| {
            let mut style = (*cc.egui_ctx.global_style()).clone();
            style.visuals = egui::Visuals::dark();
            style.visuals.widgets.inactive.weak_bg_fill = TRACK;
            style.visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
            style.visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
            style.visuals.widgets.active.corner_radius = CornerRadius::same(6);
            style.spacing.item_spacing = Vec2::new(6.0, 6.0);
            cc.egui_ctx.set_global_style(style);
            Ok(Box::new(Panel::new(port, keep_open, cc.egui_ctx.clone())))
        }),
    );
    if let Err(e) = result {
        eprintln!("Could not open the details panel: {e}");
    }
}
