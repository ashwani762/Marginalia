//! marginalia-node: a stateless helper for the Marginalia phone app. It runs the AI work a phone is
//! slow at, for phones you have paired, and keeps nothing about that work once it is done.

mod admin;
mod discovery;
mod engine;
mod host;
mod metrics;
mod models_dl;
mod probe;
mod server;
mod state;

use anyhow::Result;
use clap::{Parser, Subcommand};
use marginalia_proto::DEFAULT_PORT;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "marginalia-node", version, about = "A stateless compute node for the Marginalia phone app")]
struct Cli {
    /// Where the node keeps its identity and its list of paired phones.
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,
    /// Port the local control channel listens on (this machine only).
    #[arg(long, global = true, default_value_t = DEFAULT_PORT + 1)]
    admin_port: u16,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the node.
    Run {
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
        /// The name phones see. Defaults to this machine's name.
        #[arg(long)]
        name: Option<String>,
        /// Folder holding gemma-4-E2B-it-Q4_0.gguf and embeddinggemma-2-Q8_0.gguf.
        #[arg(long)]
        models: Option<PathBuf>,
        #[arg(long)]
        chat_model: Option<PathBuf>,
        #[arg(long)]
        embed_model: Option<PathBuf>,
        /// Do not use the graphics card.
        #[arg(long)]
        cpu: bool,
        /// Longest prompt plus answer the chat model handles, in tokens. Lower it to share a small graphics card.
        #[arg(long, default_value_t = 16384)]
        ctx: u32,
        /// Start locked: a phone that asks has to be allowed (`approve`) or pair with a code.
        #[arg(long)]
        lock: bool,
    },
    /// New phones must be allowed before they can use this compute node.
    Lock,
    /// Any phone whose owner trusts this compute node may use it (the default).
    Unlock,
    /// Turn new chat and search jobs away for now. The models stay loaded and running jobs finish.
    Pause,
    /// Take jobs again after `pause`.
    Resume,
    /// Stop the running node: finish running jobs (up to 10 seconds), leave the network, exit.
    Stop,
    /// List or download the model files the node serves.
    Models {
        #[command(subcommand)]
        cmd: ModelsCmd,
    },
    /// Show a one-time code to type on the phone.
    Pair,
    /// Show what the running node serves and which phones are paired.
    Status,
    /// Show how much work the running node has done: jobs, speed, errors. Never any content.
    Stats,
    /// Let a waiting phone in. Give its id if more than one is waiting.
    Approve { who: Option<String> },
    /// Turn a waiting phone down.
    Deny { who: Option<String> },
    /// Turn a phone away for good, even when connecting is open. Give its id or name.
    Block { who: String },
    /// Let a blocked phone ask again.
    Unblock { who: String },
    /// Forget a paired phone (its id from `status`, or its name).
    Revoke { who: String },
    /// Act like a phone: connect, optionally pair, and try each job type.
    Probe {
        addr: std::net::SocketAddr,
        #[arg(long)]
        code: Option<String>,
        /// Ask to be let in without a code.
        #[arg(long)]
        knock: bool,
        /// Answer a waiting phone (its id) instead of running jobs.
        #[arg(long)]
        decide: Option<String>,
        #[arg(long)]
        deny: bool,
    },
}

#[derive(Subcommand)]
enum ModelsCmd {
    /// Show which model files are present, their sizes and the folder.
    List {
        /// The models folder. Defaults to the one `run` uses.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Download the model files from Hugging Face. Safe to stop and run again, it continues where it stopped.
    /// Set the HF_TOKEN environment variable if a repository asks for a login.
    Download {
        /// The models folder. Defaults to the one `run` uses.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Only the chat model or only the search model.
        #[arg(long, value_enum)]
        only: Option<models_dl::Only>,
        /// Which chat model. The node itself loads gemma-4-E2B-it-Q4_0.gguf unless told otherwise with `run --chat-model`.
        #[arg(long, value_enum, default_value = "e2b")]
        chat_model: models_dl::ChatChoice,
    },
}

/// Resolves when the system asks the process to end (systemd stop, `kill`). Never on Windows.
async fn terminated() {
    #[cfg(unix)]
    {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
            return;
        }
    }
    std::future::pending::<()>().await
}

fn state_dir(cli: &Cli) -> PathBuf {
    cli.state_dir.clone().or_else(|| dirs::config_dir().map(|d| d.join("marginalia-node"))).unwrap_or_else(|| PathBuf::from("marginalia-node-state"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "marginalia_node=info".into())).init();
    let cli = Cli::parse();
    let dir = state_dir(&cli);
    match &cli.cmd {
        Cmd::Models { cmd } => match cmd {
            ModelsCmd::List { dir } => models_dl::list(dir.clone())?,
            ModelsCmd::Download { dir, only, chat_model } => models_dl::download(dir.clone(), *only, *chat_model)?,
        },
        Cmd::Pair => {
            let r = admin::ask(cli.admin_port, json!({"cmd": "pair"})).await?;
            println!("On the phone: Settings, Compute nodes, Add a compute node. Type this code within {} minutes:\n\n    {}\n", r["minutes"], r["code"].as_str().unwrap_or(""));
            println!("This node is called {} and can be reached at:", r["name"]);
            for a in r["addresses"].as_array().into_iter().flatten() {
                println!("    {}", a.as_str().unwrap_or(""));
            }
        }
        Cmd::Status => println!("{}", serde_json::to_string_pretty(&admin::ask(cli.admin_port, json!({"cmd": "status"})).await?)?),
        Cmd::Stats => println!("{}", metrics::report(&admin::ask(cli.admin_port, json!({"cmd": "stats"})).await?)),
        Cmd::Pause => {
            admin::ask(cli.admin_port, json!({"cmd": "pause"})).await?;
            println!("Paused. New jobs are turned away until you run `marginalia-node resume`. Jobs already running finish.");
        }
        Cmd::Resume => {
            admin::ask(cli.admin_port, json!({"cmd": "resume"})).await?;
            println!("Resumed. The node takes jobs again.");
        }
        Cmd::Stop => {
            admin::ask(cli.admin_port, json!({"cmd": "shutdown"})).await?;
            println!("Stopping. Running jobs get up to 10 seconds to finish.");
        }
        Cmd::Lock => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "lock"})).await?),
        Cmd::Unlock => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "unlock"})).await?),
        Cmd::Block { who } => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "block", "who": who})).await?),
        Cmd::Unblock { who } => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "unblock", "who": who})).await?),
        Cmd::Approve { who } => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "approve", "who": who})).await?),
        Cmd::Deny { who } => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "deny", "who": who})).await?),
        Cmd::Revoke { who } => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "revoke", "who": who})).await?),
        Cmd::Probe { addr, code, knock, decide, deny } => probe::run(*addr, dir.join("probe"), code.clone(), *knock, decide.clone(), *deny).await?,
        Cmd::Run { port, name, models, chat_model, embed_model, cpu, ctx, lock } => {
            let store = Arc::new(state::Store::open(dir.clone())?);
            let identity = store.identity()?;
            if *lock {
                store.set_locked(true)?;
            }
            let locked_before = store.locked();
            let name = name.clone().unwrap_or_else(|| hostname::get().map(|h| h.to_string_lossy().to_string()).unwrap_or_else(|_| "helper-node".into()));
            let models_dir = models.clone().unwrap_or_else(models_dl::default_dir);
            let missing = models_dl::missing_for_run(&models_dir, chat_model.is_some(), embed_model.is_some());
            if missing.len() == 2 {
                anyhow::bail!("{}", models_dl::missing_message(&models_dir));
            }
            for m in &missing {
                tracing::warn!("the {} model {} is not in {}, so this node will not do {} jobs. Get it with: marginalia-node models download --only {}", m.role, m.file, models_dir.display(), m.role, m.role);
            }
            tracing::info!("loading models from {}", models_dir.display());
            let opts = host::Options { models_dir, chat_model: chat_model.clone(), embed_model: embed_model.clone(), cpu_only: *cpu, ctx: *ctx };
            let host = Arc::new(tokio::task::spawn_blocking(move || host::Host::start(&opts)).await??);
            let shared = Arc::new(server::Shared { store, host: host.clone(), identity: identity.clone(), name: name.clone(), pairing: Default::default(), metrics: metrics::Metrics::new(), waiting: Default::default(), denied: Default::default(), locked: std::sync::atomic::AtomicBool::new(*lock || locked_before), paused: Default::default(), stop: Default::default() });
            let ep = server::endpoint(&shared, *port)?;
            let mdns = discovery::advertise(&name, *port, &identity.fingerprint());
            match &mdns {
                Ok(_) => tracing::info!("announcing {name} on the local network"),
                Err(e) => tracing::warn!("could not announce on the network (phones can still be given the address): {e:#}"),
            }
            tracing::info!("node {name} ready on port {port}; {} phone(s) paired. Run `marginalia-node pair` to add one.", shared.store.list().len());
            tokio::spawn(admin::serve(shared.clone(), cli.admin_port, *port));
            tokio::select! {
                _ = server::serve(shared.clone(), ep.clone()) => {}
                _ = tokio::signal::ctrl_c() => tracing::info!("stopping"),
                _ = terminated() => tracing::info!("stopping (asked to terminate)"),
                _ = shared.stop.notified() => tracing::info!("stopping (asked with `stop`)"),
            }
            // Stop taking new connections and new jobs, then give running jobs a moment to finish.
            shared.paused.store(true, std::sync::atomic::Ordering::Relaxed);
            ep.set_server_config(None);
            if let Ok(d) = &mdns {
                let _ = d.shutdown();
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while shared.in_flight() > 0 && std::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if shared.in_flight() > 0 {
                tracing::warn!("{} job(s) were still running after 10 seconds and were cut off", shared.in_flight());
            } else {
                // Lets the last answers reach the phones before the connections close.
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
            ep.close(0u32.into(), b"bye");
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), ep.wait_idle()).await;
            host.shutdown();
            tracing::info!("stopped");
            // A model thread that is still busy must not keep the process alive.
            std::process::exit(0);
        }
    }
    Ok(())
}
