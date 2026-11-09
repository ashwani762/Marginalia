//! The tray's menu: a small window that opens beside the icon, fades in and settles, lights the row under the pointer with a
//! highlight that glides from row to row on a spring, and answers the keys (arrows, Home, End, Enter, Escape). A switch row
//! (Start at sign in) moves its knob on a spring. It draws only while something moves, so it costs nothing at rest.
//!
//! It is its own process (`marginalia-node-tray --menu`), started by the tray with the rows as a line of JSON on its input
//! (`menuspec.rs`). It does nothing but draw and say what was chosen: one line, `action:<Name>`, on its output. The tray carries
//! the action out. That keeps it the same on every system: the position comes in as an argument, and nothing here asks the
//! operating system for anything but the Reduce Motion setting (`os::reduced_motion`).

use crate::anim::{clamp01, lerp, Fade, Spring};
use crate::menuspec::{self, Icon, Nav, Row, Spec};
use crate::kit;
use crate::model::{Action, State};
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Ui};
use std::io::Write;
use std::time::{Duration, Instant};

const BG: Color32 = Color32::from_rgb(38, 38, 41);
const EDGE: Color32 = Color32::from_rgb(70, 70, 74);
const TEXT: Color32 = kit::TEXT;
const DIM: Color32 = kit::DIM;
const FAINT: Color32 = kit::FAINT;
const RED: Color32 = Color32::from_rgb(255, 110, 100);
const LINE: Color32 = Color32::from_rgb(62, 62, 66);
const ACCENT: Color32 = kit::ACCENT;
const TRACK_OFF: Color32 = Color32::from_rgb(76, 76, 80);

/// A colour with part of its opacity, as the window fades.
fn fade(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(clamp01(a))
}

/// The window's title, which is also how the system's border on it is found.
const TITLE: &str = "Marginalia node menu";

pub struct Popover {
    spec: Spec,
    tops: Vec<f32>,
    nav: Nav,
    /// 0 to 1 as the window comes in. A little springy, so it settles.
    enter: Spring,
    /// The highlight: where its top is (in the card), and how visible it is.
    hl_y: Spring,
    hl_a: Spring,
    /// One for each row: a switch's knob, 0 off and 1 on. Unused for the others.
    knobs: Vec<Spring>,
    closing: Option<Fade>,
    /// Closing waits until then (a switch shows its knob arrive before the window goes).
    close_at: Option<Instant>,
    pressed: Option<usize>,
    born: Instant,
    was_focused: bool,
    reduced: bool,
    started_enter: bool,
    last_pointer: Option<Pos2>,
    /// The window's border is taken off once the system has made the window.
    border_done: bool,
}

impl Popover {
    pub fn new(spec: Spec) -> Popover {
        let tops = menuspec::row_tops(&spec);
        let nav = Nav::new(&spec);
        let knobs = spec
            .rows
            .iter()
            .map(|r| match r {
                Row::Item { switch: Some(on), .. } => Spring::new(if *on { 1.0 } else { 0.0 }, 1.0, 0.24),
                _ => Spring::new(0.0, 1.0, 0.2),
            })
            .collect();
        Popover {
            spec,
            tops,
            nav,
            // Critically damped: nothing about opening a menu carries momentum, so it arrives and stays.
            enter: Spring::new(0.0, 1.0, 0.26),
            hl_y: Spring::new(0.0, 1.0, 0.11),
            hl_a: Spring::new(0.0, 1.0, 0.08),
            knobs,
            closing: None,
            close_at: None,
            pressed: None,
            born: Instant::now(),
            was_focused: false,
            reduced: crate::os::reduced_motion(),
            started_enter: false,
            last_pointer: None,
            border_done: false,
        }
    }

    fn row_rect(&self, card: Rect, i: usize) -> Rect {
        let top = card.top() + self.tops[i];
        Rect::from_min_size(pos2(card.left() + menuspec::PAD, top), vec2(card.width() - 2.0 * menuspec::PAD, menuspec::row_height(&self.spec.rows[i])))
    }

    fn row_at(&self, card: Rect, p: Pos2) -> Option<usize> {
        (0..self.spec.rows.len()).find(|&i| self.row_rect(card, i).contains(p))
    }

    fn begin_close(&mut self) {
        if self.closing.is_none() {
            self.closing = Some(Fade::new(1.0, 0.0, 0.16));
        }
    }

    /// A row was chosen: say so on the output at once (the tray starts on it right away), then leave.
    fn choose(&mut self, i: usize) {
        if self.closing.is_some() || self.close_at.is_some() {
            return;
        }
        let Some(Row::Item { action, switch, .. }) = self.spec.rows.get(i).cloned() else { return };
        let mut out = std::io::stdout();
        let _ = writeln!(out, "action:{}", action.name());
        let _ = out.flush();
        if let Some(on) = switch {
            // The knob goes to its new place while the window is still there to show it.
            self.knobs[i].set_target(if on { 0.0 } else { 1.0 });
            self.close_at = Some(Instant::now() + Duration::from_millis(260));
        } else {
            self.begin_close();
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        use egui::Key;
        let (down, up, home, end, enter, esc) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowDown) || (i.key_pressed(Key::Tab) && !i.modifiers.shift),
                i.key_pressed(Key::ArrowUp) || (i.key_pressed(Key::Tab) && i.modifiers.shift),
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
                i.key_pressed(Key::Enter) || i.key_pressed(Key::Space),
                i.key_pressed(Key::Escape),
            )
        });
        if esc {
            self.begin_close();
        }
        if down {
            self.nav.down();
        }
        if up {
            self.nav.up();
        }
        if home {
            self.nav.first();
        }
        if end {
            self.nav.last();
        }
        if enter {
            if let Some(i) = self.nav.selected() {
                self.choose(i);
            }
        }
    }

    /// Focus gone (a click elsewhere) closes the menu, as every menu does. Not before it has had focus once.
    fn focus(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.viewport().focused);
        if focused == Some(true) {
            self.was_focused = true;
        }
        if self.was_focused && focused == Some(false) {
            self.begin_close();
        }
        // A window that never got focus (another program holds it) must not stay for ever.
        if !self.was_focused && self.born.elapsed() > Duration::from_secs(20) {
            self.begin_close();
        }
    }

    fn pointer(&mut self, ctx: &egui::Context, card: Rect) {
        let (pos, moved, pressed, released) = ctx.input(|i| (i.pointer.hover_pos(), i.pointer.delta() != vec2(0.0, 0.0), i.pointer.primary_pressed(), i.pointer.primary_released()));
        let over = pos.and_then(|p| self.row_at(card, p));
        // The pointer moving takes the selection. A pointer that is only resting does not take it back from the keys.
        if moved && pos != self.last_pointer {
            self.nav.point(over);
        }
        self.last_pointer = pos;
        if pressed {
            self.pressed = over;
        }
        if released {
            if let (Some(a), Some(b)) = (self.pressed, over) {
                if a == b {
                    self.choose(a);
                }
            }
            self.pressed = None;
        }
    }

    /// Moves every spring on by the time since the last frame, and says whether anything is still moving.
    fn animate(&mut self, dt: f32) -> bool {
        if !self.started_enter {
            self.started_enter = true;
            self.enter.set_target(1.0);
        }
        // The highlight appears where its row is, then glides from row to row.
        match self.nav.selected() {
            Some(i) => {
                let target = self.tops[i];
                if self.hl_a.value() < 0.02 {
                    self.hl_y.snap(target);
                } else {
                    self.hl_y.set_target(target);
                }
                self.hl_a.set_target(1.0);
            }
            None => self.hl_a.set_target(0.0),
        }
        let r = self.reduced;
        self.enter.step(dt, r);
        self.hl_y.step(dt, r);
        self.hl_a.step(dt, r);
        for k in &mut self.knobs {
            k.step(dt, r);
        }
        if let Some(f) = &mut self.closing {
            f.step(dt, r);
        }
        if let Some(t) = self.close_at {
            if Instant::now() >= t {
                self.close_at = None;
                self.begin_close();
            }
        }
        self.enter.moving() || self.hl_y.moving() || self.hl_a.moving() || self.knobs.iter().any(|k| k.moving()) || self.closing.is_some() || self.close_at.is_some()
    }

    fn paint(&self, painter: &Painter, window: Rect) {
        let out = self.closing.map(|f| f.value()).unwrap_or(1.0);
        let alpha = clamp01(self.enter.value() * 1.5) * out;
        let card = window.shrink(menuspec::SHADOW);

        painter.add(egui::epaint::Shadow { offset: [0, 3], blur: 12, spread: 0, color: Color32::from_black_alpha((120.0 * alpha) as u8) }.as_shape(card, CornerRadius::same(12)));
        painter.rect_filled(card, CornerRadius::same(12), fade(BG, alpha * 0.99));
        painter.rect_stroke(card.shrink(0.5), CornerRadius::same(12), Stroke::new(1.0, fade(EDGE, alpha)), StrokeKind::Inside);
        // Light catching the top edge.
        painter.line_segment([pos2(card.left() + 12.0, card.top() + 1.5), pos2(card.right() - 12.0, card.top() + 1.5)], Stroke::new(1.0, Color32::from_white_alpha((14.0 * alpha) as u8)));

        self.paint_header(painter, card, alpha);

        // The highlight, under the rows.
        let ha = self.hl_a.value();
        if ha > 0.003 {
            let pressed = self.pressed.is_some() && self.pressed == self.nav.selected();
            let rect = Rect::from_min_size(pos2(card.left() + menuspec::PAD, card.top() + self.hl_y.value()), vec2(card.width() - 2.0 * menuspec::PAD, menuspec::ROW_H));
            // Lit the moment the pointer goes down: a shade deeper while held.
            painter.rect_filled(rect, CornerRadius::same(6), fade(if pressed { kit::mix(ACCENT, Color32::BLACK, 0.16) } else { ACCENT }, alpha * ha));
        }

        for (i, row) in self.spec.rows.iter().enumerate() {
            let rect = self.row_rect(card, i);
            match row {
                Row::Separator => {
                    let y = rect.center().y;
                    painter.line_segment([pos2(rect.left() + 10.0, y), pos2(rect.right() - 10.0, y)], Stroke::new(1.0, fade(LINE, alpha)));
                }
                Row::Note(t) => {
                    painter.text(pos2(rect.left() + 12.0, rect.center().y), Align2::LEFT_CENTER, t, FontId::proportional(12.5), fade(FAINT, alpha));
                }
                Row::Item { text, icon, switch, danger, .. } => {
                    // How much this row is the selected one, so its words brighten as the highlight arrives.
                    let lit = if self.nav.selected() == Some(i) { ha } else { 0.0 };
                    let rest = if *danger { RED } else { TEXT };
                    let words = blend(rest, Color32::WHITE, lit);
                    let picture = blend(if *danger { RED } else { DIM }, Color32::WHITE, lit);
                    draw_icon(painter, *icon, pos2(rect.left() + 18.0, rect.center().y), fade(picture, alpha));
                    painter.text(pos2(rect.left() + 36.0, rect.center().y), Align2::LEFT_CENTER, text, FontId::proportional(13.0), fade(words, alpha));
                    if switch.is_some() {
                        draw_switch(painter, pos2(rect.right() - 26.0, rect.center().y), self.knobs[i].value(), alpha, lit);
                    }
                }
            }
        }
    }

    fn paint_header(&self, painter: &Painter, card: Rect, alpha: f32) {
        let left = card.left() + menuspec::PAD + 12.0;
        let mid_y = card.top() + menuspec::PAD + menuspec::HEADER_H / 2.0 - 1.0;
        let colour = kit::state_colour(self.spec.state);
        // The state's colour as a dot with a soft ring, the way the icon's badge is.
        painter.circle_filled(pos2(left + 6.0, mid_y), 10.0, fade(colour, alpha * 0.16));
        painter.circle_filled(pos2(left + 6.0, mid_y), 5.0, fade(colour, alpha));
        let max_w = card.right() - menuspec::PAD - 12.0 - (left + 26.0);
        let title = fit(painter, &self.spec.title, kit::semibold(13.5), max_w);
        painter.text(pos2(left + 26.0, mid_y - 8.0), Align2::LEFT_CENTER, title, kit::semibold(13.5), fade(TEXT, alpha));
        let sub = fit(painter, &self.spec.subtitle, FontId::proportional(12.0), max_w);
        painter.text(pos2(left + 26.0, mid_y + 9.0), Align2::LEFT_CENTER, sub, FontId::proportional(12.0), fade(DIM, alpha));
    }
}

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = clamp01(t);
    Color32::from_rgb(lerp(a.r() as f32, b.r() as f32, t) as u8, lerp(a.g() as f32, b.g() as f32, t) as u8, lerp(a.b() as f32, b.b() as f32, t) as u8)
}

/// The text cut to fit `max_w`, with dots at the cut.
fn fit(painter: &Painter, text: &str, font: FontId, max_w: f32) -> String {
    let width = |s: &str| painter.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x;
    if width(text) <= max_w {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let s: String = chars.iter().collect::<String>() + "...";
        if width(&s) <= max_w {
            return s;
        }
    }
    String::new()
}

/// `lit` is how much its row is highlighted: on the accent colour the track goes pale so it still reads.
fn draw_switch(painter: &Painter, centre: Pos2, on: f32, alpha: f32, lit: f32) {
    let track = Rect::from_center_size(centre, vec2(32.0, 19.0));
    let colour = blend(blend(TRACK_OFF, ACCENT, on), Color32::from_rgb(190, 182, 252), lit * 0.75);
    painter.rect_filled(track, CornerRadius::same(10), fade(colour, alpha));
    let x = lerp(track.left() + 9.5, track.right() - 9.5, on);
    painter.circle_filled(pos2(x, centre.y + 0.6), 8.0, Color32::from_black_alpha((46.0 * alpha) as u8));
    painter.circle_filled(pos2(x, centre.y), 7.8, fade(Color32::WHITE, alpha));
}

/// The row pictures, drawn with lines and shapes so there are no image files. Each fits a box about 14 points across.
pub(crate) fn draw_icon(p: &Painter, icon: Icon, c: Pos2, col: Color32) {
    let stroke = Stroke::new(1.6, col);
    match icon {
        Icon::None => {}
        Icon::Pause => {
            for dx in [-3.2, 3.2] {
                p.rect_filled(Rect::from_center_size(pos2(c.x + dx, c.y), vec2(3.0, 10.0)), CornerRadius::same(1), col);
            }
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![pos2(c.x - 3.5, c.y - 5.5), pos2(c.x - 3.5, c.y + 5.5), pos2(c.x + 5.5, c.y)], col, Stroke::NONE));
        }
        Icon::Window => {
            p.rect_stroke(Rect::from_center_size(c, vec2(13.0, 11.0)), CornerRadius::same(2), stroke, StrokeKind::Inside);
            p.line_segment([pos2(c.x - 6.0, c.y - 2.0), pos2(c.x + 6.0, c.y - 2.0)], stroke);
        }
        Icon::Gear => {
            p.circle_stroke(c, 3.0, stroke);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::TAU / 8.0;
                let (s, co) = a.sin_cos();
                p.line_segment([pos2(c.x + co * 5.0, c.y + s * 5.0), pos2(c.x + co * 7.0, c.y + s * 7.0)], Stroke::new(2.0, col));
            }
            p.circle_stroke(c, 5.0, Stroke::new(1.4, col));
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(c, vec2(9.0, 9.0)), CornerRadius::same(2), col);
        }
        Icon::Exit => {
            p.add(Shape::line(vec![pos2(c.x - 1.0, c.y - 6.0), pos2(c.x - 6.0, c.y - 6.0), pos2(c.x - 6.0, c.y + 6.0), pos2(c.x - 1.0, c.y + 6.0)], stroke));
            p.line_segment([pos2(c.x - 2.0, c.y), pos2(c.x + 6.0, c.y)], stroke);
            p.add(Shape::line(vec![pos2(c.x + 3.0, c.y - 3.0), pos2(c.x + 6.0, c.y), pos2(c.x + 3.0, c.y + 3.0)], stroke));
        }
        Icon::Chart => {
            // Three bars of different heights.
            for (dx, h) in [(-4.5, 5.0), (0.0, 11.0), (4.5, 8.0)] {
                p.rect_filled(Rect::from_min_max(pos2(c.x + dx - 1.7, c.y + 5.5 - h), pos2(c.x + dx + 1.7, c.y + 5.5)), CornerRadius::same(1), col);
            }
        }
        Icon::Phone => {
            p.rect_stroke(Rect::from_center_size(c, vec2(9.0, 14.0)), CornerRadius::same(2), stroke, StrokeKind::Inside);
            p.circle_filled(pos2(c.x, c.y + 4.2), 0.9, col);
        }
        Icon::Stack => {
            // Three layers, the top one drawn as a diamond.
            p.add(Shape::line(vec![pos2(c.x - 6.0, c.y - 1.0), pos2(c.x, c.y - 4.5), pos2(c.x + 6.0, c.y - 1.0), pos2(c.x, c.y + 2.5), pos2(c.x - 6.0, c.y - 1.0)], stroke));
            p.add(Shape::line(vec![pos2(c.x - 6.0, c.y + 2.2), pos2(c.x, c.y + 5.5), pos2(c.x + 6.0, c.y + 2.2)], stroke));
        }
        Icon::Power => {
            // An open ring with a stroke at the top.
            let pts: Vec<Pos2> = (0..=16).map(|k| {
                let a = (-60.0f32 + k as f32 * 300.0 / 16.0).to_radians();
                pos2(c.x + a.sin() * 5.4, c.y - a.cos() * 5.4)
            }).collect();
            p.add(Shape::line(pts, stroke));
            p.line_segment([pos2(c.x, c.y - 7.0), pos2(c.x, c.y - 1.5)], stroke);
        }
    }
}

impl eframe::App for Popover {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let window = ui.max_rect();
        let dt = ctx.input(|i| i.stable_dt);
        if !self.started_enter {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if !self.border_done {
            self.border_done = crate::os::remove_window_border(TITLE);
        }
        self.keys(&ctx);
        self.focus(&ctx);
        let card = window.shrink(menuspec::SHADOW);
        self.pointer(&ctx, card);
        let moving = self.animate(dt);
        // The window grows out of the edge nearest the icon and goes back the same way, text and all.
        let out = self.closing.map(|f| f.value()).unwrap_or(1.0);
        let scale = lerp(0.94, 1.0, clamp01(self.enter.value())) * lerp(0.96, 1.0, out);
        let edge = if self.spec.rise { card.bottom() } else { card.top() };
        let at = vec2(card.center().x, edge);
        ctx.set_transform_layer(ui.layer_id(), egui::emath::TSTransform::new(at * (1.0 - scale), scale));
        self.paint(ui.painter(), window);
        if self.closing.as_ref().map(|f| f.done()).unwrap_or(false) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if moving {
            ctx.request_repaint();
        } else if let Some(t) = self.close_at {
            ctx.request_repaint_after(t.saturating_duration_since(Instant::now()));
        } else {
            // At rest nothing is drawn. A look at the focus now and then is all the window does.
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

/// Shows the menu and returns when it closes. `at` is the window's top left corner in points, or none to let the system place it.
pub fn run(spec: Spec, at: Option<(f32, f32)>) {
    let (w, h) = menuspec::window_size(&spec);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_app_id("marginalia-node-tray-menu")
        .with_inner_size([w, h])
        .with_resizable(false)
        .with_decorations(false)
        .with_transparent(true)
        // The card has its own shadow: the system's one for a window without a frame would draw a box round it.
        .with_has_shadow(false)
        .with_always_on_top()
        .with_taskbar(false);
    if let Some((x, y)) = at {
        viewport = viewport.with_position([x, y]);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let result = eframe::run_native(
        "Marginalia node menu",
        options,
        Box::new(move |cc| {
            let mut style = (*cc.egui_ctx.global_style()).clone();
            style.visuals = egui::Visuals::dark();
            // Nothing behind the card but the shadow: the window is see-through everywhere else.
            style.visuals.panel_fill = Color32::TRANSPARENT;
            style.visuals.window_fill = Color32::TRANSPARENT;
            style.visuals.extreme_bg_color = Color32::TRANSPARENT;
            style.visuals.window_stroke = Stroke::NONE;
            style.visuals.window_shadow = egui::epaint::Shadow::NONE;
            style.interaction.selectable_labels = false;
            cc.egui_ctx.set_global_style(style);
            kit::install_fonts(&cc.egui_ctx);
            Ok(Box::new(Popover::new(spec)))
        }),
    );
    if let Err(e) = result {
        eprintln!("Could not show the menu: {e}");
        std::process::exit(1);
    }
}

/// What the window shows when started by hand without rows: every kind of row, so it can be looked at and tried.
pub fn sample() -> Spec {
    Spec {
        title: "falcon".into(),
        subtitle: "Running, 2 phones".into(),
        state: State::Running,
        rows: vec![
            Row::Item { text: "Pause".into(), action: Action::Pause, icon: Icon::Pause, switch: None, danger: false },
            Row::Item { text: "Show details".into(), action: Action::ShowDetails, icon: Icon::Window, switch: None, danger: false },
            Row::Item { text: "Settings".into(), action: Action::ShowSettings, icon: Icon::Gear, switch: None, danger: false },
            Row::Item { text: "Start at sign in".into(), action: Action::ToggleStartup, icon: Icon::Power, switch: Some(false), danger: false },
            Row::Separator,
            Row::Item { text: "Stop node".into(), action: Action::Stop, icon: Icon::Stop, switch: None, danger: true },
            Row::Item { text: "Quit tray".into(), action: Action::Quit, icon: Icon::Exit, switch: None, danger: false },
        ],
        rise: true,
    }
}
