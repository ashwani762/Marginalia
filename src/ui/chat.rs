//! Ask: retrieval-augmented chat over documents and past meetings.

use super::anim::{fade_in_up, live};
use super::components::{brand_mark, card, chip, doc_icon, empty_state, eyebrow, page_title, pal, segment_item, segmented_track};
use crate::app::{GenTarget, NoteKind, Services};
use crate::engine::{ChatMessage, GenEvent, GenParams, GenRequest, Priority};
use crate::rag::store::DocKind;
use crate::rag::{Hit, Scope};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::component::{h_flex, v_flex, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

struct Turn {
    id: u64,
    question: String,
    answer: Entity<TextViewState>,
    answer_text: String,
    hits: Vec<Hit>,
    retrieving: bool,
    generating: bool,
    cancel: Option<Arc<AtomicBool>>,
    stats: Option<String>,
    show_sources: bool,
}

pub struct ChatView {
    services: Arc<Services>,
    input: Entity<TextareaState>,
    turns: Vec<Turn>,
    next_id: u64,
    scope: Scope,
    /// Starter questions derived from the user's own meetings and documents.
    suggestions: Vec<String>,
    scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl ChatView {
    pub fn new(services: Arc<Services>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextareaState::new(window, cx).placeholder("Ask about your documents or past meetings… (Enter to send, Shift+Enter for a new line)").auto_grow(1, 6));
        let subs = vec![
            cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift, .. } = ev {
                    if !*shift {
                        this.send(window, cx);
                    }
                }
            }),
        ];
        Self {
            services,
            input,
            turns: Vec::new(),
            next_id: 1,
            scope: Scope::All,
            suggestions: Vec::new(),
            scroll: ScrollHandle::new(),
            _subs: subs,
        }
    }

    /// Rebuild the starter questions from what is actually in the app.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let short = |t: &str| crate::rag::snippet_of(t.trim(), 38);
        let mut out: Vec<String> = Vec::new();
        if let Ok(meetings) = self.services.kb.store.list_meetings() {
            let named = |m: &&crate::rag::store::Meeting| {
                let t = m.title.trim();
                !(t.is_empty() || t.starts_with("Untitled") || t.starts_with("Meeting ·"))
            };
            if let Some(m) = meetings.iter().filter(named).find(|m| !m.summary.trim().is_empty()) {
                out.push(format!("What were the key points of “{}”?", short(&m.title)));
            } else if let Some(m) = meetings.iter().find(|m| m.segment_count > 0) {
                out.push(format!("What was said in the meeting on {}?", chrono::DateTime::from_timestamp(m.started_at, 0)
                    .map(|d| d.with_timezone(&chrono::Local).format("%b %-d").to_string())
                    .unwrap_or_default()));
            }
            if meetings.iter().filter(|m| !m.summary.trim().is_empty()).count() >= 2 {
                out.push("List every action item from my recent meetings".to_string());
            }
        }
        if let Ok(docs) = self.services.kb.documents() {
            for d in docs.iter().filter(|d| d.kind != crate::rag::store::DocKind::Meeting).take(2) {
                out.push(format!("Summarize “{}”", short(&d.title)));
            }
        }
        out.truncate(3);
        self.suggestions = out;
        cx.notify();
    }

    /// Ask a question programmatically (command palette).
    pub fn ask(&mut self, question: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.set_value(question, window, cx));
        self.send(window, cx);
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.input.read(cx).value().trim().to_string();
        if q.is_empty() {
            return;
        }
        if self.services.engine.llm().is_none() {
            self.services
                .notify(NoteKind::Warning, "Chat model is still loading.");
            return;
        }
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        let id = self.next_id;
        self.next_id += 1;
        let answer = cx.new(|cx| TextViewState::markdown("", cx));
        self.turns.push(Turn {
            id,
            question: q.clone(),
            answer,
            answer_text: String::new(),
            hits: Vec::new(),
            retrieving: true,
            generating: false,
            cancel: None,
            stats: None,
            show_sources: false,
        });
        self.scroll.scroll_to_bottom();
        let sv = self.services.clone();
        let scope = self.scope;
        let task = cx.spawn(async move |this, cx| {
            let sv2 = sv.clone();
            let q2 = q.clone();
            let hits = cx
                .background_executor()
                .spawn(async move { sv2.kb.search(&q2, 6, scope).unwrap_or_default() })
                .await;
            let _ = this.update(cx, |v, cx| v.on_hits(id, hits, cx));
        });
        task.detach();
        cx.notify();
    }

    fn on_hits(&mut self, id: u64, hits: Vec<Hit>, cx: &mut Context<Self>) {
        // Short conversational memory: last 3 exchanges.
        let history: Vec<(String, String)> = self
            .turns
            .iter()
            .filter(|t| t.id != id && !t.answer_text.is_empty())
            .rev()
            .take(3)
            .map(|t| (t.question.clone(), t.answer_text.clone()))
            .collect();
        let Some(turn) = self.turns.iter_mut().find(|t| t.id == id) else {
            return;
        };
        turn.retrieving = false;
        turn.generating = true;
        turn.hits = hits;
        // Token budget: keep prompt + answer inside the context window (≈3.6 chars/token).
        let n_ctx = self.services.settings().chat_context.max(2048) as usize;
        let answer_tokens = 600usize;
        let budget_chars = n_ctx.saturating_sub(answer_tokens + 400) * 36 / 10;
        let passage_budget = budget_chars * 6 / 10;
        let history_budget = budget_chars * 25 / 100;
        let passages = if turn.hits.is_empty() {
            "(no relevant passages found)".to_string()
        } else {
            let per = (passage_budget / turn.hits.len().max(1)).clamp(300, 2200);
            turn.hits
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let kind = if h.kind == DocKind::Meeting {
                        "meeting"
                    } else {
                        "document"
                    };
                    format!(
                        "[{}] {} — {} ({})\n{}",
                        i + 1,
                        h.title,
                        h.locator(),
                        kind,
                        crate::rag::snippet_of(&h.context, per)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        let mut messages = vec![ChatMessage::system(CHAT_SYSTEM)];
        let mut used = 0usize;
        let mut kept: Vec<(String, String)> = Vec::new();
        for (q, a) in history {
            let a = crate::rag::snippet_of(&a, 700);
            if used + q.len() + a.len() > history_budget {
                break;
            }
            used += q.len() + a.len();
            kept.push((q, a));
        }
        for (q, a) in kept.into_iter().rev() {
            messages.push(ChatMessage::user(q));
            messages.push(ChatMessage::assistant(a));
        }
        let question = turn.question.clone();
        messages.push(ChatMessage::user(format!(
            "Passages:\n{passages}\n\nQuestion: {question}"
        )));
        let cancel = Arc::new(AtomicBool::new(false));
        turn.cancel = Some(cancel.clone());
        if let Some(llm) = self.services.engine.llm() {
            llm.generate(GenRequest {
                images: Vec::new(),
                messages,
                params: GenParams {
                    max_tokens: 600,
                    temperature: 0.35,
                    ..Default::default()
                },
                priority: Priority::High,
                cancel,
                on_event: self.services.gen_sink(GenTarget::Chat(id)),
            });
        }
        cx.notify();
    }

    pub fn on_gen(
        &mut self,
        target: GenTarget,
        event: GenEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let GenTarget::Chat(id) = target else { return };
        let Some(turn) = self.turns.iter_mut().find(|t| t.id == id) else {
            return;
        };
        match event {
            GenEvent::Token(t) => {
                turn.answer_text.push_str(&t);
                turn.answer.update(cx, |s, cx| s.push_str(&t, cx));
                self.scroll.scroll_to_bottom();
            }
            GenEvent::Done(s) => {
                turn.generating = false;
                let tps = if s.elapsed_ms > 0 {
                    s.generated_tokens as f64 / (s.elapsed_ms as f64 / 1000.0)
                } else {
                    0.0
                };
                turn.stats = Some(format!(
                    "{} tokens · {:.1} tok/s · local",
                    s.generated_tokens, tps
                ));
            }
            GenEvent::Error(e) => {
                turn.generating = false;
                turn.stats = Some(format!("error: {e}"));
                if crate::app::is_backend_failure(&e) && self.services.settings().use_gpu {
                    crate::app::degrade_llm_to_cpu(self.services.clone());
                }
            }
        }
        cx.notify();
    }

    fn stop(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(t) = self.turns.iter().find(|t| t.id == id) {
            if let Some(c) = &t.cancel {
                c.store(true, Ordering::Relaxed);
            }
        }
        cx.notify();
    }

    fn render_turn(&self, t: &Turn, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let id = t.id;
        let show = t.show_sources;
        let turn = v_flex()
            .gap_3()
            .child(
                h_flex().justify_end().child(
                    div()
                        .max_w(relative(0.78))
                        .px_4()
                        .py_2p5()
                        .rounded(p.radius())
                        .bg(p.accent_gradient(135.))
                        .text_sm()
                        .text_color(p.on_accent())
                        .line_height(rems(1.45))
                        .child(t.question.clone()),
                ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(live(
                        brand_mark(28., cx),
                        SharedString::from(format!("brand-{id}")),
                    ))
                    .child(
                        card(cx)
                            .flex_1()
                            .min_w_0()
                            .p_4()
                            .gap_3()
                            .child(if t.retrieving {
                                ShimmerText::new("searching your knowledge base…")
                                    .id(SharedString::from(format!("t-sh-{id}")))
                                    .into_any_element()
                            } else {
                                div()
                                    .text_sm()
                                    .text_color(p.text)
                                    .line_height(rems(1.55))
                                    .child(
                                        TextView::new(&t.answer).selectable(true).stream_fade(true),
                                    )
                                    .into_any_element()
                            })
                            .when(t.generating && !t.retrieving, |el| {
                                el.child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            ShimmerText::new("generating")
                                                .id(SharedString::from(format!("t-gen-{id}"))),
                                        )
                                        .child(
                                            Button::new(SharedString::from(format!("stop-{id}")))
                                                .ghost()
                                                .small()
                                                .icon(IconName::Square)
                                                .label("Stop")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.stop(id, cx)
                                                })),
                                        ),
                                )
                            })
                            .when(!t.hits.is_empty(), |el| {
                                el.child(
                                    v_flex()
                                        .gap_2()
                                        .child(
                                            h_flex()
                                                .id(SharedString::from(format!("src-toggle-{id}")))
                                                .gap_2()
                                                .items_center()
                                                .cursor_pointer()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if let Some(t) =
                                                        this.turns.iter_mut().find(|t| t.id == id)
                                                    {
                                                        t.show_sources = !t.show_sources;
                                                        cx.notify();
                                                    }
                                                }))
                                                .child(eyebrow("Sources", cx))
                                                .children(t.hits.iter().enumerate().map(
                                                    |(i, h)| {
                                                        h_flex()
                                                            .gap_1()
                                                            .items_center()
                                                            .px_2()
                                                            .py_0p5()
                                                            .rounded_full()
                                                            .bg(p.surface_3)
                                                            .border_1()
                                                            .border_color(p.line)
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .font_weight(FontWeight::BOLD)
                                                                    .text_color(p.accent_2)
                                                                    .child(format!("{}", i + 1)),
                                                            )
                                                            .child(
                                                                Icon::new(doc_icon(h.kind))
                                                                    .xsmall()
                                                                    .text_color(p.text_dim),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(p.text_dim)
                                                                    .max_w(px(180.))
                                                                    .truncate()
                                                                    .child(h.title.clone()),
                                                            )
                                                    },
                                                ))
                                                .child(
                                                    Icon::new(if show {
                                                        IconName::ChevronDown
                                                    } else {
                                                        IconName::ChevronRight
                                                    })
                                                    .xsmall()
                                                    .text_color(p.text_faint),
                                                ),
                                        )
                                        .when(show, |el| {
                                            el.children(t.hits.iter().enumerate().map(|(i, h)| {
                                                super::components::hit_card(h, i, cx)
                                            }))
                                        }),
                                )
                            })
                            .when_some(t.stats.clone(), |el, s| {
                                el.child(div().text_xs().text_color(p.text_faint).child(s))
                            }),
                    ),
            );
        fade_in_up(turn, SharedString::from(format!("turn-{id}")), 0).into_any_element()
    }
}

impl Render for ChatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let scope = self.scope;
        v_flex()
            .size_full()
            .p_5()
            .gap_4()
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(div().flex_1().child(page_title("Ask", "Answers grounded in your documents and meetings, with exact citations.", cx)))
                    .child(
                        segmented_track(cx)
                            .child(
                                segment_item("scope-all", "Everything", None, scope == Scope::All, cx).on_click(cx.listener(|this, _, _, cx| {
                                    this.scope = Scope::All;
                                    cx.notify();
                                })),
                            )
                            .child(
                                segment_item("scope-docs", "Documents", None, scope == Scope::Documents, cx).on_click(cx.listener(|this, _, _, cx| {
                                    this.scope = Scope::Documents;
                                    cx.notify();
                                })),
                            )
                            .child(
                                segment_item("scope-meet", "Meetings", None, scope == Scope::Meetings, cx).on_click(cx.listener(|this, _, _, cx| {
                                    this.scope = Scope::Meetings;
                                    cx.notify();
                                })),
                            ),
                    ),
            )
            .child(if self.turns.is_empty() {
                let has_data = !self.suggestions.is_empty();
                card(cx)
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .size_full()
                            .items_center()
                            .justify_center()
                            .gap_4()
                            .child(
                                empty_state(
                                    IconName::MessagesSquare,
                                    if has_data { "Ask about what you have on file" } else { "Nothing to ask about yet" },
                                    if has_data {
                                        "Answers come with numbered citations you can expand to the exact passage."
                                    } else {
                                        "Record a meeting or add documents in Knowledge, then ask questions about them here."
                                    },
                                    cx,
                                )
                                .h_auto(),
                            )
                            .when(has_data, |el| {
                                el.child(
                                    h_flex().flex_wrap().justify_center().gap_2().pb_8().children(
                                        self.suggestions.iter().enumerate().map(|(i, text)| {
                                            let q = text.clone();
                                            Button::new(SharedString::from(format!("sug-{i}")))
                                                .outline()
                                                .small()
                                                .label(text.clone())
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.input.update(cx, |s, cx| s.set_value(q.clone(), window, cx));
                                                    this.send(window, cx);
                                                }))
                                        }),
                                    ),
                                )
                            }),
                    )
                    .into_any_element()
            } else {
                div()
                    .id("chat-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(v_flex().gap_6().pb_4().children(self.turns.iter().map(|t| self.render_turn(t, cx))))
                    .into_any_element()
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .p_2()
                    .rounded(p.radius())
                    .bg(p.surface_2)
                    .border_1()
                    .border_color(p.line)
                    .shadow(p.shadow(0.8))
                    .child(div().flex_1().child(Textarea::new(&self.input).appearance(false).text_sm()))
                    .child(Button::new("send").primary().icon(IconName::SendHorizontal).on_click(cx.listener(|this, _, window, cx| this.send(window, cx))))
                    .child(chip("local", p.success, cx)),
            )
    }
}

const CHAT_SYSTEM: &str = "You are Marginalia, a private on-device assistant. Answer the user's question using the provided passages from their documents and meeting transcripts. Cite passages inline with their number in square brackets, like [2], right after the sentence they support. If the passages are insufficient, say what is missing instead of guessing, and answer from general knowledge only when clearly labeled as such. Be concise and well formatted in Markdown.";
