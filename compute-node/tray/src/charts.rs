//! Live charts for the details window: smooth scrolling area charts that show the last two minutes.
//!
//! The picture is drawn one second behind the clock and every point is placed by its time, so the chart glides to the left at a
//! steady pace instead of jumping once a second, and a new reading arrives just ahead of the head of the line. The head glows and
//! breathes, stretches where jobs ran are tinted, and moving the pointer over a chart reads out that moment.

use crate::usage::value_at;
use eframe::egui::{epaint::Mesh, Color32, CornerRadius, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2};

/// How far behind the clock the chart is drawn, in milliseconds. A little more than the one second between readings, so there is
/// always a reading on each side of the head of the line.
pub const DELAY_MS: f64 = 1100.0;

/// One line on a chart. `points` are `(time in ms, value from 0 to 1)`.
pub struct Line {
    pub points: Vec<(u64, f32)>,
    pub colour: Color32,
    pub fill: bool,
    pub width: f32,
    pub head: bool,
}

/// A point on a smooth curve through `points`, four steps between each pair (Catmull-Rom).
pub fn smooth(points: &[Pos2]) -> Vec<Pos2> {
    const STEPS: usize = 4;
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    let mut out = Vec::with_capacity(n * STEPS);
    for i in 0..n - 1 {
        let (p0, p1, p2, p3) = (points[i.saturating_sub(1)], points[i], points[i + 1], points[(i + 2).min(n - 1)]);
        for s in 0..STEPS {
            let t = s as f32 / STEPS as f32;
            let (t2, t3) = (t * t, t * t * t);
            let f = |a: f32, b: f32, c: f32, d: f32| 0.5 * ((2.0 * b) + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3);
            out.push(Pos2::new(f(p0.x, p1.x, p2.x, p3.x), f(p0.y, p1.y, p2.y, p3.y)));
        }
    }
    out.push(points[n - 1]);
    out
}

/// What a chart gives back: its widget (for a tooltip) and the time (ms) under the pointer when the pointer is over it.
pub struct Hit {
    pub response: Response,
    pub at_ms: Option<f64>,
}

/// Draws one chart.
#[allow(clippy::too_many_arguments)]
pub fn chart(ui: &mut Ui, size: Vec2, display_ms: f64, window_ms: f64, bands: &[(u64, u64)], band: Color32, lines: &[Line], back: Color32, grid: Color32) -> Hit {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(5), back);
    for f in [0.25, 0.5, 0.75] {
        let y = rect.bottom() - rect.height() * f;
        painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(1.0, grid.gamma_multiply(if f == 0.5 { 1.0 } else { 0.5 })));
    }
    let left_ms = display_ms - window_ms;
    let x_of = |t: f64| rect.left() + ((t - left_ms) / window_ms) as f32 * rect.width();
    for (a, b) in bands {
        let (x0, x1) = (x_of(*a as f64).max(rect.left()), x_of((*b as f64).max(*a as f64 + 1000.0)).min(rect.right()));
        if x1 > x0 {
            painter.rect_filled(Rect::from_min_max(Pos2::new(x0, rect.top()), Pos2::new(x1, rect.bottom())), CornerRadius::ZERO, band);
        }
    }
    let y_of = |v: f32| rect.bottom() - 1.5 - v.clamp(0.0, 1.0) * (rect.height() - 3.0);
    let time = ui.input(|i| i.time) as f32;
    for line in lines {
        // The points inside the window, one before it so the line enters from the edge, and the value at the head of the line.
        let mut pts: Vec<Pos2> = Vec::new();
        let first_in = line.points.iter().position(|(t, _)| (*t as f64) >= left_ms).unwrap_or(line.points.len());
        if first_in > 0 && first_in <= line.points.len() {
            let (t, v) = line.points[first_in - 1];
            pts.push(Pos2::new(x_of(t as f64), y_of(v)));
        }
        for (t, v) in line.points[first_in..].iter().filter(|(t, _)| (*t as f64) <= display_ms) {
            pts.push(Pos2::new(x_of(*t as f64), y_of(*v)));
        }
        if let Some(v) = value_at(&line.points, display_ms) {
            pts.push(Pos2::new(rect.right(), y_of(v)));
        }
        if pts.len() < 2 {
            continue;
        }
        let curve: Vec<Pos2> = smooth(&pts).into_iter().map(|p| Pos2::new(p.x, p.y.clamp(rect.top() + 1.0, rect.bottom() - 1.0))).collect();
        if line.fill {
            let mut mesh = Mesh::default();
            for p in &curve {
                mesh.colored_vertex(*p, line.colour.gamma_multiply(0.42));
                mesh.colored_vertex(Pos2::new(p.x, rect.bottom()), Color32::TRANSPARENT);
            }
            for i in 0..(curve.len() as u32 - 1) {
                let k = i * 2;
                mesh.add_triangle(k, k + 1, k + 2);
                mesh.add_triangle(k + 1, k + 3, k + 2);
            }
            painter.add(Shape::mesh(mesh));
        }
        painter.add(Shape::line(curve.clone(), Stroke::new(line.width, line.colour)));
        if line.head {
            let p = *curve.last().unwrap();
            let breathe = 0.5 + 0.5 * (time * 3.2).sin();
            painter.circle_filled(p, 4.5 + 3.0 * breathe, line.colour.gamma_multiply(0.14 + 0.10 * (1.0 - breathe)));
            painter.circle_filled(p, 2.3, line.colour);
        }
    }
    let hover = resp.hover_pos().filter(|p| rect.contains(*p)).map(|p| left_ms + ((p.x - rect.left()) / rect.width()) as f64 * window_ms);
    if let Some(t) = hover {
        let x = x_of(t);
        painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(1.0, Color32::from_white_alpha(70)));
        for line in lines {
            if let Some(v) = value_at(&line.points, t) {
                painter.circle_filled(Pos2::new(x, y_of(v)), 3.0, line.colour);
            }
        }
    }
    Hit { response: resp, at_ms: hover }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_smooth_curve_passes_through_its_points_and_is_denser() {
        let pts = vec![Pos2::new(0.0, 10.0), Pos2::new(10.0, 20.0), Pos2::new(20.0, 5.0), Pos2::new(30.0, 15.0)];
        let c = smooth(&pts);
        assert_eq!(c.first(), pts.first());
        assert_eq!(c.last(), pts.last());
        assert!(c.len() > pts.len() * 3);
        // Every original point is on the curve (each segment starts exactly on its first point).
        for p in &pts[..pts.len() - 1] {
            assert!(c.contains(p), "{p:?}");
        }
        // Left to right: the curve never goes back in time.
        assert!(c.windows(2).all(|w| w[1].x >= w[0].x - 1e-3));
        assert_eq!(smooth(&pts[..2]).len(), 2);
    }
}
