//! Knowledge base: import documents and links, watch ingestion, test retrieval.

use super::anim::fade_in_up;
use super::components::{card, card_header, chip, doc_color, doc_icon, empty_state, eyebrow, hit_card, icon_tile, page_title, pal, score_bars};
use super::theme::with_alpha;
use crate::app::{NoteKind, Services};
use crate::models::human_bytes;
use crate::rag::store::{DocKind, Document};
use crate::rag::{Hit, IngestEvent, IngestInput, IngestStage, Scope};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::assets::IconName;
use gpui_kit::component::{h_flex, v_flex, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

struct JobRow {
    label: String,
    stage: IngestStage,
    finished_at: Option<Instant>,
}

pub struct KnowledgeView {
    services: Arc<Services>,
    docs: Vec<Document>,
    jobs: BTreeMap<u64, JobRow>,
    url_input: Entity<InputState>,
    search_input: Entity<InputState>,
    results: Vec<Hit>,
    searching: bool,
    search_ms: Option<u128>,
    last_query: String,
    _subs: Vec<Subscription>,
}

impl KnowledgeView {
    pub fn new(services: Arc<Services>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url_input = cx.new(|cx| InputState::new(window, cx).placeholder("https://… paste a link to add a web page or PDF"));
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Test retrieval: ask anything about your documents"));
        let subs = vec![
            cx.subscribe_in(&url_input, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.add_url(window, cx);
                }
            }),
            cx.subscribe_in(&search_input, window, |this, _, ev: &InputEvent, _w, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.run_search(cx);
                }
            }),
        ];
        let mut v = Self {
            services,
            docs: Vec::new(),
            jobs: BTreeMap::new(),
            url_input,
            search_input,
            results: Vec::new(),
            searching: false,
            search_ms: None,
            last_query: String::new(),
            _subs: subs,
        };
        v.refresh(cx);
        v
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Ok(d) = self.services.kb.documents() {
            self.docs = d.into_iter().filter(|d| d.kind != DocKind::Meeting).collect();
        }
        cx.notify();
    }

    fn enqueue_paths(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        let mut n = 0;
        for p in paths {
            if p.is_dir() {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    for e in rd.flatten() {
                        let path = e.path();
                        if path.is_file() && self.enqueue_one(IngestInput::File(path)) {
                            n += 1;
                        }
                    }
                }
            } else if self.enqueue_one(IngestInput::File(p)) {
                n += 1;
            }
        }
        if n == 0 {
            self.services.notify(NoteKind::Warning, "Models are still loading; try again in a moment.");
        }
        cx.notify();
    }

    fn enqueue_one(&mut self, input: IngestInput) -> bool {
        let label = match &input {
            IngestInput::File(p) => p.file_name().and_then(|s| s.to_str()).unwrap_or("file").to_string(),
            IngestInput::Url(u) => u.clone(),
        };
        match self.services.kb.enqueue(input) {
            Some(id) => {
                self.jobs.insert(id, JobRow { label, stage: IngestStage::Queued, finished_at: None });
                true
            }
            None => false,
        }
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let picked = rfd::FileDialog::new()
            .set_title("Add to knowledge base")
            .add_filter(
                "Documents",
                &["pdf", "docx", "pptx", "xlsx", "odt", "odp", "ods", "epub", "rtf", "md", "markdown", "txt", "html", "htm", "csv", "tsv", "json", "rst", "srt", "vtt"],
            )
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "bmp"])
            .add_filter("Audio", &["wav", "mp3", "m4a", "aac", "mp4", "flac", "ogg", "opus"])
            .add_filter("All files", &["*"])
            .pick_files();
        if let Some(paths) = picked {
            self.enqueue_paths(paths, cx);
        }
    }

    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        if let Some(dir) = rfd::FileDialog::new().set_title("Add a folder").pick_folder() {
            self.enqueue_paths(vec![dir], cx);
        }
    }

    fn add_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.url_input.read(cx).value().trim().to_string();
        if url.is_empty() {
            return;
        }
        let url = if url.starts_with("http://") || url.starts_with("https://") { url } else { format!("https://{url}") };
        if self.enqueue_one(IngestInput::Url(url)) {
            self.url_input.update(cx, |s, cx| s.set_value("", window, cx));
        } else {
            self.services.notify(NoteKind::Warning, "Models are still loading; try again in a moment.");
        }
        cx.notify();
    }

    fn remove_doc(&mut self, id: i64, cx: &mut Context<Self>) {
        match self.services.kb.remove_document(id) {
            Ok(()) => self.refresh(cx),
            Err(e) => self.services.notify(NoteKind::Error, format!("Remove failed: {e:#}")),
        }
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        let q = self.search_input.read(cx).value().trim().to_string();
        if q.is_empty() {
            self.results.clear();
            cx.notify();
            return;
        }
        self.searching = true;
        self.last_query = q.clone();
        let sv = self.services.clone();
        let started = Instant::now();
        let task = cx.spawn(async move |this, cx| {
            let hits = cx.background_executor().spawn(async move { sv.kb.search(&q, 8, Scope::All) }).await;
            let _ = this.update(cx, |v, cx| {
                v.searching = false;
                v.search_ms = Some(started.elapsed().as_millis());
                match hits {
                    Ok(h) => v.results = h,
                    Err(e) => v.services.notify(NoteKind::Error, format!("Search failed: {e:#}")),
                }
                cx.notify();
            });
        });
        task.detach();
        cx.notify();
    }

    pub fn on_ingest(&mut self, ev: IngestEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let done = matches!(ev.stage, IngestStage::Done { .. } | IngestStage::Failed { .. } | IngestStage::Skipped { .. });
        let row = self.jobs.entry(ev.job_id).or_insert(JobRow { label: ev.label.clone(), stage: IngestStage::Queued, finished_at: None });
        row.label = ev.label;
        row.stage = ev.stage.clone();
        if done {
            row.finished_at = Some(Instant::now());
            match &ev.stage {
                IngestStage::Done { title, chunks, .. } => {
                    self.services.notify(NoteKind::Success, format!("Added “{title}” · {chunks} passages"));
                    self.refresh(cx);
                }
                IngestStage::Failed { error } => self.services.notify(NoteKind::Error, format!("{}: {error}", row.label)),
                IngestStage::Skipped { reason } => self.services.notify(NoteKind::Info, format!("{}: {reason}", row.label)),
                _ => {}
            }
        }
        // Drop finished rows after a while.
        self.jobs.retain(|_, r| r.finished_at.map(|t| t.elapsed().as_secs() < 25).unwrap_or(true));
        cx.notify();
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().flex_1().child(page_title(
                        "Knowledge",
                        format!(
                            "{} documents · {} passages · hybrid semantic + keyword search with exact quotes",
                            self.docs.len(),
                            self.services.kb.chunk_count()
                        ),
                        cx,
                    )))
                    .child(Button::new("add-folder").outline().icon(IconName::FolderOpen).label("Add folder").on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx))))
                    .child(Button::new("add-files").primary().icon(IconName::FileUp).label("Add files").on_click(cx.listener(|this, _, _, cx| this.pick_files(cx)))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .p_1p5()
                    .rounded(p.radius_sm())
                    .bg(p.surface_2)
                    .border_1()
                    .border_color(p.line)
                    .child(div().flex_1().child(Input::new(&self.url_input).appearance(false).prefix(Icon::new(IconName::Globe).small().text_color(p.text_faint))))
                    .child(Button::new("add-url").small().icon(IconName::Plus).label("Add link").on_click(cx.listener(|this, _, window, cx| this.add_url(window, cx)))),
            )
    }

    fn render_jobs(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.jobs.is_empty() {
            return None;
        }
        let p = pal(cx);
        Some(
            card(cx).child(card_header("Importing", IconName::LoaderCircle, cx)).child(v_flex().p_3().gap_2().children(self.jobs.iter().map(|(id, j)| {
                let (text, value, color) = match &j.stage {
                    IngestStage::Queued => ("queued".to_string(), 0.02, p.text_faint),
                    IngestStage::Extracting => ("extracting text".to_string(), 0.1, p.accent_2),
                    IngestStage::Chunking => ("chunking by section & sentence".to_string(), 0.2, p.accent_2),
                    IngestStage::Embedding { done, total } => (format!("embedding {done}/{total}"), 0.2 + 0.8 * (*done as f32 / (*total).max(1) as f32), p.accent_2),
                    IngestStage::Done { chunks, .. } => (format!("done · {chunks} passages"), 1.0, p.success),
                    IngestStage::Skipped { reason } => (reason.clone(), 1.0, p.warning),
                    IngestStage::Failed { error } => (error.clone(), 1.0, p.danger),
                };
                let busy = !matches!(j.stage, IngestStage::Done { .. } | IngestStage::Failed { .. } | IngestStage::Skipped { .. });
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::MEDIUM).text_color(p.text).child(j.label.clone()))
                            .child(if busy { ShimmerText::new(text).id(SharedString::from(format!("job-{id}"))).into_any_element() } else { div().text_xs().text_color(color).child(text).into_any_element() }),
                    )
                    .child(Progress::new(SharedString::from(format!("job-p-{id}"))).value(value * 100.0).h(px(5.)))
            }))),
        )
    }

    fn render_docs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        card(cx)
            .flex_1()
            .min_h_0()
            .child(card_header("Documents", IconName::LibraryBig, cx).child(div().flex_1()).child(div().text_xs().text_color(p.text_faint).child("drop files anywhere on this page")))
            .child(if self.docs.is_empty() {
                empty_state(
                    IconName::FileUp,
                    "Build your knowledge base",
                    "Add PDFs, Office and OpenDocument files, EPUB, Markdown, text, images, audio recordings or web links. Marginalia indexes them locally so Live Assistance and Ask can cite them.",
                    cx,
                )
                .into_any_element()
            } else {
                div()
                    .id("docs-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .child(v_flex().gap_1().children(self.docs.iter().enumerate().map(|(i, d)| {
                        let id = d.id;
                        let when = chrono::DateTime::from_timestamp(d.added_at, 0)
                            .map(|t| t.with_timezone(&chrono::Local).format("%b %-d").to_string())
                            .unwrap_or_default();
                        let row = h_flex()
                            .id(SharedString::from(format!("doc-{id}")))
                            .gap_3()
                            .items_center()
                            .px_3()
                            .py_2()
                            .rounded(p.radius_sm())
                            .hover(move |s| s.bg(p.surface_3))
                            .child(icon_tile(doc_icon(d.kind), doc_color(d.kind, cx), 34., cx))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().truncate().text_sm().font_weight(FontWeight::MEDIUM).text_color(p.text).child(d.title.clone()))
                                    .child(div().truncate().text_xs().text_color(p.text_faint).child(format!(
                                        "{} · {} passages · {} words · {} · {}",
                                        d.kind.as_str(),
                                        d.chunk_count,
                                        d.word_count,
                                        if d.bytes > 0 { human_bytes(d.bytes) } else { "web".into() },
                                        when
                                    ))),
                            )
                            .child(
                                Button::new(SharedString::from(format!("rm-{id}")))
                                    .ghost()
                                    .small()
                                    .tooltip("Remove from the knowledge base")
                                    .icon(IconName::Trash)
                                    .on_click(cx.listener(move |this, _, _, cx| this.remove_doc(id, cx))),
                            );
                        fade_in_up(row, SharedString::from(format!("doc-anim-{id}")), (i as u64).min(10) * 30)
                    })))
                    .into_any_element()
            })
    }

    fn render_search(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        card(cx)
            .flex_1()
            .min_h_0()
            .child(
                card_header("Retrieval playground", IconName::Search, cx)
                    .child(div().flex_1())
                    .when_some(self.search_ms, |el, ms| el.child(chip(format!("{ms} ms"), p.success, cx))),
            )
            .child(div().p_3().child(Input::new(&self.search_input).prefix(Icon::new(IconName::Search).small().text_color(p.text_faint))))
            .child(if self.searching {
                div().p_4().child(ShimmerText::new("embedding query · routing sections · fusing dense + BM25 · rescoring sentences…").id("search-shimmer")).into_any_element()
            } else if self.results.is_empty() {
                div().px_4().pb_4().text_xs().text_color(p.text_faint).child(if self.last_query.is_empty() {
                    "Try a question your documents can answer. Results show the exact passage and where it came from.".to_string()
                } else {
                    format!("No passages matched “{}”.", self.last_query)
                }).into_any_element()
            } else {
                div()
                    .id("results-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .pb_3()
                    .child(
                        v_flex().gap_2().child(eyebrow(format!("{} passages", self.results.len()), cx)).children(self.results.iter().enumerate().map(|(i, h)| {
                            fade_in_up(
                                v_flex().gap_1p5().child(hit_card(h, i, cx)).child(div().px_1().child(score_bars(h.dense, h.lexical, cx))),
                                SharedString::from(format!("hit-{}-{}", i, h.chunk_id)),
                                i as u64 * 40,
                            )
                        })),
                    )
                    .into_any_element()
            })
    }
}

impl Render for KnowledgeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let jobs = self.render_jobs(cx);
        v_flex()
            .id("knowledge-drop")
            .size_full()
            .p_5()
            .gap_4()
            .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(with_alpha(p.accent_2, 0.06)).border_2().border_color(p.accent_2))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.enqueue_paths(paths.paths().to_vec(), cx);
            }))
            .child(self.render_toolbar(cx))
            .when_some(jobs, |el, j| el.child(j))
            .child(h_flex().flex_1().min_h_0().gap_4().child(div().flex_1().min_w_0().h_full().child(self.render_docs(cx))).child(div().flex_1().min_w_0().h_full().child(self.render_search(cx))))
    }
}
