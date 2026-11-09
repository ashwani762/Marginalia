//! What the tray's menu window shows and where it goes. The tray builds a `Spec` from what it knows and hands it over as a line
//! of JSON, so the window opens with everything it needs and asks the node nothing. The window only draws it and says which
//! action was chosen: every side effect stays in the tray.
//!
//! Plain data and plain arithmetic, with no operating system and no drawing, so it is the same on Windows and on Linux and is
//! tested without a screen: the rows, the sizes, the place next to the icon, and the keys that move the selection.

use crate::model::{Action, Entry, Poll, State, View};
use serde_json::{json, Value};

/// A picture beside a row. Drawn by the window with simple shapes, so no image files are needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    None,
    Pause,
    Play,
    Window,
    Gear,
    Stop,
    Exit,
    Power,
    /// For the window's sidebar: the overview, the phones and the models.
    Chart,
    Phone,
    Stack,
}

impl Icon {
    fn name(self) -> &'static str {
        match self {
            Icon::None => "none",
            Icon::Pause => "pause",
            Icon::Play => "play",
            Icon::Window => "window",
            Icon::Gear => "gear",
            Icon::Stop => "stop",
            Icon::Exit => "exit",
            Icon::Power => "power",
            Icon::Chart => "chart",
            Icon::Phone => "phone",
            Icon::Stack => "stack",
        }
    }

    fn from_name(s: &str) -> Icon {
        [Icon::Pause, Icon::Play, Icon::Window, Icon::Gear, Icon::Stop, Icon::Exit, Icon::Power, Icon::Chart, Icon::Phone, Icon::Stack].into_iter().find(|i| i.name() == s).unwrap_or(Icon::None)
    }

    fn of(action: Action) -> Icon {
        match action {
            Action::Pause => Icon::Pause,
            Action::Resume | Action::Start => Icon::Play,
            Action::ShowDetails => Icon::Window,
            Action::ShowSettings => Icon::Gear,
            Action::Stop => Icon::Stop,
            Action::ToggleStartup => Icon::Power,
            Action::Quit => Icon::Exit,
            Action::Lock | Action::Unlock => Icon::None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Separator,
    /// A line that cannot be chosen, for example "Starting node".
    Note(String),
    Item {
        text: String,
        action: Action,
        icon: Icon,
        /// Some for a switch row: whether it is on.
        switch: Option<bool>,
        /// Drawn in red: it ends something.
        danger: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub title: String,
    pub subtitle: String,
    pub state: State,
    pub rows: Vec<Row>,
    /// True when the window sits above the icon (a taskbar at the bottom), so it rises into place. False: it drops.
    pub rise: bool,
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl Spec {
    /// From what the tray knows: the same entries as the native menu (`menu_with_startup`), the first of which becomes the
    /// heading of the window and the switch for starting at sign in a row with a switch.
    pub fn from_view(v: &View, startup: bool) -> Spec {
        let (title, subtitle) = match &v.poll {
            Poll::Down if v.starting => ("Marginalia node".to_string(), "Starting, this can take a minute".to_string()),
            Poll::Down => ("Marginalia node".to_string(), "Not running".to_string()),
            Poll::Up(i) => {
                let word = match v.state() {
                    State::Paused => "Paused",
                    State::Busy => "Busy",
                    State::Locked => "Running, locked",
                    _ => "Running",
                };
                let mut parts = vec![word.to_string()];
                if i.running_jobs > 0 {
                    parts.push(plural(i.running_jobs, "job", "jobs"));
                }
                parts.push(plural(i.phones.len() as u64, "phone", "phones"));
                (i.name.clone(), parts.join(", "))
            }
        };
        let mut rows = Vec::new();
        let mut seen_heading = false;
        for e in crate::model::menu_with_startup(v, startup) {
            match e {
                // The first line of text is the heading the window already has.
                Entry::Info(_) if !seen_heading => seen_heading = true,
                Entry::Info(t) => rows.push(Row::Note(t.trim_end_matches('.').to_string())),
                Entry::Separator => {
                    // A separator right after the heading, or two in a row, is only space.
                    if !matches!(rows.last(), None | Some(Row::Separator)) {
                        rows.push(Row::Separator);
                    }
                }
                Entry::Item(text, action) => {
                    let switch = (action == Action::ToggleStartup).then_some(startup);
                    let text = if switch.is_some() { "Start at sign in".to_string() } else { text };
                    rows.push(Row::Item { text, action, icon: Icon::of(action), switch, danger: action == Action::Stop });
                }
            }
        }
        while matches!(rows.last(), Some(Row::Separator)) {
            rows.pop();
        }
        // The switch sits with the settings, after the last of the rows above the first separator, and not between Stop and Quit.
        if let Some(at) = rows.iter().position(|r| matches!(r, Row::Item { action: Action::ToggleStartup, .. })) {
            let row = rows.remove(at);
            let first_sep = rows.iter().position(|r| matches!(r, Row::Separator)).unwrap_or(rows.len());
            rows.insert(first_sep.min(at), row);
        }
        Spec { title, subtitle, state: v.state(), rows, rise: true }
    }

    pub fn to_json(&self) -> String {
        let rows: Vec<Value> = self
            .rows
            .iter()
            .map(|r| match r {
                Row::Separator => json!({"t": "sep"}),
                Row::Note(t) => json!({"t": "note", "text": t}),
                Row::Item { text, action, icon, switch, danger } => json!({"t": "item", "text": text, "action": action.name(), "icon": icon.name(), "switch": switch, "danger": danger}),
            })
            .collect();
        json!({"title": self.title, "subtitle": self.subtitle, "state": format!("{:?}", self.state), "rise": self.rise, "rows": rows}).to_string()
    }

    pub fn from_json(s: &str) -> Option<Spec> {
        let v: Value = serde_json::from_str(s).ok()?;
        let state = [State::Running, State::Busy, State::Paused, State::Locked, State::Down].into_iter().find(|st| format!("{st:?}") == v["state"].as_str().unwrap_or(""))?;
        let mut rows = Vec::new();
        for r in v["rows"].as_array()? {
            rows.push(match r["t"].as_str()? {
                "sep" => Row::Separator,
                "note" => Row::Note(r["text"].as_str()?.to_string()),
                "item" => Row::Item {
                    text: r["text"].as_str()?.to_string(),
                    action: Action::from_name(r["action"].as_str()?)?,
                    icon: Icon::from_name(r["icon"].as_str().unwrap_or("none")),
                    switch: r["switch"].as_bool(),
                    danger: r["danger"].as_bool().unwrap_or(false),
                },
                _ => return None,
            });
        }
        Some(Spec { title: v["title"].as_str()?.to_string(), subtitle: v["subtitle"].as_str()?.to_string(), state, rows, rise: v["rise"].as_bool().unwrap_or(true) })
    }
}

// ---------------------------------------------------------------------------------------------
// Sizes, in points. The window is the card plus room on every side for its shadow.
// ---------------------------------------------------------------------------------------------

pub const CARD_W: f32 = 248.0;
pub const SHADOW: f32 = 18.0;
pub const PAD: f32 = 6.0;
pub const HEADER_H: f32 = 54.0;
pub const ROW_H: f32 = 30.0;
pub const SEP_H: f32 = 11.0;

pub fn row_height(r: &Row) -> f32 {
    match r {
        Row::Separator => SEP_H,
        _ => ROW_H,
    }
}

/// The card's height: padding, the heading, every row, padding.
pub fn card_height(spec: &Spec) -> f32 {
    PAD + HEADER_H + spec.rows.iter().map(row_height).sum::<f32>() + PAD
}

/// The window's size: the card and its shadow room.
pub fn window_size(spec: &Spec) -> (f32, f32) {
    (CARD_W + 2.0 * SHADOW, card_height(spec) + 2.0 * SHADOW)
}

/// Where each row starts, from the top of the card.
pub fn row_tops(spec: &Spec) -> Vec<f32> {
    let mut y = PAD + HEADER_H;
    spec.rows
        .iter()
        .map(|r| {
            let top = y;
            y += row_height(r);
            top
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Where the window goes
// ---------------------------------------------------------------------------------------------

/// A rectangle in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The window's top left corner in points (pixels divided by the scale).
    pub x: f32,
    pub y: f32,
    /// True when the window is above the icon.
    pub rise: bool,
}

/// How far the card keeps from the icon and from the screen's edge, in points.
const GAP: f32 = 6.0;
const EDGE: f32 = 8.0;

/// The window next to the tray `icon` on the `monitor` (both in pixels, `scale` pixels to a point), on the side with room: above
/// an icon in the lower half of the screen, below one in the upper half, and as near the icon's middle as the screen allows.
pub fn place(icon: Rect, monitor: Rect, size: (f32, f32), scale: f32) -> Placement {
    let s = scale.max(0.5);
    let (w, h) = (size.0 * s, size.1 * s);
    let shadow = SHADOW * s;
    let icon_mid_x = icon.x + icon.w / 2.0;
    let rise = icon.y + icon.h / 2.0 > monitor.y + monitor.h / 2.0;
    let mut y = if rise { icon.y - h + shadow - GAP * s } else { icon.y + icon.h - shadow + GAP * s };
    let mut x = icon_mid_x - w / 2.0;
    // The card (not its shadow room) stays on the screen.
    let (min_x, max_x) = (monitor.x + EDGE * s - shadow, monitor.x + monitor.w - w - EDGE * s + shadow);
    let (min_y, max_y) = (monitor.y + EDGE * s - shadow, monitor.y + monitor.h - h - EDGE * s + shadow);
    x = x.clamp(min_x, max_x.max(min_x));
    y = y.clamp(min_y, max_y.max(min_y));
    Placement { x: x / s, y: y / s, rise }
}

// ---------------------------------------------------------------------------------------------
// The keys
// ---------------------------------------------------------------------------------------------

/// Which row is selected. The arrow keys skip what cannot be chosen and go round at the ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nav {
    chosen: Vec<bool>,
    selected: Option<usize>,
}

impl Nav {
    pub fn new(spec: &Spec) -> Nav {
        Nav { chosen: spec.rows.iter().map(|r| matches!(r, Row::Item { .. })).collect(), selected: None }
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The pointer is over a row (or over none).
    pub fn point(&mut self, row: Option<usize>) {
        self.selected = row.filter(|i| self.chosen.get(*i).copied().unwrap_or(false));
    }

    pub fn down(&mut self) {
        self.go(1);
    }

    pub fn up(&mut self) {
        self.go(-1);
    }

    pub fn first(&mut self) {
        self.selected = self.chosen.iter().position(|c| *c);
    }

    pub fn last(&mut self) {
        self.selected = self.chosen.iter().rposition(|c| *c);
    }

    fn go(&mut self, step: i32) {
        let n = self.chosen.len() as i32;
        if !self.chosen.iter().any(|c| *c) {
            return;
        }
        // Nothing selected yet: down goes to the first, up to the last.
        let mut i = match self.selected {
            Some(i) => i as i32,
            None if step > 0 => -1,
            None => n,
        };
        loop {
            i = (i + step).rem_euclid(n);
            if self.chosen[i as usize] {
                self.selected = Some(i as usize);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Info;

    fn up(name: &str) -> View {
        View { poll: Poll::Up(Box::new(Info { name: name.into(), ..Default::default() })), starting: false }
    }

    fn down(starting: bool) -> View {
        View { poll: Poll::Down, starting }
    }

    fn items(spec: &Spec) -> Vec<(String, Action)> {
        spec.rows
            .iter()
            .filter_map(|r| match r {
                Row::Item { text, action, .. } => Some((text.clone(), *action)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_running_node_gets_its_name_and_a_line_about_its_work() {
        let mut i = Info { name: "falcon".into(), running_jobs: 2, ..Default::default() };
        i.phones = vec![Default::default(), Default::default(), Default::default()];
        let s = Spec::from_view(&View { poll: Poll::Up(Box::new(i)), starting: false }, false);
        assert_eq!((s.title.as_str(), s.subtitle.as_str()), ("falcon", "Busy, 2 jobs, 3 phones"));
    }

    #[test]
    fn a_stopped_or_starting_node_says_so() {
        assert_eq!(Spec::from_view(&down(false), false).subtitle, "Not running");
        assert_eq!(Spec::from_view(&down(true), false).subtitle, "Starting, this can take a minute");
        assert_eq!(Spec::from_view(&down(false), false).state, State::Down);
    }

    #[test]
    fn the_rows_are_the_native_menu_without_its_heading() {
        let v = up("falcon");
        let s = Spec::from_view(&v, false);
        let texts: Vec<_> = items(&s).into_iter().map(|(t, _)| t).collect();
        assert_eq!(texts, ["Pause", "Show details", "Settings", "Start at sign in", "Stop node", "Quit tray"]);
        assert!(!matches!(s.rows.first(), Some(Row::Separator)), "no space left where the heading was");
        assert!(!matches!(s.rows.last(), Some(Row::Separator)));
    }

    #[test]
    fn starting_at_sign_in_is_a_switch_that_shows_its_state() {
        for on in [true, false] {
            let s = Spec::from_view(&up("n"), on);
            let row = s.rows.iter().find_map(|r| match r {
                Row::Item { action: Action::ToggleStartup, switch, text, .. } => Some((switch.unwrap(), text.clone())),
                _ => None,
            });
            assert_eq!(row, Some((on, "Start at sign in".to_string())));
        }
    }

    #[test]
    fn stopping_is_red_and_nothing_else_is() {
        let s = Spec::from_view(&up("n"), false);
        for r in &s.rows {
            if let Row::Item { action, danger, .. } = r {
                assert_eq!(*danger, *action == Action::Stop);
            }
        }
    }

    #[test]
    fn a_starting_node_shows_a_note_that_cannot_be_chosen() {
        let s = Spec::from_view(&down(true), false);
        assert!(s.rows.iter().any(|r| matches!(r, Row::Note(t) if t.contains("Starting"))));
        assert!(!items(&s).iter().any(|(_, a)| *a == Action::Start));
    }

    #[test]
    fn it_survives_the_trip_as_json() {
        for v in [up("falcon"), down(false), down(true)] {
            for on in [true, false] {
                let s = Spec::from_view(&v, on);
                assert_eq!(Spec::from_json(&s.to_json()), Some(s));
            }
        }
        assert_eq!(Spec::from_json("not json"), None);
        assert_eq!(Spec::from_json("{}"), None);
        assert_eq!(Spec::from_json(r#"{"title":"a","subtitle":"b","state":"Running","rows":[{"t":"item","text":"x","action":"Nope"}]}"#), None);
    }

    #[test]
    fn the_rows_fit_the_card_exactly() {
        let s = Spec::from_view(&up("falcon"), false);
        let tops = row_tops(&s);
        let last = s.rows.len() - 1;
        assert_eq!(tops[last] + row_height(&s.rows[last]) + PAD, card_height(&s));
        assert_eq!(tops[0], PAD + HEADER_H);
        let (w, h) = window_size(&s);
        assert_eq!((w, h), (CARD_W + 36.0, card_height(&s) + 36.0));
    }

    const SCREEN: Rect = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };

    #[test]
    fn an_icon_in_a_bottom_taskbar_gets_the_window_above_it_in_its_middle() {
        let icon = Rect { x: 1700.0, y: 1048.0, w: 16.0, h: 32.0 };
        let p = place(icon, SCREEN, (288.0, 300.0), 1.0);
        assert!(p.rise);
        assert_eq!(p.x, 1708.0 - 144.0, "centred on the icon");
        // The card's lower edge is the gap above the icon.
        assert_eq!(p.y + 300.0 - SHADOW, 1048.0 - GAP);
    }

    #[test]
    fn an_icon_in_a_top_bar_gets_the_window_below_it() {
        let icon = Rect { x: 1800.0, y: 0.0, w: 24.0, h: 28.0 };
        let p = place(icon, SCREEN, (288.0, 300.0), 1.0);
        assert!(!p.rise);
        assert_eq!(p.y + SHADOW, 28.0 + GAP);
    }

    #[test]
    fn the_window_stays_on_the_screen() {
        let right = place(Rect { x: 1900.0, y: 1050.0, w: 16.0, h: 30.0 }, SCREEN, (288.0, 300.0), 1.0);
        assert_eq!(right.x + 288.0 - SHADOW, 1920.0 - EDGE, "the card's right edge keeps its distance from the screen's edge");
        let left = place(Rect { x: 2.0, y: 1050.0, w: 16.0, h: 30.0 }, SCREEN, (288.0, 300.0), 1.0);
        assert_eq!(left.x + SHADOW, EDGE);
        let tall = place(Rect { x: 900.0, y: 1050.0, w: 16.0, h: 30.0 }, SCREEN, (288.0, 1500.0), 1.0);
        assert!(tall.y >= EDGE - SHADOW, "a window taller than the screen does not start above it");
    }

    #[test]
    fn a_sharper_screen_gives_the_same_place_in_points() {
        // 150 percent: the same spot is 1.5 times as many pixels, and the result is in points.
        let a = place(Rect { x: 1000.0, y: 700.0, w: 16.0, h: 30.0 }, Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }, (288.0, 300.0), 1.0);
        let b = place(Rect { x: 1500.0, y: 1050.0, w: 24.0, h: 45.0 }, Rect { x: 0.0, y: 0.0, w: 2880.0, h: 1620.0 }, (288.0, 300.0), 1.5);
        assert!((a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01, "{a:?} {b:?}");
    }

    #[test]
    fn a_second_monitor_to_the_left_is_handled_in_its_own_coordinates() {
        let second = Rect { x: -1920.0, y: 0.0, w: 1920.0, h: 1080.0 };
        let p = place(Rect { x: -100.0, y: 1050.0, w: 16.0, h: 30.0 }, second, (288.0, 300.0), 1.0);
        assert!(p.x >= -1920.0 + EDGE - SHADOW && p.x + 288.0 - SHADOW <= -EDGE + 0.01, "{p:?}");
    }

    #[test]
    fn the_arrow_keys_skip_what_cannot_be_chosen_and_go_round() {
        let s = Spec::from_view(&up("n"), false);
        let mut nav = Nav::new(&s);
        assert_eq!(nav.selected(), None);
        nav.down();
        let first = nav.selected().unwrap();
        assert!(matches!(s.rows[first], Row::Item { .. }));
        // Down through everything lands only on items and comes back to the first.
        let n_items = s.rows.iter().filter(|r| matches!(r, Row::Item { .. })).count();
        for _ in 0..n_items {
            nav.down();
            assert!(matches!(s.rows[nav.selected().unwrap()], Row::Item { .. }));
        }
        assert_eq!(nav.selected(), Some(first));
        nav.up();
        let last = nav.selected().unwrap();
        assert!(matches!(s.rows[last], Row::Item { .. }) && last > first, "up from the first goes round to the last");
    }

    #[test]
    fn up_with_nothing_selected_starts_at_the_end() {
        let s = Spec::from_view(&up("n"), false);
        let mut nav = Nav::new(&s);
        nav.up();
        let mut end = Nav::new(&s);
        end.last();
        assert_eq!(nav.selected(), end.selected());
    }

    #[test]
    fn the_pointer_only_selects_rows_that_can_be_chosen() {
        let s = Spec::from_view(&down(true), false);
        let mut nav = Nav::new(&s);
        let note = s.rows.iter().position(|r| matches!(r, Row::Note(_))).unwrap();
        nav.point(Some(note));
        assert_eq!(nav.selected(), None);
        nav.point(Some(999));
        assert_eq!(nav.selected(), None);
    }

    #[test]
    fn a_menu_with_nothing_to_choose_does_not_loop() {
        let s = Spec { title: "t".into(), subtitle: "s".into(), state: State::Down, rows: vec![Row::Note("a".into()), Row::Separator], rise: true };
        let mut nav = Nav::new(&s);
        nav.down();
        nav.up();
        nav.first();
        nav.last();
        assert_eq!(nav.selected(), None);
    }

    #[test]
    fn plain_wording_in_every_line() {
        for v in [up("falcon"), down(false), down(true)] {
            let s = Spec::from_view(&v, true);
            let mut lines = vec![s.title.clone(), s.subtitle.clone()];
            for r in &s.rows {
                match r {
                    Row::Note(t) => lines.push(t.clone()),
                    Row::Item { text, .. } => lines.push(text.clone()),
                    Row::Separator => {}
                }
            }
            for l in lines {
                assert!(!l.contains(';') && !l.contains('\u{2014}') && !l.ends_with("..."), "bad wording: {l}");
            }
        }
    }
}
