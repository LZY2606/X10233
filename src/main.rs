use std::net::SocketAddr;
use std::sync::Arc;

use cycle_slip_bench::api::build_router;
use cycle_slip_bench::clock::SystemClock;
use cycle_slip_bench::store::Store;

fn parse_addr(args: &[String]) -> SocketAddr {
    if let Some(pos) = args.iter().position(|a| a == "--addr") {
        if let Some(v) = args.get(pos + 1) {
            return v.parse().expect("valid --addr socket address");
        }
    }
    "127.0.0.1:5348".parse().unwrap()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let addr = parse_addr(&args);

    let db_path = std::env::current_dir().unwrap().join("cycle-slip-bench.sqlite");
    let store = Arc::new(Store::open(&db_path, Arc::new(SystemClock)).expect("open sqlite"));
    let app = build_router(store);
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    println!("GNSS cycle-slip bench on http://{addr}");
    axum::serve(listener, app).await.expect("server");
}
