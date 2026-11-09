//! The details window: a normal window with a sidebar of pages (Overview, Phones, Models, Settings) that opens when the tray
//! icon is clicked. It is its own process (`marginalia-node-tray --panel`), started by the tray, and it stays until it is closed.
//! It polls the node's local admin channel every 0.7 seconds while it is open and shows counts, names and timings only.
//! Everything it can do goes through the same admin commands as the command line.

use crate::anim::{clamp01, Spring};
use crate::charts::{self, Line};
use crate::control::{Controller, Pairing, POLL_CHANGING};
use crate::kit;
use crate::menuspec::Icon;
use crate::model::{self, Action, Activity, Download, Info, ModelItem, Phone, Poll, RunDraft, State, View, Waiting};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use crate::popover::draw_icon;
use crate::usage::{self, History, Load, Reading};
use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, Stroke, Ui, Vec2};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The window's size, and the least it can be made.
const WINDOW: [f32; 2] = [940.0, 660.0];
const WINDOW_MIN: [f32; 2] = [780.0, 540.0];
const SIDEBAR_W: f32 = 212.0;
/// A sidebar row, and the space between rows.
const NAV_H: f32 = 32.0;
const NAV_GAP: f32 = 2.0;
/// The longest a page's content runs across, so lines stay short in a wide window.
const CONTENT_W: f32 = 720.0;
/// The window asks the node what it is doing this often, quick enough to catch a node that takes a second to stop.
const POLL: Duration = POLL_CHANGING;
const SPARK_COLUMNS: usize = 60;

const BG: Color32 = kit::BG;
const SIDEBAR: Color32 = kit::SIDEBAR;
const TRACK: Color32 = kit::TRACK;
const TEXT: Color32 = kit::TEXT;
const DIM: Color32 = kit::DIM;
const GREEN: Color32 = kit::GREEN;
const AMBER: Color32 = kit::ORANGE;
const BLUE: Color32 = kit::BLUE;
const VIOLET: Color32 = kit::PURPLE;
const GREY: Color32 = Color32::from_rgb(130, 134, 142);
const TEAL: Color32 = Color32::from_rgb(70, 200, 200);
const ORANGE: Color32 = Color32::from_rgb(255, 150, 80);
const RED: Color32 = kit::RED;
/// The sidebar's icon on the selected row: the indigo, lightened to read on its own tint.
const ACCENT_TEXT: Color32 = Color32::from_rgb(176, 166, 255);

/// The pages of the window, in the order of the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Overview,
    Phones,
    Models,
    Settings,
}

impl Page {
    const ALL: [Page; 4] = [Page::Overview, Page::Phones, Page::Models, Page::Settings];

    fn title(self) -> &'static str {
        match self {
            Page::Overview => "Overview",
            Page::Phones => "Phones",
            Page::Models => "Models",
            Page::Settings => "Settings",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Page::Overview => Icon::Chart,
            Page::Phones => Icon::Phone,
            Page::Models => Icon::Stack,
            Page::Settings => Icon::Gear,
        }
    }

    fn index(self) -> usize {
        Page::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    /// By the name on the command line (`--page phones`).
    pub fn from_name(name: &str) -> Option<Page> {
        Page::ALL.into_iter().find(|p| p.title().eq_ignore_ascii_case(name))
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

/// While the Models page shows, lists the models on disk and reads the download progress: at once when it opens, then every 1.5 seconds.
fn models_worker(ctx: egui::Context, tx: Sender<Msg>, visible: Arc<AtomicBool>, home: Arc<Mutex<Option<(String, String)>>>) {
    let mut looked: Option<Instant> = None;
    loop {
        std::thread::sleep(Duration::from_millis(100));
        if !visible.load(Ordering::Relaxed) {
            looked = None;
            continue;
        }
        if looked.map(|t| t.elapsed() < Duration::from_millis(1500)).unwrap_or(false) {
            continue;
        }
        let Some((state, models)) = home.lock().ok().and_then(|h| h.clone()) else { continue };
        looked = Some(Instant::now());
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
    /// The last code shown, so its card can close with its words still on it.
    pairing_last: Option<(Pairing, Instant)>,
    confirm: Option<(String, Confirm)>,
    /// "Stop" asks once more before it does anything.
    confirm_stop: bool,
    /// Start or Stop just pressed: what the node was told to become, and when. Shown at once, before the node has answered.
    acted: Option<(State, Instant)>,
    notice: Option<(String, Instant)>,
    /// A switch the person just moved, shown at its new place until the node says the same: (name, value, when).
    pending: Vec<(&'static str, bool, Instant)>,
    /// The page showing.
    page: Page,
    /// Where the sidebar's selected row is, in rows, and how far the page has come in (0 to 1) since it was chosen.
    nav: Spring,
    page_in: Spring,
    /// Which way the last move through the sidebar went: 1 down, -1 up. The new page comes from that side.
    page_dir: f32,
    /// When the page was chosen, and which of waiting, not running or the pages is showing.
    page_at: Instant,
    page_kind: u8,
    /// How high the charts were last, so the space is kept while the first readings are on their way.
    charts_h: Cell<f32>,
    /// The last message shown, kept so its capsule can leave with its words still on it.
    toast: String,
    /// The address just copied, and when, so its button can say so for a moment.
    copied: Option<(String, Instant)>,
    /// The Marginalia mark, for the top of the sidebar.
    logo: Option<egui::TextureHandle>,
    /// The system's blurred material behind the window: unknown until the window exists.
    backdrop: Option<bool>,
    reduced: bool,
    /// The window is told once to follow the dark theme.
    theme_set: bool,
    /// What the Settings page is editing, filled from the node the first time it is shown.
    draft: Option<(RunDraft, String)>,
    /// Whether the tray starts at sign in, as the General card shows it.
    startup: bool,
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
    fn new(port: u16, page: Page, ctx: egui::Context) -> Panel {
        let (tx, rx) = channel();
        let (cmds, cmd_rx) = channel();
        {
            let (ctx, tx) = (ctx.clone(), tx.clone());
            std::thread::spawn(move || usage_worker(port, ctx, tx));
        }
        let settings_visible = Arc::new(AtomicBool::new(page == Page::Models));
        let home: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
        {
            let (ctx, tx, v, h) = (ctx.clone(), tx.clone(), settings_visible.clone(), home.clone());
            std::thread::spawn(move || models_worker(ctx, tx, v, h));
        }
        {
            let h = home.clone();
            std::thread::spawn(move || worker(port, ctx, tx, cmd_rx, h));
        }
        Panel { rx, cmds, view: None, history: History::default(), activity: Activity::default(), started: Instant::now(), pairing: None, pairing_last: None, confirm: None, confirm_stop: false, acted: None, notice: None, pending: Vec::new(), page, nav: Spring::new(page.index() as f32, 1.0, 0.2), page_in: Spring::new(1.0, 1.0, 0.2), page_dir: 1.0, page_at: Instant::now(), page_kind: 0, charts_h: Cell::new(156.0), toast: String::new(), copied: None, logo: None, backdrop: None, reduced: crate::os::reduced_motion(), theme_set: false, draft: None, startup: crate::autostart::enabled(), applying: false, applying_down: false, models: None, fetching: None, settings_visible, home }
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
                        (Poll::Down | Poll::Stopping, true) => self.applying_down = true,
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
                        Poll::Down | Poll::Stopping => {
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

    /// Sets the window up once it exists: dark title bar, whatever the system theme is.
    fn window_behaviour(&mut self, ctx: &egui::Context) {
        if !self.theme_set {
            self.theme_set = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
        }
        if self.backdrop.is_none() {
            self.backdrop = crate::os::apply_backdrop("Marginalia node");
        }
    }

    /// The state to show: what the node says, or what it was just told to become while it has not answered yet.
    fn showing(&self, state: State) -> State {
        showing_state(state, self.acted.filter(|(_, t)| t.elapsed() < ACT_WAIT).map(|(s, _)| s))
    }

    /// What a switch shows: the place the person just put it, for a few seconds, then what the node says.
    fn shown(&self, key: &'static str, actual: bool) -> bool {
        self.pending.iter().find(|(k, _, t)| *k == key && t.elapsed() < Duration::from_secs(4)).map(|(_, v, _)| *v).unwrap_or(actual)
    }

    fn expect(&mut self, key: &'static str, value: bool) {
        self.pending.retain(|(k, _, _)| *k != key);
        self.pending.push((key, value, Instant::now()));
    }

    fn go(&mut self, page: Page) {
        if page != self.page {
            self.page_dir = if page.index() > self.page.index() { 1.0 } else { -1.0 };
            self.page = page;
            self.page_at = Instant::now();
            self.begin_page();
            // The model files are only looked at while the Models page is showing.
            self.settings_visible.store(page == Page::Models, Ordering::Relaxed);
        }
    }

    /// The new content comes in from here.
    fn begin_page(&mut self) {
        self.page_in.snap(0.0);
        self.page_in.set_target(1.0);
    }

    /// Moves the springs on, and says whether any is still moving.
    fn animate(&mut self, ctx: &egui::Context) -> bool {
        // Ctrl and a number goes straight to that page.
        for (n, key) in [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4].into_iter().enumerate() {
            if ctx.input(|i| i.modifiers.command && i.key_pressed(key)) {
                self.go(Page::ALL[n]);
            }
        }
        let dt = ctx.input(|i| i.stable_dt);
        self.nav.set_target(self.page.index() as f32);
        self.nav.step(dt, self.reduced);
        self.page_in.step(dt, self.reduced);
        self.nav.moving() || self.page_in.moving()
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
    Frame::NONE.fill(kit::GROUP).stroke(stroke).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(kit::PAD as i8, 12)).show(ui, |ui| {
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

fn dot(ui: &mut Ui, colour: Color32, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(radius * 2.0 + 2.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), radius, colour);
}

fn truncated(ui: &mut Ui, text: RichText) {
    ui.add(egui::Label::new(text).truncate());
}

fn now_epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Panel {
    /// The first thing on the Overview: is the node all right, and what can be done about it. Its state in large type with the state's
    /// colour glowing beside it, the three figures that say how it has been doing, and under them Start, Pause and Stop. A node that is
    /// not there has no figures. The dot only pulses while the node is working.
    fn hero(&mut self, ui: &mut Ui, state: State, i: Option<&Info>) {
        let state = self.showing(state);
        let colour = kit::state_colour(state);
        let busy = state == State::Busy;
        let paused = i.map(|i| !self.shown("accept", !i.paused)).unwrap_or(false);
        let r = Frame::NONE.fill(kit::GROUP).corner_radius(CornerRadius::same(14)).inner_margin(Margin { left: 16, right: 12, top: 16, bottom: 14 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                let c = rect.center();
                let pulse = if busy { 0.5 + 0.5 * (ui.input(|i| i.time) as f32 * 3.2).sin() } else { 0.45 };
                ui.painter().add(egui::epaint::Shadow { offset: [0, 0], blur: 26, spread: 2, color: colour.gamma_multiply(0.22 + 0.26 * pulse) }.as_shape(egui::Rect::from_center_size(c, Vec2::splat(12.0)), CornerRadius::same(6)));
                ui.painter().circle_filled(c, 6.5, colour);
                ui.add_space(4.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if let Some(i) = i {
                        // Closer together in a narrow window, so the sentence beside them still fits.
                        let fig = if ui.available_width() < 460.0 { 66.0 } else { 78.0 };
                        kit::figure(ui, fig, &model::uptime_short(i.uptime_s), "up for");
                        kit::divider(ui, 34.0);
                        let phones = kit::spring(ui.ctx(), egui::Id::NULL.with("count-phones"), i.phones.len() as f32, 1.0, 0.5).round() as u64;
                        kit::figure(ui, fig, &phones.to_string(), if i.phones.len() == 1 { "phone" } else { "phones" });
                        kit::divider(ui, 34.0);
                        let served = kit::spring(ui.ctx(), egui::Id::NULL.with("count-served"), i.served as f32, 1.0, 0.5).round() as u64;
                        kit::figure(ui, fig, &served.to_string(), if i.served == 1 { "job served" } else { "jobs served" });
                        ui.add_space(10.0);
                    }
                    // The figures take their room first, and the sentence ends in dots if a narrow window leaves it too little.
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(kit::title(hero_word(state), 24.0));
                            let line = match (i, state) {
                                (Some(i), _) => model::status_line(i),
                                (None, State::Starting) => "Loading its models, this can take a minute".to_string(),
                                (None, State::Stopping) => "Finishing the jobs it has".to_string(),
                                _ => "Your phone works on its own until it is back".to_string(),
                            };
                            ui.add(egui::Label::new(RichText::new(line).size(12.5).color(DIM)).truncate());
                        });
                    });
                });
            });
            ui.add_space(12.0);
            self.controls(ui, state, paused);
        });
        kit::sheen(ui, r.response.rect, 14.0, 1.0);
        if busy {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
    }

    /// Start, Pause and Stop, always in the same three places, each lit only when it can be used. Stop asks once, in the same row.
    fn controls(&mut self, ui: &mut Ui, state: State, paused: bool) {
        let can = Controls::for_state(state);
        if !can.stop {
            self.confirm_stop = false;
        }
        const W: f32 = 98.0;
        let width = ui.available_width();
        let (mut yes, mut no) = (false, false);
        if self.confirm_stop {
            ui.allocate_ui_with_layout(Vec2::new(width, 28.0), Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                ui.spacing_mut().item_spacing.x = 8.0;
                yes = kit::button(ui, "stop-yes", "Yes, stop it", kit::Kind::Destructive, "");
                no = kit::button(ui, "stop-no", "Cancel", kit::Kind::Plain, "");
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new("Stop the node? Phones lose it until it is started").size(12.5).color(TEXT)).truncate());
                });
            });
        } else {
            ui.allocate_ui_with_layout(Vec2::new(width, 28.0), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if kit::button_with(ui, "node-start", Some(Icon::Play), if state == State::Starting { "Starting" } else { "Start" }, kit::Kind::Primary, Some(W), can.start, "Start the node on this machine") {
                    self.send(Cmd::Click(Action::Start));
                    self.acted = Some((State::Starting, Instant::now()));
                }
                if kit::button_with(ui, "node-pause", Some(if paused { Icon::Play } else { Icon::Pause }), if paused { "Resume" } else { "Pause" }, kit::Kind::Plain, Some(W), can.pause, if paused { "Take new jobs again" } else { "Stop taking new jobs. Jobs already running finish" }) {
                    self.expect("accept", paused);
                    self.send(Cmd::Click(if paused { Action::Resume } else { Action::Pause }));
                }
                if kit::button_with(ui, "node-stop", Some(Icon::Stop), if state == State::Stopping { "Stopping" } else { "Stop" }, kit::Kind::Danger, Some(W), can.stop, "Finish the running jobs and shut the node down") {
                    self.confirm_stop = true;
                }
            });
        }
        if yes {
            self.send(Cmd::Click(Action::Stop));
            self.acted = Some((State::Stopping, Instant::now()));
            self.confirm_stop = false;
        }
        if no {
            self.confirm_stop = false;
        }
        if self.acted.map(|(_, t)| t.elapsed() < ACT_WAIT).unwrap_or(false) {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    fn speed(&self, ui: &mut Ui, i: &Info) {
        kit::heading(ui, "What it runs");
        kit::group(ui, |ui, g| {
            let mut rows = vec![
                ("Chat", model::gauge(i.chat_tok_s, model::CHAT_TOK_S_MAX), model::chat_speed_text(i.chat_tok_s), BLUE, &i.chat_model),
                ("Search", model::gauge(i.search_per_s, model::SEARCH_PER_S_MAX), model::search_speed_text(i.search_per_s), GREEN, &i.search_model),
            ];
            if i.speech_model.is_some() {
                rows.push(("Speech", model::gauge(i.speech_x, model::SPEECH_X_MAX), model::speech_speed_text(i.speech_x), AMBER, &i.speech_model));
            }
            for (label, fraction, text, colour, name) in rows {
                g.row(ui, kit::ROW_TALL, |ui| {
                    kit::text_block(ui, 170.0, kit::ROW_TALL, kit::NAME_H + kit::LINE_H, |ui| {
                        ui.label(RichText::new(label).size(13.5).color(TEXT));
                        truncated(ui, RichText::new(name.clone().unwrap_or_else(|| "not served".into())).size(11.0).color(DIM));
                    });
                    let w = (ui.available_width() - 150.0).max(40.0);
                    kit::meter(ui, label, w, fraction, colour);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(kit::PAD);
                        ui.label(RichText::new(text).size(12.5).color(TEXT));
                    });
                });
            }
            // Reading photos has no speed to show: it is either set up or not.
            if i.chat_model.is_some() {
                g.setting(ui, "Photos", i.vision_model.as_deref(), |ui| {
                    ui.label(RichText::new(model::photos_text(i)).size(12.5).color(if i.vision_model.is_some() { TEXT } else { DIM }));
                });
            }
            if let Some(r) = &i.runs_on {
                g.setting(ui, "Runs on", i.runs_on_detail.as_deref(), |ui| {
                    ui.label(RichText::new(r).size(12.5).color(TEXT)).on_hover_text("Change it on the Settings page, or with: marginalia-node run-on auto|gpu|cpu|mixed. COMMANDS.md has the details.");
                });
            }
            // Telling speakers apart is opt-in, so "off" is a normal state, shown in the quiet colour.
            g.setting(ui, "Speakers", None, |ui| {
                ui.label(RichText::new(model::speakers_text(i)).size(12.5).color(if i.speakers { TEXT } else { DIM }))
                    .on_hover_text("Opt in with: marginalia-node models download --only speakers. The helper program marginalia-node-speakers must be next to the node.");
            });
        });
    }

    /// The four live charts: processor, memory, graphics card and network, the last two minutes of each.
    fn live_charts(&self, ui: &mut Ui) -> f32 {
        const WINDOW_MS: f64 = 120_000.0;
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0);
        let display_ms = now_ms - charts::DELAY_MS;
        let h = &self.history;
        let Some(last) = h.latest().cloned() else { return 0.0 };
        let bands = h.busy_spans();
        let band = BLUE.gamma_multiply(0.13);
        let grid = Color32::from_white_alpha(8);
        let back = Color32::from_rgb(33, 33, 36);
        let ctx = ui.ctx().clone();
        let smooth = |id: &str, v: f32| ctx.animate_value_with_time(egui::Id::unique(("live", id)), v, 0.45);
        // A line turns amber and then red as the thing it shows fills up, passing through the colours between.
        let by_load = |id: &str, base: Color32, fraction: f32| {
            let level = smooth(id, match usage::load_of(fraction) {
                Load::Calm => 0.0,
                Load::Busy => 1.0,
                Load::Hot => 2.0,
            });
            if level <= 1.0 { kit::mix(base, AMBER, level) } else { kit::mix(AMBER, RED, level - 1.0) }
        };
        let head = |ui: &mut Ui, colour: Color32, name: &str, main: String, sub: String| {
            let measure = |ui: &Ui, text: &str, font: FontId| ui.painter().layout_no_wrap(text.to_string(), font, TEXT).size().x;
            let needed = 8.0 + 6.0 + measure(ui, name, FontId::proportional(11.0)) + 12.0 + measure(ui, &main, FontId::new(12.0, kit::semibold_family())) + 6.0 + measure(ui, &sub, FontId::proportional(10.5));
            let room = ui.available_width() >= needed;
            ui.horizontal(|ui| {
                dot(ui, colour, 3.0);
                ui.label(RichText::new(name).size(11.0).color(DIM));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // The smaller figure gives way when a narrow window has no room for both.
                    if room {
                        ui.label(RichText::new(sub).size(10.5).color(DIM));
                    }
                    ui.label(RichText::new(main).size(12.0).color(TEXT).family(kit::semibold_family()));
                });
            });
        };
        let peak = h.readings.iter().map(|r| r.net_rx.max(r.net_tx) as f32).fold(0.0, f32::max);
        let top = usage::nice_max(peak, 20_000.0);
        // The scale changes in steps, the lines must not: they ease to the new scale.
        let scale = smooth("net-scale", top).max(1.0);
        // Two across, so the four fit on one screen.
        let gap = 18.0;
        let col = (ui.available_width() - gap) / 2.0;
        let size = Vec2::new(col, 42.0);
        let hover_text = |t: f64, parts: Vec<String>| {
            let ago = ((now_ms - charts::DELAY_MS - t) / 1000.0).max(0.0);
            format!("{ago:.0} s ago: {}", parts.join(", "))
        };

        ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.vertical(|ui| {
        ui.set_width(col);
        ui.spacing_mut().item_spacing.x = 6.0;
        // Processor
        let cpu = smooth("cpu", last.cpu);
        head(ui, by_load("cpu-load", BLUE, last.cpu / 100.0), "Processor", format!("{cpu:.0}%"), format!("this node {:.0}%", smooth("node_cpu", last.node_cpu)));
        let (cpu_l, node_l) = (h.series(|r| Some(r.cpu / 100.0)), h.series(|r| Some(r.node_cpu / 100.0)));
        let lines = [Line { points: cpu_l.clone(), colour: by_load("cpu-load", BLUE, last.cpu / 100.0), fill: true, width: 1.6, head: true }, Line { points: node_l, colour: Color32::WHITE.gamma_multiply(0.85), fill: false, width: 1.1, head: false }];
        let hit = charts::chart(ui, size, display_ms, WINDOW_MS, &bands, band, &lines, back, grid);
        if let Some(t) = hit.at_ms {
            let at = |s: &Vec<(u64, f32)>| usage::value_at(s, t).map(|v| v * 100.0).unwrap_or(0.0);
            hit.response.on_hover_ui_at_pointer(|ui| {
                ui.label(hover_text(t, vec![format!("processor {:.0}%", at(&cpu_l)), format!("this node {:.0}%", at(&lines[1].points))]));
            });
        }
        });
        ui.add_space(gap);
        ui.vertical(|ui| {
        ui.set_width(col);
        ui.spacing_mut().item_spacing.x = 6.0;
        // Memory
        head(ui, by_load("mem-load", VIOLET, last.mem_fraction()), "Memory", format!("{} of {}", usage::size_text(last.mem_used), usage::size_text(last.mem_total)), format!("this node {}", usage::size_text(last.node_rss)));
        let (mem_l, node_m) = (h.series(|r| Some(r.mem_fraction())), h.series(|r| Some(r.node_mem_fraction())));
        let lines = [Line { points: mem_l.clone(), colour: by_load("mem-load", VIOLET, last.mem_fraction()), fill: true, width: 1.6, head: true }, Line { points: node_m, colour: Color32::WHITE.gamma_multiply(0.85), fill: false, width: 1.1, head: false }];
        let hit = charts::chart(ui, size, display_ms, WINDOW_MS, &bands, band, &lines, back, grid);
        if let Some(t) = hit.at_ms {
            let total = last.mem_total as f32;
            hit.response.on_hover_ui_at_pointer(|ui| {
                ui.label(hover_text(t, vec![format!("memory {}", usage::size_text((usage::value_at(&mem_l, t).unwrap_or(0.0) * total) as u64)), format!("this node {}", usage::size_text((usage::value_at(&lines[1].points, t).unwrap_or(0.0) * total) as u64))]));
            });
        }
        });
        });
        ui.add_space(8.0);

        ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        // Graphics card, when the node can read one
        if h.has_gpu() {
            ui.vertical(|ui| {
            ui.set_width(col);
            ui.spacing_mut().item_spacing.x = 6.0;
            let gpu_pct = last.gpu.unwrap_or(0.0);
            let name = h.gpu_name.clone().unwrap_or_else(|| "Graphics card".into());
            let vram = match (last.vram_used, last.vram_total) {
                (Some(u), Some(t)) => format!("{} of {}", usage::size_text(u), usage::size_text(t)),
                _ => String::new(),
            };
            let temp = last.gpu_temp.map(|c| format!("{c:.0} C")).unwrap_or_default();
            head(ui, by_load("gpu-load", GREEN, gpu_pct / 100.0), "Graphics", format!("{:.0}%", smooth("gpu", gpu_pct)), [vram, temp].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(", "));
            let (gpu_l, vram_l) = (h.series(|r| r.gpu.map(|g| g / 100.0)), h.series(|r| r.vram_fraction()));
            let lines = [Line { points: gpu_l.clone(), colour: by_load("gpu-load", GREEN, gpu_pct / 100.0), fill: true, width: 1.6, head: true }, Line { points: vram_l, colour: AMBER.gamma_multiply(0.9), fill: false, width: 1.1, head: false }];
            let hit = charts::chart(ui, size, display_ms, WINDOW_MS, &bands, band, &lines, back, grid);
                if let Some(t) = hit.at_ms {
                    hit.response.on_hover_ui_at_pointer(|ui| {
                    ui.label(hover_text(t, vec![name.clone(), format!("busy {:.0}%", usage::value_at(&gpu_l, t).unwrap_or(0.0) * 100.0), format!("memory {:.0}%", usage::value_at(&lines[1].points, t).unwrap_or(0.0) * 100.0)]));
                    });
            }
            });
            ui.add_space(gap);
        }

        ui.vertical(|ui| {
        ui.set_width(col);
        ui.spacing_mut().item_spacing.x = 6.0;
        // Network, both ways on one scale that grows and shrinks with the traffic
        head(ui, TEAL, "Network", format!("in {}", usage::rate_text(smooth("rx", last.net_rx as f32))), format!("out {}", usage::rate_text(smooth("tx", last.net_tx as f32))));
        let (rx_l, tx_l) = (h.series(|r| Some(r.net_rx as f32 / scale)), h.series(|r| Some(r.net_tx as f32 / scale)));
        let lines = [Line { points: rx_l.clone(), colour: TEAL, fill: true, width: 1.6, head: true }, Line { points: tx_l, colour: ORANGE, fill: false, width: 1.3, head: true }];
        let hit = charts::chart(ui, size, display_ms, WINDOW_MS, &bands, band, &lines, back, grid);
            if let Some(t) = hit.at_ms {
                hit.response.on_hover_ui_at_pointer(|ui| {
                ui.label(hover_text(t, vec![format!("in {}", usage::rate_text(usage::value_at(&rx_l, t).unwrap_or(0.0) * scale)), format!("out {}", usage::rate_text(usage::value_at(&lines[1].points, t).unwrap_or(0.0) * scale))]));
                });
        }
        });
        });
        // Keeps the charts gliding.
        ui.ctx().request_repaint_after(Duration::from_millis(33));
        top
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
            let mut scale = None;
            if !self.history.readings.is_empty() {
                let top = ui.cursor().top();
                scale = Some(self.live_charts(ui));
                self.charts_h.set(ui.cursor().top() - top);
            } else if self.started.elapsed() < Duration::from_secs(4) || i.uptime_s < 8 {
                // The readings are on their way: their space is kept, so the page does not jump when they come.
                ui.add_space(self.charts_h.get());
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
                if let Some(top) = scale {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(format!("Last 2 min, network up to {}", usage::rate_text(top))).size(11.0).color(DIM));
                    });
                }
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
        let mut answer: Option<(&'static str, String)> = None;
        kit::heading(ui, if asking.len() == 1 { "Wants to join" } else { "Want to join" });
        kit::group(ui, |ui, g| {
            for w in asking {
                g.row(ui, kit::ROW_TALL, |ui| {
                    kit::avatar(ui, &w.name, 32.0);
                    ui.add_space(6.0);
                    let width = (ui.available_width() - 180.0).max(100.0);
                    kit::text_block(ui, width, kit::ROW_TALL, kit::NAME_H + kit::LINE_H, |ui| {
                        truncated(ui, RichText::new(&w.name).size(13.5).color(TEXT));
                        ui.label(RichText::new("Asking to use this node").size(12.0).color(AMBER));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(kit::PAD);
                        if kit::button(ui, ("allow", &w.id), "Allow", kit::Kind::Primary, "Let this phone use the node") {
                            answer = Some(("approve", w.id.clone()));
                        }
                        ui.add_space(2.0);
                        if kit::button(ui, ("deny", &w.id), "Deny", kit::Kind::Plain, "Turn this phone down") {
                            answer = Some(("deny", w.id.clone()));
                        }
                    });
                });
            }
        });
        if let Some((what, id)) = answer {
            self.send(Cmd::Phone(what, id));
        }
    }

    fn phones(&mut self, ui: &mut Ui, phones: &[Phone]) {
        kit::heading(ui, &format!("Paired phones ({})", phones.len()));
        if phones.is_empty() {
            kit::group(ui, |ui, g| {
                g.row(ui, kit::ROW, |ui| {
                    ui.label(dim("No phones yet. Open the app on a phone, Settings, Compute nodes, or use Show pairing code."));
                });
            });
            return;
        }
        let now = now_epoch();
        let confirm = self.confirm.clone();
        let (mut ask, mut yes, mut no): (Option<(String, Confirm)>, Option<(String, Confirm)>, bool) = (None, None, false);
        kit::group(ui, |ui, g| {
            for p in phones {
                let confirming = match &confirm {
                    Some((id, what)) if *id == p.id => Some(*what),
                    _ => None,
                };
                g.row(ui, kit::ROW_TALL, |ui| {
                    kit::avatar(ui, &p.name, 32.0);
                    ui.add_space(6.0);
                    let width = (ui.available_width() - 220.0).max(120.0);
                    kit::text_block(ui, width, kit::ROW_TALL, kit::NAME_H + kit::LINE_H, |ui| {
                        truncated(ui, RichText::new(&p.name).size(13.5).color(TEXT));
                        match confirming {
                            Some(what) => ui.label(RichText::new(if what == Confirm::Block { "Block this phone for good?" } else { "Forget this phone?" }).size(11.5).color(AMBER)),
                            None => ui.label(dim(model::phone_line(p, now))),
                        };
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(kit::PAD);
                        match confirming {
                            Some(what) => {
                                if kit::button(ui, ("yes", &p.id), "Yes", kit::Kind::Destructive, "") {
                                    yes = Some((p.id.clone(), what));
                                }
                                if kit::button(ui, ("no", &p.id), "No", kit::Kind::Plain, "") {
                                    no = true;
                                }
                            }
                            None => {
                                if kit::button(ui, ("remove", &p.id), "Remove", kit::Kind::Plain, "Forget this phone. It can pair again later.") {
                                    ask = Some((p.id.clone(), Confirm::Remove));
                                }
                                if kit::button(ui, ("block", &p.id), "Block", kit::Kind::Danger, "Turn this phone away for good, even when new phones can join") {
                                    ask = Some((p.id.clone(), Confirm::Block));
                                }
                            }
                        }
                    });
                });
            }
        });
        if ask.is_some() {
            self.confirm = ask;
        }
        if no {
            self.confirm = None;
        }
        if let Some((id, what)) = yes {
            self.send(Cmd::Phone(if what == Confirm::Block { "block" } else { "revoke" }, id));
            self.confirm = None;
        }
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
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| truncated(ui, RichText::new(&p.name).size(13.0).family(kit::semibold_family()).color(TEXT)));
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
                        truncated(ui, RichText::new(&n.name).size(13.0).family(kit::semibold_family()).color(TEXT));
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

    fn addresses(&mut self, ui: &mut Ui, i: &Info) {
        kit::heading(ui, "Reach it at");
        let just_copied = self.copied.as_ref().filter(|(_, t)| t.elapsed() < Duration::from_millis(1500)).map(|(a, _)| a.clone());
        let mut copied = None;
        kit::group(ui, |ui, g| {
            if i.addresses.is_empty() {
                g.row(ui, kit::ROW, |ui| {
                    ui.label(dim("No network address found"));
                });
            }
            for a in &i.addresses {
                g.setting(ui, a, None, |ui| {
                    let done = just_copied.as_deref() == Some(a.as_str());
                    if kit::button(ui, ("copy", a), if done { "Copied" } else { "Copy" }, kit::Kind::Plain, "Copy this address, to add the node on a phone by hand") {
                        ui.ctx().copy_text(a.clone());
                        copied = Some(a.clone());
                    }
                });
            }
        });
        if let Some(a) = copied {
            self.copied = Some((a, Instant::now()));
            ui.ctx().request_repaint_after(Duration::from_millis(1600));
        }
    }

    /// What the node uses now, as the Settings page shows it.
    fn current_settings(i: &Info) -> (RunDraft, String) {
        (RunDraft::from_setting(i.run_setting.as_deref()), model::chat_choice(i).unwrap_or("e2b").to_string())
    }

    fn settings_page(&mut self, ui: &mut Ui, i: &Info) {
        let current = Self::current_settings(i);
        // Kept as chosen while the node restarts, and filled again from the node once it is back.
        if self.draft.is_none() {
            self.draft = Some(current.clone());
        }
        let Some((mut run, mut chat)) = self.draft.clone() else { return };

        kit::heading(ui, "General");
        let mut startup = self.startup;
        let mut startup_clicked = false;
        kit::group(ui, |ui, g| {
            g.setting(ui, "Start at sign in", Some("The tray and the node come up by themselves"), |ui| {
                startup_clicked = kit::switch(ui, "startup", &mut startup);
            });
        });
        if startup_clicked {
            match crate::autostart::set(startup) {
                Ok(()) => self.startup = startup,
                Err(e) => self.notice = Some((e, Instant::now())),
            }
        }

        kit::heading(ui, "Models");
        kit::group(ui, |ui, g| {
            g.setting(ui, "Chat model", Some("E2B is faster. E4B answers better and needs 2 GB more"), |ui| {
                let mut idx = if chat == "e4b" { 1 } else { 0 };
                if kit::segmented(ui, "chat", &["E2B", "E4B"], &mut idx) {
                    chat = if idx == 1 { "e4b" } else { "e2b" }.into();
                }
            });
            let line = RunDraft::MODES.iter().find(|(v, _, _)| *v == run.mode).map(|(_, _, l)| *l);
            g.setting(ui, "Run the models on", line, |ui| {
                let labels: Vec<&str> = RunDraft::MODES.iter().map(|(_, n, _)| *n).collect();
                let mut idx = RunDraft::MODES.iter().position(|(v, _, _)| *v == run.mode).unwrap_or(0);
                if kit::segmented(ui, "mode", &labels, &mut idx) {
                    let value = RunDraft::MODES[idx].0;
                    if value == "custom" && run.mode != "custom" {
                        // Start from where the models are now.
                        run.chat = i.now_chat.unwrap_or(run.chat);
                        run.search = i.now_search.unwrap_or(run.search);
                        run.speech_gpu = i.now_speech_gpu.unwrap_or(run.speech_gpu);
                        run.photos_gpu = i.now_photos_gpu.unwrap_or(run.photos_gpu);
                    }
                    run.mode = value.into();
                }
            });
            // Auto decides by itself, so this is the one case where what is happening is not what was chosen.
            let auto = run.mode == "auto";
            kit::reveal(ui, "right-now", auto, |ui| {
                g.setting(ui, "Right now", None, |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (name, share) in [("Photos", i.now_photos_gpu.map(|g| if g { 100 } else { 0 })), ("Speech", i.now_speech_gpu.map(|g| if g { 100 } else { 0 })), ("Search", i.now_search), ("Chat", i.now_chat)] {
                        if let Some(sh) = share {
                            let (text, colour) = match sh {
                                100 => (format!("{name} on card"), BLUE),
                                0 => (format!("{name} on processor"), GREY),
                                p => (format!("{name} {p}%"), VIOLET),
                            };
                            kit::pill(ui, &text, colour);
                        }
                    }
                });
            });
            let custom = run.mode == "custom";
            kit::reveal(ui, "custom-rows", custom, |ui| {
                for (label, hint, which) in [("Chat", "How much of it is on the card, the rest uses the processor", 0), ("Search", "How much of it is on the card", 1)] {
                    g.setting(ui, label, Some(hint), |ui| {
                        let v = if which == 0 { &mut run.chat } else { &mut run.search };
                        // Its own fixed space, so the slider stays where it is as the words change length.
                        ui.allocate_ui_with_layout(Vec2::new(76.0, 22.0), Layout::right_to_left(Align::Center), |ui| {
                            ui.set_min_width(76.0);
                            ui.label(RichText::new(match *v { 0 => "processor".to_string(), 100 => "card".to_string(), p => format!("{p}% on card") }).size(12.5).color(DIM));
                        });
                        kit::slider(ui, ("share", which), 170.0, v, 5);
                    });
                }
                for (label, which) in [("Speech", 0), ("Photos", 1)] {
                    g.setting(ui, label, Some("Runs on one side only"), |ui| {
                        let gpu = if which == 0 { &mut run.speech_gpu } else { &mut run.photos_gpu };
                        let mut idx = if *gpu { 0 } else { 1 };
                        if kit::segmented(ui, ("side", which), &["Card", "Processor"], &mut idx) {
                            *gpu = idx == 0;
                        }
                    });
                }
            });
            g.setting(ui, "Tell speakers apart on", Some("The card needs the GPU helper and cuDNN"), |ui| {
                let mut idx = if run.speakers_gpu { 0 } else { 1 };
                if kit::segmented(ui, "speakers", &["Card", "Processor"], &mut idx) {
                    run.speakers_gpu = idx == 0;
                }
            });
        });

        if let Some(dir) = &i.log_dir {
            kit::heading(ui, "Logs");
            kit::group(ui, |ui, g| {
                g.setting(ui, "node.log", Some("Keeps the last 5 files, 5 MB each. Events and numbers only"), |ui| {
                    if kit::button(ui, "open-logs", "Open folder", kit::Kind::Plain, "Open the folder with the log files") {
                        open_folder(dir);
                    }
                });
            });
            truncated(ui, RichText::new(dir).font(FontId::monospace(10.5)).color(DIM));
        }
        ui.add_space(4.0);
        if !self.applying {
            self.draft = Some((run, chat));
        }
    }

    /// The model files: which are there, a Get button for the missing ones, and the download in progress.
    fn models_section(&mut self, ui: &mut Ui) {
        let Some((items, dl)) = self.models.clone() else {
            // Usually here within a frame or two: a message is only for when it takes a while.
            if self.page_at.elapsed() > Duration::from_millis(900) {
                kit::group(ui, |ui, g| {
                    g.row(ui, kit::ROW, |ui| {
                        ui.label(dim("Looking at the models folder..."));
                    });
                });
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(120));
            }
            return;
        };
        let running = matches!(dl, Some(Download::Running { .. }));
        let mut start: Option<ModelItem> = None;
        let mut restart = false;
        kit::group(ui, |ui, g| {
            for it in &items {
                let busy = running && self.fetching.as_ref().map(|(l, _)| l == &it.label).unwrap_or(false);
                g.setting(ui, &it.label, None, |ui| {
                    if it.ready {
                        kit::check(ui, GREEN);
                        ui.label(RichText::new(model::size_text(it.bytes)).size(12.5).color(DIM));
                    } else if busy {
                        kit::pill(ui, "Downloading", BLUE);
                    } else {
                        if !running && kit::button(ui, ("get", &it.label), "Get", kit::Kind::Plain, "Download it from Hugging Face") {
                            start = Some(it.clone());
                        }
                        ui.label(dim("Not downloaded"));
                    }
                });
            }
            match &dl {
                Some(Download::Running { file, done, total }) => {
                    let f = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
                    g.row(ui, kit::ROW_TALL, |ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 5.0;
                            kit::meter(ui, "download", ui.available_width() - kit::PAD, f, BLUE);
                            ui.label(dim(format!("{file}  {}  {}", if *total > 0 { format!("{}%", (f * 100.0) as u32) } else { String::new() }, model::size_text(*done))));
                        });
                    });
                }
                Some(Download::Failed(e)) if self.fetching.is_some() => {
                    g.row(ui, kit::ROW, |ui| {
                        ui.label(RichText::new(e).size(11.5).color(RED));
                    });
                }
                Some(Download::Done) if self.fetching.as_ref().map(|(_, r)| *r).unwrap_or(false) => {
                    g.setting(ui, "Downloaded", Some("Restart the node so it loads the new model"), |ui| {
                        if kit::button(ui, "restart-node", "Restart node", kit::Kind::Primary, "Restart the node so it loads the new model") {
                            restart = true;
                        }
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

    /// Save and Undo float over the page only while there is something to save, rising from the bottom edge and going back the same way.
    fn save_capsule(&mut self, ctx: &egui::Context, area: egui::Rect, i: &Info, on_page: bool) {
        let current = Self::current_settings(i);
        let Some((run, chat)) = self.draft.clone() else { return };
        let changed = (run.clone(), chat.clone()) != current;
        let applying = self.applying;
        let k = kit::spring(ctx, egui::Id::NULL.with("save-capsule"), if on_page && (changed || applying) { 1.0 } else { 0.0 }, 1.0, 0.26);
        if k < 0.01 {
            return;
        }
        let (w, h) = (440.0, 50.0);
        let pos = egui::pos2(area.center().x - w / 2.0, area.bottom() - h - 18.0 + (1.0 - k) * (h + 26.0));
        let (mut save, mut undo) = (false, false);
        egui::Area::new(egui::Id::NULL.with("save-area")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            ui.set_opacity(clamp01(k * 1.5));
            let r = Frame::NONE
                .fill(kit::RAISED)
                .corner_radius(CornerRadius::same(14))
                .stroke(Stroke::new(1.0, Color32::from_white_alpha(14)))
                .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 26, spread: 0, color: Color32::from_black_alpha(130) })
                .inner_margin(Margin { left: 18, right: 11, top: 11, bottom: 11 })
                .show(ui, |ui| {
                    ui.set_width(w - 29.0);
                    ui.allocate_ui_with_layout(Vec2::new(w - 29.0, 28.0), Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        {
                            if kit::button(ui, "save", if applying { "Restarting" } else { "Save and restart" }, if applying { kit::Kind::Plain } else { kit::Kind::Primary }, "Save the settings and restart the node") && changed && !applying {
                                save = true;
                            }
                            if !applying && kit::button(ui, "undo", "Undo", kit::Kind::Plain, "Back to what the node uses now") {
                                undo = true;
                            }
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.label(RichText::new(if applying { "Restarting the node, about a minute" } else { "Saving restarts the node" }).size(12.5).color(kit::SOFT));
                        });
                    });
                });
            kit::sheen(ui, r.response.rect, 14.0, 1.0);
        });
        if save {
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
        if undo {
            self.draft = Some(current);
        }
    }

    /// The code to type on a new phone: each character on its own tile so it reads at a glance, and a ring that empties as it runs out.
    fn pairing_card(&mut self, ui: &mut Ui) {
        if self.pairing.as_ref().map(|(_, until)| *until <= Instant::now()).unwrap_or(false) {
            self.pairing = None;
        }
        if self.pairing.is_some() {
            self.pairing_last = self.pairing.clone();
        }
        let open = self.pairing.is_some();
        let Some((p, until)) = self.pairing_last.clone() else {
            // Nothing to show yet, but the card learns it is closed, so that it opens when a code comes.
            kit::reveal(ui, "pairing", false, |_| {});
            return;
        };
        let left = until.saturating_duration_since(Instant::now()).as_secs_f32();
        let (code, addresses, minutes) = (p.code.clone(), p.addresses.clone(), p.minutes);
        let mut hide = false;
        kit::reveal(ui, "pairing", open, |ui| {
        ui.add_space(6.0);
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.set_max_width(ui.available_width() - 190.0);
                    ui.label(RichText::new("Type this code on the phone").font(kit::semibold(14.0)).color(TEXT));
                    ui.label(RichText::new("Settings, Compute nodes, Add a compute node").size(12.0).color(DIM));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if kit::button(ui, "hide-code", "Done", kit::Kind::Plain, "Hide the code") {
                        hide = true;
                    }
                    ui.add_space(6.0);
                    kit::ring(ui, 20.0, left / (minutes.max(1) * 60) as f32, if left < 30.0 { AMBER } else { kit::ACCENT });
                    ui.label(RichText::new(model::countdown(left as u64)).size(12.5).color(DIM));
                });
            });
            ui.add_space(12.0);
            // The dash only groups the characters, so it is a small mark between tiles and not a tile of its own.
            let n = code.chars().filter(|c| *c != '-').count() as f32;
            let total = n * 44.0 + (n - 1.0) * 6.0 + if code.contains('-') { 20.0 } else { 0.0 };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
                for ch in code.chars() {
                    if ch == '-' {
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(14.0, 56.0), Sense::hover());
                        ui.painter().rect_filled(egui::Rect::from_center_size(rect.center(), Vec2::new(9.0, 2.5)), CornerRadius::same(1), DIM);
                    } else {
                        kit::code_tile(ui, ch);
                    }
                }
            });
            if let Some(a) = addresses.first() {
                ui.add_space(10.0);
                ui.vertical_centered(|ui| ui.label(RichText::new(format!("Or add it by address, {a}")).size(12.0).color(DIM)));
            }
        });
        });
        if hide {
            self.pairing = None;
        }
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }

    /// The sidebar: the mark with the node's state on it as a badge, as on the tray icon, the node's name, then the pages, the selected one
    /// under a highlight that glides to it.
    fn sidebar(&mut self, ui: &mut Ui, view: &Option<View>, info: Option<&Info>) {
        let (title, sub, colour) = match view {
            Some(v) => match &v.poll {
                Poll::Up(i) => (i.name.clone(), hero_word(self.showing(v.state())).to_string(), kit::state_colour(self.showing(v.state()))),
                Poll::Stopping | Poll::Down => ("Marginalia node".to_string(), hero_word(self.showing(v.state())).to_string(), kit::state_colour(self.showing(v.state()))),
            },
            None => ("Marginalia node".to_string(), "Looking for it".to_string(), kit::FAINT),
        };
        // Everything in the sidebar that is not a control moves the window.
        kit::drag_region(ui, ui.max_rect().expand(14.0), "sidebar");
        ui.add_space(6.0);
        let logo = self.logo.get_or_insert_with(|| ui.ctx().load_texture("mark", egui::ColorImage::from_rgba_unmultiplied([96, 96], &crate::glyph::mark_rgba(96)), egui::TextureOptions::LINEAR)).id();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
            ui.painter().image(logo, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            let badge = rect.right_bottom() - Vec2::splat(4.0);
            ui.painter().circle_filled(badge, 6.5, kit::SIDEBAR);
            ui.painter().circle_filled(badge, 4.5, colour);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add_space(0.0);
                truncated(ui, RichText::new(title).font(kit::semibold(13.5)).color(TEXT));
                ui.label(RichText::new(sub).size(12.0).color(DIM));
            });
        });
        ui.add_space(18.0);

        let width = ui.available_width();
        let top = ui.cursor().min;
        // The highlight first, so the rows draw over it.
        let y = self.nav.value() * (NAV_H + NAV_GAP);
        ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(top.x, top.y + y), Vec2::new(width, NAV_H)), CornerRadius::same(7), Color32::from_white_alpha(26));
        let mut chosen = None;
        // The rows are NAV_H + NAV_GAP apart exactly, whatever the usual space between things is: the highlight is placed by that.
        ui.spacing_mut().item_spacing.y = 0.0;
        for page in Page::ALL {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, NAV_H), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            // How much this row is the selected one, as the highlight arrives or leaves it.
            let lit = clamp01(1.0 - (self.nav.value() - page.index() as f32).abs());
            // Pressing answers at once. The page changes when the pointer comes up.
            let down = kit::spring(ui.ctx(), resp.id.with("down"), if resp.is_pointer_button_down_on() { 1.0 } else { 0.0 }, 1.0, 0.08);
            let hover = kit::spring(ui.ctx(), resp.id.with("hover"), if resp.hovered() && lit < 0.5 { 1.0 } else { 0.0 }, 1.0, 0.1);
            if hover > 0.01 || down > 0.01 {
                ui.painter().rect_filled(rect, CornerRadius::same(7), Color32::from_white_alpha((10.0 * hover + 14.0 * down) as u8));
            }
            draw_icon(ui.painter(), page.icon(), egui::pos2(rect.left() + 18.0, rect.center().y), kit::mix(kit::DIM, ACCENT_TEXT, lit));
            ui.painter().text(egui::pos2(rect.left() + 36.0, rect.center().y), egui::Align2::LEFT_CENTER, page.title(), FontId::proportional(13.5), kit::mix(kit::SOFT, Color32::WHITE, lit));
            // The phones waiting to be let in, on the Phones row.
            if let (Page::Phones, Some(i)) = (page, info) {
                if !i.asking.is_empty() {
                    let c = egui::pos2(rect.right() - 16.0, rect.center().y);
                    ui.painter().circle_filled(c, 9.0, AMBER);
                    ui.painter().text(c, egui::Align2::CENTER_CENTER, i.asking.len().to_string(), kit::semibold(11.0), Color32::BLACK);
                }
            }
            if resp.clicked() {
                chosen = Some(page);
            }
            ui.add_space(NAV_GAP);
        }
        if let Some(p) = chosen {
            self.go(p);
        }
    }

    /// The big title of a page, and a line under it.
    fn page_title(&self, ui: &mut Ui, title: &str, line: &str) {
        ui.label(kit::title(title, 26.0));
        // The line keeps its height while its words are still unknown, so the page below does not jump when they come.
        ui.label(RichText::new(if line.is_empty() { " " } else { line }).size(13.0).color(DIM));
        ui.add_space(8.0);
    }

    /// Whether new phones can join, as a switch. A switch moved is shown at its new place at once, and the node's word follows.
    fn node_card(&mut self, ui: &mut Ui, i: &Info) {
        let mut open = self.shown("open", !i.locked);
        let mut open_moved = false;
        kit::heading(ui, "Node");
        kit::group(ui, |ui, g| {
            g.setting(ui, "Let new phones join", Some("Turn it off and each new phone must be allowed first"), |ui| {
                open_moved = kit::switch(ui, "open", &mut open);
            });
        });
        if open_moved {
            self.expect("open", open);
            self.send(Cmd::Click(if open { Action::Unlock } else { Action::Lock }));
        }
    }

    /// The node is not there: the mark, one sentence, and the one thing to do about it. `compact` is for under the Overview's first card,
    /// which already says all that and has the button: just the mark, and the spinner while something is happening.
    fn empty_state(&mut self, ui: &mut Ui, state: State, compact: bool) {
        let (working, title, line) = match state {
            State::Starting => (true, "Starting the node", "It is loading its models. This can take a minute."),
            State::Stopping => (true, "Stopping the node", "It is finishing the jobs it has."),
            _ => (false, "The node is not running", "Your phone works on its own until it is back."),
        };
        ui.add_space((ui.available_height() * if compact { 0.16 } else { 0.22 }).max(30.0));
        ui.vertical_centered(|ui| {
            if let Some(t) = &self.logo {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(72.0), Sense::hover());
                let tint = if state == State::Starting { Color32::WHITE } else { Color32::from_white_alpha(120) };
                ui.painter().image(t.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), tint);
            }
            if !compact {
                ui.add_space(16.0);
                ui.label(kit::title(title, 22.0));
                ui.add_space(2.0);
                ui.label(RichText::new(line).size(13.0).color(DIM));
            }
            ui.add_space(18.0);
            if working {
                kit::spinner(ui, 22.0, kit::SOFT);
            } else if !compact && kit::button(ui, "start", "Start node", kit::Kind::Primary, "Start marginalia-node on this machine") {
                self.send(Cmd::Click(Action::Start));
            }
        });
    }

    /// A message that floats down over the page for a few seconds, and goes back up with its words still on it.
    fn toast(&mut self, ctx: &egui::Context, area: egui::Rect) {
        if let Some((n, _)) = &self.notice {
            self.toast = n.clone();
        }
        let k = kit::spring(ctx, egui::Id::NULL.with("toast"), if self.notice.is_some() { 1.0 } else { 0.0 }, 1.0, 0.32);
        if k < 0.01 || self.toast.is_empty() {
            return;
        }
        let pos = egui::pos2(area.center().x, area.top() + 14.0 - (1.0 - k) * 70.0);
        let text = self.toast.clone();
        egui::Area::new(egui::Id::NULL.with("toast-area")).order(egui::Order::Foreground).pivot(egui::Align2::CENTER_TOP).fixed_pos(pos).show(ctx, |ui| {
            ui.set_opacity(clamp01(k * 1.5));
            ui.set_max_width((area.width() - 60.0).min(520.0));
            let r = Frame::NONE
                .fill(kit::RAISED)
                .corner_radius(CornerRadius::same(14))
                .stroke(Stroke::new(1.0, Color32::from_white_alpha(14)))
                .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 26, spread: 0, color: Color32::from_black_alpha(130) })
                .inner_margin(Margin::symmetric(16, 11))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        dot(ui, RED, 4.0);
                        ui.add(egui::Label::new(RichText::new(text).size(13.0).color(TEXT)).wrap());
                    });
                });
            kit::sheen(ui, r.response.rect, 14.0, 1.0);
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
        let moving = self.animate(&ctx);
        let window = ui.max_rect();
        let radius = if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) { 0 } else { 8 };

        let view = self.view.clone();
        let info = match &view {
            // A node on its way out has nothing left to set: the pages give way to saying so.
            Some(View { poll: Poll::Up(i), .. }) if !i.stopping => Some((**i).clone()),
            _ => None,
        };

        // See-through where the system has a material to show behind it, solid where it does not.
        let sidebar = if self.backdrop == Some(true) { Color32::from_rgba_unmultiplied(18, 18, 20, 205) } else { SIDEBAR };
        egui::Panel::left("nav").exact_size(SIDEBAR_W).resizable(false).show_separator_line(false).frame(Frame::NONE.fill(sidebar).corner_radius(CornerRadius { nw: radius, sw: radius, ne: 0, se: 0 }).inner_margin(Margin { left: 12, right: 12, top: 14, bottom: 12 })).show(ui, |ui| {
            self.sidebar(ui, &view, info.as_ref());
        });

        let mut content = egui::Rect::NOTHING;
        egui::CentralPanel::no_frame().show(ui, |ui| {
            content = ui.max_rect();
            ui.painter().rect_filled(content, CornerRadius { nw: 0, sw: 0, ne: radius, se: radius }, BG);
            // The strip along the top moves the window, where there is no title bar to take hold of.
            kit::drag_region(ui, egui::Rect::from_min_size(content.min, Vec2::new(content.width(), 32.0)), "top");
            // The page comes in from the side it was moved towards as one piece. It is partly there from the very first frame, so a click
            // never shows an empty window, and it moves by whole pixels so the text stays sharp.
            let kind = match (&view, &info) {
                (None, _) => 0,
                (Some(_), None) => 1,
                _ => 2,
            };
            if kind != self.page_kind {
                self.page_kind = kind;
                self.page_dir = 1.0;
                self.begin_page();
            }
            let arrived = clamp01(self.page_in.value());
            let ppp = ctx.pixels_per_point();
            let travel = ((1.0 - arrived) * 8.0 * self.page_dir * ppp).round() / ppp;
            let moved = ui.max_rect().translate(Vec2::new(0.0, travel));
            ui.scope_builder(egui::UiBuilder::new().max_rect(moved), |ui| {
            ui.set_opacity(0.3 + 0.7 * clamp01(arrived * 1.25));
            Frame::NONE.inner_margin(Margin { left: 26, right: 12, top: 12, bottom: 0 }).show(ui, |ui| {
                // Clear of the window's buttons in the top right corner.
                ui.add_space(26.0);
                match (&view, &info) {
                    (None, _) => {
                        ui.add_space((ui.available_height() * 0.36).max(30.0));
                        ui.vertical_centered(|ui| kit::spinner(ui, 22.0, kit::SOFT));
                    }
                    (Some(v), None) => {
                        let state = v.state();
                        if self.page == Page::Overview {
                            // The same column as when the node is up, so the card stays where it was.
                            let room = ui.available_width() - 14.0;
                            let col = room.min(CONTENT_W);
                            ui.horizontal_top(|ui| {
                                ui.add_space((room - col) / 2.0);
                                ui.vertical(|ui| {
                                    ui.set_width(col);
                                    self.hero(ui, state, None);
                                    let state = self.showing(state);
                                    self.empty_state(ui, state, true);
                                });
                            });
                        } else {
                            self.empty_state(ui, state, false);
                        }
                    }
                    (Some(v), Some(i)) => {
                        let state = v.state();
                        // Each page keeps its own place.
                        let out = egui::ScrollArea::vertical().id_salt(self.page.title()).auto_shrink([false, false]).show(ui, |ui| {
                            // The column keeps its width in a wide window and stays in the middle of it, with the scroll bar outside.
                            let room = ui.available_width() - 14.0;
                            let col = room.min(CONTENT_W);
                            ui.horizontal_top(|ui| {
                                ui.add_space((room - col) / 2.0);
                                ui.vertical(|ui| {
                                    ui.set_width(col);
                                    ui.spacing_mut().item_spacing.y = 8.0;
                                    self.page_content(ui, state, i);
                                    ui.add_space(if self.page == Page::Settings { 76.0 } else { 20.0 });
                                });
                            });
                        });
                        // Soft edges where the page runs under the top and bottom of the window, only when there is more that way.
                        let r = out.inner_rect;
                        let offset = out.state.clamped_offset().y;
                        if offset > 1.0 {
                            kit::edge_fade(ui, egui::Rect::from_min_max(r.left_top(), egui::pos2(r.right(), r.top() + 22.0)), BG, true);
                        }
                        if offset + r.height() < out.content_size.y - 1.0 {
                            kit::edge_fade(ui, egui::Rect::from_min_max(egui::pos2(r.left(), r.bottom() - 30.0), r.right_bottom()), BG, false);
                        }
                    }
                }
            });
            });
        });
        // Asked on every page, so it leaves the way it came when the page changes.
        if let Some(i) = &info {
            let on_page = self.page == Page::Settings;
            self.save_capsule(&ctx, content, i, on_page);
        }
        self.toast(&ctx, content);
        let buttons = kit::caption_buttons(&ctx, window, radius);
        kit::resize_edges(&ctx, window, buttons);

        if moving || self.nav.moving() || self.page_in.moving() {
            ctx.request_repaint();
        }
    }
}

impl Panel {
    /// The page's content, under its title.
    fn page_content(&mut self, ui: &mut Ui, state: State, i: &Info) {
        match self.page {
            Page::Overview => {
                self.hero(ui, state, Some(i));
                self.node_card(ui, i);
                kit::heading(ui, "Activity");
                self.activity(ui, i);
                self.speed(ui, i);
                self.addresses(ui, i);
            }
            Page::Phones => {
                let mut pair = false;
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.set_max_width(ui.available_width() - 170.0);
                        self.page_title(ui, "Phones", "The phones that use this node, and how to add one");
                    });
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        ui.with_layout(Layout::top_down(Align::Max), |ui| {
                            // Level with the middle of the title, and with the right edge of what is below it.
                            ui.add_space(5.0);
                            pair = kit::button(ui, "pair", "Show pairing code", kit::Kind::Primary, "Show a code to type on a new phone");
                        });
                    });
                });
                if pair {
                    self.send(Cmd::Pair);
                }
                self.pairing_card(ui);
                self.asking(ui, &i.asking);
                self.phones(ui, &i.phones);
                self.peers(ui, i);
            }
            Page::Models => {
                let line = match &self.models {
                    Some((items, _)) => match items.iter().filter(|m| !m.ready).count() {
                        0 => "Every file the node can use is here".to_string(),
                        n => format!("{n} still to download"),
                    },
                    None => String::new(),
                };
                self.page_title(ui, "Models", &line);
                self.models_section(ui);
            }
            Page::Settings => {
                self.page_title(ui, "Settings", "How the node starts and where its models run");
                self.settings_page(ui, i);
            }
        }
    }
}

/// How long a press of Start or Stop is shown before the node has answered.
const ACT_WAIT: Duration = Duration::from_millis(1500);

/// Which of Start, Pause and Stop can be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Controls {
    start: bool,
    pause: bool,
    stop: bool,
}

impl Controls {
    fn for_state(state: State) -> Controls {
        match state {
            State::Running | State::Busy | State::Locked | State::Paused => Controls { start: false, pause: true, stop: true },
            State::Down => Controls { start: true, pause: false, stop: false },
            State::Starting | State::Stopping => Controls { start: false, pause: false, stop: false },
        }
    }
}

/// What to show for a node in `state` that was just told to become `asked`. Only while it has not moved yet: once the node says anything
/// else, the node is right.
fn showing_state(state: State, asked: Option<State>) -> State {
    match (state, asked) {
        (State::Down, Some(State::Starting)) => State::Starting,
        (State::Running | State::Busy | State::Locked | State::Paused, Some(State::Stopping)) => State::Stopping,
        _ => state,
    }
}

/// The state in one word, for the large type on the Overview and under the name in the sidebar.
fn hero_word(state: State) -> &'static str {
    match state {
        State::Running | State::Locked => "Running",
        State::Busy => "Working",
        State::Paused => "Paused",
        State::Down => "Not running",
        State::Starting => "Starting",
        State::Stopping => "Stopping",
    }
}

pub fn run(port: u16, page: Page, pair: bool) {
    let (rgba, w, h) = (crate::glyph::mark_rgba(64), 64, 64);
    let viewport = egui::ViewportBuilder::default()
        .with_title("Marginalia node")
        .with_app_id("marginalia-node-tray")
        .with_inner_size(WINDOW)
        .with_min_inner_size(WINDOW_MIN)
        // See-through only where the sidebar asks for it. Everything else is painted solid.
        .with_transparent(true)
        // The window has its own title bar, drawn in the sidebar.
        .with_decorations(false)
        .with_resizable(true)
        .with_icon(egui::IconData { rgba, width: w, height: h });
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let result = eframe::run_native(
        "Marginalia node",
        options,
        Box::new(move |cc| {
            let mut style = (*cc.egui_ctx.global_style()).clone();
            style.visuals = egui::Visuals::dark();
            style.visuals.panel_fill = BG;
            // Tooltips, the scroll bar and text selection in the same materials as everything else.
            style.visuals.window_fill = kit::RAISED;
            style.visuals.window_corner_radius = CornerRadius::same(8);
            style.visuals.window_stroke = Stroke::new(1.0, Color32::from_white_alpha(14));
            style.visuals.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(110) };
            style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, kit::SOFT);
            style.visuals.selection.bg_fill = kit::ACCENT;
            style.spacing.scroll = egui::style::ScrollStyle::floating();
            style.visuals.widgets.inactive.weak_bg_fill = TRACK;
            style.visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
            style.visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
            style.visuals.widgets.active.corner_radius = CornerRadius::same(6);
            style.spacing.item_spacing = Vec2::new(6.0, 6.0);
            // Text is not selectable, so a click on a card's words reaches the card.
            style.interaction.selectable_labels = false;
            cc.egui_ctx.set_global_style(style);
            kit::install_fonts(&cc.egui_ctx);
            let panel = Panel::new(port, page, cc.egui_ctx.clone());
            if pair {
                panel.send(Cmd::Pair);
            }
            Ok(Box::new(panel))
        }),
    );
    if let Err(e) = result {
        eprintln!("Could not open the details window: {e}");
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;

    #[test]
    fn a_running_node_can_be_paused_or_stopped_and_not_started() {
        for s in [State::Running, State::Busy, State::Locked, State::Paused] {
            assert_eq!(Controls::for_state(s), Controls { start: false, pause: true, stop: true }, "{s:?}");
        }
    }

    #[test]
    fn a_node_that_is_down_can_only_be_started() {
        assert_eq!(Controls::for_state(State::Down), Controls { start: true, pause: false, stop: false });
    }

    #[test]
    fn nothing_can_be_pressed_while_it_is_changing() {
        let none = Controls { start: false, pause: false, stop: false };
        assert_eq!(Controls::for_state(State::Starting), none);
        assert_eq!(Controls::for_state(State::Stopping), none);
    }

    #[test]
    fn a_press_shows_at_once_until_the_node_says_otherwise() {
        assert_eq!(showing_state(State::Down, Some(State::Starting)), State::Starting);
        assert_eq!(showing_state(State::Busy, Some(State::Stopping)), State::Stopping);
        assert_eq!(showing_state(State::Running, Some(State::Starting)), State::Running, "it has come up: the node is right");
        assert_eq!(showing_state(State::Down, Some(State::Stopping)), State::Down, "it has gone: the node is right");
        assert_eq!(showing_state(State::Down, None), State::Down);
    }
}

#[cfg(test)]
mod page_tests {
    use super::*;

    #[test]
    fn pages_are_found_by_name_in_any_case() {
        assert_eq!(Page::from_name("phones"), Some(Page::Phones));
        assert_eq!(Page::from_name("SETTINGS"), Some(Page::Settings));
        assert_eq!(Page::from_name("models"), Some(Page::Models));
        assert_eq!(Page::from_name("overview"), Some(Page::Overview));
        assert_eq!(Page::from_name("nonsense"), None);
    }

    #[test]
    fn the_sidebar_lists_every_page_once_in_order() {
        let names: Vec<_> = Page::ALL.iter().map(|p| p.title()).collect();
        assert_eq!(names, ["Overview", "Phones", "Models", "Settings"]);
        for (i, p) in Page::ALL.iter().enumerate() {
            assert_eq!(p.index(), i);
        }
    }

    #[test]
    fn every_state_has_a_word() {
        assert_eq!(hero_word(State::Running), "Running");
        assert_eq!(hero_word(State::Busy), "Working");
        assert_eq!(hero_word(State::Paused), "Paused");
        assert_eq!(hero_word(State::Down), "Not running");
        assert_eq!(hero_word(State::Starting), "Starting");
        assert_eq!(hero_word(State::Stopping), "Stopping");
    }

    #[test]
    fn page_titles_use_plain_wording() {
        for p in Page::ALL {
            assert!(!p.title().contains(';') && !p.title().ends_with("..."));
        }
    }
}
