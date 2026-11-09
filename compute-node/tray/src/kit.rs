//! The look and the building blocks of the details window and the menu: type, colour, inset groups of rows, a switch, a segmented
//! control, buttons, meters. The rules they follow:
//!
//! - Feedback starts when the pointer goes down, not when it comes up. What is chosen is decided when it comes up.
//! - Everything moves on springs, from where it is now, so a new target in mid-motion bends the path instead of jumping. Springs are
//!   critically damped: a click carries no momentum, so nothing overshoots.
//! - Weight, size and tracking make the hierarchy together, and the system's own font is used where it has one.
//!
//! Every moving part keeps its spring in egui's own memory under its id, so a block needs no state of its own and a new one starts
//! where it is. Nothing here asks the system for anything but its fonts and the Reduce Motion setting. The same code runs on Windows
//! and on Linux.

use crate::anim::{clamp01, lerp, Spring};
use crate::model::State;
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Id, Layout, Rect, RichText, Sense, Stroke, Ui};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(28, 28, 30);
pub const SIDEBAR: Color32 = Color32::from_rgb(21, 21, 23);
pub const GROUP: Color32 = Color32::from_rgb(40, 40, 43);
/// A surface floating over the page.
pub const RAISED: Color32 = Color32::from_rgb(54, 54, 58);
pub const HAIR: Color32 = Color32::from_rgb(56, 56, 60);
pub const TRACK: Color32 = Color32::from_rgb(62, 62, 66);
pub const TEXT: Color32 = Color32::from_rgb(245, 245, 247);
pub const SOFT: Color32 = Color32::from_rgb(209, 209, 214);
pub const DIM: Color32 = Color32::from_rgb(152, 152, 157);
pub const FAINT: Color32 = Color32::from_rgb(99, 99, 102);
/// The mark's indigo.
pub const ACCENT: Color32 = Color32::from_rgb(124, 108, 246);
pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(142, 128, 250);
pub const GREEN: Color32 = Color32::from_rgb(48, 209, 88);
pub const BLUE: Color32 = Color32::from_rgb(64, 156, 255);
pub const ORANGE: Color32 = Color32::from_rgb(255, 159, 10);
pub const RED: Color32 = Color32::from_rgb(255, 69, 58);
pub const PURPLE: Color32 = Color32::from_rgb(191, 90, 242);

/// The row height without and with a line of explanation under its name.
pub const ROW: f32 = 44.0;
pub const ROW_TALL: f32 = 56.0;
const PAD: f32 = 16.0;

/// The two usual springs: things that glide to a new place, and quick answers to the pointer.
const GLIDE: (f32, f32) = (1.0, 0.28);
const ANSWER: (f32, f32) = (1.0, 0.10);

const SEMIBOLD: &str = "semibold";

pub fn state_colour(s: State) -> Color32 {
    match s {
        State::Running => GREEN,
        State::Busy => BLUE,
        State::Paused => ORANGE,
        State::Locked => PURPLE,
        State::Down => RED,
    }
}

/// A heavier cut of the interface font. `install_fonts` must have run.
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, semibold_family())
}

pub fn semibold_family() -> FontFamily {
    FontFamily::Name(SEMIBOLD.into())
}

/// A large title. Big type reads too loose at normal spacing, so it is drawn a little tighter the larger it is.
pub fn title(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text.into()).font(semibold(size)).extra_letter_spacing(-size * 0.014).color(TEXT)
}

/// The font files to try, regular and semibold, best first.
fn font_candidates() -> Vec<(String, String)> {
    let mut list = Vec::new();
    if cfg!(windows) {
        let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        list.push((format!(r"{dir}\Fonts\segoeui.ttf"), format!(r"{dir}\Fonts\seguisb.ttf")));
    } else {
        for (a, b) in [
            ("/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf", "/usr/share/fonts/truetype/ubuntu/Ubuntu-M.ttf"),
            ("/usr/share/fonts/opentype/inter/Inter-Regular.otf", "/usr/share/fonts/opentype/inter/Inter-SemiBold.otf"),
            ("/usr/share/fonts/truetype/inter/Inter-Regular.ttf", "/usr/share/fonts/truetype/inter/Inter-SemiBold.ttf"),
            ("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf"),
            ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
        ] {
            list.push((a.to_string(), b.to_string()));
        }
    }
    list
}

/// Uses the system's interface font when it can be found, with egui's own fonts behind it for anything it lacks. Without one, egui's
/// own stand in, so a machine with unusual fonts still shows everything.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut semibold = fallback.clone();
    let found = font_candidates().into_iter().find_map(|(r, b)| Some((std::fs::read(r).ok()?, std::fs::read(b).ok()?)));
    if let Some((regular, heavy)) = found {
        fonts.font_data.insert("system".into(), Arc::new(FontData::from_owned(regular)));
        fonts.font_data.insert("system-semibold".into(), Arc::new(FontData::from_owned(heavy)));
        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.insert(0, "system".into());
        }
        semibold.insert(0, "system-semibold".into());
    }
    fonts.families.insert(semibold_family(), semibold);
    ctx.set_fonts(fonts);
}

/// A spring in egui's memory: where `id`'s value is now, moved on by the time since the last frame towards `target`. It starts at the
/// target. Asks for frames while it moves and not when it is at rest.
pub fn spring(ctx: &egui::Context, id: Id, target: f32, damping: f32, response: f32) -> f32 {
    let dt = ctx.input(|i| i.stable_dt);
    let reduced = reduced(ctx);
    let mut s = ctx.data_mut(|d| d.get_temp::<Spring>(id)).unwrap_or_else(|| Spring::new(target, damping, response));
    s.set_target(target);
    s.step(dt, reduced);
    if s.moving() {
        ctx.request_repaint();
    }
    ctx.data_mut(|d| d.insert_temp(id, s));
    s.value()
}

/// Reduce Motion, asked of the system once and kept.
fn reduced(ctx: &egui::Context) -> bool {
    let id = Id::NULL.with("kit-reduced-motion");
    if let Some(v) = ctx.data(|d| d.get_temp::<bool>(id)) {
        return v;
    }
    let v = crate::os::reduced_motion();
    ctx.data_mut(|d| d.insert_temp(id, v));
    v
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = clamp01(t);
    Color32::from_rgb(lerp(a.r() as f32, b.r() as f32, t) as u8, lerp(a.g() as f32, b.g() as f32, t) as u8, lerp(a.b() as f32, b.b() as f32, t) as u8)
}

/// The focus ring is for people using the keys: it shows after Tab and goes when the pointer is used.
fn keys_in_use(ctx: &egui::Context) -> bool {
    let id = Id::NULL.with("kit-keys-in-use");
    let (tab, pointer) = ctx.input(|i| (i.key_pressed(egui::Key::Tab), i.pointer.any_pressed()));
    let mut on = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    if tab {
        on = true;
    }
    if pointer {
        on = false;
    }
    ctx.data_mut(|d| d.insert_temp(id, on));
    on
}

fn focus_ring(ui: &Ui, resp: &egui::Response, rect: Rect, radius: u8) {
    if resp.has_focus() && keys_in_use(ui.ctx()) {
        ui.painter().rect_stroke(rect.expand(2.0), CornerRadius::same(radius + 2), Stroke::new(2.0, ACCENT_LIGHT), egui::StrokeKind::Outside);
    }
}

// ---------------------------------------------------------------------------------------------
// Headings and groups
// ---------------------------------------------------------------------------------------------

/// A quiet title above a group.
pub fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(text).font(semibold(12.5)).color(SOFT));
    ui.add_space(2.0);
}

/// Rows of one kind of thing in one rounded block, each row set off from the one above by a hairline.
pub struct Group {
    rows: usize,
}

pub fn group(ui: &mut Ui, add: impl FnOnce(&mut Ui, &mut Group)) {
    let mut g = Group { rows: 0 };
    let r = egui::Frame::NONE.fill(GROUP).corner_radius(CornerRadius::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        add(ui, &mut g);
    });
    sheen(ui, r.response.rect, 12.0, 1.0);
}

/// Light catching the top edge of a surface: one faint line, inset by the corners.
pub fn sheen(ui: &Ui, rect: Rect, radius: f32, alpha: f32) {
    let y = rect.top() + 0.5;
    ui.painter().line_segment([pos2(rect.left() + radius, y), pos2(rect.right() - radius, y)], Stroke::new(1.0, Color32::from_white_alpha((11.0 * alpha) as u8)));
}

impl Group {
    /// One row of `height`, its content laid out left to right and centred. The first row has no line above it.
    pub fn row(&mut self, ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui)) -> egui::Response {
        let width = ui.available_width();
        if self.rows > 0 {
            let y = ui.cursor().top();
            let left = ui.cursor().left();
            ui.painter().line_segment([pos2(left + PAD, y), pos2(left + width, y)], Stroke::new(1.0, HAIR));
        }
        self.rows += 1;
        ui.allocate_ui_with_layout(vec2(width, height), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_size(vec2(width, height));
            ui.add_space(PAD);
            add(ui);
        })
        .response
    }

    /// A name (and a line under it) at the left, and the control `add` at the right.
    pub fn setting(&mut self, ui: &mut Ui, title: &str, line: Option<&str>, add: impl FnOnce(&mut Ui)) {
        let height = if line.is_some() { ROW_TALL } else { ROW };
        self.row(ui, height, |ui| {
            // The control takes the room it needs, and the words get what is left. Words too long for it end in dots and show in full on hover.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(PAD);
                add(ui);
                ui.add_space(14.0);
                let width = ui.available_width().max(40.0);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    text_block(ui, width, height, if line.is_some() { NAME_H + LINE_H } else { NAME_H }, |ui| {
                        ui.add(egui::Label::new(RichText::new(title).size(13.5).color(TEXT)).truncate());
                        if let Some(l) = line {
                            ui.add(egui::Label::new(RichText::new(l).size(12.0).color(DIM)).truncate());
                        }
                    });
                });
            });
        });
    }
}

/// The height of a name, and of a line of explanation under it, with the space between.
pub const NAME_H: f32 = 18.0;
pub const LINE_H: f32 = 17.0;

/// Lines of text one above the other, `width` wide, centred in a row `height` high. `content` is how high the lines are together
/// (`NAME_H`, plus `LINE_H` for a line under it): the space above is what is left over, halved. A name alone sits in the middle of
/// its row and a name with a line under it as a pair.
pub fn text_block(ui: &mut Ui, width: f32, height: f32, content: f32, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(vec2(width, height), Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.set_max_width(width);
        ui.add_space(((height - content) / 2.0).max(0.0));
        add(ui);
    });
}

/// A soft fade at the top or bottom edge of a scrolling area, where content meets what is above or below it: no hard line.
pub fn edge_fade(ui: &Ui, rect: Rect, colour: Color32, solid_at_top: bool) {
    let mut mesh = egui::Mesh::default();
    let (top, bottom) = if solid_at_top { (colour, Color32::TRANSPARENT) } else { (Color32::TRANSPARENT, colour) };
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    ui.painter().add(egui::Shape::mesh(mesh));
}

// ---------------------------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------------------------

/// A switch. The knob stretches the moment it is pressed, runs to its side on a spring, and the track takes the accent colour as it
/// goes. True when clicked.
pub fn switch(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, on: &mut bool) -> bool {
    let id = ui.scope_id().with(("kit-switch", id));
    let (rect, resp) = ui.allocate_exact_size(vec2(42.0, 25.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let ctx = ui.ctx().clone();
    let k = spring(&ctx, id, if *on { 1.0 } else { 0.0 }, GLIDE.0, 0.24);
    let hover = spring(&ctx, id.with("h"), if resp.hovered() { 1.0 } else { 0.0 }, ANSWER.0, ANSWER.1);
    let press = spring(&ctx, id.with("p"), if resp.is_pointer_button_down_on() { 1.0 } else { 0.0 }, ANSWER.0, ANSWER.1);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(13), mix(mix(TRACK, Color32::from_rgb(76, 76, 80), hover), ACCENT, k));
    let (r, inset) = (10.5, 2.0);
    let x = lerp(rect.left() + inset + r, rect.right() - inset - r, k);
    // The stretch reaches towards the side the knob would go to.
    let stretch = press * 5.0;
    let (left, right) = (x - r - stretch * k, x + r + stretch * (1.0 - k));
    let knob = Rect::from_min_max(pos2(left, rect.center().y - r), pos2(right, rect.center().y + r));
    painter.add(egui::epaint::Shadow { offset: [0, 1], blur: 4, spread: 0, color: Color32::from_black_alpha(70) }.as_shape(knob, CornerRadius::same(11)));
    painter.rect_filled(knob, CornerRadius::same(11), Color32::WHITE);
    focus_ring(ui, &resp, rect, 13);
    let clicked = resp.clicked();
    if clicked {
        *on = !*on;
    }
    clicked
}

/// A choice of one among a few, side by side. The pill glides to the chosen one and its label lights as the pill arrives. Returns true
/// when the choice was changed.
pub fn segmented(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, options: &[&str], selected: &mut usize) -> bool {
    let id = ui.scope_id().with(("kit-segmented", id));
    let font = FontId::proportional(12.5);
    let widths: Vec<f32> = options.iter().map(|o| ui.painter().layout_no_wrap(o.to_string(), font.clone(), TEXT).size().x + 24.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let (outer, _) = ui.allocate_exact_size(vec2(total, 28.0), Sense::hover());
    let ctx = ui.ctx().clone();
    // Where each part is.
    let mut lefts = Vec::new();
    let mut x = outer.left() + 2.0;
    for w in &widths {
        lefts.push(x);
        x += w;
    }
    let sel = (*selected).min(options.len().saturating_sub(1));
    let pill_x = spring(&ctx, id.with("x"), lefts[sel], GLIDE.0, GLIDE.1);
    let pill_w = spring(&ctx, id.with("w"), widths[sel], GLIDE.0, GLIDE.1);
    ui.painter().rect_filled(outer, CornerRadius::same(8), Color32::from_rgb(30, 30, 32));
    let mut responses = Vec::new();
    for (i, w) in widths.iter().enumerate() {
        let rect = Rect::from_min_size(pos2(lefts[i], outer.top() + 2.0), vec2(*w, 24.0));
        responses.push((rect, ui.interact(rect, id.with(i), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)));
    }
    let held = responses[sel].1.is_pointer_button_down_on();
    let press = spring(&ctx, id.with("p"), if held { 1.0 } else { 0.0 }, ANSWER.0, ANSWER.1);
    let pill = Rect::from_min_size(pos2(pill_x, outer.top() + 2.0), vec2(pill_w, 24.0)).shrink(press * 1.0);
    ui.painter().add(egui::epaint::Shadow { offset: [0, 1], blur: 3, spread: 0, color: Color32::from_black_alpha(60) }.as_shape(pill, CornerRadius::same(6)));
    ui.painter().rect_filled(pill, CornerRadius::same(6), Color32::from_rgb(92, 92, 96));
    let mut changed = false;
    for (i, label) in options.iter().enumerate() {
        let (rect, resp) = &responses[i];
        // How much the pill is under this part now, so the label lights as it comes and dims as it goes.
        let lit = clamp01(1.0 - ((pill_x + pill_w / 2.0) - rect.center().x).abs() / (rect.width() * 0.9));
        let down = spring(&ctx, id.with(("d", i)), if resp.is_pointer_button_down_on() && i != sel { 1.0 } else { 0.0 }, ANSWER.0, ANSWER.1);
        let hover = spring(&ctx, id.with(("h", i)), if resp.hovered() && i != sel { 1.0 } else { 0.0 }, ANSWER.0, ANSWER.1);
        let c = mix(mix(mix(DIM, TEXT, hover), DIM, down), Color32::WHITE, lit);
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font.clone(), c);
        focus_ring(ui, resp, *rect, 6);
        if resp.clicked() && i != sel {
            *selected = i;
            changed = true;
        }
    }
    changed
}

/// What kind of action a button is, which gives its colour.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Primary,
    /// Ends something, asked for once: red words on a quiet ground.
    Danger,
    /// The answer to "are you sure": solid red.
    Destructive,
}

/// A button that answers the pointer: it lifts on hover and sinks the moment it is pressed. True when clicked.
pub fn button(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, text: &str, kind: Kind, hint: &str) -> bool {
    let id = ui.scope_id().with(("kit-button", id));
    let font = if matches!(kind, Kind::Primary | Kind::Destructive) { semibold(13.0) } else { FontId::proportional(13.0) };
    let w = ui.painter().layout_no_wrap(text.to_string(), font.clone(), TEXT).size().x + 26.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w.max(64.0), 28.0), Sense::click());
    let mut resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if !hint.is_empty() {
        resp = resp.on_hover_text(hint);
    }
    let ctx = ui.ctx().clone();
    let hover = spring(&ctx, id.with("h"), if resp.hovered() { 1.0 } else { 0.0 }, ANSWER.0, ANSWER.1);
    let press = spring(&ctx, id.with("p"), if resp.is_pointer_button_down_on() { 1.0 } else { 0.0 }, ANSWER.0, 0.06);
    let (base, lifted, words) = match kind {
        Kind::Plain => (Color32::from_rgb(68, 68, 72), Color32::from_rgb(84, 84, 88), TEXT),
        Kind::Primary => (ACCENT, ACCENT_LIGHT, Color32::WHITE),
        Kind::Danger => (Color32::from_rgb(64, 42, 44), Color32::from_rgb(82, 48, 50), Color32::from_rgb(255, 120, 110)),
        Kind::Destructive => (Color32::from_rgb(214, 58, 50), RED, Color32::WHITE),
    };
    let r = rect.shrink(press * 1.0);
    let fill = mix(mix(base, lifted, hover), Color32::BLACK, press * 0.14);
    ui.painter().rect_filled(r, CornerRadius::same(7), fill);
    sheen(ui, r, 7.0, 1.2);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, text, font, words);
    focus_ring(ui, &resp, rect, 7);
    resp.clicked()
}

/// A meter: the filled part glides to `fraction` (0 to 1) when it changes.
pub fn meter(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, width: f32, fraction: f32, colour: Color32) {
    let id = ui.scope_id().with(("kit-meter", id));
    let (rect, _) = ui.allocate_exact_size(vec2(width.max(20.0), 6.0), Sense::hover());
    let f = spring(ui.ctx(), id, clamp01(fraction), GLIDE.0, 0.4);
    ui.painter().rect_filled(rect, CornerRadius::same(3), Color32::from_white_alpha(20));
    if f > 0.002 {
        let mut fill = rect;
        fill.set_width((rect.width() * f).max(6.0));
        ui.painter().rect_filled(fill, CornerRadius::same(3), colour);
    }
}

/// A figure and its caption, one above the other, in `width`.
pub fn figure(ui: &mut Ui, width: f32, value: &str, caption: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 46.0), Sense::hover());
    ui.painter().text(pos2(rect.center().x, rect.top() + 15.0), Align2::CENTER_CENTER, value, semibold(20.0), TEXT);
    ui.painter().text(pos2(rect.center().x, rect.top() + 36.0), Align2::CENTER_CENTER, caption, FontId::proportional(11.5), DIM);
}

/// A thin upright line between things in a row.
pub fn divider(ui: &mut Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, height), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, HAIR);
}

/// The window's own close, minimise and zoom buttons. They go grey when the window is not in front, and show what they do when the
/// pointer is near any of them.
pub fn window_controls(ui: &mut Ui) {
    const D: f32 = 12.0;
    const GAP: f32 = 8.0;
    let (area, _) = ui.allocate_exact_size(vec2(3.0 * D + 2.0 * GAP, D), Sense::hover());
    let ctx = ui.ctx().clone();
    let (focused, maximized, near) = ctx.input(|i| (i.viewport().focused.unwrap_or(true), i.viewport().maximized.unwrap_or(false), i.pointer.hover_pos().is_some_and(|p| area.expand(5.0).contains(p))));
    let id = ui.scope_id().with("kit-window-controls");
    let shown = spring(&ctx, id.with("near"), if near { 1.0 } else { 0.0 }, ANSWER.0, ANSWER.1);
    let awake = spring(&ctx, id.with("focus"), if focused || near { 1.0 } else { 0.0 }, GLIDE.0, 0.18);
    let colours = [Color32::from_rgb(255, 95, 87), Color32::from_rgb(254, 188, 46), Color32::from_rgb(40, 200, 64)];
    for (n, base) in colours.into_iter().enumerate() {
        let c = pos2(area.left() + D / 2.0 + n as f32 * (D + GAP), area.center().y);
        let hit = Rect::from_center_size(c, vec2(D + GAP, D + 10.0));
        let resp = ui.interact(hit, id.with(n), Sense::click());
        let down = spring(&ctx, id.with(("down", n)), if resp.is_pointer_button_down_on() { 1.0 } else { 0.0 }, ANSWER.0, 0.06);
        let fill = mix(mix(Color32::from_rgb(74, 74, 78), base, awake), Color32::BLACK, down * 0.28);
        ui.painter().circle_filled(c, D / 2.0, fill);
        let mark = Color32::from_black_alpha((150.0 * shown) as u8);
        if shown > 0.02 {
            let stroke = Stroke::new(1.3, mark);
            match n {
                0 => {
                    ui.painter().line_segment([c + vec2(-2.6, -2.6), c + vec2(2.6, 2.6)], stroke);
                    ui.painter().line_segment([c + vec2(-2.6, 2.6), c + vec2(2.6, -2.6)], stroke);
                }
                1 => {
                    ui.painter().line_segment([c + vec2(-3.2, 0.0), c + vec2(3.2, 0.0)], stroke);
                }
                _ => {
                    // Two small corners pointing apart.
                    ui.painter().add(egui::Shape::convex_polygon(vec![c + vec2(-3.0, 1.2), c + vec2(-3.0, -3.0), c + vec2(1.2, -3.0)], mark, Stroke::NONE));
                    ui.painter().add(egui::Shape::convex_polygon(vec![c + vec2(3.0, -1.2), c + vec2(3.0, 3.0), c + vec2(-1.2, 3.0)], mark, Stroke::NONE));
                }
            }
        }
        if resp.clicked() {
            ctx.send_viewport_cmd(match n {
                0 => egui::ViewportCommand::Close,
                1 => egui::ViewportCommand::Minimized(true),
                _ => egui::ViewportCommand::Maximized(!maximized),
            });
        }
    }
}

/// Makes `rect` move the window when dragged and zoom it when double clicked. Call it before what is drawn over it: whatever comes
/// later and can be clicked takes the pointer first.
pub fn drag_region(ui: &mut Ui, rect: Rect, name: &str) {
    let resp = ui.interact(rect, ui.scope_id().with(("kit-drag", name)), Sense::click_and_drag());
    let ctx = ui.ctx().clone();
    if resp.double_clicked() {
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    } else if resp.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

/// Which edge or corner of `window` the point is on, if any.
pub fn edge_at(window: Rect, p: egui::Pos2) -> Option<egui::ResizeDirection> {
    use egui::ResizeDirection as D;
    const EDGE: f32 = 5.0;
    const CORNER: f32 = 14.0;
    if !window.expand(1.0).contains(p) {
        return None;
    }
    let (l, r, t, b) = (p.x - window.left(), window.right() - p.x, p.y - window.top(), window.bottom() - p.y);
    let near_side = l < CORNER || r < CORNER;
    let near_end = t < CORNER || b < CORNER;
    Some(match () {
        _ if t < EDGE && l < CORNER || l < EDGE && t < CORNER => D::NorthWest,
        _ if t < EDGE && r < CORNER || r < EDGE && t < CORNER => D::NorthEast,
        _ if b < EDGE && l < CORNER || l < EDGE && b < CORNER => D::SouthWest,
        _ if b < EDGE && r < CORNER || r < EDGE && b < CORNER => D::SouthEast,
        _ if t < EDGE && !near_side => D::North,
        _ if b < EDGE && !near_side => D::South,
        _ if l < EDGE && !near_end => D::West,
        _ if r < EDGE && !near_end => D::East,
        _ => return None,
    })
}

/// Resizing from the edges of a window with no frame of its own: the pointer shows it, and a press starts it.
pub fn resize_edges(ctx: &egui::Context, window: Rect) {
    use egui::{CursorIcon as C, ResizeDirection as D};
    let (pos, pressed, maximized) = ctx.input(|i| (i.pointer.latest_pos(), i.pointer.primary_pressed(), i.viewport().maximized.unwrap_or(false)));
    let (Some(p), false) = (pos, maximized) else { return };
    let Some(dir) = edge_at(window, p) else { return };
    ctx.set_cursor_icon(match dir {
        D::North | D::South => C::ResizeVertical,
        D::East | D::West => C::ResizeHorizontal,
        D::NorthWest | D::SouthEast => C::ResizeNwSe,
        D::NorthEast | D::SouthWest => C::ResizeNeSw,
    });
    if pressed {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
    }
}

/// The waiting sign: spokes round a centre, the brightness running round them.
pub fn spinner(ui: &mut Ui, size: f32, colour: Color32) {
    const SPOKES: usize = 12;
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let t = ui.input(|i| i.time);
    let head = ((t * 1.1).fract() * SPOKES as f64) as usize;
    let c = rect.center();
    let (inner, outer) = (size * 0.27, size * 0.5);
    for n in 0..SPOKES {
        let (s, co) = (n as f32 * std::f32::consts::TAU / SPOKES as f32).sin_cos();
        let behind = (head + SPOKES - n) % SPOKES;
        let a = 1.0 - behind as f32 / SPOKES as f32;
        ui.painter().line_segment([c + vec2(s, -co) * inner, c + vec2(s, -co) * outer], Stroke::new(size * 0.09, colour.gamma_multiply(0.18 + 0.82 * a)));
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
}

/// A round picture with the first letter of a name, for a phone.
pub fn avatar(ui: &mut Ui, name: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    // The same name gets the same colour each time.
    let hues = [ACCENT, BLUE, Color32::from_rgb(52, 199, 190), Color32::from_rgb(255, 130, 90), PURPLE, Color32::from_rgb(90, 190, 110)];
    let pick = name.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize)) % hues.len();
    ui.painter().circle_filled(rect.center(), size / 2.0, mix(GROUP, hues[pick], 0.75));
    let letter = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, letter, semibold(size * 0.45), Color32::WHITE);
}

/// A tick.
pub fn check(ui: &mut Ui, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
    let c = rect.center();
    ui.painter().add(egui::Shape::line(vec![c + vec2(-4.5, 0.5), c + vec2(-1.2, 3.8), c + vec2(4.8, -3.6)], Stroke::new(1.8, colour)));
}

/// One character in its own tile, for a code to be read off the screen and typed.
pub fn code_tile(ui: &mut Ui, ch: char) {
    let (rect, _) = ui.allocate_exact_size(vec2(44.0, 56.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(10), Color32::from_rgb(30, 30, 33));
    sheen(ui, rect, 10.0, 0.8);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, ch, semibold(28.0), TEXT);
}

/// A ring that empties as time runs out, drawn from the top going clockwise.
pub fn ring(ui: &mut Ui, size: f32, fraction: f32, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let c = rect.center();
    let r = size / 2.0 - 1.5;
    ui.painter().circle_stroke(c, r, Stroke::new(2.4, Color32::from_white_alpha(22)));
    let f = clamp01(fraction);
    let n = ((f * 64.0) as usize).max(2);
    let pts: Vec<_> = (0..=n).map(|k| {
        let a = k as f32 / n as f32 * f * std::f32::consts::TAU;
        c + vec2(a.sin(), -a.cos()) * r
    }).collect();
    ui.painter().add(egui::Shape::line(pts, Stroke::new(2.4, colour)));
}

/// A slider in steps of `step` from 0 to 100. The fill follows the pointer exactly, the knob grows while held. True when it changed.
pub fn slider(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, width: f32, value: &mut u8, step: u8) -> bool {
    let id = ui.scope_id().with(("kit-slider", id));
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click_and_drag());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let ctx = ui.ctx().clone();
    let mut changed = false;
    let (left, right) = (rect.left() + 9.0, rect.right() - 9.0);
    if let Some(p) = resp.interact_pointer_pos() {
        let raw = clamp01((p.x - left) / (right - left)) * 100.0;
        let v = ((raw / step as f32).round() as u8).saturating_mul(step).min(100);
        if v != *value {
            *value = v;
            changed = true;
        }
    }
    let held = spring(&ctx, id.with("p"), if resp.is_pointer_button_down_on() { 1.0 } else { 0.0 }, ANSWER.0, ANSWER.1);
    let f = spring(&ctx, id, *value as f32 / 100.0, GLIDE.0, 0.12);
    let y = rect.center().y;
    let x = lerp(left, right, f);
    let track = Rect::from_min_max(pos2(left - 2.0, y - 2.0), pos2(right + 2.0, y + 2.0));
    ui.painter().rect_filled(track, CornerRadius::same(2), Color32::from_white_alpha(26));
    ui.painter().rect_filled(Rect::from_min_max(track.left_top(), pos2(x, track.bottom())), CornerRadius::same(2), ACCENT);
    let r = 8.5 + held * 1.5;
    ui.painter().circle_filled(pos2(x, y + 0.8), r + 0.4, Color32::from_black_alpha(60));
    ui.painter().circle_filled(pos2(x, y), r, Color32::WHITE);
    focus_ring(ui, &resp, rect, 6);
    changed
}

/// A small coloured label.
pub fn pill(ui: &mut Ui, text: &str, colour: Color32) {
    let font = FontId::proportional(12.0);
    let w = ui.painter().layout_no_wrap(text.to_string(), font.clone(), colour).size().x + 18.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(11), colour.gamma_multiply(0.16));
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, font, colour);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_goes_from_one_colour_to_the_other_and_stays_between() {
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 0.0), Color32::BLACK);
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 1.0), Color32::WHITE);
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 5.0), Color32::WHITE, "past the end it stops");
        let m = mix(Color32::from_rgb(0, 100, 200), Color32::from_rgb(100, 200, 0), 0.5);
        assert_eq!((m.r(), m.g(), m.b()), (50, 150, 100));
    }

    #[test]
    fn a_spring_in_memory_starts_at_its_target_then_moves_on_and_settles() {
        let ctx = egui::Context::default();
        let id = Id::NULL.with("test-spring");
        let mut seen = Vec::new();
        for frame in 0..90 {
            // One frame at a time, 60 to the second.
            ctx.begin_pass(egui::RawInput { predicted_dt: 1.0 / 60.0, time: Some(frame as f64 / 60.0), ..Default::default() });
            let target = if frame < 5 { 0.0 } else { 1.0 };
            seen.push(spring(&ctx, id, target, 0.9, 0.25));
            let _ = ctx.end_pass();
        }
        assert_eq!(seen[0], 0.0, "no motion on the first frame");
        assert!(seen[10] > 0.0 && seen[10] < 1.0, "moving: {}", seen[10]);
        assert!((seen[89] - 1.0).abs() < 0.002, "arrived: {}", seen[89]);
    }

    #[test]
    fn the_edges_of_a_window_resize_it_and_its_middle_does_not() {
        use egui::ResizeDirection as D;
        let w = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        assert_eq!(edge_at(w, pos2(400.0, 300.0)), None);
        assert_eq!(edge_at(w, pos2(2.0, 300.0)), Some(D::West));
        assert_eq!(edge_at(w, pos2(798.0, 300.0)), Some(D::East));
        assert_eq!(edge_at(w, pos2(400.0, 2.0)), Some(D::North));
        assert_eq!(edge_at(w, pos2(400.0, 598.0)), Some(D::South));
        assert_eq!(edge_at(w, pos2(2.0, 2.0)), Some(D::NorthWest));
        assert_eq!(edge_at(w, pos2(798.0, 598.0)), Some(D::SouthEast));
        assert_eq!(edge_at(w, pos2(10.0, 3.0)), Some(D::NorthWest), "the corner is easy to hit along both edges");
        assert_eq!(edge_at(w, pos2(900.0, 300.0)), None, "outside the window");
    }

    #[test]
    fn every_state_has_its_own_colour() {
        let all = [State::Running, State::Busy, State::Paused, State::Locked, State::Down];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(state_colour(*a), state_colour(*b));
            }
        }
    }

    #[test]
    fn the_fallback_fonts_always_give_a_semibold_family() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        // Without a semibold family, asking for it would panic at the first label.
        ctx.begin_pass(egui::RawInput::default());
        let w = ctx.fonts_mut(|f| f.layout_no_wrap("Marginalia".to_string(), semibold(14.0), TEXT).size().x);
        let _ = ctx.end_pass();
        assert!(w > 10.0);
    }
}
