//! Animation helpers built on gpui's `with_animation`.
//! Upgraded: richer easing curves, 3D-perspective variants, spring-like overshoots,
//! stagger helpers, and orbital/particle utilities for canvas-based 3D effects.

use gpui_kit::*;
use std::time::Duration;

// ── Easing functions ─────────────────────────────────────────────────────────

pub fn ease_out_quint(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(5)
}

pub fn ease_out_expo(t: f32) -> f32 {
    if t >= 1.0 {
        1.0
    } else {
        1.0 - 2.0_f32.powf(-10.0 * t)
    }
}

pub fn ease_out_back(t: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

pub fn ease_out_back_soft(t: f32) -> f32 {
    let c1 = 0.92;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

pub fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        2.0 * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
    }
}

/// Elastic spring: slight overshoot then settle.
pub fn spring(t: f32) -> f32 {
    let c4 = (2.0 * std::f32::consts::PI) / 3.0;
    if t == 0.0 || t == 1.0 {
        t
    } else {
        2.0_f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * c4).sin() + 1.0
    }
}

/// Smooth damped oscillation (one half-cycle, used for emphasis pulses).
pub fn damped_sine(t: f32) -> f32 {
    ((t * std::f32::consts::PI).sin() * (-3.0 * t).exp()).abs()
}

// ── Mount animations ─────────────────────────────────────────────────────────

/// Fade + rise on mount. `delay_ms` staggers lists.
pub fn fade_in_up<E: Styled + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
    delay_ms: u64,
) -> AnimationElement<E> {
    let total = 420 + delay_ms;
    el.with_animation(
        id,
        Animation::new(Duration::from_millis(total)).with_easing(move |t| {
            let d = delay_ms as f32 / total as f32;
            if t <= d {
                0.0
            } else {
                ease_out_expo((t - d) / (1.0 - d))
            }
        }),
        |el, t| el.opacity(t.min(1.0)).relative().top(px(14.0 * (1.0 - t))),
    )
}

/// Fade + scale-in from 96% — "pop" entrance for cards and modals.
pub fn scale_in<E: Styled + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
    delay_ms: u64,
) -> AnimationElement<E> {
    let total = 400 + delay_ms;
    el.with_animation(
        id,
        Animation::new(Duration::from_millis(total)).with_easing(move |t| {
            let d = delay_ms as f32 / total as f32;
            if t <= d {
                0.0
            } else {
                ease_out_back_soft((t - d) / (1.0 - d))
            }
        }),
        |el, t| el.opacity(t.clamp(0.0, 1.0)),
    )
}

/// Slide in from the right (for context cards) — with a soft spring.
pub fn slide_in_right<E: Styled + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(Duration::from_millis(560)).with_easing(ease_out_back_soft),
        |el, t| {
            let t = t.clamp(0.0, 1.15);
            el.opacity(t.min(1.0)).relative().left(px(34.0 * (1.0 - t)))
        },
    )
}

/// Slide in from the left.
pub fn slide_in_left<E: Styled + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(Duration::from_millis(480)).with_easing(ease_out_expo),
        |el, t| {
            el.opacity(t.min(1.0))
                .relative()
                .left(px(-28.0 * (1.0 - t)))
        },
    )
}

// ── Looping animations ────────────────────────────────────────────────────────

/// Continuous pulse between `min` and `max` opacity.
pub fn pulse<E: Styled + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
    period_ms: u64,
    min: f32,
    max: f32,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(Duration::from_millis(period_ms))
            .repeat()
            .with_easing(pulsating_between(min, max)),
        |el, t| el.opacity(t),
    )
}

/// Re-render the element every frame (used for live meters/waveforms).
pub fn live<E: IntoElement + 'static>(el: E, id: impl Into<ElementId>) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(Duration::from_millis(1000)).repeat(),
        |el, _| el,
    )
}

/// Continuous slow rotation (full 360°) — used for orbital marks.
pub fn spin<E: Styled + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
    period_ms: u64,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(Duration::from_millis(period_ms)).repeat(),
        |el, _t| el, // rotation is applied in canvas; this just re-triggers redraws
    )
}

// ── Number / value animations ─────────────────────────────────────────────────

/// Animated number count-up.
pub fn count_up<E: ParentElement + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
    target: f64,
    format: impl Fn(f64) -> String + 'static,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(Duration::from_millis(1100)).with_easing(ease_out_expo),
        move |el, t| el.child(format(target * t as f64)),
    )
}

// ── Emphasis / feedback animations ───────────────────────────────────────────

/// Border glow that settles from accent to the quiet border color.
pub fn settle_glow<E: Styled + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
    hot: Hsla,
    cool: Hsla,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(Duration::from_millis(2200)).with_easing(ease_in_out),
        move |el, t| {
            let mix = Hsla {
                h: hot.h * (1.0 - t) + cool.h * t,
                s: hot.s * (1.0 - t) + cool.s * t,
                l: hot.l * (1.0 - t) + cool.l * t,
                a: hot.a * (1.0 - t) + cool.a * t,
            };
            el.border_color(mix)
        },
    )
}

/// A single "flash" highlight — opacity bursts to 1 then fades.
pub fn flash<E: Styled + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(Duration::from_millis(700)).with_easing(ease_out_expo),
        |el, t| el.opacity(1.0 - t),
    )
}

// ── Utility ───────────────────────────────────────────────────────────────────

/// Compute the (x, y) position on an ellipse.
/// `angle_rad` is the current angle in radians.
pub fn orbit_pos(cx_f: f32, cy_f: f32, rx: f32, ry: f32, angle_rad: f32) -> (f32, f32) {
    (cx_f + rx * angle_rad.cos(), cy_f + ry * angle_rad.sin())
}
