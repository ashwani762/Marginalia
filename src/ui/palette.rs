//! Command palette (Ctrl+K): one place to jump to any meeting, document or action.
//!
//! Expert users should never have to hunt through navigation: type a few letters, press Enter.
//! Anything that is not a known item can be asked as a question.

use super::components::{icon_tile, pal};
use super::theme::{hexa, with_alpha, TypeScale};
use super::Route;
use crate::app::Services;
use crate::rag::store::DocKind;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum Command {
    NewMeeting,
    ToggleRecording,
    ToggleTranscript,
    FocusSearch,
    Go(Route),
    ToggleTheme,
    OpenMeeting(i64),
    Ask(String),
}

pub enum PaletteEvent {
    Run(Command),
    Close,
}

#[derive(Clone)]
struct Item {
    label: String,
    hint: String,
    icon: IconName,
    cmd: Command,
}

pub struct Palette {
    services: Arc<Services>,
    input: Entity<InputState>,
    items: Vec<Item>,
    selected: usize,
    _subs: Vec<Subscription>,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Palette {
    pub fn new(services: Arc<Services>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a command, a meeting, or a question…"));
        let subs = vec![cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _w, cx| match ev {
            InputEvent::Change => {
                this.selected = 0;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.run_selected(cx),
            _ => {}
        })];
        Self { services, input, items: Vec::new(), selected: 0, _subs: subs }
    }

    /// Rebuild the item list (fresh meetings and documents) and focus the field.
    pub fn open(&mut self, recording: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut items = vec![
            if recording {
                Item { label: "Stop recording".into(), hint: "Ctrl+Enter".into(), icon: IconName::Square, cmd: Command::ToggleRecording }
            } else {
                Item { label: "Start recording".into(), hint: "Ctrl+N".into(), icon: IconName::Mic, cmd: Command::NewMeeting }
            },
            Item { label: "Show or hide transcript".into(), hint: "Ctrl+Shift+T".into(), icon: IconName::Captions, cmd: Command::ToggleTranscript },
            Item { label: "Search meetings".into(), hint: "Ctrl+F".into(), icon: IconName::Search, cmd: Command::FocusSearch },
            Item { label: "Go to Meetings".into(), hint: "Ctrl+1".into(), icon: IconName::Mic, cmd: Command::Go(Route::Meeting) },
            Item { label: "Go to Knowledge".into(), hint: "Ctrl+2".into(), icon: IconName::LibraryBig, cmd: Command::Go(Route::Knowledge) },
            Item { label: "Go to Ask".into(), hint: "Ctrl+3".into(), icon: IconName::MessageSquare, cmd: Command::Go(Route::Chat) },
            Item { label: "Go to Stats".into(), hint: "Ctrl+4".into(), icon: IconName::ChartColumn, cmd: Command::Go(Route::Stats) },
            Item { label: "Go to Settings".into(), hint: "Ctrl+5".into(), icon: IconName::Settings, cmd: Command::Go(Route::Settings) },
            Item { label: "Switch theme".into(), hint: "Paper / Graphite".into(), icon: IconName::SunMoon, cmd: Command::ToggleTheme },
        ];
        if let Ok(meetings) = self.services.kb.store.list_meetings() {
            for m in meetings.into_iter().take(40) {
                let when = chrono::DateTime::from_timestamp(m.started_at, 0)
                    .map(|d| d.with_timezone(&chrono::Local).format("%b %-d, %H:%M").to_string())
                    .unwrap_or_default();
                let title = if m.title.trim().is_empty() { "Untitled meeting".to_string() } else { m.title.clone() };
                items.push(Item { label: title, hint: format!("Meeting · {when}"), icon: IconName::Mic, cmd: Command::OpenMeeting(m.id) });
            }
        }
        if let Ok(docs) = self.services.kb.documents() {
            for d in docs.into_iter().filter(|d| d.kind != DocKind::Meeting).take(40) {
                items.push(Item {
                    label: d.title.clone(),
                    hint: format!("Document · {}", d.kind.as_str()),
                    icon: IconName::FileText,
                    cmd: Command::Go(Route::Knowledge),
                });
            }
        }
        self.items = items;
        self.selected = 0;
        self.input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn filtered(&self, cx: &App) -> Vec<Item> {
        let query = self.input.read(cx).value().trim().to_string();
        let tokens: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
        let mut scored: Vec<(u8, usize, &Item)> = Vec::new();
        for (i, it) in self.items.iter().enumerate() {
            let l = it.label.to_lowercase();
            if tokens.is_empty() {
                scored.push((1, i, it));
            } else if tokens.iter().all(|t| l.contains(t.as_str())) {
                scored.push((if l.starts_with(&tokens[0]) { 0 } else { 1 }, i, it));
            }
        }
        scored.sort_by_key(|(s, i, _)| (*s, *i));
        let mut out: Vec<Item> = scored.into_iter().take(8).map(|(_, _, it)| it.clone()).collect();
        if !query.is_empty() {
            out.push(Item {
                label: format!("Ask: {query}"),
                hint: "Ask your documents and meetings".into(),
                icon: IconName::Sparkles,
                cmd: Command::Ask(query),
            });
        }
        out
    }

    fn run_selected(&mut self, cx: &mut Context<Self>) {
        let list = self.filtered(cx);
        if let Some(item) = list.get(self.selected.min(list.len().saturating_sub(1))) {
            cx.emit(PaletteEvent::Run(item.cmd.clone()));
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.filtered(cx).len();
        if n > 0 {
            self.selected = ((self.selected as isize + delta).rem_euclid(n as isize)) as usize;
        }
        cx.notify();
    }
}

impl Render for Palette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let list = self.filtered(cx);
        let sel = self.selected.min(list.len().saturating_sub(1));
        let scrim = if p.dark { hexa(0x000000, 0.55) } else { hexa(0x2B2926, 0.28) };
        v_flex()
            .absolute()
            .inset_0()
            .items_center()
            .pt(px(96.))
            .bg(scrim)
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Close)))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| match ev.keystroke.key.as_str() {
                "down" => {
                    this.step(1, cx);
                    cx.stop_propagation();
                }
                "up" => {
                    this.step(-1, cx);
                    cx.stop_propagation();
                }
                "escape" => {
                    cx.emit(PaletteEvent::Close);
                    cx.stop_propagation();
                }
                _ => {}
            }))
            .child(
                v_flex()
                    .w(px(600.))
                    .max_w_full()
                    .rounded(p.radius())
                    .bg(p.surface_2)
                    .border_1()
                    .border_color(p.line_strong)
                    .shadow(p.shadow(2.0))
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        h_flex()
                            .px_4()
                            .py_3()
                            .gap_3()
                            .items_center()
                            .border_b_1()
                            .border_color(p.line)
                            .child(Icon::new(IconName::Search).small().text_color(p.text_faint))
                            .child(div().flex_1().child(Input::new(&self.input).appearance(false)))
                            .child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(5.))
                                    .bg(p.surface_3)
                                    .t_caption()
                                    .text_color(p.text_dim)
                                    .child("Esc"),
                            ),
                    )
                    .child(
                        v_flex().p_2().gap_0p5().max_h(px(380.)).children(list.into_iter().enumerate().map(|(i, it)| {
                            let active = i == sel;
                            let cmd = it.cmd.clone();
                            h_flex()
                                .id(SharedString::from(format!("pal-{i}")))
                                .h(px(40.))
                                .px_3()
                                .gap_3()
                                .items_center()
                                .rounded(p.radius_sm())
                                .cursor_pointer()
                                .when(active, |el| el.bg(p.tonal()))
                                .hover(move |s| s.bg(with_alpha(p.text, 0.06)))
                                .on_click(cx.listener(move |_, _, _, cx| cx.emit(PaletteEvent::Run(cmd.clone()))))
                                .child(icon_tile(it.icon, if active { p.accent_2 } else { p.text_dim }, 24., cx))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .t_body()
                                        .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                                        .text_color(p.text)
                                        .child(it.label),
                                )
                                .child(div().t_caption().text_color(p.text_faint).child(it.hint))
                        })),
                    )
                    .child(
                        h_flex()
                            .px_4()
                            .py_2()
                            .gap_4()
                            .border_t_1()
                            .border_color(p.line)
                            .child(div().t_caption().text_color(p.text_faint).child("↑ ↓  navigate"))
                            .child(div().t_caption().text_color(p.text_faint).child("Enter  run"))
                            .child(div().t_caption().text_color(p.text_faint).child("Esc  close")),
                    ),
            )
    }
}
