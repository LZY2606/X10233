use std::net::SocketAddr;
use std::sync::Mutex;

use carrier_break_desk::clock::FixedClock;
use carrier_break_desk::service::ProjectService;
use carrier_break_desk::web::build_router;
use carrier_break_desk::FIXED_SEED_MS;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "carrier-break-desk", version, about = "Local-first GNSS cycle-slip review desk")]
struct Args {
    /// Listen address, fixed for acceptance demos: 127.0.0.1:5348
    #[arg(long, default_value = "127.0.0.1:5348")]
    addr: SocketAddr,
    /// SQLite database file (local first; created next to the executable)
    #[arg(long, default_value = "carrier-break-desk.sqlite3")]
    db: String,
    /// Recreate the embedded offline case project on startup if the DB is empty
    #[arg(long, default_value_t = true)]
    seed_case: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,carrier_break_desk=debug".into()),
        )
        .init();

    let args = Args::parse();
    let service = ProjectService::open(
        &args.db,
        std::sync::Arc::new(FixedClock::new(FIXED_SEED_MS, 1)),
        /* seeded rng material */ 0,
    )?;
    if args.seed_case {
        service.ensure_seed_project()?;
    }
    let state = std::sync::Arc::new(Mutex::new(service));
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(args.addr).await?;
    tracing::info!("carrier-break-desk listening on http://{}", args.addr);
    axum::serve(listener, app).await?;
    Ok(())
}
