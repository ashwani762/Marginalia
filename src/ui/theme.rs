//! Marginalia visual language: deep graphite surfaces, indigo→violet accent,
//! generous radii, soft shadows. Applied on top of gpui-component's theme.

use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;

pub const DISPLAY_FONT: &str = "Inter Display";
pub const BODY_FONT: &str = "Inter";
pub const MONO_FONT: &str = "DejaVu Sans Mono";
pub const SERIF_FONT: &str = "EB Garamond";

#[derive(Clone, Copy)]
pub struct Palette {
    pub accent: Hsla,
    pub accent_2: Hsla,
    pub accent_3: Hsla,
    pub glow: Hsla,
    pub surface: Hsla,
    pub surface_2: Hsla,
    pub surface_3: Hsla,
    pub surface_4: Hsla,
    pub line: Hsla,
    pub line_strong: Hsla,
    pub highlight: Hsla,
    pub text: Hsla,
    pub text_dim: Hsla,
    pub text_faint: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
    /// Fill for solid danger controls (white label >= 4.5:1).
    pub danger_fill: Hsla,
    pub mic: Hsla,
    pub system: Hsla,
    pub dark: bool,
}

pub fn hex(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn hexa(hex: u32, alpha: f32) -> Hsla {
    let mut c: Hsla = rgb(hex).into();
    c.a = alpha;
    c
}

pub fn with_alpha(mut c: Hsla, a: f32) -> Hsla {
    c.a = a;
    c
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            // accent = fill (white label 4.9:1); accent_2 = ink for text/glyphs (6.8:1)
            accent: hex(0x6A58F0),
            accent_2: hex(0xA78BFA),
            accent_3: hex(0x22D3EE),
            glow: hexa(0x7C6CF6, 0.50),
            surface: hex(0x080B12),
            surface_2: hex(0x0D1119),
            surface_3: hex(0x131826),
            surface_4: hex(0x1A2130),
            line: hexa(0xFFFFFF, 0.08),
            line_strong: hexa(0xFFFFFF, 0.16),
            highlight: hexa(0xFFFFFF, 0.06),
            text: hex(0xEDF0F7),
            text_dim: hex(0x9AA4B8),
            text_faint: hex(0x8794AB), // was 5F6A82 (3.2:1); now >= 5.0:1 on every surface
            success: hex(0x34D399),
            warning: hex(0xFBBF24),
            danger: hex(0xF87171),
            danger_fill: hex(0xD33A3A),
            mic: hex(0x67E8F9),
            system: hex(0xC4B5FD),
            dark: true,
        }
    } else {
        // "Paper": warm gallery neutrals, ink for actions, pigments used sparingly.
        Palette {
            accent: hex(0x151413),   // Ink
            accent_2: hex(0x9C442D), // Terracotta ink (text-safe, >= 5.2:1)
            accent_3: hex(0x1F3FBF), // Klein blue
            glow: hexa(0xB4533A, 0.18),
            surface: hex(0xF6F3EE),   // Paper
            surface_2: hex(0xFBFAF7), // Ivory
            surface_3: hex(0xEDE7DD), // Bone
            surface_4: hex(0xE4DCCF), // Linen
            line: hex(0xDAD2C5),      // Hairline
            line_strong: hex(0xC9BFAF),
            highlight: hexa(0xFFFFFF, 0.8),
            text: hex(0x151413),       // Ink
            text_dim: hex(0x4A4642),   // Graphite
            text_faint: hex(0x645E55), // Stone, deepened from 8F887E (2.9:1) to >= 4.7:1
            success: hex(0x55624A),    // Sage ink (pigment 7C8A6E was 3.0:1)
            warning: hex(0x80590E),    // Ochre ink (pigment C8962E was 2.2:1)
            danger: hex(0xA23B2A),
            danger_fill: hex(0xA23B2A),
            mic: hex(0x1F3FBF),
            system: hex(0x9C442D),
            dark: false,
        }
    }
}

impl Palette {
    /// Tonal fill for secondary-prominence controls (accent at low alpha).
    pub fn tonal(&self) -> Hsla {
        with_alpha(self.accent_2, if self.dark { 0.16 } else { 0.12 })
    }
    pub fn tonal_hover(&self) -> Hsla {
        with_alpha(self.accent_2, if self.dark { 0.24 } else { 0.18 })
    }
    /// Headline face: Inter Display on graphite, a serif on paper.
    pub fn display_font(&self) -> &'static str {
        if self.dark {
            DISPLAY_FONT
        } else {
            SERIF_FONT
        }
    }
    pub fn radius(&self) -> Pixels {
        if self.dark {
            px(14.)
        } else {
            px(8.)
        }
    }
    pub fn radius_sm(&self) -> Pixels {
        if self.dark {
            px(10.)
        } else {
            px(6.)
        }
    }
    /// Accent gradient for brand marks and primary surfaces.
    pub fn accent_gradient(&self, angle: f32) -> Background {
        if self.dark {
            // gpui's linear_gradient only supports two stops; we span from the
            // primary accent to the lavender highlight for maximum richness.
            linear_gradient(
                angle,
                linear_color_stop(self.accent, 0.),
                linear_color_stop(hex(0xC4B5FD), 1.),
            )
        } else {
            linear_gradient(
                angle,
                linear_color_stop(hex(0x2B2926), 0.),
                linear_color_stop(self.accent, 1.),
            )
        }
    }
    pub fn on_accent(&self) -> Hsla {
        if self.dark {
            hex(0xFFFFFF)
        } else {
            hex(0xFBFAF7)
        }
    }
    pub fn shadow(&self, strength: f32) -> Vec<BoxShadow> {
        if self.dark {
            // Three layers: a deep ambient, a medium diffuse, a tight contact
            vec![
                BoxShadow {
                    color: hexa(0x000000, 0.55 * strength),
                    offset: point(px(0.), px(20.)),
                    blur_radius: px(48.),
                    spread_radius: px(-12.),
                    inset: false,
                },
                BoxShadow {
                    color: hexa(0x000000, 0.35 * strength),
                    offset: point(px(0.), px(8.)),
                    blur_radius: px(24.),
                    spread_radius: px(-8.),
                    inset: false,
                },
                BoxShadow {
                    color: hexa(0x000000, 0.20 * strength),
                    offset: point(px(0.), px(2.)),
                    blur_radius: px(6.),
                    spread_radius: px(-2.),
                    inset: false,
                },
            ]
        } else {
            // Light: a warm ink shadow
            vec![BoxShadow {
                color: hexa(0x3B2A1C, 0.14 * strength),
                offset: point(px(0.), px(8.)),
                blur_radius: px(20.),
                spread_radius: px(-8.),
                inset: false,
            }]
        }
    }

    /// A coloured glow shadow — used for glowing buttons and active marks.
    pub fn glow_shadow(&self, color: Hsla, strength: f32) -> Vec<BoxShadow> {
        vec![
            BoxShadow {
                color: with_alpha(color, 0.45 * strength),
                offset: point(px(0.), px(0.)),
                blur_radius: px(24.),
                spread_radius: px(-4.),
                inset: false,
            },
            BoxShadow {
                color: with_alpha(color, 0.25 * strength),
                offset: point(px(0.), px(8.)),
                blur_radius: px(32.),
                spread_radius: px(-8.),
                inset: false,
            },
        ]
    }

    /// Dark-mode only inset top highlight that reads as a glass surface.
    pub fn glass_inset(&self) -> Option<BoxShadow> {
        if self.dark {
            Some(BoxShadow {
                color: hexa(0xFFFFFF, 0.08),
                offset: point(px(0.), px(1.)),
                blur_radius: px(0.),
                spread_radius: px(0.),
                inset: true,
            })
        } else {
            None
        }
    }
}

/// Apply Marginalia's palette to the global component theme.
pub fn apply(dark: bool, cx: &mut App) {
    Theme::change(
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    let p = palette(dark);
    // `Theme::update` reconciles the semantic tokens (which buttons and inputs
    // read) from the colors we set and refreshes every window.
    Theme::update(cx, |theme| {
        theme.font_family = BODY_FONT.into();
        theme.font_size = px(14.5);
        theme.mono_font_family = MONO_FONT.into();
        theme.radius = p.radius_sm();
        theme.radius_lg = p.radius();
        theme.shadow = p.dark;
        // Toasts sit bottom-right so they never cover page actions (top-right).
        theme.notification.placement = Anchor::BottomRight;
        theme.notification.margins = gpui_kit::base::Edges { top: px(16.), right: px(20.), bottom: px(20.), left: px(20.) };
        theme.notification.width = px(340.);
        theme.notification.max_items = 3;
        let white = hex(0xFFFFFF);
        let c = &mut theme.colors;
        c.background = p.surface;
        c.foreground = p.text;
        c.border = p.line;
        c.muted = p.surface_3;
        c.muted_foreground = p.text_dim;
        c.primary = p.accent;
        c.primary_hover = if p.dark { hex(0x5F4BE8) } else { hex(0x2B2926) };
        c.primary_active = if p.dark { hex(0x5440D8) } else { hex(0x000000) };
        c.primary_foreground = white;
        c.accent = p.surface_3;
        c.accent_foreground = p.text;
        c.secondary = p.surface_3;
        c.secondary_hover = p.surface_4;
        c.secondary_active = p.line_strong;
        c.secondary_foreground = p.text;
        // Buttons
        c.button = p.surface_3;
        c.button_hover = p.surface_4;
        c.button_active = p.line_strong;
        c.button_foreground = p.text;
        c.button_primary = p.accent;
        c.button_primary_hover = if p.dark { hex(0x5F4BE8) } else { hex(0x2B2926) };
        c.button_primary_active = if p.dark { hex(0x5440D8) } else { hex(0x000000) };
        c.button_primary_foreground = white;
        c.button_secondary = p.surface_3;
        c.button_secondary_hover = p.surface_4;
        c.button_secondary_active = p.line_strong;
        c.button_secondary_foreground = p.text;
        c.button_danger = p.danger_fill;
        c.button_danger_hover = if p.dark { hex(0xC22F2F) } else { hex(0x8A2F20) };
        c.button_danger_active = if p.dark { hex(0xAA2626) } else { hex(0x75281B) };
        c.button_danger_foreground = white;
        c.button_success = p.success;
        c.button_success_foreground = hex(0x062B22);
        c.button_warning = p.warning;
        c.button_warning_foreground = hex(0x2B1D04);
        c.button_info = p.accent_3;
        c.button_info_foreground = hex(0x042A30);
        c.sidebar = p.surface_2;
        c.sidebar_border = p.line;
        c.sidebar_foreground = p.text;
        c.sidebar_accent = p.surface_3;
        c.sidebar_accent_foreground = p.text;
        c.sidebar_primary = p.accent;
        c.sidebar_primary_foreground = white;
        c.input = p.line_strong;
        c.ring = p.accent;
        c.popover = p.surface_2;
        c.popover_foreground = p.text;
        c.list = p.surface_2;
        c.list_hover = p.surface_3;
        c.list_active = p.surface_3;
        c.list_active_border = p.accent;
        c.success = p.success;
        c.success_foreground = hex(0x062B22);
        c.warning = p.warning;
        c.warning_foreground = hex(0x2B1D04);
        c.danger = p.danger;
        c.danger_foreground = white;
        c.info = p.accent_3;
        c.info_foreground = hex(0x042A30);
        c.link = p.accent_2;
        c.link_hover = p.accent;
        c.scrollbar = hexa(0x000000, 0.0);
        c.scrollbar_thumb = p.line_strong;
        c.scrollbar_thumb_hover = p.text_faint;
        c.progress_bar = p.accent;
        c.skeleton = p.surface_3;
        c.title_bar = p.surface_2;
        c.title_bar_border = p.line;
        c.tab_bar = p.surface_2;
        c.tab = p.surface_2;
        c.tab_active = p.surface_3;
        c.tab_foreground = p.text_dim;
        c.tab_active_foreground = p.text;
        c.switch = p.line_strong;
        c.switch_thumb = white;
        c.selection = if p.dark {
            hexa(0x7C6CF6, 0.35)
        } else {
            hexa(0xB4533A, 0.22)
        };
        c.caret = if p.dark { p.accent } else { p.accent_2 };
        c.drop_target = with_alpha(p.accent_2, 0.12);
        c.drag_border = p.accent;
    });
    cx.refresh_windows();
}


/// Marginalia type scale (px / line-height). See DESIGN.md section 3.2.
pub trait TypeScale: Styled + Sized {
    /// 12/16 — timestamps, eyebrows. Never smaller.
    fn t_caption(self) -> Self {
        self.text_size(px(12.)).line_height(px(16.))
    }
    /// 13/18 — helper text.
    fn t_footnote(self) -> Self {
        self.text_size(px(13.)).line_height(px(18.))
    }
    /// 14.5/22 — UI body.
    fn t_body(self) -> Self {
        self.text_size(px(14.5)).line_height(px(22.))
    }
    /// 16/26 — surfaces people read or write for minutes (notes, summaries).
    fn t_writing(self) -> Self {
        self.text_size(px(16.)).line_height(px(26.))
    }
    /// 18/24 — panel titles.
    fn t_title3(self) -> Self {
        self.text_size(px(18.)).line_height(px(24.))
    }
    /// 22/28 — page titles.
    fn t_title2(self) -> Self {
        self.text_size(px(22.)).line_height(px(28.))
    }
    /// 28/32 — meeting title.
    fn t_title1(self) -> Self {
        self.text_size(px(28.)).line_height(px(32.))
    }
    /// 36/41 — hero only; leading 1.15.
    fn t_display(self) -> Self {
        self.text_size(px(36.)).line_height(px(41.))
    }
}
impl<T: Styled + Sized> TypeScale for T {}

/// Motion tokens. See DESIGN.md section 3.4.
pub mod motion {
    use std::time::Duration;
    pub const MICRO: Duration = Duration::from_millis(120);
    pub const STANDARD: Duration = Duration::from_millis(200);
    pub const EMPHASIS: Duration = Duration::from_millis(320);
    pub const PAGE: Duration = Duration::from_millis(480);
}
