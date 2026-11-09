//! The building blocks of the details window, in the manner of the Settings windows of Hop and PeekABoo Notes: inset groups of
//! rows, a switch whose knob runs on a spring, a segmented control whose pill glides to the chosen part with that part's label
//! lighting up as it arrives, buttons that answer the pointer (a lift on hover, a press), and meters that glide to a new value.
//!
//! Every moving part keeps its spring in egui's own memory under its id, so a block needs no state of its own and a new one
//! starts where it is (no motion on the first frame). Nothing here asks the system for anything but the Reduce Motion setting,
//! read once. The same code runs on Windows and on Linux.

use crate::anim::{clamp01, lerp, Spring};
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Rect, Sense, Stroke, Ui};

pub const GROUP: Color32 = Color32::from_rgb(31, 34, 41);
pub const HAIR: Color32 = Color32::from_rgb(46, 50, 60);
pub const TRACK: Color32 = Color32::from_rgb(54, 58, 69);
pub const TEXT: Color32 = Color32::from_rgb(232, 234, 238);
pub const SOFT: Color32 = Color32::from_rgb(190, 195, 205);
pub const DIM: Color32 = Color32::from_rgb(142, 148, 160);
pub const ACCENT: Color32 = Color32::from_rgb(124, 108, 246);

/// The row height without and with a line of explanation under its name.
pub const ROW: f32 = 46.0;
pub const ROW_TALL: f32 = 58.0;
const PAD: f32 = 14.0;

/// A spring in egui's memory: where `id`'s value is now, moved on by the time since the last frame towards `target`. It starts
/// at the target. Asks for frames while it moves and not when it is at rest.
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

// ---------------------------------------------------------------------------------------------
// Headings and groups
// ---------------------------------------------------------------------------------------------

/// A quiet title above a group.
pub fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text).size(12.0).strong().color(DIM));
    ui.add_space(2.0);
}

/// Rows of one kind of thing in one rounded block, each row set off from the one above by a hairline.
pub struct Group {
    rows: usize,
}

pub fn group(ui: &mut Ui, add: impl FnOnce(&mut Ui, &mut Group)) {
    let mut g = Group { rows: 0 };
    egui::Frame::NONE.fill(GROUP).corner_radius(CornerRadius::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        add(ui, &mut g);
    });
}

impl Group {
    /// One row of `height`, its content laid out left to right and centred. The first row has no line above it.
    pub fn row(&mut self, ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui)) -> egui::Response {
        let width = ui.available_width();
        if self.rows > 0 {
            let y = ui.cursor().top();
            let left = ui.cursor().left();
            ui.painter().line_segment([pos2(left + PAD, y), pos2(left + width - PAD, y)], Stroke::new(1.0, HAIR));
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
            // Room is kept for the control.
            let width = (ui.available_width() - 190.0).max(120.0);
            text_block(ui, width, height, if line.is_some() { NAME_H + LINE_H } else { NAME_H }, |ui| {
                ui.label(egui::RichText::new(title).size(13.5).color(TEXT));
                if let Some(l) = line {
                    ui.label(egui::RichText::new(l).size(11.5).color(DIM));
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(PAD);
                add(ui);
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

// ---------------------------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------------------------

/// A switch. The knob runs on a spring to its side and the track takes the accent colour as it goes. True when clicked.
pub fn switch(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, on: &mut bool) -> bool {
    let id = ui.scope_id().with(("kit-switch", id));
    let (rect, resp) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let k = spring(ui.ctx(), id, if *on { 1.0 } else { 0.0 }, 0.76, 0.26);
    let hover = spring(ui.ctx(), id.with("h"), if resp.hovered() { 1.0 } else { 0.0 }, 1.0, 0.12);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(11), mix(mix(TRACK, Color32::from_rgb(70, 75, 90), hover), ACCENT, k));
    let x = lerp(rect.left() + 11.0, rect.right() - 11.0, k);
    painter.circle_filled(pos2(x, rect.center().y + 0.8), 9.0, Color32::from_black_alpha(50));
    painter.circle_filled(pos2(x, rect.center().y), 8.4, Color32::WHITE);
    if resp.has_focus() {
        painter.rect_stroke(rect.expand(2.0), CornerRadius::same(13), Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
    }
    let clicked = resp.clicked();
    if clicked {
        *on = !*on;
    }
    clicked
}

/// A choice of one among a few, side by side. The pill glides to the chosen one and its label lights as the pill arrives.
/// Returns true when the choice was changed.
pub fn segmented(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, options: &[&str], selected: &mut usize) -> bool {
    let id = ui.scope_id().with(("kit-segmented", id));
    let font = FontId::proportional(12.5);
    let widths: Vec<f32> = options.iter().map(|o| ui.painter().layout_no_wrap(o.to_string(), font.clone(), TEXT).size().x + 26.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let (outer, _) = ui.allocate_exact_size(vec2(total, 30.0), Sense::hover());
    let ctx = ui.ctx().clone();
    // Where each part is.
    let mut lefts = Vec::new();
    let mut x = outer.left() + 3.0;
    for w in &widths {
        lefts.push(x);
        x += w;
    }
    let sel = (*selected).min(options.len().saturating_sub(1));
    let pill_x = spring(&ctx, id.with("x"), lefts[sel], 0.86, 0.22);
    let pill_w = spring(&ctx, id.with("w"), widths[sel], 0.86, 0.22);
    ui.painter().rect_filled(outer, CornerRadius::same(9), TRACK);
    ui.painter().rect_filled(Rect::from_min_size(pos2(pill_x, outer.top() + 3.0), vec2(pill_w, 24.0)), CornerRadius::same(7), mix(TRACK, ACCENT, 0.62));
    let mut changed = false;
    for (i, label) in options.iter().enumerate() {
        let rect = Rect::from_min_size(pos2(lefts[i], outer.top() + 3.0), vec2(widths[i], 24.0));
        let resp = ui.interact(rect, id.with(i), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        // How much the pill is under this part now, so the label lights as it comes and dims as it goes.
        let lit = clamp01(1.0 - ((pill_x + pill_w / 2.0) - rect.center().x).abs() / (rect.width() * 0.9));
        let hover = spring(&ctx, id.with(("h", i)), if resp.hovered() && i != sel { 1.0 } else { 0.0 }, 1.0, 0.12);
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font.clone(), mix(mix(DIM, SOFT, hover), Color32::WHITE, lit));
        if resp.has_focus() {
            ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.5, ACCENT), egui::StrokeKind::Inside);
        }
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
    Danger,
}

/// A button that answers the pointer: it lifts in colour on hover and sinks a little when pressed. True when clicked.
pub fn button(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, text: &str, kind: Kind, hint: &str) -> bool {
    let id = ui.scope_id().with(("kit-button", id));
    let font = FontId::proportional(12.5);
    let w = ui.painter().layout_no_wrap(text.to_string(), font.clone(), TEXT).size().x + 28.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w.max(72.0), 30.0), Sense::click());
    let mut resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if !hint.is_empty() {
        resp = resp.on_hover_text(hint);
    }
    let ctx = ui.ctx().clone();
    let hover = spring(&ctx, id.with("h"), if resp.hovered() { 1.0 } else { 0.0 }, 1.0, 0.12);
    let press = spring(&ctx, id.with("p"), if resp.is_pointer_button_down_on() { 1.0 } else { 0.0 }, 1.0, 0.08);
    let (base, lifted) = match kind {
        Kind::Plain => (TRACK, Color32::from_rgb(70, 75, 90)),
        Kind::Primary => (Color32::from_rgb(38, 120, 66), Color32::from_rgb(48, 146, 80)),
        Kind::Danger => (Color32::from_rgb(98, 44, 46), Color32::from_rgb(128, 54, 56)),
    };
    let r = rect.shrink(press * 1.2);
    ui.painter().rect_filled(r, CornerRadius::same(8), mix(base, lifted, hover));
    ui.painter().text(r.center(), Align2::CENTER_CENTER, text, font, if kind == Kind::Plain { mix(SOFT, Color32::WHITE, hover) } else { Color32::WHITE });
    if resp.has_focus() {
        ui.painter().rect_stroke(rect.expand(1.5), CornerRadius::same(9), Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
    }
    resp.clicked()
}

/// A meter: the filled part glides to `fraction` (0 to 1) when it changes.
pub fn meter(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, width: f32, fraction: f32, colour: Color32) {
    let id = ui.scope_id().with(("kit-meter", id));
    let (rect, _) = ui.allocate_exact_size(vec2(width.max(20.0), 8.0), Sense::hover());
    let f = spring(ui.ctx(), id, clamp01(fraction), 0.9, 0.35);
    ui.painter().rect_filled(rect, CornerRadius::same(4), TRACK);
    if f > 0.002 {
        let mut fill = rect;
        fill.set_width((rect.width() * f).max(8.0));
        ui.painter().rect_filled(fill, CornerRadius::same(4), colour);
    }
}

/// A big number with a caption under it, for the figures at the top of the overview.
pub fn stat(ui: &mut Ui, width: f32, value: &str, caption: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 74.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(12), GROUP);
    ui.painter().text(pos2(rect.center().x, rect.top() + 28.0), Align2::CENTER_CENTER, value, FontId::proportional(24.0), TEXT);
    ui.painter().text(pos2(rect.center().x, rect.top() + 54.0), Align2::CENTER_CENTER, caption, FontId::proportional(12.0), DIM);
}

/// A small coloured label.
pub fn pill(ui: &mut Ui, text: &str, colour: Color32) {
    let font = FontId::proportional(11.5);
    let w = ui.painter().layout_no_wrap(text.to_string(), font.clone(), colour).size().x + 18.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(11), colour.gamma_multiply(0.18));
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
}
