//! Root view: sidebar navigation, onboarding overlay, event dispatch.

pub mod anim;
pub mod chat;
pub mod components;
pub mod knowledge;
pub mod meeting;
pub mod settings;
pub mod stats;
pub mod theme;

use crate::app::{EngineStatus, GenTarget, NoteKind, Services, UiEvent};
use crate::models::{self, DownloadProgress};
use anim::{fade_in_up, live, pulse, slide_in_left};
use components::{brand_mark, card, dot, glow_dot, pal};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme, Icon, Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::sync::Arc;
use theme::with_alpha;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Meeting,
    Knowledge,
    Chat,
    Stats,
    Settings,
}

impl Route {
    fn label(self) -> &'static str {
        match self {
            Route::Meeting => "Meetings",
            Route::Knowledge => "Knowledge",
            Route::Chat => "Ask",
            Route::Stats => "Stats",
            Route::Settings => "Settings",
        }
    }
    fn icon(self) -> IconName {
        match self {
            Route::Meeting => IconName::Mic,
            Route::Knowledge => IconName::LibraryBig,
            Route::Chat => IconName::MessageSquare,
            Route::Stats => IconName::ChartColumn,
            Route::Settings => IconName::Settings,
        }
    }
    const PRIMARY: [Route; 3] = [Route::Meeting, Route::Knowledge, Route::Chat];
    const SECONDARY: [Route; 2] = [Route::Stats, Route::Settings];
}

pub struct Marginalia {
    services: Arc<Services>,
    route: Route,
    route_gen: u64,
    status: EngineStatus,
    downloads: HashMap<&'static str, DownloadProgress>,
    download_errors: HashMap<&'static str, String>,
    meeting: Entity<meeting::MeetingView>,
    knowledge: Entity<knowledge::KnowledgeView>,
    chat: Entity<chat::ChatView>,
    stats: Entity<stats::StatsView>,
    settings: Entity<settings::SettingsView>,
    _pump: Task<()>,
}

impl Marginalia {
    pub fn new(
        services: Arc<Services>,
        rx: smol::channel::Receiver<UiEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let meeting = cx.new(|cx| meeting::MeetingView::new(services.clone(), window, cx));
        let knowledge = cx.new(|cx| knowledge::KnowledgeView::new(services.clone(), window, cx));
        let chat = cx.new(|cx| chat::ChatView::new(services.clone(), window, cx));
        let stats = cx.new(|cx| stats::StatsView::new(services.clone(), cx));
        let settings = cx.new(|cx| settings::SettingsView::new(services.clone(), window, cx));

        let pump = cx.spawn_in(window, async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                let res = this.update_in(cx, |root, window, cx| root.dispatch(ev, window, cx));
                if res.is_err() {
                    break;
                }
            }
        });

        Self {
            services,
            route: Route::Meeting,
            route_gen: 0,
            status: EngineStatus::Loading("Starting".into()),
            downloads: HashMap::new(),
            download_errors: HashMap::new(),
            meeting,
            knowledge,
            chat,
            stats,
            settings,
            _pump: pump,
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.status, EngineStatus::Ready)
    }

    fn dispatch(&mut self, ev: UiEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            UiEvent::Download(p) => {
                self.download_errors.remove(p.id);
                self.downloads.insert(p.id, p);
                cx.notify();
            }
            UiEvent::DownloadFailed { id, error } => {
                self.download_errors.insert(id, error);
                cx.notify();
            }
            UiEvent::Engine(status) => {
                let became_ready = matches!(status, EngineStatus::Ready) && !self.is_ready();
                self.status = status.clone();
                if became_ready {
                    window.push_notification(
                        Notification::success(
                            "All models loaded. Everything runs on this machine.",
                        ),
                        cx,
                    );
                    self.knowledge.update(cx, |v, cx| v.refresh(cx));
                    self.meeting.update(cx, |v, cx| v.refresh_meetings(cx));
                }
                if let EngineStatus::Failed(e) = &status {
                    window.push_notification(Notification::error(e.clone()), cx);
                }
                let ready = self.is_ready();
                self.settings.update(cx, |v, cx| v.set_ready(ready, cx));
                cx.notify();
            }
            UiEvent::Transcript(t) => self
                .meeting
                .update(cx, |v, cx| v.on_transcript(t, window, cx)),
            UiEvent::Ingest(ev) => self
                .knowledge
                .update(cx, |v, cx| v.on_ingest(ev, window, cx)),
            UiEvent::Gen { target, event } => match target {
                GenTarget::Chat(_) => self
                    .chat
                    .update(cx, |v, cx| v.on_gen(target, event, window, cx)),
                _ => self
                    .meeting
                    .update(cx, |v, cx| v.on_gen(target, event, window, cx)),
            },
            UiEvent::Notify { kind, message } => {
                let n = match kind {
                    NoteKind::Info => Notification::info(message),
                    NoteKind::Success => Notification::success(message),
                    NoteKind::Warning => Notification::warning(message),
                    NoteKind::Error => Notification::error(message),
                };
                window.push_notification(n, cx);
            }
        }
    }

    fn go(&mut self, route: Route, cx: &mut Context<Self>) {
        if self.route != route {
            self.route = route;
            self.route_gen += 1;
            match route {
                Route::Stats => self.stats.update(cx, |v, cx| v.refresh(cx)),
                Route::Knowledge => self.knowledge.update(cx, |v, cx| v.refresh(cx)),
                Route::Meeting => self.meeting.update(cx, |v, cx| v.refresh_meetings(cx)),
                _ => {}
            }
            cx.notify();
        }
    }

    fn nav_item(&self, r: Route, i: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let active = self.route == r;
        let recording = r == Route::Meeting && self.meeting.read(cx).is_recording();
        let item = h_flex()
            .id(SharedString::from(format!("nav-{}", r.label())))
            .px_2p5()
            .py_1p5()
            .gap_3()
            .items_center()
            .rounded(p.radius_sm())
            .cursor_pointer()
            .relative()
            .when(active, |el| {
                el.bg(p.surface_3)
                    .border_1()
                    .border_color(p.line_strong)
                    .shadow(p.glow_shadow(p.accent, if p.dark { 0.35 } else { 0.0 }))
            })
            .when(!active, |el| {
                el.border_1().border_color(with_alpha(p.line, 0.0))
            })
            .hover(move |s| s.bg(p.surface_3))
            .on_click(cx.listener(move |this, _, _, cx| this.go(r, cx)))
            // Left accent strip for active route
            .when(active && p.dark, |el| {
                el.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(4.))
                        .bottom(px(4.))
                        .w(px(3.))
                        .rounded_full()
                        .bg(p.accent_gradient(180.)),
                )
            })
            .child(
                div()
                    .size(px(28.))
                    .rounded(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(active, |el| {
                        el.bg(p.accent_gradient(135.))
                            .shadow(p.glow_shadow(p.accent, if p.dark { 0.6 } else { 0.0 }))
                    })
                    .child(Icon::new(r.icon()).small().text_color(if active {
                        p.on_accent()
                    } else {
                        p.text_dim
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if active { p.text } else { p.text_dim })
                    .child(r.label()),
            )
            .when(recording, |el| {
                el.child(pulse(glow_dot(p.danger, 7.), "rec-dot", 1200, 0.35, 1.0))
            });
        fade_in_up(
            item,
            SharedString::from(format!("nav-anim-{i}")),
            (i as u64) * 40,
        )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let status = self.status.clone();
        v_flex()
            .w(px(240.))
            .h_full()
            .flex_shrink_0()
            .bg(p.surface_2)
            .border_r_1()
            .border_color(p.line)
            .px_3()
            .py_4()
            .gap_1()
            .child(fade_in_up(
                h_flex()
                    .px_1()
                    .pb_4()
                    .gap_3()
                    .items_center()
                    .child(live(brand_mark(36., cx), "sidebar-brand"))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .font_family(p.display_font())
                                    .text_lg()
                                    .font_weight(if p.dark {
                                        FontWeight::BOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    })
                                    .text_color(p.text)
                                    .child("Marginalia"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .child("Notes in the margins"),
                            ),
                    ),
                "sidebar-logo",
                0,
            ))
            .child(
                div().pb_3().child(
                    Button::new("sidebar-new")
                        .primary()
                        .icon(IconName::Mic)
                        .label("New meeting")
                        .w_full()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.go(Route::Meeting, cx);
                            this.meeting.update(cx, |v, cx| v.start_new(window, cx));
                        })),
                ),
            )
            .children(
                Route::PRIMARY
                    .iter()
                    .enumerate()
                    .map(|(i, r)| self.nav_item(*r, i, cx)),
            )
            .child(div().flex_1())
            .children(
                Route::SECONDARY
                    .iter()
                    .enumerate()
                    .map(|(i, r)| self.nav_item(*r, i + 3, cx)),
            )
            .child(div().h(px(6.)))
            .child(self.render_status_chip(status, cx))
    }

    fn render_status_chip(&self, status: EngineStatus, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let gpu = self.services.settings().use_gpu;
        let (color, text, busy) = match &status {
            EngineStatus::Ready => (
                p.success,
                if gpu {
                    "Models ready · GPU".to_string()
                } else {
                    "Models ready · CPU".to_string()
                },
                false,
            ),
            EngineStatus::Downloading => (p.warning, "Downloading models…".to_string(), true),
            EngineStatus::Loading(s) => (p.warning, format!("{s}…"), true),
            EngineStatus::Failed(_) => (p.danger, "Engine failed, see Settings".to_string(), false),
        };
        h_flex()
            .px_2p5()
            .py_2()
            .gap_2()
            .items_center()
            .border_t_1()
            .border_color(p.line)
            .child(if busy {
                pulse(dot(color, 7.), "status-dot", 1000, 0.3, 1.0).into_any_element()
            } else {
                glow_dot(color, 7.).into_any_element()
            })
            .child(
                div()
                    .text_xs()
                    .text_color(p.text_faint)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(text),
            )
    }

    fn render_onboarding(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let required = self.services.required_models();
        let paths = self.services.paths.clone();
        let failed = matches!(self.status, EngineStatus::Failed(_));
        v_flex()
            .absolute()
            .inset_0()
            .items_center()
            .justify_center()
            .bg(with_alpha(p.surface, 0.92))
            .child(
                card(cx)
                    .w(px(600.))
                    .p_6()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(live(brand_mark(44., cx), "onboarding-brand"))
                            .child(
                                v_flex()
                                    .child(div().font_family(p.display_font()).text_xl().font_weight(if p.dark { FontWeight::BOLD } else { FontWeight::MEDIUM }).text_color(p.text).child("Setting up your local models"))
                                    .child(div().text_sm().text_color(p.text_dim).child("Downloaded once, then everything runs offline on this machine.")),
                            ),
                    )
                    .children(required.iter().enumerate().map(|(i, spec)| {
                        let present = models::is_present(&paths, spec);
                        let prog = self.downloads.get(spec.id).cloned();
                        let err = self.download_errors.get(spec.id).cloned();
                        let (value, detail) = if present {
                            (1.0, "Ready".to_string())
                        } else if let Some(pr) = &prog {
                            let pct = if pr.total > 0 { pr.downloaded as f32 / pr.total as f32 } else { 0.0 };
                            (pct, format!("{} / {} · {}/s", models::human_bytes(pr.downloaded), models::human_bytes(pr.total), models::human_bytes(pr.bytes_per_sec as u64)))
                        } else {
                            (0.0, if spec.bytes > 0 { models::human_bytes(spec.bytes) } else { "Queued".into() })
                        };
                        let row = v_flex()
                            .gap_2()
                            .p_3()
                            .rounded(p.radius_sm())
                            .bg(p.surface_3)
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(Icon::new(if present { IconName::CircleCheck } else { IconName::Download }).small().text_color(if present { p.success } else { p.accent_2 }))
                                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(spec.label))
                                    .child(div().flex_1())
                                    .child(div().text_xs().text_color(p.text_dim).child(detail)),
                            )
                            .child(div().text_xs().text_color(p.text_faint).child(spec.description))
                            .child(Progress::new(SharedString::from(format!("dl-{}", spec.id))).value(value * 100.0).h(px(6.)))
                            .when_some(err, |el, e| el.child(div().text_xs().text_color(p.danger).child(e)));
                        fade_in_up(row, SharedString::from(format!("dl-row-{i}")), i as u64 * 80)
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(match &self.status {
                                EngineStatus::Loading(s) => pulse(
                                    h_flex().gap_2().items_center().child(dot(p.warning, 8.)).child(div().text_sm().text_color(p.text_dim).child(format!("{s}…"))),
                                    "load-pulse",
                                    1100,
                                    0.5,
                                    1.0,
                                )
                                .into_any_element(),
                                EngineStatus::Failed(e) => div().text_sm().text_color(p.danger).child(e.clone()).into_any_element(),
                                _ => div().text_sm().text_color(p.text_dim).child("Large files resume automatically if interrupted.").into_any_element(),
                            })
                            .child(div().flex_1())
                            .when(failed, |el| {
                                el.child(Button::new("retry-boot").label("Retry").icon(IconName::RefreshCw).on_click(cx.listener(|this, _, _, cx| {
                                    this.status = EngineStatus::Loading("Retrying".into());
                                    crate::app::bootstrap(this.services.clone());
                                    cx.notify();
                                })))
                            }),
                    ),
            )
    }
}

impl Render for Marginalia {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let show_onboarding = !self.is_ready();
        let content: AnyElement = match self.route {
            Route::Meeting => self.meeting.clone().into_any_element(),
            Route::Knowledge => self.knowledge.clone().into_any_element(),
            Route::Chat => self.chat.clone().into_any_element(),
            Route::Stats => self.stats.clone().into_any_element(),
            Route::Settings => self.settings.clone().into_any_element(),
        };
        let content = fade_in_up(
            div().size_full().child(content),
            SharedString::from(format!("route-{}", self.route_gen)),
            0,
        );
        div()
            .size_full()
            .relative()
            .bg(p.surface)
            .text_color(p.text)
            .font_family(theme::BODY_FONT)
            .child(
                h_flex().size_full().child(self.render_sidebar(cx)).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_hidden()
                        .child(content),
                ),
            )
            .when(show_onboarding, |el| el.child(self.render_onboarding(cx)))
    }
}
