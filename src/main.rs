//! Marginalia — local-first meeting notes, transcription and knowledge assistant.
//! Rust + GPUI, llama.cpp (Gemma 4, EmbeddingGemma 2) and whisper.cpp, all in-process.

mod app;
mod audio;
mod cli;
mod config;
mod engine;
mod models;
mod rag;
mod ui;

use app::{Services, UiEvent};
use gpui_kit::assets::AllAssets;
use gpui_kit::*;
use parking_lot::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("marginalia=info,warn")),
        )
        .init();

    let paths = config::paths();
    let settings = config::Settings::load(&paths);
    let engine = match engine::Engine::new(paths.clone()) {
        Ok(e) => Arc::new(e),
        Err(e) => {
            eprintln!("failed to initialize inference backend: {e:#}");
            std::process::exit(1);
        }
    };
    let kb = match rag::Kb::open(&paths.db_path) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("failed to open database: {e:#}");
            std::process::exit(1);
        }
    };
    let (events_tx, events_rx) = smol::channel::unbounded::<UiEvent>();
    let services = Arc::new(Services {
        paths,
        settings: RwLock::new(settings.clone()),
        engine,
        kb,
        events: events_tx,
        meters: RwLock::new(None),
        download_cancel: Arc::new(AtomicBool::new(false)),
        degraded: AtomicBool::new(false),
    });

    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        let result = cli::run(services.clone(), args);
        services.engine.shutdown();
        if let Err(e) = result {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    let app = gpui_kit::application().with_assets(AllAssets);
    app.run(move |cx| {
        gpui_kit::init(cx);
        ui::theme::apply(settings.resolve_dark(cx.window_appearance()), cx);
        cx.activate(true);

        let bounds = Bounds::centered(None, size(px(1380.), px(880.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Marginalia".into()), ..Default::default() }),
            window_min_size: Some(size(px(980.), px(640.))),
            ..Default::default()
        };
        let services2 = services.clone();
        let rx = events_rx.clone();
        let (handle, root) = gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| ui::Marginalia::new(services2, rx, window, cx))
        })
        .expect("open window");
        ui::install_shortcuts(handle, root, cx);

        app::bootstrap(services.clone());

        // Release GPU buffers before the CUDA runtime tears down.
        let sv = services.clone();
        cx.on_app_quit(move |_| {
            let sv = sv.clone();
            async move {
                sv.engine.shutdown();
            }
        })
        .detach();
    });
}
