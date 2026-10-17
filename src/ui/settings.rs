//! Settings, ordered by how often people change things: audio and meeting behaviour first,
//! appearance next, and heavy model options tucked under "Advanced".
//!
//! Light controls apply instantly (no Save button). Only model changes, which reload
//! multi-gigabyte weights, are staged and confirmed with an explicit "Apply and reload".

use super::components::{card, card_header, choice_chip, chip, page_title, pal, segment_item, segmented_track};
use super::theme::{with_alpha, TypeScale};
use crate::app::Services;
use crate::audio::list_input_devices;
use crate::config::{Appearance, ChatModel, Settings, WhisperModel};
use crate::models;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{h_flex, v_flex, Disableable, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::Arc;

pub struct SettingsView {
    services: Arc<Services>,
    /// Working copy: light fields mirror `services.settings()`, heavy fields are staged.
    draft: Settings,
    language: Entity<InputState>,
    chunk_tokens: Entity<InputState>,
    devices: Vec<(String, String)>,
    ready: bool,
    advanced_open: bool,
    _subs: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(services: Arc<Services>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let s = services.settings();
        let language = cx.new(|cx| InputState::new(window, cx).placeholder("auto"));
        language.update(cx, |st, cx| st.set_value(s.language.clone(), window, cx));
        let chunk_tokens = cx.new(|cx| InputState::new(window, cx).placeholder("220"));
        chunk_tokens.update(cx, |st, cx| st.set_value(s.chunk_tokens.to_string(), window, cx));
        let subs = vec![
            cx.subscribe_in(&language, window, |this, _, ev: &InputEvent, _w, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit_light(cx);
                }
            }),
            cx.subscribe_in(&chunk_tokens, window, |this, _, ev: &InputEvent, _w, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit_light(cx);
                }
            }),
        ];
        Self {
            services,
            draft: s,
            language,
            chunk_tokens,
            devices: list_input_devices(),
            ready: false,
            advanced_open: false,
            _subs: subs,
        }
    }

    pub fn set_ready(&mut self, ready: bool, cx: &mut Context<Self>) {
        self.ready = ready;
        cx.notify();
    }

    fn needs_reload(&self) -> bool {
        let cur = self.services.settings();
        self.draft.chat_model != cur.chat_model
            || self.draft.whisper_model != cur.whisper_model
            || self.draft.use_gpu != cur.use_gpu
            || self.draft.chat_context != cur.chat_context
    }

    /// Apply the light settings immediately, leaving staged model choices alone.
    fn commit_light(&mut self, cx: &mut Context<Self>) {
        let mut cur = self.services.settings();
        let before = cur.clone();
        let lang = self.language.read(cx).value().trim().to_string();
        cur.language = if lang.is_empty() { "auto".into() } else { lang };
        if let Ok(n) = self.chunk_tokens.read(cx).value().trim().parse::<usize>() {
            cur.chunk_tokens = n.clamp(96, 512);
        }
        cur.capture_system_audio = self.draft.capture_system_audio;
        cur.mic_device = self.draft.mic_device.clone();
        cur.live_assistance = self.draft.live_assistance;
        cur.appearance = self.draft.appearance;
        self.services.save_settings(cur.clone());
        if cur.appearance != before.appearance {
            super::theme::apply(cur.resolve_dark(cx.window_appearance()), cx);
        }
        if cur.language != before.language {
            if let Some(w) = self.services.engine.whisper() {
                w.set_language(cur.language.clone());
            }
        }
        self.services.kb.set_chunk_tokens(cur.chunk_tokens);
        cx.notify();
    }

    fn apply_models(&mut self, cx: &mut Context<Self>) {
        let mut cur = self.services.settings();
        cur.chat_model = self.draft.chat_model;
        cur.whisper_model = self.draft.whisper_model;
        cur.use_gpu = self.draft.use_gpu;
        cur.chat_context = self.draft.chat_context;
        self.services.save_settings(cur);
        self.ready = false;
        crate::app::reload_models(self.services.clone());
        cx.notify();
    }

    fn option_card(
        &self,
        id: &'static str,
        title: &'static str,
        desc: &'static str,
        selected: bool,
        present: bool,
        on: impl Fn(&mut Self) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = pal(cx);
        v_flex()
            .id(id)
            .flex_1()
            .min_w(px(220.))
            .gap_1()
            .p_3()
            .rounded(p.radius_sm())
            .bg(if selected { with_alpha(p.accent_2, 0.10) } else { p.surface_3 })
            .border_1()
            .border_color(if selected { p.accent_2 } else { p.line })
            .cursor_pointer()
            .hover(move |s| s.border_color(p.accent_2))
            .on_click(cx.listener(move |this, _, _, cx| {
                on(this);
                cx.notify();
            }))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Icon::new(if selected { IconName::CircleCheck } else { IconName::Circle })
                            .small()
                            .text_color(if selected { p.accent_2 } else { p.text_faint }),
                    )
                    .child(div().t_body().font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(title))
                    .child(div().flex_1())
                    .child(chip(
                        if present { "downloaded" } else { "will download" },
                        if present { p.success } else { p.text_dim },
                        cx,
                    )),
            )
            .child(div().t_footnote().text_color(p.text_dim).child(desc))
    }

    fn section(&self, title: &'static str, icon: IconName, body: impl IntoElement, cx: &mut Context<Self>) -> Div {
        card(cx).child(card_header(title, icon, cx)).child(v_flex().px_4().py_2().child(body))
    }

    /// A setting row: label + help on the left, control on the right, hairline below.
    fn row(
        &self,
        label: &'static str,
        help: &'static str,
        control: impl IntoElement,
        last: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = pal(cx);
        h_flex()
            .gap_4()
            .items_center()
            .min_h(px(56.))
            .py_2p5()
            .when(!last, |el| el.border_b_1().border_color(p.line))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().t_body().font_weight(FontWeight::MEDIUM).text_color(p.text).child(label))
                    .child(div().t_footnote().text_color(p.text_dim).child(help)),
            )
            .child(div().flex_shrink_0().child(control))
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let paths = self.services.paths.clone();
        let d = self.draft.clone();
        let gpu_built = cfg!(any(feature = "cuda", feature = "vulkan"));
        let devices = self.devices.clone();
        let needs_reload = self.needs_reload();

        // ---------- Audio & meeting ----------
        let audio_body = v_flex()
            .child(self.row(
                "Capture system audio",
                "Hear the other side of any call or video through PipeWire. No virtual cables needed.",
                Switch::new("sys-audio").checked(d.capture_system_audio).on_click(cx.listener(|this, v: &bool, _, cx| {
                    this.draft.capture_system_audio = *v;
                    this.commit_light(cx);
                })),
                false,
                cx,
            ))
            .child(
                v_flex()
                    .gap_2()
                    .py_2p5()
                    .border_b_1()
                    .border_color(p.line)
                    .child(div().t_body().font_weight(FontWeight::MEDIUM).text_color(p.text).child("Microphone"))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                choice_chip("mic-default", "System default", d.mic_device.is_none(), cx).on_click(cx.listener(|this, _, _, cx| {
                                    this.draft.mic_device = None;
                                    this.commit_light(cx);
                                })),
                            )
                            .children(devices.into_iter().enumerate().map(|(i, (id, label))| {
                                let sel = d.mic_device.as_deref() == Some(id.as_str());
                                let n2 = id.clone();
                                choice_chip(SharedString::from(format!("mic-{i}")), label, sel, cx).on_click(cx.listener(move |this, _, _, cx| {
                                    this.draft.mic_device = Some(n2.clone());
                                    this.commit_light(cx);
                                }))
                            })),
                    ),
            )
            .child(self.row(
                "Transcription language",
                "A code like en, hi or de, or auto to detect. Press Enter to apply.",
                div().w(px(120.)).child(Input::new(&self.language)),
                false,
                cx,
            ))
            .child(self.row(
                "Live Assistance",
                "Notice questions in the conversation and answer them from your documents, with citations.",
                Switch::new("live-assist").checked(d.live_assistance).on_click(cx.listener(|this, v: &bool, _, cx| {
                    this.draft.live_assistance = *v;
                    this.commit_light(cx);
                })),
                true,
                cx,
            ));

        // ---------- Appearance ----------
        let appearance_body = self.row(
            "Theme",
            "System follows your desktop. Graphite is dark; Paper is warm and light.",
            segmented_track(cx)
                .child(
                    segment_item("ap-sys", "System", None, d.appearance == Appearance::System, cx).on_click(cx.listener(|this, _, _, cx| {
                        this.draft.appearance = Appearance::System;
                        this.commit_light(cx);
                    })),
                )
                .child(
                    segment_item("ap-light", "Paper", None, d.appearance == Appearance::Light, cx).on_click(cx.listener(|this, _, _, cx| {
                        this.draft.appearance = Appearance::Light;
                        this.commit_light(cx);
                    })),
                )
                .child(
                    segment_item("ap-dark", "Graphite", None, d.appearance == Appearance::Dark, cx).on_click(cx.listener(|this, _, _, cx| {
                        this.draft.appearance = Appearance::Dark;
                        this.commit_light(cx);
                    })),
                ),
            true,
            cx,
        );

        // ---------- Knowledge ----------
        let knowledge_body = v_flex()
            .child(self.row(
                "Passage size",
                "Target tokens per indexed passage (96 to 512). Smaller gives sharper citations, larger gives more context. Applies to new documents.",
                div().w(px(100.)).child(Input::new(&self.chunk_tokens)),
                false,
                cx,
            ))
            .child(
                v_flex()
                    .gap_0p5()
                    .py_2p5()
                    .child(div().t_footnote().text_color(p.text_dim).child(format!("Data  ·  {}", paths.data_dir.display())))
                    .child(div().t_footnote().text_color(p.text_dim).child(format!("Models  ·  {}", paths.models_dir.display()))),
            );

        // ---------- Advanced: models ----------
        let models_body = v_flex()
            .gap_3()
            .py_2p5()
            .child(div().t_footnote().text_color(p.text_dim).child("Chat model"))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(self.option_card(
                        "m-e2b",
                        "Gemma 4 E2B",
                        "Fastest. Great on CPU, instant on GPU. Default.",
                        d.chat_model == ChatModel::GemmaE2B,
                        models::is_present(&paths, &models::GEMMA_4_E2B),
                        |this| this.draft.chat_model = ChatModel::GemmaE2B,
                        cx,
                    ))
                    .child(self.option_card(
                        "m-e4b",
                        "Gemma 4 E4B",
                        "Stronger writing and reasoning. About 4.6 GB; best with a GPU.",
                        d.chat_model == ChatModel::GemmaE4B,
                        models::is_present(&paths, &models::GEMMA_4_E4B),
                        |this| this.draft.chat_model = ChatModel::GemmaE4B,
                        cx,
                    )),
            )
            .child(div().t_footnote().text_color(p.text_dim).child("Transcription model"))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(self.option_card(
                        "w-small",
                        "Whisper small",
                        "Light and quick on CPU. Good for clear audio.",
                        d.whisper_model == WhisperModel::Small,
                        models::is_present(&paths, &models::WHISPER_SMALL),
                        |this| this.draft.whisper_model = WhisperModel::Small,
                        cx,
                    ))
                    .child(self.option_card(
                        "w-turbo",
                        "Whisper large-v3-turbo",
                        "Best accuracy for meetings. Recommended with a GPU.",
                        d.whisper_model == WhisperModel::LargeTurbo,
                        models::is_present(&paths, &models::WHISPER_LARGE_TURBO),
                        |this| this.draft.whisper_model = WhisperModel::LargeTurbo,
                        cx,
                    )),
            )
            .child(self.row(
                "GPU acceleration",
                if gpu_built {
                    "Run Gemma, EmbeddingGemma and Whisper on your NVIDIA GPU. Falls back to CPU automatically if memory runs out."
                } else {
                    "This build has no GPU backend; everything runs on the CPU."
                },
                Switch::new("gpu").checked(d.use_gpu && gpu_built).on_click(cx.listener(|this, v: &bool, _, cx| {
                    this.draft.use_gpu = *v;
                    cx.notify();
                })),
                false,
                cx,
            ))
            .child(self.row(
                "Chat context window",
                "How much text the chat model can read at once. Larger uses more memory; long chats are trimmed to fit.",
                segmented_track(cx).children([4096u32, 8192, 16384].into_iter().map(|n| {
                    segment_item(SharedString::from(format!("ctx-{n}")), format!("{}k", n / 1024), None, d.chat_context == n, cx)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.draft.chat_context = n;
                            cx.notify();
                        }))
                })),
                true,
                cx,
            ))
            .when(needs_reload, |el| {
                el.child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .p_3()
                        .rounded(p.radius_sm())
                        .bg(with_alpha(p.warning, 0.12))
                        .border_1()
                        .border_color(with_alpha(p.warning, 0.35))
                        .child(Icon::new(IconName::TriangleAlert).small().text_color(p.warning))
                        .child(
                            div()
                                .flex_1()
                                .t_footnote()
                                .text_color(p.text)
                                .child("Changing models reloads them and may download new files."),
                        )
                        .child(
                            Button::new("apply-models")
                                .primary()
                                .small()
                                .label("Apply and reload")
                                .disabled(!self.ready)
                                .on_click(cx.listener(|this, _, _, cx| this.apply_models(cx))),
                        ),
                )
            });

        let advanced_open = self.advanced_open;
        let advanced = card(cx)
            .child(
                h_flex()
                    .id("advanced-toggle")
                    .px_4()
                    .py_3()
                    .gap_2p5()
                    .items_center()
                    .cursor_pointer()
                    .hover(move |s| s.bg(p.surface_3))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.advanced_open = !this.advanced_open;
                        cx.notify();
                    }))
                    .child(super::components::icon_tile(IconName::Cpu, p.accent_2, 26., cx))
                    .child(div().t_body().font_weight(FontWeight::SEMIBOLD).text_color(p.text).child("Advanced: models and hardware"))
                    .child(div().flex_1())
                    .when(needs_reload, |el| el.child(chip("unapplied changes", p.warning, cx)))
                    .child(
                        Icon::new(if advanced_open { IconName::ChevronDown } else { IconName::ChevronRight })
                            .small()
                            .text_color(p.text_dim),
                    ),
            )
            .when(advanced_open, |el| el.child(div().px_4().pb_3().border_t_1().border_color(p.line).child(models_body)));

        v_flex()
            .size_full()
            .px_6()
            .pt_5()
            .gap_4()
            .child(page_title("Settings", "Changes apply as you make them.", cx))
            .child(
                div().id("settings-scroll").flex_1().min_h_0().overflow_y_scroll().child(
                    v_flex()
                        .max_w(px(760.))
                        .gap_4()
                        .pb_6()
                        .child(self.section("Audio and meetings", IconName::Mic, audio_body, cx))
                        .child(self.section("Appearance", IconName::SunMoon, appearance_body, cx))
                        .child(self.section("Knowledge base", IconName::LibraryBig, knowledge_body, cx))
                        .child(advanced),
                ),
            )
    }
}
