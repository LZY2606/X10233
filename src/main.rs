//! Entry point: `cycle_slip_bench --addr 127.0.0.1:5348 --db ./csbench.db`.
use cycle_slip_bench::{api, store::Store, vtime::VClock};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let mut addr: String = "127.0.0.1:5348".into();
    let mut db: String = "csbench.db".into();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--addr" => addr = args.next().unwrap_or(addr),
            "--db" => db = args.next().unwrap_or(db),
            "-h" | "--help" => {
                println!("usage: cycle_slip_bench [--addr 127.0.0.1:5348] [--db csbench.db]");
                return;
            }
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    let clock = VClock::seeded();
    let store = Store::open(&db, clock).expect("open sqlite store");
    let app = api::app(store);
    let socket: SocketAddr = addr.parse().expect("valid socket addr");
    let listener = tokio::net::TcpListener::bind(socket).await.expect("bind addr");
    println!("cycle-slip bench listening on http://{socket} (db {db})");
    axum::serve(listener, app).await.expect("server");
}
