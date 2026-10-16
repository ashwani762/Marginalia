//! Settings: models, hardware, audio, appearance, knowledge.

use super::components::{card, card_header, chip, page_title, pal};
use crate::app::{NoteKind, Services};
use crate::audio::list_input_devices;
use crate::config::{ChatModel, Settings, WhisperModel};
use crate::models;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::assets::IconName;
use gpui_kit::component::{h_flex, v_flex, Disableable, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::Arc;

pub struct SettingsView {
    services: Arc<Services>,
    draft: Settings,
    saved: Settings,
    language: Entity<InputState>,
    chunk_tokens: Entity<InputState>,
    devices: Vec<(String, String)>,
    ready: bool,
}

impl SettingsView {
    pub fn new(services: Arc<Services>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let s = services.settings();
        let language = cx.new(|cx| InputState::new(window, cx).placeholder("auto"));
        language.update(cx, |st, cx| st.set_value(s.language.clone(), window, cx));
        let chunk_tokens = cx.new(|cx| InputState::new(window, cx).placeholder("220"));
        chunk_tokens.update(cx, |st, cx| st.set_value(s.chunk_tokens.to_string(), window, cx));
        Self { services, draft: s.clone(), saved: s, language, chunk_tokens, devices: list_input_devices(), ready: false }
    }

    pub fn set_ready(&mut self, ready: bool, cx: &mut Context<Self>) {
        self.ready = ready;
        cx.notify();
    }

    fn needs_reload(&self) -> bool {
        self.draft.chat_model != self.saved.chat_model
            || self.draft.whisper_model != self.saved.whisper_model
            || self.draft.use_gpu != self.saved.use_gpu
            || self.draft.chat_context != self.saved.chat_context
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        self.draft.language = self.language.read(cx).value().trim().to_string();
        if self.draft.language.is_empty() {
            self.draft.language = "auto".into();
        }
        if let Ok(n) = self.chunk_tokens.read(cx).value().trim().parse::<usize>() {
            self.draft.chunk_tokens = n.clamp(96, 512);
        }
        let reload = self.needs_reload();
        let language_changed = self.draft.language != self.saved.language;
        self.services.save_settings(self.draft.clone());
        self.saved = self.draft.clone();
        super::theme::apply(self.draft.dark_mode, cx);
        self.services.kb.set_chunk_tokens(self.draft.chunk_tokens);
        if reload {
            self.ready = false;
            crate::app::reload_models(self.services.clone());
        } else if language_changed {
            if let Some(w) = self.services.engine.whisper() {
                w.set_language(self.draft.language.clone());
            }
            self.services.notify(NoteKind::Success, "Settings saved");
        } else {
            self.services.notify(NoteKind::Success, "Settings saved");
        }
        cx.notify();
    }

    fn option_card(&self, id: &'static str, title: &'static str, desc: &'static str, selected: bool, present: bool, on: impl Fn(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        v_flex()
            .id(id)
            .flex_1()
            .min_w(px(220.))
            .gap_1()
            .p_3()
            .rounded(px(12.))
            .bg(if selected { super::theme::with_alpha(p.accent_2, 0.12) } else { p.surface_3 })
            .border_1()
            .border_color(if selected { p.accent_2 } else { p.line })
            .cursor_pointer()
            .hover(move |s| s.border_color(p.accent_2))
            .on_click(cx.listener(move |this, _, _, cx| {
                on(this, cx);
                cx.notify();
            }))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(if selected { IconName::CircleCheck } else { IconName::Circle }).small().text_color(if selected { p.accent_2 } else { p.text_faint }))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(title))
                    .child(div().flex_1())
                    .child(chip(if present { "downloaded" } else { "will download" }, if present { p.success } else { p.text_faint }, cx)),
            )
            .child(div().text_xs().text_color(p.text_dim).child(desc))
    }

    fn section(&self, title: &'static str, icon: IconName, body: impl IntoElement, cx: &mut Context<Self>) -> impl IntoElement {
        card(cx).child(card_header(title, icon, cx)).child(v_flex().p_4().gap_3().child(body))
    }

    fn row(&self, label: &'static str, help: &'static str, control: impl IntoElement, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        h_flex()
            .gap_4()
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).text_color(p.text).child(label))
                    .child(div().text_xs().text_color(p.text_dim).child(help)),
            )
            .child(control)
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let paths = self.services.paths.clone();
        let d = self.draft.clone();
        let gpu_built = cfg!(any(feature = "cuda", feature = "vulkan"));
        let devices = self.devices.clone();

        let models_body = v_flex()
            .gap_3()
            .child(div().text_xs().text_color(p.text_faint).child("Chat model"))
            .child(
                h_flex()
                    .gap_3()
                    .child(self.option_card(
                        "m-e2b",
                        "Gemma 4 E2B",
                        "Fastest. Great on CPU, instant on GPU. Default.",
                        d.chat_model == ChatModel::GemmaE2B,
                        models::is_present(&paths, &models::GEMMA_4_E2B),
                        |this, _| this.draft.chat_model = ChatModel::GemmaE2B,
                        cx,
                    ))
                    .child(self.option_card(
                        "m-e4b",
                        "Gemma 4 E4B",
                        "Stronger writing and reasoning. ~4.6 GB, best with GPU.",
                        d.chat_model == ChatModel::GemmaE4B,
                        models::is_present(&paths, &models::GEMMA_4_E4B),
                        |this, _| this.draft.chat_model = ChatModel::GemmaE4B,
                        cx,
                    )),
            )
            .child(div().text_xs().text_color(p.text_faint).child("Transcription model"))
            .child(
                h_flex()
                    .gap_3()
                    .child(self.option_card(
                        "w-small",
                        "Whisper small",
                        "Light and fast on CPU. Good for clear audio.",
                        d.whisper_model == WhisperModel::Small,
                        models::is_present(&paths, &models::WHISPER_SMALL),
                        |this, _| this.draft.whisper_model = WhisperModel::Small,
                        cx,
                    ))
                    .child(self.option_card(
                        "w-turbo",
                        "Whisper large-v3-turbo",
                        "Best accuracy for meetings. Recommended with GPU.",
                        d.whisper_model == WhisperModel::LargeTurbo,
                        models::is_present(&paths, &models::WHISPER_LARGE_TURBO),
                        |this, _| this.draft.whisper_model = WhisperModel::LargeTurbo,
                        cx,
                    )),
            )
            .child(self.row(
                "Use GPU acceleration",
                if gpu_built { "Offloads Gemma, EmbeddingGemma and Whisper to your NVIDIA GPU (CUDA)." } else { "This build has no GPU backend; everything runs on CPU." },
                Switch::new("gpu").checked(d.use_gpu && gpu_built).on_click(cx.listener(|this, v: &bool, _, cx| {
                    this.draft.use_gpu = *v;
                    cx.notify();
                })),
                cx,
            ))
            .child(self.row(
                "Chat context window",
                "Tokens the chat model can see at once. Larger uses more memory.",
                h_flex().gap_1().children([4096u32, 8192, 16384].into_iter().map(|n| {
                    let sel = d.chat_context == n;
                    Button::new(SharedString::from(format!("ctx-{n}")))
                        .label(format!("{}k", n / 1024))
                        .xsmall()
                        .when(sel, |b| b.primary())
                        .when(!sel, |b| b.ghost())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.draft.chat_context = n;
                            cx.notify();
                        }))
                })),
                cx,
            ));

        let audio_body = v_flex()
            .gap_3()
            .child(self.row(
                "Capture system audio",
                "Hear the other side of any call via PipeWire's monitor. No virtual cables needed.",
                Switch::new("sys-audio").checked(d.capture_system_audio).on_click(cx.listener(|this, v: &bool, _, cx| {
                    this.draft.capture_system_audio = *v;
                    cx.notify();
                })),
                cx,
            ))
            .child(div().text_xs().text_color(p.text_faint).child("Microphone"))
            .child(
                div().flex().flex_wrap().gap_2().child(
                    Button::new("mic-default")
                        .label("System default (PipeWire)")
                        .small()
                        .when(d.mic_device.is_none(), |b| b.primary())
                        .when(d.mic_device.is_some(), |b| b.outline())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.draft.mic_device = None;
                            cx.notify();
                        })),
                )
                .children(devices.into_iter().enumerate().map(|(i, (id, label))| {
                    let sel = d.mic_device.as_deref() == Some(id.as_str());
                    let n2 = id.clone();
                    Button::new(SharedString::from(format!("mic-{i}")))
                        .label(label)
                        .small()
                        .when(sel, |b| b.primary())
                        .when(!sel, |b| b.outline())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.draft.mic_device = Some(n2.clone());
                            cx.notify();
                        }))
                })),
            )
            .child(self.row(
                "Transcription language",
                "ISO code like en, hi, de — or auto to detect.",
                div().w(px(120.)).child(Input::new(&self.language)),
                cx,
            ))
            .child(self.row(
                "Live Assistance",
                "Watch the conversation for questions and surface cited answers proactively.",
                Switch::new("live-assist").checked(d.live_assistance).on_click(cx.listener(|this, v: &bool, _, cx| {
                    this.draft.live_assistance = *v;
                    cx.notify();
                })),
                cx,
            ));

        let appearance_body = self.row(
            "Dark mode",
            "Marginalia's graphite theme. Light mode follows the component defaults.",
            Switch::new("dark").checked(d.dark_mode).on_click(cx.listener(|this, v: &bool, _, cx| {
                this.draft.dark_mode = *v;
                super::theme::apply(*v, cx);
                cx.notify();
            })),
            cx,
        );

        let knowledge_body = v_flex()
            .gap_3()
            .child(self.row(
                "Passage size (tokens)",
                "Target size of each indexed passage (96–512). Smaller = more precise citations; larger = more context.",
                div().w(px(100.)).child(Input::new(&self.chunk_tokens)),
                cx,
            ))
            .child(div().text_xs().text_color(p.text_faint).child(format!("Data: {}", paths.data_dir.display())))
            .child(div().text_xs().text_color(p.text_faint).child(format!("Models: {}", paths.models_dir.display())));

        v_flex()
            .size_full()
            .p_5()
            .gap_4()
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(div().flex_1().child(page_title("Settings", "Models, hardware, audio and appearance.", cx)))
                    .when(self.needs_reload(), |el| el.child(chip("model reload required", p.warning, cx)))
                    .child(
                        Button::new("apply")
                            .primary()
                            .icon(IconName::Check)
                            .label(if self.needs_reload() { "Apply & reload models" } else { "Save" })
                            .disabled(!self.ready)
                            .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
                    ),
            )
            .child(
                div().id("settings-scroll").flex_1().min_h_0().overflow_y_scroll().child(
                    v_flex()
                        .gap_4()
                        .pb_4()
                        .child(self.section("Models", IconName::Cpu, models_body, cx))
                        .child(self.section("Audio & live meeting", IconName::Mic, audio_body, cx))
                        .child(self.section("Appearance", IconName::SunMoon, appearance_body, cx))
                        .child(self.section("Knowledge base", IconName::LibraryBig, knowledge_body, cx)),
                ),
            )
    }
}
