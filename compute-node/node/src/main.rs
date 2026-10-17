//! marginalia-node: a stateless helper for the Marginalia phone app. It runs the AI work a phone is
//! slow at, for phones you have paired, and keeps nothing about that work once it is done.

mod admin;
mod discovery;
mod engine;
mod host;
mod metrics;
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
    /// Port the local control channel listens on (this computer only).
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
        /// The name phones see. Defaults to this computer's name.
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
        /// Start locked: a phone that asks has to be allowed (`approve`) or pair with a code.
        #[arg(long)]
        lock: bool,
    },
    /// New phones must be allowed before they can use this computer.
    Lock,
    /// Any phone whose owner trusts this computer may use it (the default).
    Unlock,
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

fn state_dir(cli: &Cli) -> PathBuf {
    cli.state_dir.clone().or_else(|| dirs::config_dir().map(|d| d.join("marginalia-node"))).unwrap_or_else(|| PathBuf::from("marginalia-node-state"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "marginalia_node=info".into())).init();
    let cli = Cli::parse();
    let dir = state_dir(&cli);
    match &cli.cmd {
        Cmd::Pair => {
            let r = admin::ask(cli.admin_port, json!({"cmd": "pair"})).await?;
            println!("On the phone: Settings, Home computers, Add a computer. Type this code within {} minutes:\n\n    {}\n", r["minutes"], r["code"].as_str().unwrap_or(""));
            println!("This computer is called {} and can be reached at:", r["name"]);
            for a in r["addresses"].as_array().into_iter().flatten() {
                println!("    {}", a.as_str().unwrap_or(""));
            }
        }
        Cmd::Status => println!("{}", serde_json::to_string_pretty(&admin::ask(cli.admin_port, json!({"cmd": "status"})).await?)?),
        Cmd::Stats => println!("{}", stats_text(&admin::ask(cli.admin_port, json!({"cmd": "stats"})).await?)),
        Cmd::Lock => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "lock"})).await?),
        Cmd::Unlock => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "unlock"})).await?),
        Cmd::Block { who } => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "block", "who": who})).await?),
        Cmd::Unblock { who } => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "unblock", "who": who})).await?),
        Cmd::Approve { who } => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "approve", "who": who})).await?),
        Cmd::Deny { who } => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "deny", "who": who})).await?),
        Cmd::Revoke { who } => println!("{}", admin::ask(cli.admin_port, json!({"cmd": "revoke", "who": who})).await?),
        Cmd::Probe { addr, code, knock, decide, deny } => probe::run(*addr, dir.join("probe"), code.clone(), *knock, decide.clone(), *deny).await?,
        Cmd::Run { port, name, models, chat_model, embed_model, cpu, lock } => {
            let store = Arc::new(state::Store::open(dir.clone())?);
            let identity = store.identity()?;
            if *lock {
                store.set_locked(true)?;
            }
            let locked_before = store.locked();
            let name = name.clone().unwrap_or_else(|| hostname::get().map(|h| h.to_string_lossy().to_string()).unwrap_or_else(|_| "home-node".into()));
            let models_dir = models.clone().or_else(|| dirs::data_dir().map(|d| d.join("marginalia/models"))).unwrap_or_default();
            tracing::info!("loading models from {}", models_dir.display());
            let opts = host::Options { models_dir, chat_model: chat_model.clone(), embed_model: embed_model.clone(), cpu_only: *cpu };
            let host = Arc::new(tokio::task::spawn_blocking(move || host::Host::start(&opts)).await??);
            let shared = Arc::new(server::Shared { store, host: host.clone(), identity: identity.clone(), name: name.clone(), pairing: Default::default(), metrics: metrics::Metrics::new(), waiting: Default::default(), denied: Default::default(), locked: std::sync::atomic::AtomicBool::new(*lock || locked_before) });
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
            }
            if let Ok(d) = mdns {
                let _ = d.shutdown();
            }
            ep.close(0u32.into(), b"bye");
            host.shutdown();
        }
    }
    Ok(())
}

fn ago(secs: u64) -> String {
    match secs {
        s if s < 90 => format!("{s} s"),
        s if s < 5400 => format!("{} min", s / 60),
        s => format!("{:.1} h", s as f64 / 3600.0),
    }
}

fn stats_text(v: &serde_json::Value) -> String {
    let n = |x: &serde_json::Value| x.as_u64().unwrap_or(0);
    let f = |x: &serde_json::Value| x.as_f64().unwrap_or(0.0);
    let (c, s) = (&v["chat"], &v["search"]);
    let mut out = format!(
        "{} up for {}\n\nChat    {} jobs, {} failed, {} tokens written, {:.0} tokens a second while working\nSearch  {} jobs, {} failed, {} passages, {:.0} passages a second while working\nPaired {} times, {} unknown phones turned away\n",
        v["name"].as_str().unwrap_or("node"), ago(n(&v["uptime_s"])),
        n(&c["jobs"]), n(&c["errors"]), n(&c["generated_tokens"]), f(&c["tokens_per_s"]),
        n(&s["jobs"]), n(&s["errors"]), n(&s["passages"]), f(&s["passages_per_s"]),
        n(&v["pairings"]), n(&v["refused"]),
    );
    out.push_str("\nPhones\n");
    for p in v["phones"].as_array().into_iter().flatten() {
        out.push_str(&format!("  {} ({}): {} chat, {} search\n", p["name"].as_str().unwrap_or(""), p["id"].as_str().unwrap_or(""), n(&p["chat_jobs"]), n(&p["search_jobs"])));
    }
    out.push_str("\nLatest jobs (newest first, sizes only)\n");
    for r in v["recent"].as_array().into_iter().flatten().take(10) {
        out.push_str(&format!("  {} {} {} ms, {}{}\n", r["kind"].as_str().unwrap_or(""), if r["ok"].as_bool().unwrap_or(false) { "ok    " } else { "failed" }, n(&r["ms"]), n(&r["size"]), if r["kind"] == "chat" { " tokens" } else { " passages" }));
    }
    out
}
