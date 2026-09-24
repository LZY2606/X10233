mod engine;
mod hash;
mod model;
mod parser;
mod store;
mod web;

use std::sync::Arc;
use store::Store;

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut addr = "127.0.0.1:5348".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--addr" {
            addr = args.next().ok_or("--addr requires a value")?;
        } else {
            return Err(format!("unknown argument: {arg}"));
        }
    }
    let database = std::env::var("CARRIER_BREAK_DB").unwrap_or_else(|_| "carrier-break-station.sqlite".to_string());
    let store = Arc::new(Store::open(&database)?);
    let app = web::router(store);
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| e.to_string())?;
    println!("carrier-break-station listening on http://{addr} (db: {database})");
    axum::serve(listener, app).with_graceful_shutdown(async {
        tokio::signal::ctrl_c().await.ok();
    }).await.map_err(|e| e.to_string())
}
