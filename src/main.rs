use carrier_break_desk::{api, clock::WallClock, service::Desk};
use std::sync::Arc;

#[derive(Debug)]
struct Args {
    addr: String,
    db: String,
}

fn parse_args() -> Args {
    let mut addr = "127.0.0.1:5348".to_string();
    let mut db = ".carrier_break/desk.sqlite3".to_string();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--addr" => addr = iter.next().expect("--addr requires a value"),
            "--db" => db = iter.next().expect("--db requires a value"),
            "--help" | "-h" => {
                println!("carrier-break-desk --addr 127.0.0.1:5348 [--db PATH]");
                std::process::exit(0);
            }
            other => panic!("unknown argument: {other}"),
        }
    }
    Args { addr, db }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args();
    if let Some(parent) = std::path::Path::new(&args.db).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let desk = Arc::new(Desk::open(&args.db, Arc::new(WallClock))?);
    let listener = tokio::net::TcpListener::bind(&args.addr).await?;
    println!("carrier-break-desk listening on http://{}", args.addr);
    axum::serve(listener, api::router(desk)).await?;
    Ok(())
}
