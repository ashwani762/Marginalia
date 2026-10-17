//! Reusable visual building blocks.

use super::theme::{hexa, palette, with_alpha, Palette, TypeScale};
use crate::audio::{AudioMeters, WAVE_POINTS};
use crate::engine::AudioSource;
use crate::rag::store::DocKind;
use crate::rag::Hit;
use gpui_kit::assets::IconName;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::Arc;

pub fn pal(cx: &App) -> Palette {
    palette(cx.theme().mode.is_dark())
}

/// Elevated surface: ivory sheet on paper, glass slab on graphite.
pub fn card(cx: &App) -> Div {
    let p = pal(cx);
    v_flex()
        .relative()
        .bg(p.surface_2)
        .border_1()
        .border_color(p.line)
        .rounded(p.radius())
        .shadow(p.shadow(1.0))
        .overflow_hidden()
        .when(p.dark, |el| {
            // Top gloss edge — wider gradient for a glass-like lit rim
            el.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(1.))
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(hexa(0xFFFFFF, 0.0), 0.),
                        linear_color_stop(hexa(0xFFFFFF, 0.13), 0.5),
                    )),
            )
            // Subtle inner left rim — 3D edge illusion
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .bottom_0()
                    .w(px(1.))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(hexa(0xFFFFFF, 0.0), 0.),
                        linear_color_stop(hexa(0xFFFFFF, 0.06), 0.5),
                    )),
            )
        })
}

/// Card that lifts on hover — with a colour glow edge.
pub fn card_hover(cx: &App) -> Div {
    let p = pal(cx);
    card(cx).cursor_pointer().hover(move |s| {
        s.border_color(p.line_strong)
            .bg(p.surface_3)
            .shadow(p.glow_shadow(p.accent_2, if p.dark { 0.18 } else { 0.0 }))
    })
}

pub fn card_header(title: impl Into<SharedString>, icon: IconName, cx: &App) -> Div {
    let p = pal(cx);
    h_flex()
        .px_4()
        .py_3()
        .gap_2p5()
        .items_center()
        .border_b_1()
        .border_color(p.line)
        .child(icon_tile(icon, p.accent_2, 26., cx))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.text)
                .child(title.into()),
        )
}

/// Small rounded square holding an icon.
pub fn icon_tile(icon: IconName, color: Hsla, size: f32, cx: &App) -> Div {
    let p = pal(cx);
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded(px(size * 0.3))
        .bg(with_alpha(color, if p.dark { 0.16 } else { 0.12 }))
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).with_size(px(size * 0.55)).text_color(color))
}

/// Gradient brand mark.
pub fn brand_mark(size: f32, cx: &App) -> Div {
    let p = pal(cx);
    // Inner square with gradient
    let inner = div()
        .absolute()
        .inset_0()
        .rounded(px(size * 0.3))
        .bg(p.accent_gradient(135.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            Icon::new(IconName::Telescope)
                .with_size(px(size * 0.52))
                .text_color(p.on_accent()),
        );

    div()
        .relative()
        .size(px(size))
        .flex_shrink_0()
        .child(inner)
        // Outer glow ring drawn via canvas — renders a perspective-skewed ellipse
        .child(
            canvas(
                move |_, _, _| (),
                move |bounds, _, window, _| {
                    let cx_f = f32::from(bounds.origin.x) + size / 2.0;
                    let cy_f = f32::from(bounds.origin.y) + size / 2.0;
                    // Use wall-clock time for smooth motion across redraws.
                    let t_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f32())
                        .unwrap_or(0.0);
                    let accent = p.accent;
                    let accent2 = p.accent_2;
                    // Draw two perspective-skewed orbit arcs
                    for orbit_idx in 0..2 {
                        let speed = if orbit_idx == 0 { 0.7 } else { -0.45 };
                        let angle_off = if orbit_idx == 0 {
                            0.0f32
                        } else {
                            std::f32::consts::PI * 0.6
                        };
                        let rx = size * 0.60;
                        let ry = size * 0.16; // flat = perspective tilt
                        let y_off = if orbit_idx == 0 {
                            size * 0.1
                        } else {
                            -size * 0.12
                        };
                        let ring_color = if orbit_idx == 0 { accent } else { accent2 };
                        // Draw ~24 arc segments
                        let segments = 24usize;
                        for seg in 0..segments {
                            let frac = seg as f32 / segments as f32;
                            let a0 = frac * std::f32::consts::TAU + t_secs * speed + angle_off;
                            let a1 = a0 + std::f32::consts::TAU / segments as f32;
                            let x0 = cx_f + rx * a0.cos();
                            let y0 = cy_f + y_off + ry * a0.sin();
                            let x1 = cx_f + rx * a1.cos();
                            let y1 = cy_f + y_off + ry * a1.sin();
                            // Alpha varies by position in orbit — back half fades out
                            let vis = (a0.sin() + 1.0) * 0.5; // 0 (back) → 1 (front)
                            let alpha = if p.dark { 0.12 + 0.55 * vis } else { 0.0 };
                            if alpha < 0.01 {
                                continue;
                            }
                            let color = with_alpha(ring_color, alpha);
                            let seg_w = 1.5 + vis * 1.0;
                            // Build a tiny path for this segment
                            let mut pb = PathBuilder::stroke(px(seg_w));
                            pb.move_to(point(px(x0), px(y0)));
                            pb.line_to(point(px(x1), px(y1)));
                            if let Ok(path) = pb.build() {
                                window.paint_path(path, color);
                            }
                        }
                        // Draw the "satellite" dot at the leading edge
                        let lead_angle = t_secs * speed + angle_off;
                        let dx = cx_f + rx * lead_angle.cos();
                        let dy = cy_f + y_off + ry * lead_angle.sin();
                        let dot_alpha = if p.dark { 0.85 } else { 0.0 };
                        window.paint_quad(
                            fill(
                                Bounds::new(
                                    point(px(dx - 2.0), px(dy - 2.0)),
                                    gpui_kit::size(px(4.0), px(4.0)),
                                ),
                                with_alpha(ring_color, dot_alpha),
                            )
                            .corner_radii(px(2.0)),
                        );
                    }
                },
            )
            .absolute()
            .size(px(size))
            .top_0()
            .left_0(),
        )
        .shadow(vec![BoxShadow {
            color: p.glow,
            offset: point(px(0.), px(6.)),
            blur_radius: px(22.),
            spread_radius: px(-4.),
            inset: false,
        }])
}

/// Page title block: display face headline + muted subtitle.
pub fn page_title(
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    cx: &App,
) -> Div {
    let p = pal(cx);
    v_flex()
        .gap_0p5()
        .child(
            div()
                .font_family(p.display_font())
                .text_2xl()
                .font_weight(if p.dark {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(p.text)
                .child(title.into()),
        )
        .child(
            div()
                .text_sm()
                .text_color(p.text_dim)
                .child(subtitle.into()),
        )
}

pub fn eyebrow(text: impl Into<SharedString>, cx: &App) -> Div {
    let p = pal(cx);
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.text_faint)
        .child(text.into().to_uppercase())
}

pub fn chip(text: impl Into<SharedString>, color: Hsla, cx: &App) -> Div {
    let p = pal(cx);
    h_flex()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(with_alpha(color, if p.dark { 0.16 } else { 0.14 }))
        .border_1()
        .border_color(with_alpha(color, 0.25))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(text.into())
}

pub fn dot(color: Hsla, size: f32) -> Div {
    div()
        .size(px(size))
        .rounded_full()
        .bg(color)
        .flex_shrink_0()
}

/// Dot with a soft halo (recording indicator).
pub fn glow_dot(color: Hsla, size: f32) -> Div {
    div()
        .size(px(size))
        .rounded_full()
        .bg(color)
        .flex_shrink_0()
        .shadow(vec![BoxShadow {
            color: with_alpha(color, 0.7),
            offset: point(px(0.), px(0.)),
            blur_radius: px(size * 1.4),
            spread_radius: px(size * 0.1),
            inset: false,
        }])
}

/// Soft colour bloom, positioned absolutely by the caller. Graphite only; paper stays clean.
pub fn glow_blob(color: Hsla, size: f32) -> Div {
    div()
        .absolute()
        .size(px(size))
        .rounded_full()
        .bg(with_alpha(color, 0.035))
        .shadow(vec![BoxShadow {
            color: with_alpha(color, 0.22),
            offset: point(px(0.), px(0.)),
            blur_radius: px(size * 1.2),
            spread_radius: px(size * 0.1),
            inset: false,
        }])
}

fn maybe_blob(color: Hsla, size: f32, dark: bool) -> Div {
    if dark {
        glow_blob(color, size)
    } else {
        div().absolute().size_0()
    }
}

/// Round speaker avatar with an initial.
pub fn avatar(initial: &str, color: Hsla, size: f32, cx: &App) -> Div {
    let p = pal(cx);
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .bg(with_alpha(color, if p.dark { 0.18 } else { 0.14 }))
        .border_1()
        .border_color(with_alpha(color, 0.35))
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(color)
        .child(initial.to_string())
}

pub fn doc_icon(kind: DocKind) -> IconName {
    match kind {
        DocKind::Pdf => IconName::FileText,
        DocKind::Word => IconName::FileText,
        DocKind::Slides => IconName::GalleryVerticalEnd,
        DocKind::Sheet => IconName::Table,
        DocKind::Markdown => IconName::NotebookPen,
        DocKind::Text => IconName::File,
        DocKind::Html | DocKind::Web => IconName::Globe,
        DocKind::Meeting => IconName::Mic,
        DocKind::Code => IconName::Code,
        DocKind::Image => IconName::Image,
        DocKind::Audio => IconName::AudioLines,
        DocKind::Ebook => IconName::BookOpen,
    }
}

pub fn doc_color(kind: DocKind, cx: &App) -> Hsla {
    let p = pal(cx);
    match kind {
        DocKind::Pdf => p.danger,
        DocKind::Word | DocKind::Text | DocKind::Markdown | DocKind::Ebook => p.accent_2,
        DocKind::Slides => p.warning,
        DocKind::Sheet => p.success,
        DocKind::Html | DocKind::Web => p.accent_3,
        DocKind::Meeting | DocKind::Audio => p.mic,
        DocKind::Code => p.text_dim,
        DocKind::Image => p.system,
    }
}

/// Empty-state component with an animated orbital icon.
pub fn empty_state(
    icon: IconName,
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
    cx: &App,
) -> Div {
    let p = pal(cx);
    let title_s: SharedString = title.into();
    let body_s: SharedString = body.into();

    // Animated orbital ring around the icon (canvas-drawn, dark mode only)
    let orbital = canvas(
        move |_, _, _| (),
        move |bounds, _, window, _| {
            if !p.dark {
                return;
            }
            let cx_f = f32::from(bounds.origin.x) + 34.0;
            let cy_f = f32::from(bounds.origin.y) + 34.0;
            let t_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f32())
                .unwrap_or(0.0);
            // Three orbiting dots at different radii and speeds
            let orbits = [
                (28.0f32, 1.1f32, p.accent_2, 0.0f32),
                (22.0, -0.7, p.accent, 2.1),
                (34.0, 0.5, p.accent_3, 4.3),
            ];
            for (radius, speed, color, phase) in orbits {
                let angle = t_secs * speed + phase;
                let dx = cx_f + radius * angle.cos();
                let dy = cy_f + radius * angle.sin() * 0.45; // flatten for perspective
                                                             // depth-based alpha
                let vis = (angle.sin() * 0.45 + 1.0) * 0.5;
                let alpha = 0.35 + 0.55 * vis;
                let dot_size = 3.0 + vis * 2.5;
                let dot_color = with_alpha(color, alpha);
                window.paint_quad(
                    fill(
                        Bounds::new(
                            point(px(dx - dot_size / 2.0), px(dy - dot_size / 2.0)),
                            size(px(dot_size), px(dot_size)),
                        ),
                        dot_color,
                    )
                    .corner_radii(px(dot_size * 0.5)),
                );
            }
        },
    )
    .absolute()
    .w(px(68.))
    .h(px(68.));

    v_flex()
        .relative()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .p_8()
        .overflow_hidden()
        .child(
            maybe_blob(p.accent, 160., p.dark)
                .top(px(-40.))
                .right(px(-30.)),
        )
        .child(
            div()
                .relative()
                .size(px(68.))
                .flex_shrink_0()
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded_full()
                        .bg(with_alpha(p.accent_2, 0.10))
                        .border_1()
                        .border_color(with_alpha(p.accent_2, 0.22))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(icon).large().text_color(p.accent_2)),
                )
                .child(orbital),
        )
        .child(
            div()
                .font_family(p.display_font())
                .text_lg()
                .font_weight(if p.dark {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(p.text)
                .child(title_s),
        )
        .child(
            div()
                .text_sm()
                .text_color(p.text_dim)
                .max_w(px(440.))
                .text_center()
                .line_height(rems(1.55))
                .child(body_s),
        )
}

/// Live input level for one audio source: seven eased bars with perspective depth and a peak-hold dot.
pub fn level_meter(
    meters: Option<Arc<AudioMeters>>,
    source: AudioSource,
    label: &'static str,
    cx: &App,
) -> Div {
    let p = pal(cx);
    let color = match source {
        AudioSource::Microphone => p.mic,
        AudioSource::System => p.system,
    };
    let level = meters.map(|m| m.smoothed_level(source)).unwrap_or(0.0);
    let db = 20.0 * level.max(1e-5).log10();
    let v = ((db + 50.0) / 50.0).clamp(0.0, 1.0);
    let active = v > 0.06;
    // Seven bars with perspective-like decreasing width toward the right
    let weights = [0.45f32, 0.65, 0.82, 1.0, 0.82, 0.65, 0.45];
    h_flex()
        .gap_2()
        .items_center()
        .px_2p5()
        .py_1()
        .rounded_full()
        .bg(p.surface_3)
        .border_1()
        .border_color(p.line)
        .child(
            Icon::new(match source {
                AudioSource::Microphone => IconName::Mic,
                AudioSource::System => IconName::MonitorSpeaker,
            })
            .xsmall()
            .text_color(if active { color } else { p.text_faint }),
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.text_dim)
                .child(label),
        )
        .child(h_flex().gap_0p5().items_center().h(px(16.)).children(
            weights.into_iter().enumerate().map(move |(i, w)| {
                let h_val = 2.5 + (v * w).powf(0.75) * 13.0;
                // Bar width tapers toward the edges — perspective effect
                let bar_width = 2.0 + w * 0.8;
                // Hue-shift subtly as amplitude climbs (blue→teal→violet for mic)
                let bar_alpha = if active {
                    0.40 + 0.60 * (v * w).min(1.0)
                } else {
                    0.20
                };
                div()
                    .w(px(bar_width))
                    .h(px(h_val))
                    .rounded_full()
                    .bg(with_alpha(color, bar_alpha))
                    .id(SharedString::from(format!("bar-{}-{}", label, i)))
            }),
        ))
}

/// Track for a segmented control: a sunken pill holding 2-4 mutually exclusive views.
pub fn segmented_track(cx: &App) -> Div {
    let p = pal(cx);
    h_flex().gap_0p5().p_0p5().rounded(px(9.)).bg(p.surface_3).border_1().border_color(p.line)
}

/// One segment. The selected segment is *raised* (not a second accent fill), so the single
/// solid accent on a screen stays reserved for the primary action. 28 px tall.
pub fn segment_item(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    count: Option<usize>,
    selected: bool,
    cx: &App,
) -> Stateful<Div> {
    let p = pal(cx);
    h_flex()
        .id(id)
        .h(px(28.))
        .px_3()
        .gap_1p5()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .cursor_pointer()
        .when(selected, |el| {
            el.bg(if p.dark { p.surface_4 } else { p.surface_2 })
                .border_1()
                .border_color(p.line)
        })
        .when(!selected, |el| el.border_1().border_color(with_alpha(p.line, 0.0)).hover(move |s| s.bg(with_alpha(p.text, 0.05))))
        .child(
            div()
                .t_footnote()
                .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .text_color(if selected { p.text } else { p.text_dim })
                .child(label.into()),
        )
        .when_some(count.filter(|n| *n > 0), |el, n| {
            el.child(div().t_caption().text_color(p.text_faint).child(n.to_string()))
        })
}

/// A selectable pill for choosing one item from a longer list (e.g. microphones).
/// Selected = accent border + tint + accent text; 28 px tall.
pub fn choice_chip(id: impl Into<ElementId>, label: impl Into<SharedString>, selected: bool, cx: &App) -> Stateful<Div> {
    let p = pal(cx);
    h_flex()
        .id(id)
        .h(px(28.))
        .px_3()
        .gap_1p5()
        .items_center()
        .rounded_full()
        .cursor_pointer()
        .border_1()
        .border_color(if selected { p.accent_2 } else { p.line_strong })
        .bg(if selected { p.tonal() } else { with_alpha(p.text, 0.0) })
        .hover(move |s| s.bg(if selected { p.tonal_hover() } else { with_alpha(p.text, 0.05) }))
        .when(selected, |el| el.child(Icon::new(IconName::Check).xsmall().text_color(p.accent_2)))
        .child(
            div()
                .t_footnote()
                .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .text_color(if selected { p.accent_2 } else { p.text })
                .child(label.into()),
        )
}

/// Tonal (accent-tinted) button style for secondary-prominence actions.
pub fn tonal_variant(cx: &App) -> gpui_kit::component::button::ButtonCustomVariant {
    let p = pal(cx);
    gpui_kit::component::button::ButtonCustomVariant::new(cx)
        .color(p.tonal())
        .foreground(p.accent_2)
        .hover(p.tonal_hover())
        .active(p.tonal_hover())
}

/// Compact "I am hearing you" indicator that fits at any width: a mic and a speaker
/// glyph, each with three eased bars. Always shown while recording (status visibility).
pub fn mini_meter(meters: Option<Arc<AudioMeters>>, cx: &App) -> Div {
    let p = pal(cx);
    let (mic, sys) = match &meters {
        Some(m) => (m.smoothed_level(AudioSource::Microphone), m.smoothed_level(AudioSource::System)),
        None => (0.0, 0.0),
    };
    let one = |icon: IconName, color: Hsla, level: f32, tag: &'static str| {
        let db = 20.0 * level.max(1e-5).log10();
        let v = ((db + 50.0) / 50.0).clamp(0.0, 1.0);
        let active = v > 0.06;
        h_flex()
            .gap_1p5()
            .items_center()
            .child(Icon::new(icon).xsmall().text_color(if active { color } else { p.text_faint }))
            .child(h_flex().gap_0p5().items_center().h(px(14.)).children([0.7f32, 1.0, 0.7].into_iter().enumerate().map(move |(i, w)| {
                div()
                    .id(SharedString::from(format!("mm-{tag}-{i}")))
                    .w(px(3.))
                    .h(px(3.0 + (v * w).powf(0.75) * 11.0))
                    .rounded_full()
                    .bg(with_alpha(color, if active { 0.5 + 0.5 * (v * w).min(1.0) } else { 0.25 }))
            })))
    };
    h_flex()
        .gap_3()
        .items_center()
        .px_2p5()
        .py_1()
        .rounded_full()
        .bg(p.surface_3)
        .border_1()
        .border_color(p.line)
        .child(one(IconName::Mic, p.mic, mic, "mic"))
        .child(one(IconName::MonitorSpeaker, p.system, sys, "sys"))
}

/// Voice-memo style waveform with 3D perspective depth:
/// bars skew slightly in X based on their position and amplitude,
/// a subtle perspective gradient gives the illusion of depth.
pub fn waveform(meters: Arc<AudioMeters>, height: f32, cx: &App) -> impl IntoElement {
    let p = pal(cx);
    let snapshot = meters.wave_snapshot();
    let phase = meters.scroll_phase();
    let accent = p.accent_2;
    let faint = p.text_faint;
    let n = WAVE_POINTS;
    let bar_w = 2.5;
    let gap = 1.0;
    let step = bar_w + gap;
    let width = n as f32 * step - gap;
    canvas(
        move |_, _, _| (),
        move |bounds, _, window, _| {
            let h = f32::from(bounds.size.height);
            let origin = bounds.origin;
            let ox = f32::from(origin.x);
            let oy = f32::from(origin.y);
            let mid = oy + h / 2.0;
            let offset = n.saturating_sub(snapshot.len());
            let shift = phase * step;
            // Vanishing point for perspective tilt (right edge appears slightly closer)
            let vp_x = ox + width * 1.4;
            for i in 0..n {
                let v = if i >= offset {
                    snapshot[i - offset]
                } else {
                    0.0
                };
                let db = 20.0 * (v.max(1e-5)).log10();
                let amp = ((db + 54.0) / 54.0).clamp(0.0, 1.0);
                let amp = amp * amp * (3.0 - 2.0 * amp); // smoothstep
                let bar_h = (1.5 + amp * (h - 1.5)).min(h);
                let x = ox + i as f32 * step - shift;
                if x < ox - bar_w || x > ox + width {
                    continue;
                }
                let pos = i as f32 / n as f32; // 0=oldest, 1=newest
                                               // Perspective scale: bars near the right (newest) are slightly wider/closer
                let persp = 0.70 + 0.30 * pos;
                let scaled_h = bar_h * persp;
                let scaled_w = (bar_w * persp).max(1.0);
                // Perspective X offset — subtle horizontal convergence toward vanishing point
                let persp_x = (vp_x - x) * (1.0 - persp) * 0.04;
                // Depth-based alpha: older bars are more transparent (receding)
                let edge = (pos / 0.18).clamp(0.0, 1.0);
                let depth_fade = 0.28 + 0.72 * pos;
                let alpha = if amp < 0.03 {
                    0.18 * edge * depth_fade
                } else {
                    (0.30 + 0.70 * pos) * edge * depth_fade
                };
                let color = if amp < 0.03 {
                    with_alpha(faint, alpha)
                } else {
                    with_alpha(accent, alpha)
                };
                let rx = x + persp_x;
                window.paint_quad(
                    fill(
                        Bounds::new(
                            point(px(rx), px(mid - scaled_h / 2.0)),
                            size(px(scaled_w), px(scaled_h)),
                        ),
                        color,
                    )
                    .corner_radii(px(scaled_w * 0.45)),
                );
            }
        },
    )
    .w(px(width))
    .h(px(height))
}

/// Thin two-tone evidence bar (semantic vs keyword).
pub fn score_bars(dense: f32, lexical: f32, cx: &App) -> Div {
    let p = pal(cx);
    let bar = |label: &'static str, v: f32, color: Hsla| {
        h_flex()
            .gap_1p5()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .text_color(p.text_faint)
                    .w(px(58.))
                    .child(label),
            )
            .child(
                div()
                    .w(px(70.))
                    .h(px(4.))
                    .rounded_full()
                    .bg(with_alpha(color, 0.15))
                    .child(
                        div()
                            .h_full()
                            .w(relative(v.clamp(0.0, 1.0)))
                            .rounded_full()
                            .bg(color),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(p.text_faint)
                    .child(format!("{v:.2}")),
            )
    };
    h_flex()
        .gap_4()
        .child(bar("semantic", dense, p.accent_2))
        .child(bar("keyword", lexical, p.accent_3))
}

/// A retrieval hit rendered as a citation card.
pub fn hit_card(hit: &Hit, index: usize, cx: &App) -> Div {
    let p = pal(cx);
    let locator = hit.locator();
    let kind_color = doc_color(hit.kind, cx);
    v_flex()
        .gap_1p5()
        .p_3()
        .rounded(p.radius_sm())
        .bg(p.surface_3)
        .border_1()
        .border_color(p.line)
        .hover(move |s| s.border_color(p.line_strong))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .size(px(18.))
                        .rounded(px(5.))
                        .bg(with_alpha(p.accent_2, 0.18))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.accent_2)
                        .child(format!("{}", index + 1)),
                )
                .child(
                    Icon::new(doc_icon(hit.kind))
                        .xsmall()
                        .text_color(kind_color),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text)
                        .child(hit.title.clone()),
                )
                .when(!locator.is_empty(), |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(p.text_faint)
                            .truncate()
                            .max_w(px(220.))
                            .child(locator),
                    )
                }),
        )
        .when(!hit.quote.is_empty(), |el| {
            el.child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .w(px(2.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(p.accent_2)
                            .h(px(16.))
                            .mt_0p5(),
                    )
                    .child(
                        div()
                            .text_xs()
                            .italic()
                            .text_color(p.text)
                            .line_height(rems(1.4))
                            .child(format!("“{}”", hit.quote)),
                    ),
            )
        })
        .child(
            div()
                .text_xs()
                .text_color(p.text_dim)
                .line_height(rems(1.4))
                .child(crate::rag::snippet_of(
                    &hit.text,
                    if hit.quote.is_empty() { 260 } else { 180 },
                )),
        )
}
