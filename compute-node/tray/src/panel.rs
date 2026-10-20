//! The details panel: a small frameless window that opens when the tray icon is clicked. It is its
//! own process (`marginalia-node-tray --panel`), started and stopped by the tray. It polls the node's
//! local admin channel every 1.5 seconds while it is open and shows counts, names and timings only.
//! Everything it can do goes through the same admin commands as the command line.

use crate::charts::{self, Line};
use crate::control::{Controller, Pairing};
use crate::model::{self, Action, Activity, Download, Info, ModelItem, Phone, Poll, RunDraft, State, View, Waiting};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use crate::usage::{self, History, Load, Reading};
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
const TEAL: Color32 = Color32::from_rgb(70, 200, 200);
const ORANGE: Color32 = Color32::from_rgb(255, 150, 80);
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
    /// Save these `marginalia-node` command lines, then restart the node.
    Apply(Vec<Vec<String>>),
    /// Start `marginalia-node` with these download arguments.
    Download(Vec<String>),
}

enum Msg {
    View(View),
    /// The models on disk and the download in progress, if any.
    Models(Vec<ModelItem>, Option<Download>),
    /// New readings of processor, memory, graphics card and network, and the name of the graphics card.
    Usage(Vec<Reading>, Option<String>),
    Pairing(Pairing),
    Notice(String),
}

/// Asks the node for its readings once a second while the window is open: the last 2.5 minutes at first, then only the newest few.
/// A node that does not know the question (an older one) or does not answer is asked again every few seconds.
fn usage_worker(port: u16, ctx: egui::Context, tx: Sender<Msg>) {
    let mut first = true;
    loop {
        let n = if first { 150 } else { 8 };
        match crate::admin::ask_with(port, &serde_json::json!({"cmd": "resources", "n": n})) {
            Ok(v) => {
                let list: Vec<Reading> = v["samples"].as_array().into_iter().flatten().filter_map(Reading::from_json).collect();
                first = false;
                if tx.send(Msg::Usage(list, v["gpu_name"].as_str().map(String::from))).is_err() {
                    return;
                }
                ctx.request_repaint();
                std::thread::sleep(Duration::from_millis(1000));
            }
            Err(_) => {
                first = true;
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
}

/// Where a download keeps its progress: one line of JSON in the node's state folder.
fn progress_path(state_dir: &str) -> std::path::PathBuf {
    std::path::Path::new(state_dir).join("download.json")
}

/// While the Settings page shows, lists the models on disk and reads the download progress every 1.5 seconds.
fn models_worker(ctx: egui::Context, tx: Sender<Msg>, visible: Arc<AtomicBool>, home: Arc<Mutex<Option<(String, String)>>>) {
    loop {
        std::thread::sleep(Duration::from_millis(1500));
        if !visible.load(Ordering::Relaxed) {
            continue;
        }
        let Some((state, models)) = home.lock().ok().and_then(|h| h.clone()) else { continue };
        let Ok(list) = crate::os::models_list(&state, &models) else { continue };
        let path = progress_path(&state);
        let dl = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| {
            let age = std::fs::metadata(&path).ok()?.modified().ok()?.elapsed().map(|d| d.as_secs()).unwrap_or(0);
            model::download_state(&v, age, 90)
        });
        if tx.send(Msg::Models(model::model_items(&list), dl)).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

fn worker(port: u16, ctx: egui::Context, tx: Sender<Msg>, rx: Receiver<Cmd>, home: Arc<Mutex<Option<(String, String)>>>) {
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
                    Cmd::Apply(cmds) => ctl.apply(&cmds).map(|n| Msg::Notice(format!("{}: {}", n.title, n.body))),
                    Cmd::Download(args) => match home.lock().ok().and_then(|h| h.clone()) {
                        Some((state, models)) => crate::os::start_download(&state, &models, &args, &progress_path(&state)).err().map(|e| Msg::Notice(format!("Could not download: {e}"))),
                        None => Some(Msg::Notice("Could not download: the node did not say where its models are.".into())),
                    },
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
    history: History,
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
    /// The Settings page is showing instead of the overview.
    settings: bool,
    /// What the Settings page is editing, filled from the node the first time it is shown.
    draft: Option<(RunDraft, String)>,
    /// Saved and waiting for the node to come back.
    applying: bool,
    /// While applying: the node has gone down for its restart.
    applying_down: bool,
    /// The Models section: what is on disk and the download in progress.
    models: Option<(Vec<ModelItem>, Option<Download>)>,
    /// The item a download was started for, and whether the node needs a restart to use it.
    fetching: Option<(String, bool)>,
    settings_visible: Arc<AtomicBool>,
    home: Arc<Mutex<Option<(String, String)>>>,
}

impl Panel {
    fn new(port: u16, keep_open: bool, settings: bool, ctx: egui::Context) -> Panel {
        let (tx, rx) = channel();
        let (cmds, cmd_rx) = channel();
        {
            let (ctx, tx) = (ctx.clone(), tx.clone());
            std::thread::spawn(move || usage_worker(port, ctx, tx));
        }
        let settings_visible = Arc::new(AtomicBool::new(settings));
        let home: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
        {
            let (ctx, tx, v, h) = (ctx.clone(), tx.clone(), settings_visible.clone(), home.clone());
            std::thread::spawn(move || models_worker(ctx, tx, v, h));
        }
        {
            let h = home.clone();
            std::thread::spawn(move || worker(port, ctx, tx, cmd_rx, h));
        }
        Panel { rx, cmds, view: None, history: History::default(), activity: Activity::default(), started: Instant::now(), pairing: None, confirm: None, confirm_stop: false, notice: None, was_focused: false, placed: false, keep_open, settings, draft: None, applying: false, applying_down: false, models: None, fetching: None, settings_visible, home }
    }

    fn send(&self, c: Cmd) {
        let _ = self.cmds.send(c);
    }

    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::View(v) => {
                    // Settings were saved: the node goes down and comes back with them.
                    match (&v.poll, self.applying) {
                        (Poll::Down, true) => self.applying_down = true,
                        (Poll::Up(_), true) if self.applying_down => {
                            self.applying = false;
                            self.applying_down = false;
                            self.draft = None;
                        }
                        _ => {}
                    }
                    if let (Poll::Up(i), Ok(mut h)) = (&v.poll, self.home.lock()) {
                        if let (Some(st), Some(md)) = (&i.state_dir, &i.models_dir) {
                            *h = Some((st.clone(), md.clone()));
                        }
                    }
                    match &v.poll {
                        Poll::Up(i) => self.activity.push(self.started.elapsed().as_secs_f64(), (i.running_jobs + i.waiting_jobs) as u32, i.served),
                        Poll::Down => {
                            self.activity.clear();
                            self.history.clear();
                        }
                    }
                    self.view = Some(v);
                }
                Msg::Usage(list, gpu) => {
                    self.history.merge(list);
                    if gpu.is_some() {
                        self.history.gpu_name = gpu;
                    }
                }
                Msg::Models(items, dl) => self.models = Some((items, dl)),
                Msg::Pairing(p) => {
                    let until = Instant::now() + Duration::from_secs(p.minutes * 60);
                    self.pairing = Some((p, until));
                }
                Msg::Notice(n) => {
                    self.applying = false;
                    self.applying_down = false;
                    self.notice = Some((n, Instant::now()));
                }
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

/// Opens a folder in the file manager.
fn open_folder(dir: &str) {
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let program = "xdg-open";
    let _ = std::process::Command::new(program).arg(dir).spawn();
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
/// A name at the start of a row, left aligned in a fixed width so the rows line up.
fn row_label(ui: &mut Ui, text: &str, width: f32) -> egui::Response {
    ui.allocate_ui_with_layout(Vec2::new(width, 18.0), Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_width(width);
        ui.label(RichText::new(text).size(12.5).color(TEXT))
    })
    .inner
}

/// A small coloured label.
fn pill(ui: &mut Ui, text: &str, colour: Color32) {
    Frame::NONE.fill(colour.gamma_multiply(0.18)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(7, 2)).show(ui, |ui| {
        ui.label(RichText::new(text).size(11.0).color(colour));
    });
}

/// A quiet title over a group of settings.
fn section(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text.to_uppercase()).size(10.5).strong().color(DIM));
}

/// One choice as a whole card with a round marker, its name and a line on what it does. Returns true when clicked.
fn option_card(ui: &mut Ui, title: &str, line: &str, selected: bool) -> bool {
    let stroke = if selected { Stroke::new(1.5, BLUE) } else { Stroke::new(1.0, TRACK) };
    let r = Frame::NONE.fill(if selected { BLUE.gamma_multiply(0.10) } else { CARD }).stroke(stroke).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(10, 7)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            let c = rect.center();
            ui.painter().circle_stroke(c, 6.0, Stroke::new(1.5, if selected { BLUE } else { GREY }));
            if selected {
                ui.painter().circle_filled(c, 3.2, BLUE);
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(RichText::new(title).size(13.0).strong().color(TEXT));
                ui.label(RichText::new(line).size(11.0).color(DIM));
            });
        });
    });
    let resp = ui.interact(r.response.rect, ui.scope_id().with(("option", title)), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// Two sides of one choice, side by side. `on` is true for the first.
fn segmented(ui: &mut Ui, on: &mut bool, first: &str, second: &str) {
    ui.spacing_mut().item_spacing.x = 2.0;
    if choice(ui, first, *on, "") {
        *on = true;
    }
    if choice(ui, second, !*on, "") {
        *on = false;
    }
}

/// One option of a group, filled when it is the chosen one.
fn choice(ui: &mut Ui, text: &str, selected: bool, hint: &str) -> bool {
    let b = egui::Button::new(RichText::new(text).size(12.0).color(if selected { Color32::WHITE } else { TEXT })).fill(if selected { BLUE.gamma_multiply(0.55) } else { TRACK }).min_size(Vec2::new(0.0, 22.0));
    finish(ui.add(b), hint)
}

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
            // Reading photos has no speed to show: it is either set up or not.
            if i.chat_model.is_some() {
                ui.horizontal(|ui| {
                    // The same label width as the rows above, so this text and the model name under it line up with theirs.
                    ui.add_sized([52.0, 14.0], egui::Label::new(dim("Photos")));
                    ui.label(RichText::new(model::photos_text(i)).size(12.0).color(if i.vision_model.is_some() { TEXT } else { DIM }));
                });
                if let Some(m) = &i.vision_model {
                    ui.horizontal(|ui| {
                        ui.add_space(58.0);
                        truncated(ui, RichText::new(m).size(11.0).color(DIM));
                    });
                }
            }
            if let Some(r) = &i.runs_on {
                ui.horizontal(|ui| {
                    ui.add_sized([52.0, 14.0], egui::Label::new(dim("Runs on")));
                    ui.label(RichText::new(r).size(12.0).color(TEXT))
                        .on_hover_text("Change it on the Settings page, or with: marginalia-node run-on auto|gpu|cpu|mixed. COMMANDS.md has the details.");
                });
                if let Some(d) = &i.runs_on_detail {
                    ui.horizontal(|ui| {
                        ui.add_space(58.0);
                        truncated(ui, RichText::new(d).size(11.0).color(DIM));
                    });
                }
            }
            // Telling speakers apart is opt-in, so "off" is a normal state, shown in the quiet colour.
            ui.horizontal(|ui| {
                ui.add_sized([52.0, 14.0], egui::Label::new(dim("Speakers")));
                ui.label(RichText::new(model::speakers_text(i)).size(12.0).color(if i.speakers { TEXT } else { DIM }))
                    .on_hover_text("Opt in with: marginalia-node models download --only speakers. The helper program marginalia-node-speakers must be next to the node.");
            });
        });
    }

    /// The four live charts: processor, memory, graphics card and network, the last two minutes of each.
    fn live_charts(&self, ui: &mut Ui) {
        const WINDOW_MS: f64 = 120_000.0;
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0);
        let display_ms = now_ms - charts::DELAY_MS;
        let h = &self.history;
        let Some(last) = h.latest().cloned() else { return };
        let bands = h.busy_spans();
        let band = BLUE.gamma_multiply(0.13);
        let grid = Color32::from_white_alpha(14);
        let back = Color32::from_rgb(26, 28, 34);
        // A line turns amber and then red as the thing it shows fills up.
        let by_load = |base: Color32, fraction: f32| match usage::load_of(fraction) {
            Load::Calm => base,
            Load::Busy => AMBER,
            Load::Hot => RED,
        };
        let ctx = ui.ctx().clone();
        let smooth = |id: &str, v: f32| ctx.animate_value_with_time(egui::Id::unique(("live", id)), v, 0.45);
        let head = |ui: &mut Ui, colour: Color32, name: &str, main: String, sub: String| {
            ui.horizontal(|ui| {
                dot(ui, colour, 3.0);
                ui.label(RichText::new(name).size(11.0).color(DIM));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(sub).size(10.5).color(DIM));
                    ui.label(RichText::new(main).size(12.0).color(TEXT).strong());
                });
            });
        };
        let size = Vec2::new(ui.available_width(), 30.0);
        let hover_text = |t: f64, parts: Vec<String>| {
            let ago = ((now_ms - charts::DELAY_MS - t) / 1000.0).max(0.0);
            format!("{ago:.0} s ago: {}", parts.join(", "))
        };

        // Processor
        let cpu = smooth("cpu", last.cpu);
        head(ui, by_load(BLUE, last.cpu / 100.0), "Processor", format!("{cpu:.0}%"), format!("this node {:.0}%", smooth("node_cpu", last.node_cpu)));
        let (cpu_l, node_l) = (h.series(|r| Some(r.cpu / 100.0)), h.series(|r| Some(r.node_cpu / 100.0)));
        let lines = [Line { points: cpu_l.clone(), colour: by_load(BLUE, last.cpu / 100.0), fill: true, width: 1.6, head: true }, Line { points: node_l, colour: Color32::WHITE.gamma_multiply(0.85), fill: false, width: 1.1, head: false }];
        let hit = charts::chart(ui, size, display_ms, WINDOW_MS, &bands, band, &lines, back, grid);
        if let Some(t) = hit.at_ms {
            let at = |s: &Vec<(u64, f32)>| usage::value_at(s, t).map(|v| v * 100.0).unwrap_or(0.0);
            hit.response.on_hover_ui_at_pointer(|ui| {
                ui.label(hover_text(t, vec![format!("processor {:.0}%", at(&cpu_l)), format!("this node {:.0}%", at(&lines[1].points))]));
            });
        }
        ui.add_space(5.0);

        // Memory
        head(ui, by_load(VIOLET, last.mem_fraction()), "Memory", format!("{} of {}", usage::size_text(last.mem_used), usage::size_text(last.mem_total)), format!("this node {}", usage::size_text(last.node_rss)));
        let (mem_l, node_m) = (h.series(|r| Some(r.mem_fraction())), h.series(|r| Some(r.node_mem_fraction())));
        let lines = [Line { points: mem_l.clone(), colour: by_load(VIOLET, last.mem_fraction()), fill: true, width: 1.6, head: true }, Line { points: node_m, colour: Color32::WHITE.gamma_multiply(0.85), fill: false, width: 1.1, head: false }];
        let hit = charts::chart(ui, size, display_ms, WINDOW_MS, &bands, band, &lines, back, grid);
        if let Some(t) = hit.at_ms {
            let total = last.mem_total as f32;
            hit.response.on_hover_ui_at_pointer(|ui| {
                ui.label(hover_text(t, vec![format!("memory {}", usage::size_text((usage::value_at(&mem_l, t).unwrap_or(0.0) * total) as u64)), format!("this node {}", usage::size_text((usage::value_at(&lines[1].points, t).unwrap_or(0.0) * total) as u64))]));
            });
        }
        ui.add_space(5.0);

        // Graphics card, when the node can read one
        if h.has_gpu() {
            let gpu_pct = last.gpu.unwrap_or(0.0);
            let name = h.gpu_name.clone().unwrap_or_else(|| "Graphics card".into());
            let vram = match (last.vram_used, last.vram_total) {
                (Some(u), Some(t)) => format!("{} of {}", usage::size_text(u), usage::size_text(t)),
                _ => String::new(),
            };
            let temp = last.gpu_temp.map(|c| format!("{c:.0} C")).unwrap_or_default();
            head(ui, by_load(GREEN, gpu_pct / 100.0), "Graphics", format!("{:.0}%", smooth("gpu", gpu_pct)), [vram, temp].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(", "));
            let (gpu_l, vram_l) = (h.series(|r| r.gpu.map(|g| g / 100.0)), h.series(|r| r.vram_fraction()));
            let lines = [Line { points: gpu_l.clone(), colour: by_load(GREEN, gpu_pct / 100.0), fill: true, width: 1.6, head: true }, Line { points: vram_l, colour: AMBER.gamma_multiply(0.9), fill: false, width: 1.1, head: false }];
            let hit = charts::chart(ui, size, display_ms, WINDOW_MS, &bands, band, &lines, back, grid);
                if let Some(t) = hit.at_ms {
                    hit.response.on_hover_ui_at_pointer(|ui| {
                    ui.label(hover_text(t, vec![name.clone(), format!("busy {:.0}%", usage::value_at(&gpu_l, t).unwrap_or(0.0) * 100.0), format!("memory {:.0}%", usage::value_at(&lines[1].points, t).unwrap_or(0.0) * 100.0)]));
                    });
            }
            ui.add_space(5.0);
        }

        // Network, both ways on one scale that grows and shrinks with the traffic
        let peak = h.readings.iter().map(|r| r.net_rx.max(r.net_tx) as f32).fold(0.0, f32::max);
        let top = usage::nice_max(peak, 20_000.0);
        head(ui, TEAL, "Network", format!("in {}", usage::rate_text(smooth("rx", last.net_rx as f32))), format!("out {}", usage::rate_text(smooth("tx", last.net_tx as f32))));
        let (rx_l, tx_l) = (h.series(|r| Some(r.net_rx as f32 / top)), h.series(|r| Some(r.net_tx as f32 / top)));
        let lines = [Line { points: rx_l.clone(), colour: TEAL, fill: true, width: 1.6, head: true }, Line { points: tx_l, colour: ORANGE, fill: false, width: 1.3, head: true }];
        let hit = charts::chart(ui, size, display_ms, WINDOW_MS, &bands, band, &lines, back, grid);
            if let Some(t) = hit.at_ms {
                hit.response.on_hover_ui_at_pointer(|ui| {
                ui.label(hover_text(t, vec![format!("in {}", usage::rate_text(usage::value_at(&rx_l, t).unwrap_or(0.0) * top)), format!("out {}", usage::rate_text(usage::value_at(&lines[1].points, t).unwrap_or(0.0) * top))]));
                });
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("2 min ago").size(10.0).color(DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(format!("scale up to {}", usage::rate_text(top))).size(10.0).color(DIM));
            });
        });
        // Keeps the charts gliding.
        ui.ctx().request_repaint_after(Duration::from_millis(33));
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
            if self.history.readings.len() >= 3 {
                self.live_charts(ui);
            } else {
                // A node that sends no readings (an older one): the bars of finished jobs.
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

    /// Overview or Settings, under the header.
    fn tabs(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for (name, settings) in [("Overview", false), ("Settings", true)] {
                if choice(ui, name, self.settings == settings, if settings { "Models and logs" } else { "What the node is doing" }) {
                    self.settings = settings;
                    self.settings_visible.store(settings, Ordering::Relaxed);
                }
            }
        });
    }

    /// What the node uses now, as the Settings page shows it.
    fn current_settings(i: &Info) -> (RunDraft, String) {
        (RunDraft::from_setting(i.run_setting.as_deref()), model::chat_choice(i).unwrap_or("e2b").to_string())
    }

    fn settings_page(&mut self, ui: &mut Ui, i: &Info) {
        let current = Self::current_settings(i);
        if self.draft.is_none() || self.applying {
            self.draft = Some(current.clone());
        }
        let Some((mut run, mut chat)) = self.draft.clone() else { return };

        // Where things are right now, in one glance.
        if let Some(r) = &i.runs_on {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    dot(ui, GREEN, 4.0);
                    ui.label(RichText::new("Now").size(12.0).strong().color(TEXT));
                    ui.label(dim(r));
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(3.0, 4.0);
                    for (name, share) in [("Chat", i.now_chat), ("Search", i.now_search), ("Speech", i.now_speech_gpu.map(|g| if g { 100 } else { 0 })), ("Photos", i.now_photos_gpu.map(|g| if g { 100 } else { 0 }))] {
                        if let Some(sh) = share {
                            let (text, colour) = match sh {
                                100 => (format!("{name} card"), BLUE),
                                0 => (format!("{name} CPU"), GREY),
                                p => (format!("{name} {p}%"), VIOLET),
                            };
                            pill(ui, &text, colour);
                        }
                    }
                });
            });
        }

        section(ui, "Chat model");
        for (value, name, line) in [("e2b", "E2B", "Smaller, faster"), ("e4b", "E4B", "Better answers, 2 GB more memory")] {
            if option_card(ui, name, line, chat == value) {
                chat = value.into();
            }
        }

        section(ui, "Where the models run");
        for (value, name, line) in RunDraft::MODES {
            if option_card(ui, name, line, run.mode == value) {
                if value == "custom" && run.mode != "custom" {
                    // Start from where the models are now.
                    run.chat = i.now_chat.unwrap_or(run.chat);
                    run.search = i.now_search.unwrap_or(run.search);
                    run.speech_gpu = i.now_speech_gpu.unwrap_or(run.speech_gpu);
                    run.photos_gpu = i.now_photos_gpu.unwrap_or(run.photos_gpu);
                }
                run.mode = value.into();
            }
        }
        if run.mode == "custom" {
            card(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                let share = |ui: &mut Ui, label: &str, hint: &str, v: &mut u8| {
                    ui.horizontal(|ui| {
                        row_label(ui, label, 60.0).on_hover_text(hint);
                        let mut x = *v as f32;
                        ui.spacing_mut().slider_width = 130.0;
                        if ui.add(egui::Slider::new(&mut x, 0.0..=100.0).step_by(5.0).fixed_decimals(0).suffix("%").show_value(false)).changed() {
                            *v = x as u8;
                        }
                        ui.label(RichText::new(match *v { 0 => "processor".to_string(), 100 => "card".to_string(), p => format!("{p}% on card") }).size(12.0).color(DIM));
                    });
                };
                share(ui, "Chat", "Share of layers on the card. The rest uses the processor.", &mut run.chat);
                share(ui, "Search", "Share of layers on the card.", &mut run.search);
                let side = |ui: &mut Ui, label: &str, gpu: &mut bool| {
                    ui.horizontal(|ui| {
                        row_label(ui, label, 60.0).on_hover_text("Runs on one side only");
                        segmented(ui, gpu, "Card", "Processor");
                    });
                };
                side(ui, "Speech", &mut run.speech_gpu);
                side(ui, "Photos", &mut run.photos_gpu);
                ui.label(dim("Speech and photos run on one side only."));
            });
        }

        section(ui, "Speakers");
        card(ui, |ui| {
            ui.horizontal(|ui| {
                row_label(ui, "Run on", 60.0);
                let mut gpu = run.speakers_gpu;
                segmented(ui, &mut gpu, "Card", "Processor");
                run.speakers_gpu = gpu;
            });
            ui.label(dim("Card needs the GPU helper and cuDNN."));
        });

        self.models_section(ui);

        if let Some(dir) = &i.log_dir {
            section(ui, "Logs");
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("node.log").size(12.5).color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if small_button(ui, "Open folder", "Open the folder with the log files") {
                            open_folder(dir);
                        }
                    });
                });
                ui.label(dim("Keeps the last 5 files, 5 MB each. Events and numbers only."));
                truncated(ui, RichText::new(dir).font(FontId::monospace(10.5)).color(DIM));
            });
        }
        ui.add_space(4.0);
        if !self.applying {
            self.draft = Some((run, chat));
        }
    }

    /// The model files: which are there, a Get button for the missing ones, and the download in progress.
    fn models_section(&mut self, ui: &mut Ui) {
        section(ui, "Models");
        let Some((items, dl)) = self.models.clone() else {
            card(ui, |ui| {
                ui.label(dim("Looking at the models folder..."));
            });
            return;
        };
        let running = matches!(dl, Some(Download::Running { .. }));
        let mut start: Option<ModelItem> = None;
        let mut restart = false;
        card(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            for it in &items {
                ui.horizontal(|ui| {
                    row_label(ui, &it.label, 86.0);
                    let busy = running && self.fetching.as_ref().map(|(l, _)| l == &it.label).unwrap_or(false);
                    if it.ready {
                        ui.label(RichText::new(format!("Ready  {}", model::size_text(it.bytes))).size(11.5).color(GREEN));
                    } else if busy {
                        ui.label(RichText::new("Downloading").size(11.5).color(BLUE));
                    } else {
                        ui.label(RichText::new("Not downloaded").size(11.5).color(DIM));
                    }
                    if !it.ready && !running {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if small_button(ui, "Get", "Download it from Hugging Face") {
                                start = Some(it.clone());
                            }
                        });
                    }
                });
            }
            match &dl {
                Some(Download::Running { file, done, total }) => {
                    let f = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
                    ui.add_space(2.0);
                    bar(ui, ui.available_width(), f, BLUE);
                    ui.label(dim(format!("{file}  {}  {}", if *total > 0 { format!("{}%", (f * 100.0) as u32) } else { String::new() }, model::size_text(*done))));
                }
                Some(Download::Failed(e)) if self.fetching.is_some() => {
                    ui.label(RichText::new(e).size(11.0).color(RED));
                }
                Some(Download::Done) if self.fetching.as_ref().map(|(_, r)| *r).unwrap_or(false) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Downloaded. Restart to use it.").size(11.5).color(GREEN));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if small_button(ui, "Restart node", "Restart the node so it loads the new model") {
                                restart = true;
                            }
                        });
                    });
                }
                _ => {}
            }
        });
        if let Some(it) = start {
            self.fetching = Some((it.label.clone(), it.needs_restart));
            self.send(Cmd::Download(it.args.clone()));
            if let Some((_, d)) = &mut self.models {
                // Shown as running at once, the next look at the progress file confirms it.
                *d = Some(Download::Running { file: it.label.clone(), done: 0, total: 0 });
            }
        }
        if restart {
            self.fetching = None;
            self.applying = true;
            self.send(Cmd::Apply(vec![]));
        }
    }

    /// Save and Undo, kept at the bottom of the Settings page. They light up only when something changed.
    fn settings_footer(&mut self, ui: &mut Ui, i: &Info) {
        let current = Self::current_settings(i);
        let Some((run, chat)) = self.draft.clone() else { return };
        let changed = (run.clone(), chat.clone()) != current;
        ui.label(dim(if self.applying { "Restarting the node, about a minute." } else if changed { "Saving restarts the node." } else { "No changes." }));
        ui.columns(2, |cols| {
            let save = if self.applying { "Restarting..." } else { "Save and restart" };
            if big_button(&mut cols[0], save, if changed && !self.applying { Some(GREEN_BTN) } else { None }, "Save the settings and restart the node") && changed && !self.applying {
                let mut cmds = vec![];
                if chat != current.1 {
                    cmds.push(vec!["chat-model".to_string(), chat.clone()]);
                }
                if run != current.0 {
                    cmds.push(run.args());
                }
                self.applying = true;
                self.send(Cmd::Apply(cmds));
            }
            if big_button(&mut cols[1], "Undo changes", None, "Back to what the node uses now") && !self.applying {
                self.draft = Some(current.clone());
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
                if self.settings {
                    self.settings_footer(ui, i);
                } else {
                    self.footer(ui, i);
                }
            });
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            Frame::NONE.inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 2 }).show(ui, |ui| {
                match &view {
                    None => {
                        ui.label(dim("Looking for the node..."));
                    }
                    Some(v) => {
                        self.header(ui, v);
                        if info.is_some() {
                            self.tabs(ui);
                        }
                    }
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
                            if self.settings {
                                self.settings_page(ui, i);
                                return;
                            }
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

pub fn run(port: u16, keep_open: bool, settings: bool) {
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
            // Text is not selectable, so a click on a card's words reaches the card.
            style.interaction.selectable_labels = false;
            cc.egui_ctx.set_global_style(style);
            Ok(Box::new(Panel::new(port, keep_open, settings, cc.egui_ctx.clone())))
        }),
    );
    if let Err(e) = result {
        eprintln!("Could not open the details panel: {e}");
    }
}
