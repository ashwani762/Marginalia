//! Stats: what Marginalia has processed, all on-device.

use super::anim::{count_up, fade_in_up};
use super::components::{card, eyebrow, icon_tile, page_title, pal};
use crate::app::Services;
use crate::rag::store::Stats;
use gpui_kit::assets::IconName;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub struct StatsView {
    services: Arc<Services>,
    stats: Stats,
    tokens_generated: u64,
    tokens_processed: u64,
    seconds_live: u64,
    generation: u64,
}

impl StatsView {
    pub fn new(services: Arc<Services>, cx: &mut Context<Self>) -> Self {
        let mut v = Self {
            services,
            stats: Stats::default(),
            tokens_generated: 0,
            tokens_processed: 0,
            seconds_live: 0,
            generation: 0,
        };
        v.refresh(cx);
        v
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Ok(s) = self.services.kb.store.stats() {
            self.stats = s;
        }
        if let Some(llm) = self.services.engine.llm() {
            self.tokens_generated = llm.tokens_generated.load(Ordering::Relaxed);
            self.tokens_processed = llm.tokens_processed.load(Ordering::Relaxed);
        }
        if let Some(w) = self.services.engine.whisper() {
            self.seconds_live = w.seconds_transcribed.load(Ordering::Relaxed);
        }
        self.generation += 1;
        cx.notify();
    }

    fn tile(
        &self,
        idx: usize,
        icon: IconName,
        label: &'static str,
        value: f64,
        fmt: fn(f64) -> String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = pal(cx);
        let tile = card(cx)
            .p_5()
            .gap_3()
            .min_w(px(220.))
            .flex_1()
            .cursor_pointer()
            .hover(move |s| {
                s.border_color(p.line_strong)
                    .shadow(p.glow_shadow(p.accent_2, 0.22))
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(icon_tile(icon, p.accent_2, 30., cx))
                    .child(eyebrow(label, cx)),
            )
            .child(count_up(
                div()
                    .font_family(p.display_font())
                    .text_3xl()
                    .font_weight(if p.dark {
                        FontWeight::BOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(p.text),
                SharedString::from(format!("count-{idx}-{}", self.generation)),
                value,
                fmt,
            ));
        fade_in_up(
            tile,
            SharedString::from(format!("tile-{idx}-{}", self.generation)),
            idx as u64 * 60,
        )
    }
}

fn fmt_int(v: f64) -> String {
    let n = v.round() as i64;
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

fn fmt_minutes(v: f64) -> String {
    let m = (v / 60.0).round() as i64;
    if m >= 60 {
        format!("{}h {}m", m / 60, m % 60)
    } else {
        format!("{m} min")
    }
}

impl Render for StatsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let s = self.stats.clone();
        let local_tokens = (self.tokens_generated + self.tokens_processed) as f64
            + (s.words_transcribed as f64 * 1.3);
        v_flex()
            .size_full()
            .p_5()
            .gap_5()
            .child(page_title("Stats", "Everything below was processed on this computer. Nothing was sent to a server.", cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .child(self.tile(0, IconName::Mic, "Meetings transcribed", s.meetings as f64, fmt_int, cx))
                    .child(self.tile(1, IconName::Clock, "Audio transcribed", s.seconds_transcribed as f64, fmt_minutes, cx))
                    .child(self.tile(2, IconName::Captions, "Words captured", s.words_transcribed as f64, fmt_int, cx))
                    .child(self.tile(3, IconName::LibraryBig, "Documents indexed", s.documents as f64, fmt_int, cx))
                    .child(self.tile(4, IconName::Layers, "Passages searchable", s.chunks as f64, fmt_int, cx))
                    .child(self.tile(5, IconName::Lightbulb, "Questions answered live", s.questions_answered as f64, fmt_int, cx))
                    .child(self.tile(6, IconName::Cpu, "Tokens kept local", local_tokens, fmt_int, cx)),
            )
            .child(
                card(cx).p_4().gap_2().child(eyebrow("Session", cx)).child(
                    div().text_sm().text_color(p.text_dim).child(format!(
                        "Chat model generated {} tokens and read {} prompt tokens this session · {} s of audio transcribed since launch.",
                        fmt_int(self.tokens_generated as f64),
                        fmt_int(self.tokens_processed as f64),
                        self.seconds_live
                    )),
                ),
            )
    }
}
