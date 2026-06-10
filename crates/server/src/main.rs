//! `zebrafish` binary entry point (spec §5).
//!
//! Parses the CLI, opens (or restores) the world, spawns the webhook delivery
//! worker, and serves the axum app.

use clap::Parser;
use zebrafish_core::World;
use zebrafish_server::state::AppState;
use zebrafish_server::webhooks::spawn_delivery_worker;
use zebrafish_server::{app, banner};

/// A Stripe-compatible local payment emulator.
#[derive(Parser, Debug)]
#[command(name = "zebrafish", version, about)]
struct Cli {
    /// Address to bind. Defaults to localhost; the Docker image sets 0.0.0.0.
    #[arg(long, env = "ZEBRAFISH_HOST", default_value = "127.0.0.1")]
    host: String,

    /// Port to listen on.
    #[arg(long, short, env = "ZEBRAFISH_PORT", default_value_t = 4242)]
    port: u16,

    /// SQLite database path (ignored when `--ephemeral`).
    #[arg(long, default_value = "zebrafish.db")]
    db: String,

    /// Use an in-memory database that vanishes on exit.
    #[arg(long)]
    ephemeral: bool,

    /// Deterministic seed. Absent: a random seed is chosen, logged, persisted.
    #[arg(long, env = "ZEBRAFISH_SEED")]
    seed: Option<u64>,

    /// Load cascade fixtures from this directory instead of the packaged set
    /// (fixture development; spec §7).
    #[arg(long, env = "ZEBRAFISH_CASCADES_DIR")]
    cascades_dir: Option<std::path::PathBuf>,

    /// Auto-register a webhook endpoint for this URL at boot and print its
    /// signing secret (spec §14 quickstart). Idempotent per URL.
    #[arg(long, env = "ZEBRAFISH_WEBHOOK_URL")]
    webhook_url: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let db = if cli.ephemeral { ":memory:" } else { &cli.db };
    let mut world = World::open(db, cli.seed)?;

    let cascades = zebrafish_server::cascades::load(cli.cascades_dir.as_deref())?;
    let cascade_count = cascades.fixture_ids().len();
    world.set_cascade_library(cascades);

    eprintln!("{}", banner());
    eprintln!(
        "seed: {}  db: {db}  cascades: {cascade_count}",
        world.seed()
    );

    let state = AppState::new(world);
    spawn_delivery_worker(&state);

    if let Some(url) = &cli.webhook_url {
        let mut world = state.world();
        let existing = world
            .list_webhook_endpoints()?
            .into_iter()
            .find(|row| row.url == *url);
        let row = match existing {
            Some(row) => row,
            None => zebrafish_server::config::webhooks::register(&mut world, url, None, vec![])
                .map_err(|e| e.to_string())?,
        };
        eprintln!("webhook: {}  secret: {}", row.url, row.secret);
    }

    let listener = tokio::net::TcpListener::bind((cli.host.as_str(), cli.port)).await?;
    // The *resolved* address — with --port 0 this is how spawners learn the
    // real port (test harness, SDK-verifier CI jobs).
    let addr = listener.local_addr()?;
    eprintln!("listening on http://{addr}  (dashboard: /_dashboard)");
    axum::serve(listener, app(state)).await?;
    Ok(())
}
